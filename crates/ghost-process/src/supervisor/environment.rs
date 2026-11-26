//! Explicit launch overrides only. The Linux login environment never crosses the wire.

use std::ffi::OsString;
use std::fmt;
use std::io;

use serde::{Deserialize, Serialize};

use super::wire::invalid;
use crate::{LaunchSpec, ResolvedEnv, child_env};

/// Carries the reviewed `LaunchSpec::set_env` entries, never a resolved environment map.
/// Values are excluded from Debug and validation errors because overrides can contain secrets.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LaunchEnvironment(pub Vec<(String, String)>);

impl fmt::Debug for LaunchEnvironment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.0.iter().map(|(key, _)| key))
            .finish()
    }
}

impl TryFrom<&LaunchSpec> for LaunchEnvironment {
    type Error = io::Error;

    fn try_from(launch: &LaunchSpec) -> io::Result<Self> {
        let entries = launch
            .set_env
            .iter()
            .map(|(key, value)| {
                Ok((
                    key.to_str()
                        .ok_or_else(|| invalid("launch override is not UTF-8"))?
                        .to_owned(),
                    value
                        .to_str()
                        .ok_or_else(|| invalid("launch override is not UTF-8"))?
                        .to_owned(),
                ))
            })
            .collect::<io::Result<_>>()?;
        let overrides = Self(entries);
        overrides.validate()?;
        Ok(overrides)
    }
}

impl LaunchEnvironment {
    pub fn validate(&self) -> io::Result<()> {
        if self.0.len() > 64
            || self
                .0
                .iter()
                .map(|(key, value)| key.len().saturating_add(value.len()))
                .sum::<usize>()
                > 64 * 1024
            || self.0.iter().any(|(key, value)| {
                key.is_empty()
                    || key.len() > 256
                    || key.contains(['\0', '='])
                    || value.contains('\0')
            })
        {
            return Err(invalid("invalid supervisor launch overrides"));
        }
        Ok(())
    }

    pub(crate) fn entries(&self) -> Vec<(OsString, OsString)> {
        self.0
            .iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect()
    }

    /// Inspection uses the same scrub-then-overlay ordering as the production spawner.
    pub(crate) fn effective(&self, base: &ResolvedEnv) -> ResolvedEnv {
        let (vars, _) = child_env(base, &self.entries());
        ResolvedEnv {
            vars,
            ..base.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EnvKey, EnvSource};

    #[test]
    fn overrides_are_bounded_and_debug_never_prints_values() {
        let overrides =
            LaunchEnvironment(vec![("PRIVATE_TOKEN".into(), "private-test-value".into())]);
        overrides.validate().expect("valid");
        let debug = format!("{overrides:?}");
        assert!(debug.contains("PRIVATE_TOKEN"));
        assert!(!debug.contains("private-test-value"));
        for entries in [
            vec![("".into(), "x".into())],
            vec![("A=B".into(), "x".into())],
            vec![("NAME".into(), "x\0y".into())],
            vec![("NAME".into(), "x".repeat(64 * 1024))],
            vec![("NAME".into(), "x".into()); 65],
        ] {
            assert!(LaunchEnvironment(entries).validate().is_err());
        }
    }

    #[test]
    fn effective_environment_scrubs_then_applies_ordered_launch_overrides() {
        let base = ResolvedEnv {
            vars: [
                (EnvKey::from("CLAUDECODE"), "parent".into()),
                (EnvKey::from("CODEX_HOME"), "/old".into()),
            ]
            .into(),
            source: EnvSource::Fallback {
                reason: "test".into(),
            },
            warnings: Vec::new(),
            resolved_at_ms: 0,
        };
        let overrides = LaunchEnvironment(vec![
            ("CODEX_HOME".into(), "/first".into()),
            ("CODEX_HOME".into(), "/final".into()),
        ]);
        let effective = overrides.effective(&base);
        assert!(!effective.vars.contains_key(&EnvKey::from("CLAUDECODE")));
        assert_eq!(effective.vars[&EnvKey::from("CODEX_HOME")], "/final");
        assert_eq!(base.vars[&EnvKey::from("CODEX_HOME")], "/old");
    }
}
