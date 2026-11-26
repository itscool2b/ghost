//! The scrub: the one place that lists what no child may inherit.
//!
//! Children inherit the resolved env minus this denylist, nothing more: prefix families such as
//! `CLAUDE_CODE_*` and `CODEX_*` would strip documented user configuration (Bedrock and Vertex
//! users break, Codex reads the wrong home), while inherited parent-session markers make a child
//! believe it runs inside another Claude Code session. A marker found later joins the lists here,
//! with a test of its own.
//!
//! Names compare case-sensitively on Unix and ASCII case-insensitively on Windows; a name that is
//! not Unicode is kept. Values are never logged; reports carry names only.

use std::ffi::OsStr;

use super::{EnvKey, EnvMap};

/// Why a variable was removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyReason {
    /// Set by the process Ghost runs inside (Claude Code's session markers, an Electron app's
    /// Node mode): a child would think it runs inside that session.
    ParentSessionMarker,
    /// Codex's sandbox markers: a child would think it is sandboxed.
    SandboxMarker,
    /// Attaches a debugger to, or pauses, every Node-based agent.
    DebuggerHook,
}

/// Claude Code's parent-session markers, as the M0 spike saw them, and Electron's Node mode.
const SESSION_MARKERS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_EXECPATH",
    "ELECTRON_RUN_AS_NODE",
];

/// Families of Claude Code's parent-session markers (seen: `CLAUDE_CODE_SESSION_ID`,
/// `CLAUDE_CODE_SESSION_ATTENDED`, `CLAUDE_CODE_MESSAGING_SOCKET`, `CLAUDE_CODE_MESSAGING_TOKEN`,
/// `CLAUDE_CODE_BRIDGE_SESSION_ID`).
const SESSION_MARKER_PREFIXES: &[&str] = &[
    "CLAUDE_CODE_SESSION_",
    "CLAUDE_CODE_MESSAGING_",
    "CLAUDE_CODE_BRIDGE_",
];

const SANDBOX_MARKERS: &[&str] = &["CODEX_SANDBOX", "CODEX_SANDBOX_NETWORK_DISABLED"];

/// VS Code's auto-attach.
const DEBUGGER_HOOKS: &[&str] = &["VSCODE_INSPECTOR_OPTIONS"];

/// Removed only when it carries a debugger hook ([`node_options_has_debugger_hook`]).
const NODE_OPTIONS: &str = "NODE_OPTIONS";

/// Why the variable `name=value` must not reach a child, or `None` to keep it.
pub fn deny_reason(name: &OsStr, value: &OsStr) -> Option<DenyReason> {
    let name = name.to_str()?;
    if SESSION_MARKERS.iter().any(|marker| name_eq(name, marker))
        || SESSION_MARKER_PREFIXES
            .iter()
            .any(|prefix| name_starts_with(name, prefix))
    {
        return Some(DenyReason::ParentSessionMarker);
    }
    if SANDBOX_MARKERS.iter().any(|marker| name_eq(name, marker)) {
        return Some(DenyReason::SandboxMarker);
    }
    if DEBUGGER_HOOKS.iter().any(|hook| name_eq(name, hook)) {
        return Some(DenyReason::DebuggerHook);
    }
    if name_eq(name, NODE_OPTIONS) && node_options_has_debugger_hook(&value.to_string_lossy()) {
        return Some(DenyReason::DebuggerHook);
    }
    None
}

