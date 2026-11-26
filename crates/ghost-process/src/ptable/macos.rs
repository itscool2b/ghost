//! macOS (and other non-Linux Unix): the process table from `ps`. The parsers are pure and build on
//! every OS so their tests run everywhere; only the calls to `ps` and `sysctl` are Unix-only.
//!
//! `ps -axo pid=,ppid=,pgid=,lstart=,command=` with `LC_ALL=C TZ=UTC0` prints, per process:
//!
//! ```text
//!     1     0     1 Fri Oct 23 03:59:55 2026     /sbin/launchd
//!   338     1   338 Fri Oct  3 04:01:05 2026     /usr/libexec/logd
//! ```
//!
//! `lstart` is always five words (a single-digit day is space-padded), so the command is whatever
//! follows the eighth word. Reading every command line is slow (each one is a sysctl), so the
//! kill rule first lists pids and parents only, then asks for the detail of the few it needs.

#[cfg(all(unix, not(target_os = "linux")))]
use std::io;
#[cfg(all(unix, not(target_os = "linux")))]
use std::sync::OnceLock;

use super::{ProcInfo, StartTime};

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

pub(super) fn parse_ps(text: &str) -> Vec<ProcInfo> {
    text.lines().filter_map(parse_line).collect()
}

fn parse_line(line: &str) -> Option<ProcInfo> {
    let mut rest = line;
    let mut words = [""; 8];
    for slot in &mut words {
        rest = rest.trim_start();
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        if end == 0 {
            return None;
        }
        *slot = rest.get(..end)?;
        rest = rest.get(end..)?;
    }
    let [pid, ppid, pgid, weekday, month, day, time, year] = words;
    let lstart = format!("{weekday} {month} {day} {time} {year}");
    lstart_epoch(&lstart)?;
    let argv = rest
        .trim()
        .split(' ')
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect();
    Some(ProcInfo {
        pid: pid.parse().ok()?,
        ppid: ppid.parse().ok()?,
        pgid: pgid.parse().ok()?,
        sid: None,
        start: StartTime::MacLstart(lstart),
        argv,
    })
}

