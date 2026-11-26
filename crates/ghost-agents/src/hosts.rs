//! WSL discovery parsing and host identities. No shell, configuration or credentials are read here.
use ghost_protocol::agents::AgentHost;
use std::collections::HashSet;

const MAX_DISTROS: usize = 64;
const MAX_LIST_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostError {
    #[error("The WSL distribution list is too large.")]
    TooLarge,
    #[error("WSL returned invalid text for its distribution list.")]
    InvalidEncoding,
    #[error("WSL returned an invalid distribution name.")]
    InvalidDistro,
    #[error("This agent host is not supported.")]
    Unsupported,
}

fn valid_distro(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 256
        && !name.starts_with('-')
        && !name.chars().any(char::is_control)
        && !name.contains(['/', '\\'])
}

/// Decode `wsl.exe --list --quiet`. Windows commonly supplies UTF-16LE; UTF-8 is also accepted.
pub fn parse_wsl_distributions(bytes: &[u8]) -> Result<Vec<AgentHost>, HostError> {
    if bytes.len() > MAX_LIST_BYTES {
        return Err(HostError::TooLarge);
    }
    let utf16 = bytes.starts_with(&[0xff, 0xfe])
        || bytes
            .as_chunks::<2>()
            .0
            .iter()
            .take(32)
            .any(|pair| pair[1] == 0);
    let text = if utf16 {
        let bytes = bytes.strip_prefix(&[0xff, 0xfe]).unwrap_or(bytes);
        if !bytes.len().is_multiple_of(2) {
            return Err(HostError::InvalidEncoding);
        }
        let units: Vec<_> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16(&units).map_err(|_| HostError::InvalidEncoding)?
    } else {
        std::str::from_utf8(bytes)
            .map_err(|_| HostError::InvalidEncoding)?
            .to_owned()
    };
    let mut seen = HashSet::new();
    let mut hosts = Vec::new();
    for name in text
        .trim_end_matches('\0')
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        if !valid_distro(name) {
            return Err(HostError::InvalidDistro);
        }
        if seen.insert(name.to_owned()) {
            hosts.push(AgentHost::Wsl {
                distro: name.into(),
            });
        }
        if hosts.len() > MAX_DISTROS {
            return Err(HostError::TooLarge);
        }
    }
    Ok(hosts)
}

/// Namespaced identity, distinct from both a display label and command arguments.
pub fn host_key(host: &AgentHost) -> Result<String, HostError> {
    match host {
        AgentHost::Native => Ok("native".into()),
        AgentHost::Wsl { distro } if valid_distro(distro) => {
            let encoded: String = distro
                .as_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            Ok(format!("wsl:{encoded}"))
        }
        AgentHost::Wsl { .. } => Err(HostError::InvalidDistro),
        AgentHost::Unsupported => Err(HostError::Unsupported),
    }
}

/// Convert at the process boundary. Unknown hosts never fall back to native execution.
pub fn execution(host: &AgentHost) -> ghost_process::HostExecution {
    match host {
        AgentHost::Native => ghost_process::HostExecution::Native,
        AgentHost::Wsl { distro } if valid_distro(distro) => ghost_process::HostExecution::Wsl {
            distro: distro.clone(),
        },
        AgentHost::Wsl { .. } | AgentHost::Unsupported => ghost_process::HostExecution::Unsupported,
    }
}

pub fn display_name(agent: &str, host: &AgentHost) -> String {
    match host {
        AgentHost::Native => agent.into(),
        AgentHost::Wsl { distro } if valid_distro(distro) => format!("{agent} · WSL {distro}"),
        AgentHost::Wsl { .. } | AgentHost::Unsupported => format!("{agent} · Unsupported host"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_distributions_preserve_spaces_and_unicode_without_duplicates() {
        let text = "Ubuntu\r\nMy Linux\r\n开发\r\nUbuntu\r\n";
        let bytes: Vec<_> = [0xfeff]
            .into_iter()
            .chain(text.encode_utf16())
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(
            parse_wsl_distributions(&bytes).unwrap(),
            ["Ubuntu", "My Linux", "开发"].map(|distro| AgentHost::Wsl {
                distro: distro.into()
            })
        );
        assert_eq!(parse_wsl_distributions(text.as_bytes()).unwrap().len(), 3);
    }

    #[test]
    fn malformed_or_unbounded_distribution_lists_are_rejected() {
        for bytes in [vec![0xff, 0xfe, 1], vec![0xff, 0xfe, 0, 0xd8], vec![0xff]] {
            assert_eq!(
                parse_wsl_distributions(&bytes),
                Err(HostError::InvalidEncoding)
            );
        }
        for name in ["--exec", "a\0b", "../Ubuntu", "a\\b"] {
            assert!(parse_wsl_distributions(name.as_bytes()).is_err());
        }
        assert_eq!(
            parse_wsl_distributions(&vec![b'x'; MAX_LIST_BYTES + 1]),
            Err(HostError::TooLarge)
        );
    }

    #[test]
    fn host_keys_do_not_collapse_native_or_different_distributions() {
        let native = host_key(&AgentHost::Native).unwrap();
        let a = host_key(&AgentHost::Wsl {
            distro: "native".into(),
        })
        .unwrap();
        let b = host_key(&AgentHost::Wsl {
            distro: "Ubuntu".into(),
        })
        .unwrap();
        assert_ne!(native, a);
        assert_ne!(a, b);
        assert_eq!(
            host_key(&AgentHost::Unsupported),
            Err(HostError::Unsupported)
        );
    }
}