/// Whether a `NODE_OPTIONS` value attaches a debugger: a whitespace-separated token (quotes
/// honoured) that is `--inspect`, `--inspect-brk` or `--inspect-wait` (bare or `=value`), or
/// `--require`/`-r` (separate or `=`-joined) of a path containing `bootloader` or `js-debug`, which
/// is what VS Code's auto-attach injects. `--max-old-space-size=4096` and the like are kept.
pub fn node_options_has_debugger_hook(value: &str) -> bool {
    let tokens = split_quoted(value);
    let mut tokens = tokens.iter().map(String::as_str);
    while let Some(token) = tokens.next() {
        let is_inspect = ["--inspect", "--inspect-brk", "--inspect-wait"]
            .iter()
            .any(|flag| {
                token == *flag
                    || token
                        .strip_prefix(flag)
                        .is_some_and(|rest| rest.starts_with('='))
            });
        if is_inspect {
            return true;
        }
        let required = match token {
            "--require" | "-r" => tokens.next(),
            _ => token
                .strip_prefix("--require=")
                .or_else(|| token.strip_prefix("-r=")),
        };
        if required.is_some_and(|path| path.contains("bootloader") || path.contains("js-debug")) {
            return true;
        }
    }
    false
}

/// Splits on whitespace outside quotes. Double quotes take backslash escapes, as Node's own parser
/// does; single quotes are honoured too, which can only keep a quoted path in one piece.
fn split_quoted(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut quote: Option<char> = None;
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some('"'), '\\') => {
                if let Some(escaped) = chars.next() {
                    current.push(escaped);
                }
            }
            (Some(open), _) if c == open => quote = None,
            (Some(_), _) => current.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                in_token = true;
            }
            (None, _) if c.is_whitespace() => {
                if in_token {
                    tokens.push(std::mem::take(&mut current));
                    in_token = false;
                }
            }
            (None, _) => {
                current.push(c);
                in_token = true;
            }
        }
    }
    if in_token {
        tokens.push(current);
    }
    tokens
}

/// The names [`scrub`] removed, sorted. Never values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScrubReport {
    pub removed: Vec<String>,
}

/// Removes every denied variable from `vars`.
pub fn scrub(vars: &mut EnvMap) -> ScrubReport {
    let doomed: Vec<EnvKey> = vars
        .iter()
        .filter(|(name, value)| deny_reason(name.as_os_str(), value).is_some())
        .map(|(name, _)| name.clone())
        .collect();
    let mut removed: Vec<String> = doomed
        .into_iter()
        .filter_map(|name| {
            vars.remove(&name)?;
            Some(name.as_os_str().to_string_lossy().into_owned())
        })
        .collect();
    removed.sort();
    ScrubReport { removed }
}

/// Name equality as the OS sees it: exact on Unix, ASCII case-insensitive on Windows.
fn name_eq(name: &str, other: &str) -> bool {
    if cfg!(windows) {
        name.eq_ignore_ascii_case(other)
    } else {
        name == other
    }
}