/// `lstart` words joined by single spaces: `ps -o lstart=` pads the day and the column.
#[cfg(any(test, all(unix, not(target_os = "linux"))))]
pub(super) fn normalize_lstart(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Seconds since the Unix epoch of a UTC `lstart` (`Sat Oct 24 09:03:00 2026`).
pub(super) fn lstart_epoch(lstart: &str) -> Option<i64> {
    let words: Vec<&str> = lstart.split_whitespace().collect();
    let [_weekday, month, day, time, year] = words.as_slice() else {
        return None;
    };
    let month = i64::try_from(MONTHS.iter().position(|name| name == month)? + 1).ok()?;
    let day: i64 = day.parse().ok()?;
    let year: i64 = year.parse().ok()?;
    let mut clock = time.split(':').map(|part| part.parse::<i64>().ok());
    let (hour, minute, second) = (clock.next()??, clock.next()??, clock.next()??);
    if clock.next().is_some()
        || !(1..=31).contains(&day)
        || !(0..24).contains(&hour)
        || !(0..60).contains(&minute)
        || !(0..61).contains(&second)
    {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// `pid ppid pgid` triples from `ps -axo pid=,ppid=,pgid=`.
#[cfg(any(test, all(unix, not(target_os = "linux"))))]
fn parse_parents(text: &str) -> Vec<(u32, u32, u32)> {
    text.lines()
        .filter_map(|line| {
            let mut numbers = line.split_whitespace().map(|word| word.parse::<u32>().ok());
            Some((numbers.next()??, numbers.next()??, numbers.next()??))
        })
        .collect()
}

/// Runs `ps` with a fixed locale and UTC, returning its stdout. `ps` exits 1 when none of the
/// requested pids exist; that is an empty answer, not an error.
#[cfg(all(unix, not(target_os = "linux")))]
fn ps(args: &[&str]) -> io::Result<String> {
    let child = crate::spawn_command(
        std::process::Command::new("/bin/ps")
            .args(args)
            .env("LC_ALL", "C")
            .env("TZ", "UTC0")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
    )?;
    let output = child.wait_with_output()?;
    if !output.status.success() && !(output.status.code() == Some(1) && args.contains(&"-p")) {
        return Err(io::Error::other("ps could not inspect the process table"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(all(unix, not(target_os = "linux")))]
fn with_sessions(mut table: Vec<ProcInfo>) -> Vec<ProcInfo> {
    use nix::unistd::{Pid, getsid};
    for info in &mut table {
        info.sid = i32::try_from(info.pid)
            .ok()
            .and_then(|pid| getsid(Some(Pid::from_raw(pid))).ok())
            .and_then(|sid| u32::try_from(sid.as_raw()).ok());
    }
    table
}

#[cfg(all(unix, not(target_os = "linux")))]
pub(super) fn snapshot() -> io::Result<Vec<ProcInfo>> {
    let text = ps(&["-axo", "pid=,ppid=,stat=,pgid=,lstart=,command="])?;
    // `stat` sits between ppid and pgid only to spot zombies; drop it before the shared parser.
    let mut live = String::with_capacity(text.len());
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let (Some(pid), Some(ppid), Some(stat)) = (words.next(), words.next(), words.next()) else {
            continue;
        };
        if stat.starts_with('Z') {
            continue;
        }
        let rest = line
            .trim_start()
            .get(pid.len()..)
            .map(str::trim_start)
            .and_then(|rest| rest.get(ppid.len()..))
            .map(str::trim_start)
            .and_then(|rest| rest.get(stat.len()..))
            .unwrap_or_default();
        live.push_str(pid);
        live.push(' ');
        live.push_str(ppid);
        live.push_str(rest);
        live.push('\n');
    }
    Ok(with_sessions(parse_ps(&live)))
}

#[cfg(all(unix, not(target_os = "linux")))]
pub(super) fn tree_of(root: u32, pgid: u32) -> io::Result<Vec<ProcInfo>> {
    let parents = parse_parents(&ps(&["-axo", "pid=,ppid=,pgid="])?);
    let table: Vec<ProcInfo> = parents
        .iter()
        .map(|&(pid, ppid, group)| ProcInfo {
            pid,
            ppid,
            pgid: group,
            sid: None,
            start: StartTime::Unknown,
            argv: Vec::new(),
        })
        .collect();
    let mut wanted: Vec<u32> = super::wanted(&table, root, pgid).into_iter().collect();
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    wanted.sort_unstable();
    let list = wanted
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let detail = ps(&["-o", "pid=,ppid=,pgid=,lstart=,command=", "-p", &list])?;
    Ok(with_sessions(parse_ps(&detail)))
}

#[cfg(all(unix, not(target_os = "linux")))]
pub(super) fn start_time(pid: u32) -> io::Result<StartTime> {
    let text = ps(&["-o", "lstart=", "-p", &pid.to_string()])?;
    let lstart = normalize_lstart(&text);
    if lstart_epoch(&lstart).is_none() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no process {pid}"),
        ));
    }
    Ok(StartTime::MacLstart(lstart))
}

#[cfg(all(unix, not(target_os = "linux")))]
pub(super) fn running_start(pid: u32) -> Option<StartTime> {
    let text = ps(&["-o", "stat=,lstart=", "-p", &pid.to_string()]).ok()?;
    let (stat, lstart) = text.trim().split_once(char::is_whitespace)?;
    if stat.starts_with('Z') {
        return None;
    }
    let lstart = normalize_lstart(lstart);
    lstart_epoch(&lstart)?;
    Some(StartTime::MacLstart(lstart))
}

#[cfg(all(unix, not(target_os = "linux")))]
pub(super) fn checked_running_start(pid: u32) -> io::Result<Option<StartTime>> {
    let text = ps(&["-o", "stat=,lstart=", "-p", &pid.to_string()])?;
    if text.trim().is_empty() {
        return super::confirm_absent(pid);
    }
    let (stat, lstart) = text
        .trim()
        .split_once(char::is_whitespace)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unreadable process identity"))?;
    if stat.starts_with('Z') {
        return Ok(None);
    }
    let lstart = normalize_lstart(lstart);
    if lstart_epoch(&lstart).is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unreadable process identity",
        ));
    }
    Ok(Some(StartTime::MacLstart(lstart)))
}

#[cfg(all(unix, not(target_os = "linux")))]
pub(super) fn checked_group_members(pgid: u32) -> io::Result<Vec<u32>> {
    let text = ps(&["-axo", "pid=,stat=,pgid="])?;
    let mut found = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let fields: Vec<_> = line.split_whitespace().collect();
        let [pid, stat, group] = fields.as_slice() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unreadable process group",
            ));
        };
        let pid = pid
            .parse::<u32>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "unreadable process id"))?;
        let group = group
            .parse::<u32>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "unreadable process group"))?;
        if group == pgid && !stat.starts_with('Z') {
            found.push(pid);
        }
    }
    Ok(found)
}

/// The start time of each of `pids` that runs (not a zombie), from one `ps` call.
#[cfg(all(unix, not(target_os = "linux")))]
pub(super) fn running_starts(pids: &[u32]) -> std::collections::HashMap<u32, StartTime> {
    let mut found = std::collections::HashMap::new();
    if pids.is_empty() {
        return found;
    }
    let list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let Ok(text) = ps(&["-o", "pid=,stat=,lstart=", "-p", &list]) else {
        return found;
    };
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let (Some(pid), Some(stat)) = (words.next(), words.next()) else {
            continue;
        };
        if stat.starts_with('Z') {
            continue;
        }
        let lstart = words.collect::<Vec<_>>().join(" ");
        if let (Ok(pid), Some(_)) = (pid.parse::<u32>(), lstart_epoch(&lstart)) {
            found.insert(pid, StartTime::MacLstart(lstart));
        }
    }
    found
}

