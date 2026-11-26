//! `@path` imports as Claude Code and oh-my-pi read them: outside code spans and fenced blocks,
//! relative to the importing file, a backslash before each space in a path.

/// Every import token in the text, as written (without the `@`, spaces unescaped).
pub(crate) fn find(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut fence: Option<&str> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        let marker = if trimmed.starts_with("```") {
            Some("```")
        } else if trimmed.starts_with("~~~") {
            Some("~~~")
        } else {
            None
        };
        if let Some(marker) = marker {
            match fence {
                Some(open) if open == marker => fence = None,
                None => fence = Some(marker),
                _ => {}
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        let mut in_code = false;
        let chars = line.char_indices().collect::<Vec<_>>();
        let mut index = 0;
        while index < chars.len() {
            let (_, ch) = chars[index];
            if ch == '`' {
                in_code = !in_code;
                index += 1;
                continue;
            }
            let starts_token = index == 0 || chars[index - 1].1.is_whitespace();
            if ch == '@' && !in_code && starts_token {
                let mut path = String::new();
                let mut next = index + 1;
                while next < chars.len() {
                    let (_, c) = chars[next];
                    if c == '\\' && chars.get(next + 1).is_some_and(|(_, n)| *n == ' ') {
                        path.push(' ');
                        next += 2;
                        continue;
                    }
                    if c.is_whitespace() || c == '`' {
                        break;
                    }
                    path.push(c);
                    next += 1;
                }
                if looks_like_path(&path) {
                    found.push(path);
                }
                index = next;
                continue;
            }
            index += 1;
        }
    }
    found
}

/// `@AGENTS.md`, `@../AGENTS.md`, `@~/.codex/AGENTS.md`; not `@someone` or an email.
fn looks_like_path(path: &str) -> bool {
    !path.is_empty()
        && (path.contains('/') || path.contains('.'))
        && !path.ends_with(['.', ',', ':', ';', ')'])
        && path.chars().all(|c| !c.is_control() || c == '\u{1}')
}

/// True when the text holds nothing but imports, blank lines and HTML comments.
pub(crate) fn only_imports(text: &str) -> bool {
    let mut stripped = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        stripped.push_str(&rest[..start]);
        rest = match rest[start + 4..].find("-->") {
            Some(end) => &rest[start + 4 + end + 3..],
            None => "",
        };
    }
    stripped.push_str(rest);
    let joined = stripped.replace("\\ ", "\u{1}");
    joined
        .split_whitespace()
        .all(|token| token.strip_prefix('@').is_some_and(looks_like_path))
        && !find(&stripped).is_empty()
}

/// The line Ghost writes, escaping spaces the way Claude Code documents.
pub(crate) fn line(path: &str) -> String {
    format!("@{}\n", path.replace(' ', "\\ "))
}

/// The import path from a file at `from` (relative, forward slashes) to `to` in the same folder.
pub(crate) fn relative(from: &str, to: &str) -> String {
    let depth = from.matches('/').count();
    format!("{}{to}", "../".repeat(depth))
}

/// Resolves an import written in a file at `from` against the folder's relative paths.
pub(crate) fn resolve(from: &str, import: &str) -> Option<String> {
    if import.starts_with('/') || import.starts_with('~') || import.contains(':') {
        return None;
    }
    let mut parts = from.split('/').collect::<Vec<_>>();
    parts.pop();
    for part in import.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_outside_code_are_found_and_code_is_skipped() {
        let text = "@AGENTS.md\nSee `@README.md` and\n```\n@docs/x.md\n```\nAlso @docs/my\\ notes.md and mail me@example.com\n";
        assert_eq!(find(text), vec!["AGENTS.md", "docs/my notes.md"]);
        assert!(only_imports("@AGENTS.md\n"));
        assert!(only_imports("<!-- shared -->\n@AGENTS.md\n\n"));
        assert!(!only_imports("@AGENTS.md\nUse tabs.\n"));
        assert!(!only_imports(""));
        assert!(!only_imports("Read AGENTS.md first.\n"));
    }

    #[test]
    fn relative_paths_resolve_from_the_importing_file() {
        assert_eq!(relative("CLAUDE.md", "AGENTS.md"), "AGENTS.md");
        assert_eq!(relative(".claude/CLAUDE.md", "AGENTS.md"), "../AGENTS.md");
        assert_eq!(
            resolve(".claude/CLAUDE.md", "../AGENTS.md").unwrap(),
            "AGENTS.md"
        );
        assert_eq!(resolve("CLAUDE.md", "./AGENTS.md").unwrap(), "AGENTS.md");
        assert!(resolve("CLAUDE.md", "../../etc/passwd").is_none());
        assert!(resolve("CLAUDE.md", "~/.codex/AGENTS.md").is_none());
        assert_eq!(line("~/My Files/AGENTS.md"), "@~/My\\ Files/AGENTS.md\n");
    }
}
