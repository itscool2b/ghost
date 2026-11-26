//! Repository addresses and folder names. Only https:// and SSH addresses are cloned: they are
//! encrypted and run no command of the address's choosing. Every refusal says why in one sentence.

/// An address Git can clone, kept as typed apart from surrounding space and a trailing slash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Remote {
    pub(super) url: String,
    /// "owner/repository": the last two parts of the path, without `.git`.
    pub(super) repository: String,
    /// The folder name Git itself would choose: the last part, without `.git`.
    pub(super) name: String,
}

const EXAMPLE: &str = "https://github.com/owner/repository";

pub(super) fn remote(input: &str) -> Result<Remote, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Paste the repository's address.".into());
    }
    if input.len() > 2048 {
        return Err("This address is too long to be a repository address.".into());
    }
    if input.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("Remove the spaces from the address.".into());
    }
    if input.starts_with('-') {
        return Err("A repository address cannot start with a hyphen.".into());
    }
    let local = "Folders on this computer are not cloned. Use Choose a folder to add one instead.";
    let bytes = input.as_bytes();
    if (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
        || input.starts_with(['/', '\\', '.', '~'])
    {
        return Err(local.into());
    }
    if let Some((scheme, rest)) = input.split_once("://") {
        return match scheme.to_ascii_lowercase().as_str() {
            "https" => web(rest),
            "ssh" | "git+ssh" | "ssh+git" => ssh_url(rest),
            "http" => Err(
                "Use the https:// address. Plain http:// sends the repository unencrypted.".into(),
            ),
            "git" => Err(
                "git:// is unencrypted. Use the repository's https:// or SSH address instead."
                    .into(),
            ),
            "file" => Err(local.into()),
            _ => Err(format!(
                "Only https:// and SSH addresses can be cloned, such as {EXAMPLE}."
            )),
        };
    }
    if let Some((transport, _)) = input.split_once("::")
        && !transport.contains('/')
    {
        return Err(format!(
            "Only https:// and SSH addresses can be cloned, such as {EXAMPLE}."
        ));
    }
    // scp-like SSH: user@host:path, with a ':' before any '/'.
    if let Some(colon) = input.find(':')
        && input.find('/').is_none_or(|slash| colon < slash)
    {
        return scp(input, colon);
    }
    // A pasted browser address without its scheme: github.com/owner/repository.
    if let Some((host, _)) = input.split_once('/')
        && host.contains('.')
        && valid_host(host)
    {
        return web(input);
    }
    Err(format!(
        "Paste the repository's https:// or SSH address, such as {EXAMPLE}."
    ))
}

/// `https://host[:port]/path`; a bare `host/path` is given the https:// scheme.
fn web(rest: &str) -> Result<Remote, String> {
    if rest.contains(['?', '#']) {
        return Err("Remove the part after ? or #. Paste the repository's address itself.".into());
    }
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.contains('@') {
        return Err(
            "Remove the user name or token from the address. Git asks your own credential helper when it needs to sign in."
                .into(),
        );
    }
    let host = port(authority)?;
    if !valid_host(host) {
        return Err(format!(
            "The address has no valid host name, such as {EXAMPLE}."
        ));
    }
    let path = path.trim_end_matches('/');
    let parts = segments(path)?;
    if host.eq_ignore_ascii_case("github.com") {
        github(&parts)?;
    }
    Ok(named(format!("https://{authority}/{path}"), &parts))
}

fn ssh_url(rest: &str) -> Result<Remote, String> {
    if rest.contains(['?', '#']) {
        return Err("Remove the part after ? or #. Paste the repository's address itself.".into());
    }
    let (authority, path) = rest
        .split_once('/')
        .ok_or_else(|| "Add the repository's path after the host.".to_string())?;
    let host = match authority.rsplit_once('@') {
        Some((user, host)) => {
            ssh_user(user)?;
            host
        }
        None => authority,
    };
    let host = port(host)?;
    if !valid_host(host) {
        return Err("The SSH address has no valid host name.".into());
    }
    let path = path.trim_end_matches('/');
    let parts = segments(path.strip_prefix("~/").unwrap_or(path))?;
    if host.eq_ignore_ascii_case("github.com") {
        github(&parts)?;
    }
    Ok(named(format!("ssh://{authority}/{path}"), &parts))
}

