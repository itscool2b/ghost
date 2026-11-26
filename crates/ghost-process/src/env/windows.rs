//! The Windows environment: HKLM `SYSTEM\CurrentControlSet\Control\Session Manager\Environment`,
//! then HKCU `Environment` (the user wins), read through `winreg` on a blocking thread. `REG_EXPAND_SZ`
//! values are expanded; `PATH` is system, then user, then the entries of Ghost's own `PATH` not
//! yet present. `USERNAME` and `PROCESSOR_ARCHITECTURE` stay as the logon set them. The merge and
//! the expansion are pure and build on every OS, so their tests run everywhere.

use std::collections::HashSet;

#[cfg(any(windows, test))]
use super::{EnvKey, EnvMap};

/// Names the registry holds but the logon owns, kept as Ghost's own env has them: HKLM's
/// `USERNAME` is `SYSTEM` (meant for services), and its `PROCESSOR_ARCHITECTURE` is the machine's,
/// while a process under emulation has the emulated one. Windows' `CreateEnvironmentBlock` sets
/// both from the logon, and Chocolatey's `refreshenv` restores both after reading the registry.
#[cfg(any(windows, test))]
const LOGON_OWNED: &[&str] = &["USERNAME", "PROCESSOR_ARCHITECTURE"];

/// One registry value: its name, its text, and whether it is `REG_EXPAND_SZ`.
#[cfg(any(windows, test))]
pub(crate) type RegValue = (String, String, bool);

/// Expands `%NAME%` references with `lookup`, as `REG_EXPAND_SZ` values need. Names reach `lookup`
/// as written; Windows lookups ignore case. A value that `lookup` returns is expanded in turn,
/// except a name already being expanded (a cycle), which stays literally, as do unknown names and
/// an unpaired `%`. Pure.
pub fn expand_env_strings(value: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    expand(value, &lookup, &mut Vec::new())
}

fn expand(
    value: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
    active: &mut Vec<String>,
) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(open) = rest.find('%') {
        out.push_str(rest.get(..open).unwrap_or_default());
        let after = rest.get(open + 1..).unwrap_or_default();
        let Some(close) = after.find('%') else {
            out.push('%');
            rest = after;
            break;
        };
        let name = after.get(..close).unwrap_or_default();
        let folded = name.to_ascii_uppercase();
        let found = if name.is_empty() || active.contains(&folded) {
            None
        } else {
            lookup(name)
        };
        match found {
            Some(found) => {
                active.push(folded);
                out.push_str(&expand(&found, lookup, active));
                active.pop();
                rest = after.get(close + 1..).unwrap_or_default();
            }
            None if name.is_empty() => {
                // `%%`: the second `%` may open a reference of its own.
                out.push('%');
                rest = after;
            }
            None => {
                out.push('%');
                out.push_str(name);
                out.push('%');
                rest = after.get(close + 1..).unwrap_or_default();
            }
        }
    }
    out.push_str(rest);
    out
}

/// `PATH` from the system, user and process values: system entries, then user entries, then
/// process entries not yet present, de-duplicated without case, the first occurrence kept. Pure.
pub fn merge_windows_path(
    machine: Option<&str>,
    user: Option<&str>,
    process: Option<&str>,
) -> String {
    let mut seen = HashSet::new();
    let mut merged: Vec<&str> = Vec::new();
    for entry in [machine, user, process]
        .into_iter()
        .flatten()
        .flat_map(|value| value.split(';'))
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        if seen.insert(entry.to_lowercase()) {
            merged.push(entry);
        }
    }
    merged.join(";")
}

/// Whether the registry holds a key at `subkey`, for the machine or for this user: where an
/// organisation's policy for a program lives. A key that is there but may not be opened counts
/// as there. Never, off Windows.
pub fn policy_key_exists(subkey: &str) -> bool {
    #[cfg(windows)]
    {
        [winreg::HKLM, winreg::HKCU]
            .iter()
            .any(|root| match root.open_subkey(subkey) {
                Ok(_) => true,
                Err(error) => error.kind() != std::io::ErrorKind::NotFound,
            })
    }
    #[cfg(not(windows))]
    {
        let _ = subkey;
        false
    }
}

