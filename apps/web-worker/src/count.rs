//! The site's two counters: downloads, and copies of o7 that ran on a day.
//!
//! Who may do what, and how it is checked:
//! - Anyone may add one to a count; nobody can read one through the site. The totals are read in
//!   Cloudflare (`docs/usage-counts.md`). A count can be inflated by someone set on it, and that is
//!   all it can be: the only write is one fixed statement adding one to a row whose every column
//!   comes from a short fixed list or a checked version number.
//! - Nothing that identifies a person or a computer is stored or sent: no address, no identifier,
//!   no cookie, no user agent. A row is a day, an event, a platform, a version and a number.
//! - A download is always sent on to its file on GitHub, whether or not counting worked, and only
//!   to an asset of this repository's releases whose name the site knows.

/// Where installers live. The release feed may still name the repository's old path; GitHub
/// redirects that, so a download goes to the current one directly.
const RELEASES: &str = "https://github.com/itscool2b/o7/releases/download";

/// The one statement that changes a count.
pub const ADD_ONE: &str = "INSERT INTO daily (day, event, platform, version, n) VALUES (?1, ?2, ?3, ?4, 1) \
     ON CONFLICT (day, event, platform, version) DO UPDATE SET n = n + 1";

/// One thing to count, already checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Count {
    pub event: &'static str,
    pub platform: &'static str,
    pub version: String,
}

/// What the Worker does with a counting request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Count the download (unless a crawler asked) and send the browser to the file.
    Download { count: Count, location: String },
    /// Count a copy that ran today, and say how many seconds remain until the next day.
    Ran { counts: Vec<Count> },
    /// A counting address with something wrong in it: nothing is counted.
    Refused(u16),
}

/// The counting addresses: `/get/<tag>/<file>` and `/api/count`. Anything else is the site's.
pub fn plan(method: &str, path: &str, query: Option<&str>) -> Option<Plan> {
    if let Some(rest) = path.strip_prefix("/get/") {
        if !matches!(method, "GET" | "HEAD") {
            return Some(Plan::Refused(405));
        }
        return Some(download(rest).map_or(Plan::Refused(404), |(count, location)| {
            Plan::Download { count, location }
        }));
    }
    if path == "/api/count" {
        if method != "GET" {
            return Some(Plan::Refused(405));
        }
        return Some(ran(query.unwrap_or_default()).map_or(Plan::Refused(400), |counts| {
            Plan::Ran { counts }
        }));
    }
    None
}

fn download(rest: &str) -> Option<(Count, String)> {
    let (tag, file) = rest.split_once('/')?;
    let (platform, version) = installer(file)?;
    // A release is tagged v<version>; a staging build's packages, staging-v<version>.
    let tagged = tag.strip_prefix("staging-").unwrap_or(tag).strip_prefix('v') == Some(version);
    tagged.then(|| {
        (
            Count {
                event: "download",
                platform,
                version: version.into(),
            },
            format!("{RELEASES}/{tag}/{file}"),
        )
    })
}

/// The platform and version of one of the installers the Publish workflow makes, by its name.
fn installer(file: &str) -> Option<(&'static str, &str)> {
    const NAMES: [(&str, &str, &str); 6] = [
        ("o7_", "_universal.dmg", "macos"),
        ("o7_", "_x64-setup.exe", "windows-x64"),
        ("o7_", "_arm64-setup.exe", "windows-arm64"),
        ("o7_", "_amd64.AppImage", "linux-appimage"),
        ("o7_", "_amd64.deb", "linux-deb"),
        ("o7-", "-1.x86_64.rpm", "linux-rpm"),
    ];
    NAMES.iter().find_map(|(prefix, suffix, platform)| {
        let version = file.strip_prefix(prefix)?.strip_suffix(suffix)?;
        version_ok(version).then_some((*platform, version))
    })
}

/// Three dot-separated numbers, as every release has.
fn version_ok(version: &str) -> bool {
    let parts: Vec<_> = version.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| (1..=6).contains(&part.len()) && part.bytes().all(|b| b.is_ascii_digit()))
}

/// `os`, `arch`, `version` and `first` (1 the first time this copy ever counts itself), each once.
fn ran(query: &str) -> Option<Vec<Count>> {
    let (mut os, mut arch, mut version, mut first) = (None, None, None, None);
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=')?;
        let slot = match key {
            "os" => &mut os,
            "arch" => &mut arch,
            "version" => &mut version,
            "first" => &mut first,
            _ => return None,
        };
        if slot.replace(value).is_some() {
            return None;
        }
    }
    let platform = match (os?, arch?) {
        ("windows", "x86_64") => "windows-x64",
        ("windows", "aarch64") => "windows-arm64",
        ("macos", "aarch64") => "macos-arm64",
        ("macos", "x86_64") => "macos-x64",
        ("linux", "x86_64") => "linux-x64",
        ("linux", "aarch64") => "linux-arm64",
        _ => return None,
    };
    let version = version.filter(|version| version_ok(version))?;
    let first = match first.unwrap_or("0") {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    let count = |event| Count {
        event,
        platform,
        version: version.into(),
    };
    let mut counts = vec![count("ran")];
    if first {
        counts.push(count("installed"));
    }
    Some(counts)
}