/// `user@host:path`, as GitHub shows it: git@github.com:owner/repository.git.
fn scp(input: &str, colon: usize) -> Result<Remote, String> {
    let (authority, path) = (&input[..colon], &input[colon + 1..]);
    let Some((user, host)) = authority.rsplit_once('@') else {
        return Err(format!(
            "Paste the repository's https:// or SSH address, such as {EXAMPLE} or git@github.com:owner/repository.git."
        ));
    };
    ssh_user(user)?;
    if !valid_host(host) {
        return Err("The SSH address has no valid host name.".into());
    }
    if path.contains(['?', '#']) {
        return Err("Remove the part after ? or #. Paste the repository's address itself.".into());
    }
    if path.starts_with('-') {
        return Err("A repository path cannot start with a hyphen.".into());
    }
    let path = path.trim_end_matches('/');
    let trimmed = path.trim_start_matches('/');
    let parts = segments(trimmed.strip_prefix("~/").unwrap_or(trimmed))?;
    if host.eq_ignore_ascii_case("github.com") {
        github(&parts)?;
    }
    Ok(named(format!("{authority}:{path}"), &parts))
}

fn ssh_user(user: &str) -> Result<(), String> {
    if user.contains(':') {
        return Err(
            "Remove the password from the address. SSH uses your own keys and agent.".into(),
        );
    }
    if user.is_empty()
        || user.starts_with('-')
        || !user
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        return Err("The SSH user name in the address is not valid.".into());
    }
    Ok(())
}

/// The host without a valid `:port`.
fn port(authority: &str) -> Result<&str, String> {
    let Some((host, port)) = authority.rsplit_once(':') else {
        return Ok(authority);
    };
    match port.parse::<u16>() {
        Ok(number) if number > 0 && !port.starts_with('0') => Ok(host),
        _ => Err("The port in the address is not a number from 1 to 65535.".into()),
    }
}

fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && !host.starts_with(['-', '.'])
        && !host.ends_with(['-', '.'])
        && !host.contains("..")
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-".contains(&byte))
}

/// Path parts: plain names, with percent escapes only for ordinary characters.
fn segments(path: &str) -> Result<Vec<&str>, String> {
    if path.is_empty() {
        return Err("Add the repository's path after the host, such as owner/repository.".into());
    }
    let parts: Vec<&str> = path.split('/').collect();
    for part in &parts {
        if part.is_empty() || matches!(*part, "." | "..") {
            return Err("The repository path in the address is not valid.".into());
        }
        let bytes = part.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            if byte == b'%' {
                let escaped = part
                    .get(index + 1..index + 3)
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok());
                match escaped {
                    Some(value)
                        if value >= 0x20 && !matches!(value, b'.' | b'/' | b'\\' | 0x7f) =>
                    {
                        index += 3;
                        continue;
                    }
                    _ => return Err("The repository path in the address is not valid.".into()),
                }
            }
            if !(byte.is_ascii_alphanumeric() || b"._-~+@".contains(&byte)) {
                return Err("The repository path in the address is not valid.".into());
            }
            index += 1;
        }
    }
    Ok(parts)
}

/// GitHub repositories are exactly owner/repository; a deeper path is a page inside one.
fn github(parts: &[&str]) -> Result<(), String> {
    match parts {
        [_, _] => Ok(()),
        [owner, repository, ..] => Err(format!(
            "That is a page inside the repository. Use https://github.com/{owner}/{}.",
            repository.strip_suffix(".git").unwrap_or(repository)
        )),
        _ => Err("Add the repository's name, such as https://github.com/owner/repository.".into()),
    }
}

fn named(url: String, parts: &[&str]) -> Remote {
    let strip = |part: &str| part.strip_suffix(".git").unwrap_or(part).to_owned();
    let name = parts.last().map(|part| strip(part)).unwrap_or_default();
    let repository = match parts {
        [.., owner, last] => format!("{owner}/{}", strip(last)),
        _ => name.clone(),
    };
    Remote {
        url,
        repository,
        name,
    }
}

