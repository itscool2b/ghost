//! The installed harness against its latest known release. Pure: detection supplies the facts.
use ghost_protocol::agents::{AgentInstallation, LatestRelease};
use ghost_protocol::plugins::HarnessStatus;
use ghost_protocol::session::Availability;

/// The facts one status is computed from.
pub struct HarnessFacts<'a> {
    pub availability: &'a Availability,
    pub installation: Option<&'a AgentInstallation>,
    pub latest: Option<&'a LatestRelease>,
    /// A reviewed release source exists for this installation's channel.
    pub has_source: bool,
    /// The desktop turned release checks on. Tests and the CLI never fetch.
    pub checks_enabled: bool,
}

/// Every state has a defined meaning: not installed, checking (detection or the first release
/// check is still running), up to date, update available, unknown (offline or no source; the
/// installed version is still shown), and failed (the binary did not pass its version check).
pub fn harness_status(facts: &HarnessFacts<'_>) -> HarnessStatus {
    let reported = match facts.availability {
        Availability::Checking => return HarnessStatus::Checking,
        Availability::Missing { .. } => return HarnessStatus::NotInstalled,
        Availability::Broken { reason } => {
            return HarnessStatus::Failed {
                reason: reason.clone(),
            };
        }
        Availability::Ready { version, .. } => version.as_deref(),
    };
    let version = facts
        .installation
        .map(|installation| installation.version.clone())
        .or_else(|| reported.and_then(crate::parse_version));
    let Some(installed) = version.clone() else {
        return HarnessStatus::Unknown {
            version: None,
            reason: "This agent does not report a version Ghost can compare.".into(),
        };
    };
    if facts.installation.is_none() || !facts.has_source {
        return HarnessStatus::Unknown {
            version,
            reason: "Ghost has no release source for this installation. Its vendor updates it."
                .into(),
        };
    }
    if !facts.checks_enabled {
        return HarnessStatus::Unknown {
            version,
            reason: "Release checks are off in this build.".into(),
        };
    }
    let Some(latest) = facts.latest else {
        return HarnessStatus::Checking;
    };
    let Some(newest) = latest.version.clone() else {
        return HarnessStatus::Unknown {
            version,
            reason: format!(
                "{} did not answer. You may be offline; Ghost checks again later.",
                latest.source
            ),
        };
    };
    // OpenCode 2 is a separate line with a connection of its own. The registry's entry for the
    // 1.x connection has moved on to it, and nothing updates a 1.x installation across the lines,
    // so it is not this installation's update.
    let other_line = facts
        .installation
        .is_some_and(|installation| installation.agent_id == "opencode")
        && major(&installed).is_some()
        && major(&installed) != major(&newest);
    if other_line {
        return HarnessStatus::Unknown {
            version,
            reason: format!(
                "{} lists OpenCode {newest}, a separate version line. Ghost does not check for newer 1.x releases.",
                latest.source
            ),
        };
    }
    if crate::version_at_least(&installed, &newest) {
        HarnessStatus::UpToDate {
            version: installed,
            checked_at: latest.checked_at,
        }
    } else {
        HarnessStatus::UpdateAvailable {
            version: installed,
            latest: newest,
            source: latest.source.clone(),
            checked_at: latest.checked_at,
        }
    }
}