/// `kern.boottime`'s seconds, read once.
#[cfg(all(unix, not(target_os = "linux")))]
pub(super) fn boot_marker() -> Option<String> {
    static BOOT: OnceLock<Option<String>> = OnceLock::new();
    BOOT.get_or_init(|| {
        let child = crate::spawn_command(
            std::process::Command::new("/usr/sbin/sysctl")
                .args(["-n", "kern.boottime"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null()),
        )
        .ok()?;
        let output = child.wait_with_output().ok()?;
        parse_boottime(&String::from_utf8_lossy(&output.stdout))
    })
    .clone()
}

/// The `sec` of `{ sec = 1792727995, usec = 340897 } Thu Oct 22 22:59:55 2026`.
#[cfg(any(test, all(unix, not(target_os = "linux"))))]
fn parse_boottime(text: &str) -> Option<String> {
    let after = text.split("sec =").nth(1)?;
    let seconds: String = after
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    (!seconds.is_empty()).then_some(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured on macOS 26 with `LC_ALL=C TZ=UTC0 ps -axo pid=,ppid=,pgid=,lstart=,command=`
    /// (commands shortened), plus a padded single-digit day and junk lines.
    const PS: &str = "    1     0     1 Fri Oct 23 03:59:55 2026     /sbin/launchd
  338     1   338 Fri Oct 23 04:01:05 2026     /usr/libexec/logd
  340     1   340 Fri Oct  3 04:01:05 2026     /usr/libexec/UserEventAgent (System)
 5001  4242  5001 Sat Oct 24 09:03:00 2026     /opt/homebrew/bin/hermes acp
 5120  5001  5120 Sat Oct 24 09:03:01 2026     /Users/me/.hermes/venv/bin/python3 -m hermes_cli.main --profile default gateway run --quiet
 5121  5001  5001 Sat Oct 24 09:03:02 2026     /bin/zsh -c echo  two  spaces\\012next
garbage
  12   1
";

    #[test]
    fn ps_output_parses_with_padded_columns_and_days() {
        let table = parse_ps(PS);
        assert_eq!(table.len(), 6);
        assert_eq!(table[0].pid, 1);
        assert_eq!(table[0].argv, ["/sbin/launchd"]);
        assert_eq!(
            table[2].start,
            StartTime::MacLstart("Fri Oct 3 04:01:05 2026".into())
        );
        assert_eq!(table[2].argv, ["/usr/libexec/UserEventAgent", "(System)"]);
        let gateway = &table[4];
        assert_eq!(
            (gateway.pid, gateway.ppid, gateway.pgid),
            (5120, 5001, 5120)
        );
        assert_eq!(gateway.sid, None, "sessions come from getsid");
        assert_eq!(gateway.argv[2], "hermes_cli.main");
        assert_eq!(
            table[5].argv,
            ["/bin/zsh", "-c", "echo", "two", "spaces\\012next"]
        );
    }

    #[test]
    fn lstart_converts_to_epoch_seconds_in_utc() {
        assert_eq!(lstart_epoch("Thu Jan 1 00:00:00 1970"), Some(0));
        // kern.boottime on this Mac: sec = 1792727995.
        assert_eq!(
            lstart_epoch("Fri Oct 23 03:59:55 2026"),
            Some(1_792_727_995)
        );
        assert_eq!(
            lstart_epoch("Tue Feb 29 12:00:00 2028"),
            Some(1_835_438_400)
        );
        for bad in [
            "",
            "Fri Oct 23 03:59:55",
            "Fri Foo 23 03:59:55 2026",
            "Fri Oct 32 03:59:55 2026",
            "Fri Oct 23 24:00:00 2026",
            "Fri Oct 23 03:59 2026",
        ] {
            assert_eq!(lstart_epoch(bad), None, "{bad}");
        }
        assert_eq!(
            normalize_lstart("  Fri Oct  3 04:01:05 2026    \n"),
            "Fri Oct 3 04:01:05 2026"
        );
    }

    #[test]
    fn parents_and_boottime_parse() {
        assert_eq!(
            parse_parents("    1     0     1\n  338     1   338\nbad\n"),
            [(1, 0, 1), (338, 1, 338)]
        );
        assert_eq!(
            parse_boottime("{ sec = 1792727995, usec = 340897 } Thu Oct 22 22:59:55 2026\n")
                .as_deref(),
            Some("1792727995")
        );
        assert_eq!(parse_boottime("nothing"), None);
    }
}
