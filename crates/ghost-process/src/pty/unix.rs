//! The unsafe operations here are the two terminal ioctls and the pre-exec hook that attaches the
//! slave after process-wrap has created the owned session. No allocation occurs in that hook.
#![allow(
    unsafe_code,
    reason = "native controlling-terminal and window-size bindings"
)]

use super::{Prepared, PtySize, Terminal};
use crate::EnvMap;
use nix::fcntl::{FcntlArg, FdFlag, OFlag, fcntl};
use nix::pty::{Winsize, openpty};
use process_wrap::tokio::{CommandWrap, CommandWrapper};
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

fn dimensions(size: PtySize) -> Winsize {
    Winsize {
        ws_row: size.rows,
        ws_col: size.cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

struct PtyIo(Arc<AsyncFd<File>>);
impl AsyncRead for PtyIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            let mut guard = ready!(self.0.poll_read_ready(cx))?;
            match guard.try_io(|inner| {
                let mut file = inner.get_ref();
                match file.read(buffer.initialize_unfilled()) {
                    Err(error) if error.raw_os_error() == Some(nix::libc::EIO) => Ok(0),
                    result => result,
                }
            }) {
                Ok(result) => {
                    buffer.advance(result?);
                    return Poll::Ready(Ok(()));
                }
                Err(_) => continue,
            }
        }
    }
}
impl AsyncWrite for PtyIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = ready!(self.0.poll_write_ready(cx))?;
            match guard.try_io(|inner| {
                let mut file = inner.get_ref();
                file.write(bytes)
            }) {
                Ok(result) => return Poll::Ready(result),
                Err(_) => continue,
            }
        }
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

struct UnixTerminal(Arc<AsyncFd<File>>);
impl Terminal for UnixTerminal {
    fn resize(&self, size: PtySize) -> io::Result<()> {
        let size = dimensions(size);
        // SAFETY: the retained owned file is the master PTY; size is a live correctly typed value.
        if unsafe { nix::libc::ioctl(self.0.get_ref().as_raw_fd(), nix::libc::TIOCSWINSZ, &size) }
            == -1
        {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    fn close(&self) {
        // The process control ends the owned session; the master closes after the async drain.
    }
}

#[derive(Debug)]
struct ControllingTerminal;
impl CommandWrapper for ControllingTerminal {
    fn pre_spawn(
        &mut self,
        command: &mut tokio::process::Command,
        _: &CommandWrap,
    ) -> io::Result<()> {
        // SAFETY: this runs after ProcessSession's setsid hook and Rust's stdio setup. The hook
        // performs only ioctl and error capture, with no allocation or shared state.
        unsafe {
            command.pre_exec(|| {
                // Darwin exposes this request as u32 while ioctl takes an unsigned long.
                #[cfg(target_vendor = "apple")]
                let request = nix::libc::c_ulong::from(nix::libc::TIOCSCTTY);
                #[cfg(not(target_vendor = "apple"))]
                let request = nix::libc::TIOCSCTTY;
                if nix::libc::ioctl(0, request, 0) == -1 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        Ok(())
    }
}

pub(super) fn prepare(
    program: &Path,
    args: &[OsString],
    cwd: &Path,
    vars: &EnvMap,
    size: PtySize,
) -> io::Result<Prepared> {
    // openpty does not promise close-on-exec. Neither the original slave nor master belongs in
    // the child's inherited descriptor table after its three standard handles are installed,
    // nor in any other child spawned concurrently before they are marked.
    let pair = {
        let _creating = crate::spawn::creating_inheritable();
        let pair = openpty(Some(&dimensions(size)), None).map_err(io::Error::from)?;
        for descriptor in [&pair.master, &pair.slave] {
            fcntl(descriptor, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC)).map_err(io::Error::from)?;
        }
        pair
    };
    let master = File::from(pair.master);
    let flags = fcntl(&master, FcntlArg::F_GETFL).map_err(io::Error::from)?;
    fcntl(
        &master,
        FcntlArg::F_SETFL(OFlag::from_bits_retain(flags) | OFlag::O_NONBLOCK),
    )
    .map_err(io::Error::from)?;
    // SAFETY: File owns this open descriptor for the registration's full lifetime. It is never
    // replaced or closed through a raw handle; all readers, writers and resizing share this Arc.
    let master = Arc::new(unsafe { AsyncFd::register(master)? });
    let slave = File::from(pair.slave);
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(vars.iter().map(|(name, value)| (name.as_os_str(), value)))
        .stdin(slave.try_clone()?)
        .stdout(slave.try_clone()?)
        .stderr(slave);
    let mut command = crate::spawn::wrap(command, true);
    command.wrap(ControllingTerminal);
    let child = {
        let _marked = crate::spawn::inheritable_marked();
        command.spawn()?
    };
    Ok(Prepared {
        child,
        input: Box::new(PtyIo(Arc::clone(&master))),
        output: Box::new(PtyIo(Arc::clone(&master))),
        terminal: Arc::new(UnixTerminal(master)),
    })
}