fn major(version: &str) -> Option<u64> {
    crate::parse_version(version)?
        .split('.')
        .next()?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghost_protocol::agents::{AgentHost, InstallChannel, VersionVerification};

    fn installed(version: &str) -> AgentInstallation {
        AgentInstallation {
            agent_id: "claude".into(),
            provider: "claude".into(),
            host: AgentHost::Native,
            launch_path: "/opt/bin/claude".into(),
            resolved_path: Some("/opt/bin/claude".into()),
            version: version.into(),
            channel: InstallChannel::VendorStandalone,
            verification: VersionVerification::Unknown,
        }
    }
    fn ready() -> Availability {
        Availability::Ready {
            version: Some("Claude Code 2.1.312".into()),
            path: "/opt/bin/claude".into(),
        }
    }
    fn release(version: Option<&str>) -> LatestRelease {
        LatestRelease {
            version: version.map(Into::into),
            source: "npm latest".into(),
            checked_at: version.map(|_| 1_000),
            stale: version.is_none(),
        }
    }

    #[test]
    fn another_version_line_is_not_this_installations_update() {
        let ready = ready();
        let two = LatestRelease {
            version: Some("2.2.7".into()),
            source: "ACP Registry".into(),
            checked_at: Some(1_000),
            stale: false,
        };
        let status = |agent: &str, version: &str| {
            let mut installation = installed(version);
            installation.agent_id = agent.into();
            harness_status(&HarnessFacts {
                availability: &ready,
                installation: Some(&installation),
                latest: Some(&two),
                has_source: true,
                checks_enabled: true,
            })
        };
        // OpenCode 1.x is never told that OpenCode 2 is its update: nothing can install it.
        let HarnessStatus::Unknown { version, reason } = status("opencode", "1.19.4") else {
            panic!("the 2.x line is not an update of 1.x");
        };
        assert_eq!(version.as_deref(), Some("1.19.4"));
        assert!(reason.contains("separate version line"), "{reason}");
        // Within a line, and for every other agent, a newer release is an update as before.
        assert!(matches!(
            status("opencode-v2", "2.2.0"),
            HarnessStatus::UpdateAvailable { .. }
        ));
        assert!(matches!(
            status("opencode", "2.2.0"),
            HarnessStatus::UpdateAvailable { .. }
        ));
        assert!(matches!(
            status("claude", "1.9.0"),
            HarnessStatus::UpdateAvailable { .. }
        ));
    }

    #[test]
    fn every_defined_state_is_reached_from_its_facts() {
        let installation = installed("2.1.312");
        let newer = release(Some("2.1.314"));
        let same = release(Some("2.1.312"));
        let offline = release(None);
        let ready = ready();
        let facts = |latest, has_source, checks_enabled| HarnessFacts {
            availability: &ready,
            installation: Some(&installation),
            latest,
            has_source,
            checks_enabled,
        };
        assert_eq!(
            harness_status(&facts(Some(&newer), true, true)),
            HarnessStatus::UpdateAvailable {
                version: "2.1.312".into(),
                latest: "2.1.314".into(),
                source: "npm latest".into(),
                checked_at: Some(1_000),
            }
        );
        assert!(matches!(
            harness_status(&facts(Some(&same), true, true)),
            HarnessStatus::UpToDate { .. }
        ));
        assert_eq!(
            harness_status(&facts(None, true, true)),
            HarnessStatus::Checking
        );
        let HarnessStatus::Unknown { version, reason } =
            harness_status(&facts(Some(&offline), true, true))
        else {
            panic!("offline is unknown");
        };
        assert_eq!(version.as_deref(), Some("2.1.312"));
        assert!(reason.contains("offline"), "{reason}");
        assert!(matches!(
            harness_status(&facts(Some(&newer), false, true)),
            HarnessStatus::Unknown { .. }
        ));
        assert!(matches!(
            harness_status(&facts(Some(&newer), true, false)),
            HarnessStatus::Unknown { .. }
        ));
        for (availability, expected) in [
            (Availability::Checking, HarnessStatus::Checking),
            (
                Availability::Missing { reason: "x".into() },
                HarnessStatus::NotInstalled,
            ),
            (
                Availability::Broken {
                    reason: "`claude --version` failed".into(),
                },
                HarnessStatus::Failed {
                    reason: "`claude --version` failed".into(),
                },
            ),
        ] {
            let facts = HarnessFacts {
                availability: &availability,
                installation: None,
                latest: None,
                has_source: true,
                checks_enabled: true,
            };
            assert_eq!(harness_status(&facts), expected);
        }
    }

    #[test]
    fn a_newer_local_build_is_never_reported_as_an_update() {
        let installation = installed("2.1.320");
        let ready = ready();
        let latest = release(Some("2.1.314"));
        let status = harness_status(&HarnessFacts {
            availability: &ready,
            installation: Some(&installation),
            latest: Some(&latest),
            has_source: true,
            checks_enabled: true,
        });
        assert!(matches!(status, HarnessStatus::UpToDate { .. }));
    }
}
