//! Build-time conversion of the existing public output and Cloudflare header rules.
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, io, path::Path};

#[derive(Debug)]
struct Rule {
    pattern: String,
    operations: Vec<(String, Option<String>)>,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn parse_headers(text: &str) -> io::Result<Vec<Rule>> {
    let mut rules: Vec<Rule> = vec![];
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        if line.len() > 2000 || line.bytes().any(|b| b.is_ascii_control()) {
            return Err(invalid("invalid static header line"));
        }
        if !line.starts_with(' ') {
            if !line.starts_with('/')
                || line.contains([' ', ':', '\\', '?', '#'])
                || line.matches('*').count() > usize::from(line.ends_with('*'))
            {
                return Err(invalid("unsupported static header pattern"));
            }
            rules.push(Rule {
                pattern: line.into(),
                operations: vec![],
            });
            continue;
        }
        let rule = rules
            .last_mut()
            .ok_or_else(|| invalid("header without path"))?;
        let line = line.trim();
        let (name, value) = if let Some(name) = line.strip_prefix("! ") {
            (name, None)
        } else {
            let (name, value) = line
                .split_once(':')
                .ok_or_else(|| invalid("header has no value"))?;
            (name, Some(value.trim().to_owned()))
        };
        let name = name.to_ascii_lowercase();
        // Extending the source header format requires explicit support instead of silently
        // ignoring a security policy or permitting generated response framing headers.
        if !matches!(
            name.as_str(),
            "x-content-type-options"
                | "referrer-policy"
                | "x-frame-options"
                | "permissions-policy"
                | "content-security-policy"
                | "cache-control"
                | "x-robots-tag"
        ) {
            return Err(invalid("unsupported static header name"));
        }
        rule.operations.push((name, value));
    }
    if rules.first().map(|rule| rule.pattern.as_str()) != Some("/*") {
        return Err(invalid("global security headers are required"));
    }
    Ok(rules)
}

fn headers_for(rules: &[Rule], path: &str) -> BTreeMap<String, String> {
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    for rule in rules {
        let matches = rule
            .pattern
            .strip_suffix('*')
            .map_or(rule.pattern == path, |prefix| path.starts_with(prefix));
        if !matches {
            continue;
        }
        for (name, value) in &rule.operations {
            match value {
                None => {
                    headers.remove(name);
                }
                Some(value) => {
                    headers
                        .entry(name.clone())
                        .and_modify(|old| {
                            old.push_str(", ");
                            old.push_str(value);
                        })
                        .or_insert_with(|| value.clone());
                }
            }
        }
    }
    headers
}

fn mime(path: &str) -> Option<&'static str> {
    let ext = path.rsplit_once('.')?.1;
    Some(match ext {
        "html" if path.ends_with("/index.html") => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        // The skill assistants read (`/skill.md`).
        "md" => "text/markdown; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => return None,
    })
}

fn walk(
    root: &Path,
    dir: &Path,
    files: &mut BTreeMap<String, std::path::PathBuf>,
) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(invalid("public build cannot contain symlinks"));
        }
        if kind.is_dir() {
            walk(root, &path, files)?;
            continue;
        }
        if !kind.is_file() {
            return Err(invalid("public build contains a special file"));
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| invalid("asset left public build root"))?
            .to_str()
            .ok_or_else(|| invalid("non-UTF8 asset"))?
            .replace('\\', "/");
        if matches!(relative.as_str(), "_headers" | "__spa-fallback.html") {
            continue;
        }
        if relative
            .split('/')
            .any(|part| part.is_empty() || part.starts_with(['.', '_']))
            || relative
                .bytes()
                .any(|b| !b.is_ascii_alphanumeric() && !b"/-_.".contains(&b))
            || relative == "api"
            || relative.starts_with("api/")
        {
            return Err(invalid("unsupported public asset path"));
        }
        files.insert(format!("/{relative}"), path);
        if files.len() > 1024 {
            return Err(invalid("public asset count exceeds reviewed bound"));
        }
    }
    Ok(())
}