/// Prefix match with [`name_eq`]'s case rule.
fn name_starts_with(name: &str, prefix: &str) -> bool {
    name.get(..prefix.len())
        .is_some_and(|head| name_eq(head, prefix))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    fn removed(name: &str, value: &str) -> Option<DenyReason> {
        let mut vars = EnvMap::new();
        vars.insert(EnvKey::from(name), OsString::from(value));
        vars.insert(EnvKey::from("PATH"), OsString::from("/usr/bin"));
        let report = scrub(&mut vars);
        let reason = deny_reason(OsStr::new(name), OsStr::new(value));
        assert_eq!(
            report.removed.contains(&name.to_owned()),
            reason.is_some(),
            "{name}: the scrub and deny_reason agree"
        );
        assert_eq!(
            vars.contains_key(&EnvKey::from(name)),
            reason.is_none(),
            "{name}"
        );
        assert!(vars.contains_key(&EnvKey::from("PATH")), "PATH is kept");
        reason
    }

    /// One test per denylist entry, as the plan asks.
    macro_rules! scrub_removes {
        ($($test:ident: $name:literal = $value:literal => $reason:ident;)+) => {
            $(
                #[test]
                fn $test() {
                    assert_eq!(removed($name, $value), Some(DenyReason::$reason));
                }
            )+
        };
    }

    /// One test per documented variable that must pass through.
    macro_rules! scrub_keeps {
        ($($test:ident: $name:literal = $value:literal;)+) => {
            $(
                #[test]
                fn $test() {
                    assert_eq!(removed($name, $value), None);
                }
            )+
        };
    }

    scrub_removes! {
        scrub_removes_claudecode: "CLAUDECODE" = "1" => ParentSessionMarker;
        scrub_removes_claude_pid: "CLAUDE_PID" = "4242" => ParentSessionMarker;
        scrub_removes_claude_effort: "CLAUDE_EFFORT" = "high" => ParentSessionMarker;
        scrub_removes_claude_code_entrypoint: "CLAUDE_CODE_ENTRYPOINT" = "cli" => ParentSessionMarker;
        scrub_removes_claude_code_child_session: "CLAUDE_CODE_CHILD_SESSION" = "1" => ParentSessionMarker;
        scrub_removes_claude_code_execpath: "CLAUDE_CODE_EXECPATH" = "/x/claude" => ParentSessionMarker;
        scrub_removes_claude_code_session_id: "CLAUDE_CODE_SESSION_ID" = "abc" => ParentSessionMarker;
        scrub_removes_claude_code_session_attended: "CLAUDE_CODE_SESSION_ATTENDED" = "1" => ParentSessionMarker;
        scrub_removes_claude_code_messaging_socket: "CLAUDE_CODE_MESSAGING_SOCKET" = "/tmp/s" => ParentSessionMarker;
        scrub_removes_claude_code_messaging_token: "CLAUDE_CODE_MESSAGING_TOKEN" = "t0k3n" => ParentSessionMarker;
        scrub_removes_claude_code_bridge_session_id: "CLAUDE_CODE_BRIDGE_SESSION_ID" = "b" => ParentSessionMarker;
        scrub_removes_codex_sandbox: "CODEX_SANDBOX" = "seatbelt" => SandboxMarker;
        scrub_removes_codex_sandbox_network_disabled: "CODEX_SANDBOX_NETWORK_DISABLED" = "1" => SandboxMarker;
        scrub_removes_electron_run_as_node: "ELECTRON_RUN_AS_NODE" = "1" => ParentSessionMarker;
        scrub_removes_vscode_inspector_options: "VSCODE_INSPECTOR_OPTIONS" = "{}" => DebuggerHook;
        scrub_removes_node_options_with_a_debugger_hook: "NODE_OPTIONS" = "--inspect" => DebuggerHook;
    }

    scrub_keeps! {
        scrub_keeps_claude_code_use_bedrock: "CLAUDE_CODE_USE_BEDROCK" = "1";
        scrub_keeps_claude_code_use_vertex: "CLAUDE_CODE_USE_VERTEX" = "1";
        scrub_keeps_claude_code_max_output_tokens: "CLAUDE_CODE_MAX_OUTPUT_TOKENS" = "32000";
        scrub_keeps_claude_code_git_bash_path: "CLAUDE_CODE_GIT_BASH_PATH" = "C:\\Git\\bin\\bash.exe";
        scrub_keeps_claude_code_oauth_token: "CLAUDE_CODE_OAUTH_TOKEN" = "oauth";
        scrub_keeps_claude_code_disable_nonessential_traffic: "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC" = "1";
        scrub_keeps_claude_config_dir: "CLAUDE_CONFIG_DIR" = "/Users/me/.claude-work";
        scrub_keeps_codex_home: "CODEX_HOME" = "/Users/me/.codex";
        scrub_keeps_anthropic_api_key: "ANTHROPIC_API_KEY" = "sk-ant-x";
        scrub_keeps_openai_api_key: "OPENAI_API_KEY" = "sk-x";
        scrub_keeps_ssh_auth_sock: "SSH_AUTH_SOCK" = "/tmp/agent.sock";
        scrub_keeps_display: "DISPLAY" = ":0";
        scrub_keeps_wayland_display: "WAYLAND_DISPLAY" = "wayland-0";
        scrub_keeps_dbus_session_bus_address: "DBUS_SESSION_BUS_ADDRESS" = "unix:path=/run/user/1000/bus";
        scrub_keeps_https_proxy: "HTTPS_PROXY" = "http://proxy:3128";
        scrub_keeps_http_proxy: "HTTP_PROXY" = "http://proxy:3128";
        scrub_keeps_no_proxy: "NO_PROXY" = "localhost";
        scrub_keeps_node_extra_ca_certs: "NODE_EXTRA_CA_CERTS" = "/etc/ca.pem";
        scrub_keeps_ssl_cert_file: "SSL_CERT_FILE" = "/etc/cert.pem";
        scrub_keeps_path: "PATH" = "/usr/bin";
        scrub_keeps_home: "HOME" = "/Users/me";
        scrub_keeps_node_options_without_a_hook: "NODE_OPTIONS" = "--max-old-space-size=4096";
        scrub_keeps_claude_code_session_without_its_underscore: "CLAUDE_CODE_SESSION" = "x";
    }

    #[test]
    fn node_options_debugger_hook() {
        for hooked in [
            "--inspect",
            "--inspect=9229",
            "--inspect-brk",
            "--inspect-brk=127.0.0.1:9230",
            "--inspect-wait",
            "--max-old-space-size=4096 --inspect-wait=0",
            "--require /Users/me/.vscode/js-debug/bootloader.js",
            "-r /x/ms-vscode.js-debug/src/bootloader.js",
            "--require=/x/bootloader.js",
            "-r=/x/js-debug/a.js",
            r#"--require "/Users/me/Library/Application Support/Code/User/workspaceStorage/1/ms-vscode.js-debug/bootloader.js""#,
            "  --enable-source-maps\t--inspect  ",
        ] {
            assert!(node_options_has_debugger_hook(hooked), "{hooked}");
        }
        for clean in [
            "",
            "--max-old-space-size=4096",
            "--require ./tracing.js",
            "-r dotenv/config",
            "--inspect-port=9229",
            "--inspector",
            r#"--title "--inspect is mentioned here""#,
            "--require",
        ] {
            assert!(!node_options_has_debugger_hook(clean), "{clean}");
        }
    }

    #[test]
    fn quoted_values_split_like_nodes_parser() {
        assert_eq!(
            split_quoted(r#"--a "b c" 'd e' f\g "h\"i""#),
            ["--a", "b c", "d e", "f\\g", "h\"i"]
        );
        assert_eq!(split_quoted(r#""""#), [""]);
        assert!(split_quoted("   ").is_empty());
    }

    #[test]
    fn names_compare_by_the_platforms_rule() {
        let reason = removed("claudecode", "1");
        if cfg!(windows) {
            assert_eq!(reason, Some(DenyReason::ParentSessionMarker));
            assert_eq!(
                removed("Claude_Code_Session_Id", "x"),
                Some(DenyReason::ParentSessionMarker)
            );
        } else {
            assert_eq!(reason, None, "Unix names are case-sensitive");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_name_that_is_not_unicode_is_kept() {
        use std::os::unix::ffi::OsStrExt;
        let name = OsStr::from_bytes(b"CLAUDECODE\xff");
        assert_eq!(deny_reason(name, OsStr::new("1")), None);
    }

    #[test]
    fn the_report_names_every_removed_variable_sorted() {
        let mut vars: EnvMap = [
            ("PATH", "/bin"),
            ("CODEX_SANDBOX", "seatbelt"),
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_SESSION_ID", "s"),
        ]
        .into_iter()
        .map(|(name, value)| (EnvKey::from(name), OsString::from(value)))
        .collect();
        let report = scrub(&mut vars);
        assert_eq!(
            report.removed,
            ["CLAUDECODE", "CLAUDE_CODE_SESSION_ID", "CODEX_SANDBOX"]
        );
        assert_eq!(vars.len(), 1);
        assert_eq!(scrub(&mut vars), ScrubReport::default());
    }
}
