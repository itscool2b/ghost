//! Where the commands write their lines: stdout in the binary, a buffer in tests. Long commands
//! (a probe, a smoke run) print each line as it happens.

use std::fmt;
use std::io::{self, Write};
use std::sync::{Arc, Mutex, PoisonError};

/// A line writer the commands share. Cloning shares it.
#[derive(Clone)]
pub struct Out {
    sink: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl fmt::Debug for Out {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Out").finish_non_exhaustive()
    }
}

impl Out {
    pub fn stdout() -> Out {
        Out {
            sink: Arc::new(Mutex::new(Box::new(io::stdout()))),
        }
    }

    /// A writer whose lines land in the returned buffer.
    pub fn capture() -> (Out, Captured) {
        let captured = Captured::default();
        let out = Out {
            sink: Arc::new(Mutex::new(Box::new(captured.clone()))),
        };
        (out, captured)
    }

    /// Writes one line. A closed stdout (a pipe whose reader quit) is not an error worth stopping
    /// agents for, so write errors are dropped.
    pub fn line(&self, text: impl AsRef<str>) {
        let mut sink = self.sink.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = writeln!(sink, "{}", text.as_ref());
        let _ = sink.flush();
    }
}

/// What a captured [`Out`] received.
#[derive(Debug, Clone, Default)]
pub struct Captured(Arc<Mutex<Vec<u8>>>);

impl Captured {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap_or_else(PoisonError::into_inner)).into_owned()
    }
}

impl Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_lines_arrive_in_order() {
        let (out, captured) = Out::capture();
        out.line("one");
        out.clone().line(String::from("two"));
        assert_eq!(captured.text(), "one\ntwo\n");
    }
}