/// Link previews, search engines and other robots follow download links too; they are sent on
/// but not counted. A request with no user agent at all is not a browser.
pub fn robot(user_agent: Option<&str>) -> bool {
    let Some(agent) = user_agent.map(str::to_ascii_lowercase) else {
        return true;
    };
    agent.trim().is_empty()
        || ["bot", "crawl", "spider", "slurp", "preview", "facebookexternalhit"]
            .iter()
            .any(|word| agent.contains(word))
}

/// The US Eastern date of a moment (the owner's day), and the seconds from it to the next Eastern
/// midnight. Daylight saving time runs from 2:00 on the second Sunday in March to 2:00 on the
/// first Sunday in November, the rule since 2007.
pub fn eastern_day(unix: i64) -> (String, i64) {
    let offset = eastern_offset(unix);
    let day = (unix + offset).div_euclid(86_400);
    let (year, month, date) = civil(day);
    // A change of offset happens at 2:00, never at midnight, so the next midnight is found with
    // the offset it has.
    let guess = (day + 1) * 86_400 - offset;
    let next = (day + 1) * 86_400 - eastern_offset(guess);
    (format!("{year:04}-{month:02}-{date:02}"), next - unix)
}

fn eastern_offset(unix: i64) -> i64 {
    let (year, _, _) = civil(unix.div_euclid(86_400));
    let first_sunday = |month| {
        let first = days_from_civil(year, month, 1);
        first + (7 - (first + 4).rem_euclid(7)) % 7
    };
    // 2:00 EST is 7:00 UTC; 2:00 EDT is 6:00 UTC.
    let starts = (first_sunday(3) + 7) * 86_400 + 7 * 3600;
    let ends = first_sunday(11) * 86_400 + 6 * 3600;
    if (starts..ends).contains(&unix) {
        -4 * 3600
    } else {
        -5 * 3600
    }
}

/// Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn location(plan: Option<Plan>) -> Option<(Count, String)> {
        match plan {
            Some(Plan::Download { count, location }) => Some((count, location)),
            _ => None,
        }
    }

    #[test]
    fn every_installer_is_counted_by_platform_and_sent_to_its_file() {
        for (file, platform) in [
            ("o7_0.1.1748_universal.dmg", "macos"),
            ("o7_0.1.1748_x64-setup.exe", "windows-x64"),
            ("o7_0.1.1748_arm64-setup.exe", "windows-arm64"),
            ("o7_0.1.1748_amd64.AppImage", "linux-appimage"),
            ("o7_0.1.1748_amd64.deb", "linux-deb"),
            ("o7-0.1.1748-1.x86_64.rpm", "linux-rpm"),
        ] {
            let (count, to) = location(plan("GET", &format!("/get/v0.1.1748/{file}"), None))
                .unwrap_or_else(|| panic!("{file}"));
            assert_eq!(count.event, "download");
            assert_eq!(count.platform, platform);
            assert_eq!(count.version, "0.1.1748");
            assert_eq!(
                to,
                format!("https://github.com/itscool2b/o7/releases/download/v0.1.1748/{file}")
            );
        }
        let (_, to) = location(plan("HEAD", "/get/staging-v0.2.0/o7_0.2.0_amd64.deb", None)).unwrap();
        assert_eq!(
            to,
            "https://github.com/itscool2b/o7/releases/download/staging-v0.2.0/o7_0.2.0_amd64.deb"
        );
    }

    #[test]
    fn a_download_address_names_a_known_installer_or_nothing_happens() {
        for path in [
            "/get/",
            "/get/v0.1.1748",
            "/get/v0.1.1748/",
            "/get/v0.1.1748/SHA256SUMS",
            "/get/v0.1.1748/latest.json",
            "/get/v0.1.1748/o7_0.1.1748_x64-setup.exe.sig",
            "/get/v0.1.1748/o7.app.tar.gz",
            "/get/v0.1.1747/o7_0.1.1748_x64-setup.exe",
            "/get/0.1.1748/o7_0.1.1748_x64-setup.exe",
            "/get/v0.1.1748/o7_0.1_x64-setup.exe",
            "/get/v0.1.1748/o7_0.1.1748.1_x64-setup.exe",
            "/get/v0.1.1748/o7_0.1.x_x64-setup.exe",
            "/get/v0.1.1748/o7_0.1.1748_x64-setup.exe/more",
            "/get/v0.1.1748/../../../evil/o7_0.1.1748_x64-setup.exe",
            "/get/other/o7_0.1.1748_x64-setup.exe",
            "/get/staging/o7_0.1.1748_x64-setup.exe",
            "/get/staging-v0.1.1747/o7_0.1.1748_x64-setup.exe",
        ] {
            assert_eq!(plan("GET", path, None), Some(Plan::Refused(404)), "{path}");
        }
        assert_eq!(
            plan("POST", "/get/v0.1.1748/o7_0.1.1748_amd64.deb", None),
            Some(Plan::Refused(405))
        );
        // Pages are not counting addresses.
        for path in ["/get", "/download", "/", "/api/counts", "/api/count/"] {
            assert_eq!(plan("GET", path, None), None, "{path}");
        }
    }

    #[test]
    fn a_copy_counts_itself_by_platform_and_version_and_once_as_installed() {
        let ran = |query| match plan("GET", "/api/count", Some(query)) {
            Some(Plan::Ran { counts }) => counts,
            other => panic!("{query}: {other:?}"),
        };
        let counts = ran("os=windows&arch=x86_64&version=0.1.1748");
        assert_eq!(
            counts,
            vec![Count {
                event: "ran",
                platform: "windows-x64",
                version: "0.1.1748".into()
            }]
        );
        let counts = ran("os=macos&arch=aarch64&version=0.1.1748&first=1");
        assert_eq!(
            counts.iter().map(|c| c.event).collect::<Vec<_>>(),
            ["ran", "installed"]
        );
        assert!(counts.iter().all(|c| c.platform == "macos-arm64"));
        assert_eq!(ran("first=0&version=1.2.3&arch=aarch64&os=linux").len(), 1);
    }

    #[test]
    fn anything_else_in_a_count_is_refused() {
        for query in [
            "",
            "os=windows&arch=x86_64",
            "os=windows&version=0.1.1",
            "os=plan9&arch=x86_64&version=0.1.1",
            "os=windows&arch=riscv64&version=0.1.1",
            "os=windows&arch=x86_64&version=0.1",
            "os=windows&arch=x86_64&version=0.1.1&first=2",
            "os=windows&arch=x86_64&version=0.1.1&id=abc",
            "os=windows&os=linux&arch=x86_64&version=0.1.1",
            "os=windows&arch=x86_64&version=0.1.1%27",
            "os=windows&arch=x86_64&version",
        ] {
            assert_eq!(
                plan("GET", "/api/count", Some(query)),
                Some(Plan::Refused(400)),
                "{query}"
            );
        }
        assert_eq!(plan("GET", "/api/count", None), Some(Plan::Refused(400)));
        assert_eq!(
            plan("POST", "/api/count", Some("os=windows&arch=x86_64&version=0.1.1")),
            Some(Plan::Refused(405))
        );
    }

    #[test]
    fn robots_and_previews_are_not_counted() {
        for agent in [
            None,
            Some(""),
            Some("Googlebot/2.1"),
            Some("Mozilla/5.0 (compatible; bingbot/2.0)"),
            Some("Slackbot-LinkExpanding 1.0"),
            Some("facebookexternalhit/1.1"),
            Some("Mozilla/5.0 (compatible; Discordbot/2.0)"),
        ] {
            assert!(robot(agent), "{agent:?}");
        }
        assert!(!robot(Some(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/131.0 Safari/537.36"
        )));
        assert!(!robot(Some("curl/8.5.0")));
    }

    #[test]
    fn days_are_eastern_and_follow_daylight_saving_time() {
        // 2026-11-23 02:49 UTC is still 22 November in New York (EST, UTC-5).
        let (day, left) = eastern_day(1_795_402_157);
        assert_eq!(day, "2026-11-22");
        // Midnight EST on the 23rd is 05:00 UTC, 2 h 10 min 43 s later.
        assert_eq!(left, 2 * 3600 + 10 * 60 + 43);
        // Summer: 2026-07-04 03:30 UTC is 23:30 on 3 July in EDT (UTC-4), 30 minutes before midnight.
        let (day, left) = eastern_day(1_783_135_800);
        assert_eq!(day, "2026-07-03");
        assert_eq!(left, 30 * 60);
        // Around the spring change on 8 March 2026 (07:00 UTC): the day before it is 23 hours long.
        assert_eq!(eastern_day(1_772_946_000).0, "2026-03-08"); // 05:00 UTC = 00:00 EST
        assert_eq!(eastern_day(1_772_946_000).1, 23 * 3600);
        assert_eq!(eastern_day(1_772_945_999).0, "2026-03-07");
        // Around the autumn change on 1 November 2026 (06:00 UTC): that day is 25 hours long.
        assert_eq!(eastern_day(1_793_505_600).0, "2026-11-01"); // 04:00 UTC = 00:00 EDT
        assert_eq!(eastern_day(1_793_505_600).1, 25 * 3600);
        assert_eq!(eastern_day(1_793_505_599).0, "2026-10-31");
        // Leap day and the year's end.
        assert_eq!(eastern_day(1_709_208_000).0, "2024-02-29"); // 12:00 UTC
        assert_eq!(eastern_day(1_767_243_600).0, "2026-01-01"); // 05:00 UTC
        assert_eq!(eastern_day(1_767_243_599).0, "2025-12-31");
    }

    #[test]
    fn the_write_is_one_fixed_statement() {
        assert!(ADD_ONE.starts_with("INSERT INTO daily "));
        assert_eq!(ADD_ONE.matches('?').count(), 4);
        assert!(!ADD_ONE.contains(';'));
    }
}