/// The registry's environment over `base` (Ghost's own env without its launch context).
#[cfg(windows)]
pub(crate) fn registry_env(base: &EnvMap) -> std::io::Result<EnvMap> {
    use std::io;
    use winreg::enums::RegType;
    use winreg::types::FromRegValue;

    fn read(key: &winreg::RegKey) -> Vec<RegValue> {
        key.enum_values()
            .flatten()
            .filter_map(|(name, value)| {
                let expand = matches!(value.vtype, RegType::REG_EXPAND_SZ);
                let text = match value.vtype {
                    RegType::REG_SZ | RegType::REG_EXPAND_SZ => {
                        String::from_reg_value(&value).ok()?
                    }
                    _ => return None,
                };
                Some((name, text, expand))
            })
            .collect()
    }

    let system = winreg::HKLM
        .open_subkey(r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment")
        .map(|key| read(&key))?;
    let user = match winreg::HKCU.open_subkey("Environment") {
        Ok(key) => read(&key),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    Ok(merge_registry(base, &system, &user))
}

/// The registry's variables over `base`: every value of `system` (HKLM), then of `user` (HKCU,
/// which wins), `REG_EXPAND_SZ` expanded (a name missing from both falls back to `base`), and
/// `PATH` merged by [`merge_windows_path`]. The logon-owned names ([`LOGON_OWNED`]) stay as `base`
/// has them, in the expansion too. Pure.
#[cfg(any(windows, test))]
pub(crate) fn merge_registry(base: &EnvMap, system: &[RegValue], user: &[RegValue]) -> EnvMap {
    let from_registry = |name: &str| {
        !LOGON_OWNED
            .iter()
            .any(|owned| owned.eq_ignore_ascii_case(name))
    };
    let raw = |name: &str| -> Option<String> {
        user.iter()
            .chain(system.iter())
            .filter(|(known, _, _)| from_registry(known))
            .find(|(known, _, _)| known.eq_ignore_ascii_case(name))
            .map(|(_, text, _)| text.clone())
            .or_else(|| {
                base.get(&EnvKey::from(name))
                    .map(|value| value.to_string_lossy().into_owned())
            })
    };
    let resolve = |text: &str, expand: bool| -> String {
        if expand {
            expand_env_strings(text, raw)
        } else {
            text.to_owned()
        }
    };
    let path_of = |values: &[RegValue]| -> Option<String> {
        values
            .iter()
            .find(|(name, _, _)| name.eq_ignore_ascii_case("Path"))
            .map(|(_, text, expand)| resolve(text, *expand))
    };
    let mut vars = base.clone();
    for (name, text, expand) in system.iter().chain(user.iter()) {
        if name.eq_ignore_ascii_case("Path") || !from_registry(name) {
            continue;
        }
        vars.insert(EnvKey::from(name.as_str()), resolve(text, *expand).into());
    }
    let process_path = base
        .get(&EnvKey::from("PATH"))
        .map(|value| value.to_string_lossy().into_owned());
    let path = merge_windows_path(
        path_of(system).as_deref(),
        path_of(user).as_deref(),
        process_path.as_deref(),
    );
    vars.insert(EnvKey::from("Path"), path.into());
    vars
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(name: &str) -> Option<String> {
        let vars = [
            ("SystemRoot", "C:\\Windows"),
            ("USERPROFILE", "C:\\Users\\me"),
            ("JAVA_HOME", "%ProgramFiles%\\Java"),
            ("ProgramFiles", "C:\\Program Files"),
            ("LOOP_A", "%LOOP_B%"),
            ("LOOP_B", "x%LOOP_A%y"),
            ("SELF", "%self%"),
        ];
        vars.iter()
            .find(|(known, _)| known.eq_ignore_ascii_case(name))
            .map(|(_, value)| (*value).to_owned())
    }

    #[test]
    fn expansion_ignores_case_keeps_unknown_names_and_stops_cycles() {
        assert_eq!(
            expand_env_strings("%systemroot%\\System32;%USERPROFILE%\\bin", lookup),
            "C:\\Windows\\System32;C:\\Users\\me\\bin"
        );
        assert_eq!(
            expand_env_strings("%JAVA_HOME%\\bin", lookup),
            "C:\\Program Files\\Java\\bin",
            "a looked-up value is expanded too"
        );
        assert_eq!(expand_env_strings("%NOPE%\\x", lookup), "%NOPE%\\x");
        assert_eq!(expand_env_strings("100% sure", lookup), "100% sure");
        assert_eq!(expand_env_strings("%%SystemRoot%", lookup), "%C:\\Windows");
        assert_eq!(expand_env_strings("%LOOP_A%", lookup), "x%LOOP_A%y");
        assert_eq!(expand_env_strings("%SELF%", lookup), "%self%");
        assert_eq!(expand_env_strings("plain", lookup), "plain");
    }

    #[test]
    fn windows_path_merge() {
        let system = expand_env_strings(
            "%SystemRoot%\\system32;%SystemRoot%;C:\\Program Files\\Git\\cmd",
            lookup,
        );
        let user = expand_env_strings("%USERPROFILE%\\.local\\bin;C:\\WINDOWS\\SYSTEM32;", lookup);
        let process = "C:\\Windows\\system32;C:\\Users\\me\\AppData\\Local\\Temp\\npm-shim;C:\\users\\ME\\.local\\bin";
        assert_eq!(
            merge_windows_path(Some(&system), Some(&user), Some(process)),
            "C:\\Windows\\system32;C:\\Windows;C:\\Program Files\\Git\\cmd;C:\\Users\\me\\.local\\bin;C:\\Users\\me\\AppData\\Local\\Temp\\npm-shim"
        );
        assert_eq!(merge_windows_path(None, None, Some(" ; ;x ")), "x");
        assert_eq!(merge_windows_path(None, None, None), "");
    }

    fn registry(values: &[(&str, &str, bool)]) -> Vec<RegValue> {
        values
            .iter()
            .map(|(name, text, expand)| ((*name).to_owned(), (*text).to_owned(), *expand))
            .collect()
    }

    fn value<'a>(vars: &'a EnvMap, name: &str) -> Option<&'a str> {
        vars.get(&EnvKey::from(name))
            .and_then(|value| value.to_str())
    }

    #[test]
    fn the_registry_never_replaces_the_logon_user_or_architecture() {
        let base: EnvMap = [
            ("USERNAME", "me"),
            ("PROCESSOR_ARCHITECTURE", "x86"),
            ("USERPROFILE", "C:\\Users\\me"),
            ("PATH", "C:\\from-the-process"),
            ("KEPT", "from the process"),
        ]
        .into_iter()
        .map(|(name, text)| (EnvKey::from(name), text.into()))
        .collect();
        // What HKLM's Environment key holds on a stock install, and an HKCU that names a user.
        let system = registry(&[
            ("USERNAME", "SYSTEM", false),
            ("PROCESSOR_ARCHITECTURE", "AMD64", false),
            ("SystemRoot", "C:\\Windows", false),
            ("TEMP", "%SystemRoot%\\TEMP", true),
            ("Path", "%SystemRoot%\\system32", true),
        ]);
        let user = registry(&[
            ("TEMP", "%USERPROFILE%\\AppData\\Local\\Temp", true),
            ("TOOLS", "C:\\Users\\%USERNAME%\\tools", true),
            ("username", "someone-else", false),
            ("Path", "%USERPROFILE%\\bin", true),
        ]);
        let vars = merge_registry(&base, &system, &user);
        assert_eq!(value(&vars, "USERNAME"), Some("me"));
        assert_eq!(value(&vars, "PROCESSOR_ARCHITECTURE"), Some("x86"));
        assert_eq!(
            value(&vars, "TOOLS"),
            Some("C:\\Users\\me\\tools"),
            "expanded with the logon's user, never SYSTEM"
        );
        assert_eq!(
            value(&vars, "TEMP"),
            Some("C:\\Users\\me\\AppData\\Local\\Temp"),
            "the user wins"
        );
        assert_eq!(value(&vars, "SystemRoot"), Some("C:\\Windows"));
        assert_eq!(value(&vars, "KEPT"), Some("from the process"));
        assert_eq!(
            value(&vars, "Path"),
            Some("C:\\Windows\\system32;C:\\Users\\me\\bin;C:\\from-the-process")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_registry_read() {
        let mut base = super::super::process_env();
        super::super::strip_launch_context(&mut base);
        let vars = registry_env(&base).expect("the registry is readable");
        let path = vars
            .get(&EnvKey::from("PATH"))
            .expect("a PATH")
            .to_string_lossy()
            .into_owned();
        assert!(path.to_ascii_lowercase().contains("system32"), "{path}");
        assert!(!path.contains("%SystemRoot%"), "expanded: {path}");
        assert!(vars.contains_key(&EnvKey::from("SystemRoot")));
        // HKLM says USERNAME=SYSTEM; the logon's values stay.
        for name in ["USERNAME", "PROCESSOR_ARCHITECTURE"] {
            assert_eq!(
                vars.get(&EnvKey::from(name)),
                base.get(&EnvKey::from(name)),
                "{name}"
            );
        }
    }
}