fn write_headers(headers: &BTreeMap<String, String>) -> String {
    format!(
        "&[{}]",
        headers
            .iter()
            .map(|(name, value)| format!("({name:?},{value:?})"))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn content_addressed(path: &str) -> bool {
    // The pinned Vite build uses eight base64url characters, including possible '-' within
    // the hash. Splitting at the last dash would miss such fonts and JavaScript chunks.
    let stem = path.rsplit_once('.').map(|(stem, _)| stem).unwrap_or("");
    path.starts_with("/assets/")
        && stem.len() > 9
        && stem.as_bytes()[stem.len() - 9] == b'-'
        && stem.as_bytes()[stem.len() - 8..]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(b))
}

pub fn generate(root: &Path) -> io::Result<String> {
    let root = root.canonicalize()?;
    let rules = parse_headers(&fs::read_to_string(root.join("_headers"))?)?;
    let mut files = BTreeMap::new();
    walk(&root, &root, &mut files)?;
    if !files.contains_key("/index.html") {
        return Err(invalid("prerendered homepage is required"));
    }
    let mut source = String::from("pub static ASSETS: &[crate::Asset] = &[\n");
    let mut size = 0;
    for (path, file) in files {
        let mime = mime(&path).ok_or_else(|| invalid("unsupported public asset type"))?;
        let route = path
            .strip_suffix("/index.html")
            .map(|p| if p.is_empty() { "/" } else { p });
        let headers = headers_for(&rules, route.unwrap_or(&path));
        let data = fs::read(&file)?;
        size += data.len();
        if size > 16 * 1024 * 1024 {
            return Err(invalid("public bytes exceed reviewed 16 MiB bound"));
        }
        let etag = format!("\"{:x}\"", Sha256::digest(&data));
        // Vite emits content hashes in this directory. Stable public images receive revalidation.
        let hashed = content_addressed(&path);
        source.push_str(&format!("crate::Asset {{ path:{path:?}, route:{route:?}, mime:{mime:?}, etag:{etag:?}, immutable:{hashed}, headers:{}, bytes:include_bytes!({:?}) }},\n", write_headers(&headers), file.to_str().ok_or_else(|| invalid("non-UTF8 build root"))?));
    }
    source.push_str("];\n");
    source.push_str(&format!(
        "pub static COMMON: &[(&str,&str)] = {};\n",
        write_headers(&headers_for(&rules, "/"))
    ));
    Ok(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vite_hashes_can_include_dashes_and_underscores() {
        assert!(content_addressed("/assets/geist-DjL33-gN.woff2"));
        assert!(content_addressed("/assets/serif-u__WvvIK.woff"));
        assert!(!content_addressed("/images/screenshot-12345678.png"));
        assert!(!content_addressed("/assets/stable.js"));
    }

    #[test]
    fn header_policies_accumulate_by_path_and_removals_apply_only_there() {
        // The site's own shape: one global policy, then a page that adds to it. No page lifts a
        // global header today; the last rule is synthetic and keeps `! Name` covered.
        let rules = parse_headers("/*\n  X-Frame-Options: DENY\n  Content-Security-Policy: default-src 'self'\n/remote*\n  Cache-Control: no-store\n  X-Robots-Tag: noindex, nofollow\n/remote*\n  Content-Security-Policy: frame-ancestors 'none'\n/framed\n  ! X-Frame-Options\n  Content-Security-Policy: frame-ancestors https://example.test\n").unwrap();
        for path in ["/remote", "/remote/", "/remote/index.html"] {
            let headers = headers_for(&rules, path);
            assert_eq!(headers["x-frame-options"], "DENY");
            assert_eq!(
                headers["content-security-policy"],
                "default-src 'self', frame-ancestors 'none'"
            );
            assert_eq!(headers["cache-control"], "no-store");
            assert_eq!(headers["x-robots-tag"], "noindex, nofollow");
        }
        let download = headers_for(&rules, "/download");
        assert_eq!(download["x-frame-options"], "DENY");
        assert_eq!(download["content-security-policy"], "default-src 'self'");
        assert!(!download.contains_key("cache-control"));
        let framed = headers_for(&rules, "/framed");
        assert!(!framed.contains_key("x-frame-options"));
        assert_eq!(
            framed["content-security-policy"],
            "default-src 'self', frame-ancestors https://example.test"
        );
        // An exact pattern covers that path alone.
        assert_eq!(headers_for(&rules, "/framed/")["x-frame-options"], "DENY");
    }

    #[test]
    fn unknown_header_syntax_fails_closed() {
        for input in [
            "/foo/:id\n  X-Frame-Options: DENY",
            "/*\n  Set-Cookie: secret",
            "/*/x\n  X-Frame-Options: DENY",
            "/*\n  X-Frame-Options: DENY\u{0}",
        ] {
            assert!(parse_headers(input).is_err());
        }
    }

    #[test]
    fn manifest_excludes_internal_files_and_refuses_unknown_public_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("_headers"), "/*\n  X-Frame-Options: DENY\n").unwrap();
        fs::write(dir.path().join("index.html"), "<h1>Public</h1>").unwrap();
        fs::write(dir.path().join("__spa-fallback.html"), "not public").unwrap();
        let first = generate(dir.path()).unwrap();
        assert_eq!(first, generate(dir.path()).unwrap());
        assert!(!first.contains("_headers") && !first.contains("__spa-fallback"));
        fs::write(dir.path().join("secret.key"), "not public").unwrap();
        assert!(generate(dir.path()).is_err());
    }
}