pub(super) fn name(name: &str) -> Result<(), String> {
    let stem = name.split('.').next().unwrap_or("").to_ascii_lowercase();
    let reserved = matches!(stem.as_str(), "con" | "prn" | "aux" | "nul")
        || (stem.len() == 4
            && (stem.starts_with("com") || stem.starts_with("lpt"))
            && stem.as_bytes()[3].is_ascii_digit());
    if name.is_empty() {
        return Err("Name the new folder.".into());
    }
    if name.len() > 100
        || name.starts_with(['.', ' ', '-'])
        || name.ends_with(['.', ' '])
        || reserved
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_. ".contains(&byte))
    {
        return Err(
            "Use letters, numbers, spaces, dots, hyphens or underscores for the folder name."
                .into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_and_ssh_addresses_are_kept_as_typed_and_named_like_git() {
        for (input, url, repository, name) in [
            (
                " https://github.com/owner/repo ",
                "https://github.com/owner/repo",
                "owner/repo",
                "repo",
            ),
            (
                "https://github.com/owner/repo.git/",
                "https://github.com/owner/repo.git",
                "owner/repo",
                "repo",
            ),
            (
                "github.com/owner/repo",
                "https://github.com/owner/repo",
                "owner/repo",
                "repo",
            ),
            (
                "https://gitlab.example.com:8443/group/sub/project.git",
                "https://gitlab.example.com:8443/group/sub/project.git",
                "sub/project",
                "project",
            ),
            (
                "git@github.com:owner/repo.git",
                "git@github.com:owner/repo.git",
                "owner/repo",
                "repo",
            ),
            (
                "ssh://git@github.com/owner/repo.git",
                "ssh://git@github.com/owner/repo.git",
                "owner/repo",
                "repo",
            ),
            (
                "ssh://git@host.example:2222/~/repo.git",
                "ssh://git@host.example:2222/~/repo.git",
                "repo",
                "repo",
            ),
            (
                "https://dev.azure.com/org/Project%20One/_git/app",
                "https://dev.azure.com/org/Project%20One/_git/app",
                "_git/app",
                "app",
            ),
        ] {
            let remote = remote(input).unwrap_or_else(|error| panic!("{input}: {error}"));
            assert_eq!(remote.url, url, "{input}");
            assert_eq!(remote.repository, repository, "{input}");
            assert_eq!(remote.name, name, "{input}");
        }
    }

    #[test]
    fn everything_else_is_refused_with_its_reason() {
        for (input, reason) in [
            ("", "Paste"),
            ("http://github.com/o/r", "unencrypted"),
            ("git://github.com/o/r", "unencrypted"),
            ("file:///srv/repo.git", "Choose a folder"),
            ("C:\\code\\repo", "Choose a folder"),
            ("/srv/repo.git", "Choose a folder"),
            ("./repo", "Choose a folder"),
            ("\\\\server\\share\\repo", "Choose a folder"),
            ("ext::sh -c touch% /tmp/pwned", "spaces"),
            ("ext::command", "Only https://"),
            ("fd::17", "Only https://"),
            ("ftp://example.com/repo", "Only https://"),
            ("https://token@github.com/o/r", "credential helper"),
            ("https://user:secret@github.com/o/r", "credential helper"),
            ("ssh://git:secret@github.com/o/r", "password"),
            ("https://github.com/o/r?token=private", "after ?"),
            ("https://github.com/o/r#main", "after ?"),
            (
                "https://github.com/o/r/tree/main",
                "https://github.com/o/r.",
            ),
            ("https://github.com/owner", "repository's name"),
            ("https://github.com:0/o/r", "port"),
            ("https://github.com:99999/o/r", "port"),
            ("https://-github.com/o/r", "host"),
            ("https://github.com/o/%2e%2e", "not valid"),
            ("https://github.com/o/r\narg", "spaces"),
            ("https://example.com//r", "not valid"),
            ("https://example.com/../r", "not valid"),
            ("git@github.com:-o/r", "hyphen"),
            ("-oProxyCommand=calc@host:repo", "hyphen"),
            ("ssh://-oProxyCommand=calc/repo", "host"),
            ("github.com:owner/repo", "SSH address"),
            ("owner/repo", "https:// or SSH"),
            ("https://example.com", "path"),
        ] {
            let error = remote(input).expect_err(input);
            assert!(error.contains(reason), "{input}: {error}");
            assert!(
                !error.contains("secret") && !error.contains("private"),
                "{input}"
            );
        }
    }

    #[test]
    fn names_are_single_portable_nondevice_components() {
        for input in ["Orbit", "my-project", "repo.1", "My project"] {
            name(input).unwrap();
        }
        for input in [
            "", ".", "..", ".git", "a/b", "a\\b", "C:repo", "nul.txt", "COM1", "repo.", "repo ",
            "repo\n", "../repo", "-rf",
        ] {
            assert!(name(input).is_err(), "{input}");
        }
    }
}
