//! Static website responses, and the site's two counters (`count`): downloads at `/get/…` and
//! copies that ran at `/api/count`. The remote relay is a separate Worker on the more-specific
//! `/api/remote/*` route.

pub mod count;

/// The site's address.
pub const SITE: &str = "https://onghost.dev";
/// The addresses the site had before: o7.codes until 26 November 2026, auracode.si until
/// 19 November. A copy of the app from before a move still makes pairing links to its `/remote`,
/// and a phone paired there keeps its key under that origin, so that page and the files it loads
/// keep answering there until those copies have updated. Every other page is sent to [`SITE`].
const FORMER_HOSTS: &[&str] = &["o7.codes", "auracode.si"];

pub struct Asset {
    pub path: &'static str,
    pub route: Option<&'static str>,
    pub mime: &'static str,
    pub etag: &'static str,
    pub immutable: bool,
    pub headers: &'static [(&'static str, &'static str)],
    pub bytes: &'static [u8],
}

pub struct ResponsePlan {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: &'static [u8],
}

fn valid_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.starts_with("//")
        && path.len() <= 2048
        && !path
            .bytes()
            .any(|b| b.is_ascii_control() || matches!(b, b'%' | b'\\' | b'?' | b'#'))
        && !path.split('/').any(|part| matches!(part, "." | ".."))
}

fn header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    headers.retain(|(key, _)| key != name);
    headers.push((name.into(), value.into()));
}

fn error(status: u16, body: &'static [u8], head: bool, common: &[(&str, &str)]) -> ResponsePlan {
    let mut headers: Vec<_> = common
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    header(&mut headers, "cache-control", "no-store");
    header(&mut headers, "content-type", "text/plain; charset=utf-8");
    header(&mut headers, "content-length", &body.len().to_string());
    ResponsePlan {
        status,
        headers,
        body: if head { b"" } else { body },
    }
}

/// The Apple app site association: the file that lets Ghost for iPhone open the site's pairing
/// links (Universal Links). None until the app's Apple Developer Team ID is known; then it is
///
/// ```text
/// {"applinks":{"details":[{"appIDs":["TEAMID.dev.onghost.mobile"],"components":[{"/":"/remote","#":"?*.?*.?*"}]}]}}
/// ```
///
/// with `TEAMID` the Team ID. Only `/remote` with a pairing link's fragment (three parts joined
/// by dots) opens the app; every other page stays in the browser. It is compiled in rather than
/// a static file because the asset manifest takes no folder whose name starts with a dot.
const ASSOCIATION: Option<&str> = None;

/// Where Apple reads the association: the first is where it looks now, the second where it
/// looked first.
const ASSOCIATION_PATHS: [&str; 2] = [
    "/.well-known/apple-app-site-association",
    "/apple-app-site-association",
];

/// The association, at the site's own address only. Apple follows no redirect for it, so the
/// former address answers 404, as both do while there is none.
fn associated(
    association: Option<&'static str>,
    head: bool,
    host: &str,
    common: &[(&str, &str)],
) -> ResponsePlan {
    let former = FORMER_HOSTS
        .iter()
        .any(|former| host.eq_ignore_ascii_case(former));
    let Some(body) = association.filter(|_| !former) else {
        return error(404, b"Not found\n", head, common);
    };
    let mut headers: Vec<_> = common
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    header(&mut headers, "content-type", "application/json");
    header(&mut headers, "cache-control", "public, max-age=3600");
    header(&mut headers, "content-length", &body.len().to_string());
    ResponsePlan {
        status: 200,
        headers,
        body: if head { b"" } else { body.as_bytes() },
    }
}

/// Sends a page asked for at a former address to the same page at [`SITE`].
fn moved(path: &str, common: &[(&str, &str)]) -> ResponsePlan {
    let mut headers: Vec<_> = common
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    header(&mut headers, "location", &format!("{SITE}{path}"));
    header(&mut headers, "cache-control", "public, max-age=3600");
    header(&mut headers, "content-length", "0");
    ResponsePlan {
        status: 308,
        headers,
        body: b"",
    }
}

pub fn respond(
    method: &str,
    host: &str,
    path: &str,
    if_none_match: Option<&str>,
    assets: &[Asset],
    common: &[(&str, &str)],
) -> ResponsePlan {
    respond_with(
        ASSOCIATION,
        method,
        host,
        path,
        if_none_match,
        assets,
        common,
    )
}

/// [`respond`], with `association` as the Apple app site association ([`ASSOCIATION`]).
fn respond_with(
    association: Option<&'static str>,
    method: &str,
    host: &str,
    path: &str,
    if_none_match: Option<&str>,
    assets: &[Asset],
    common: &[(&str, &str)],
) -> ResponsePlan {
    let head = method == "HEAD";
    if !valid_path(path) {
        return error(400, b"Invalid path\n", head, common);
    }
    // The more-specific /api/remote/* route belongs to the relay Worker (apps/relay). Never serve
    // a page for an /api path if this catch-all is invoked without that deployment route
    // installed, or for an /api path no Worker owns.
    if path.eq_ignore_ascii_case("/api")
        || path
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("/api/"))
    {
        return error(404, b"Not found\n", head, common);
    }
    if !matches!(method, "GET" | "HEAD") {
        let mut response = error(405, b"Method not allowed\n", false, common);
        header(&mut response.headers, "allow", "GET, HEAD");
        return response;
    }
    if ASSOCIATION_PATHS.contains(&path) {
        return associated(association, head, host, common);
    }
    let asset = assets.iter().find(|asset| {
        asset.path == path
            || asset.route.is_some_and(|route| {
                route == path || (route != "/" && path.strip_suffix('/') == Some(route))
            })
    });
    // At a former address only the remote page and the files a page loads are served; every
    // other page, and anything unknown, is at the site's address now.
    if FORMER_HOSTS
        .iter()
        .any(|former| host.eq_ignore_ascii_case(former))
        && asset.is_none_or(|asset| asset.route.is_some_and(|route| route != "/remote"))
    {
        return moved(path, common);
    }
    let Some(asset) = asset else {
        return error(404, b"Not found\n", head, common);
    };
    let mut headers: Vec<_> = asset
        .headers
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    header(&mut headers, "content-type", asset.mime);
    let no_store = headers.iter().any(|(key, value)| {
        key == "cache-control" && value.split(',').any(|part| part.trim() == "no-store")
    });
    if !headers.iter().any(|(key, _)| key == "cache-control") {
        header(
            &mut headers,
            "cache-control",
            if asset.immutable {
                "public, max-age=31536000, immutable"
            } else {
                "public, max-age=0, must-revalidate"
            },
        );
    }
    if !no_store {
        header(&mut headers, "etag", asset.etag);
        if if_none_match.is_some_and(|value| {
            value.split(',').any(|part| {
                let tag = part.trim();
                tag == "*" || tag.strip_prefix("W/").unwrap_or(tag) == asset.etag
            })
        }) {
            return ResponsePlan {
                status: 304,
                headers,
                body: b"",
            };
        }
    }
    header(
        &mut headers,
        "content-length",
        &asset.bytes.len().to_string(),
    );
    ResponsePlan {
        status: 200,
        headers,
        body: if head { b"" } else { asset.bytes },
    }
}

#[cfg(target_arch = "wasm32")]
mod edge {
    use worker::*;
    include!(concat!(env!("OUT_DIR"), "/assets.rs"));

    use crate::count::{self, Count, Plan};

    /// Adds one to each count. Counting never stands between anyone and a download: without the
    /// database (a local run) or when it fails, nothing is counted and the answer is the same.
    async fn add(env: &Env, counts: &[Count]) {
        let Ok(db) = env.d1("COUNTS") else {
            return;
        };
        let (day, _) = count::eastern_day((Date::now().as_millis() / 1000) as i64);
        let statements: Vec<_> = counts
            .iter()
            .filter_map(|c| {
                db.prepare(count::ADD_ONE)
                    .bind(&[
                        day.as_str().into(),
                        c.event.into(),
                        c.platform.into(),
                        c.version.as_str().into(),
                    ])
                    .ok()
            })
            .collect();
        if let Err(error) = db.batch(statements).await {
            console_error!("count not recorded: {error}");
        }
    }

    fn plain(status: u16, body: String, common: &[(&str, &str)]) -> Result<Response> {
        let headers = Headers::new();
        for (name, value) in common {
            headers.set(name, value)?;
        }
        headers.set("cache-control", "no-store")?;
        headers.set("content-type", "text/plain; charset=utf-8")?;
        Ok(Response::ok(body)?
            .with_status(status)
            .with_headers(headers))
    }

    async fn counting(
        plan: Plan,
        request: &Request,
        env: &Env,
        common: &[(&str, &str)],
    ) -> Result<Response> {
        match plan {
            Plan::Download { count, location } => {
                let agent = request.headers().get("User-Agent")?;
                if request.method() == Method::Get && !count::robot(agent.as_deref()) {
                    add(env, &[count]).await;
                }
                let headers = Headers::new();
                for (name, value) in common {
                    headers.set(name, value)?;
                }
                headers.set("cache-control", "no-store")?;
                headers.set("x-robots-tag", "noindex, nofollow")?;
                headers.set("location", &location)?;
                Ok(Response::empty()?.with_status(302).with_headers(headers))
            }
            Plan::Ran { counts } => {
                add(env, &counts).await;
                let (_, left) = count::eastern_day((Date::now().as_millis() / 1000) as i64);
                plain(200, format!("{left}\n"), common)
            }
            Plan::Refused(status) => plain(status, "Not counted\n".into(), common),
        }
    }

    #[event(fetch)]
    async fn fetch(request: Request, env: Env, _context: Context) -> Result<Response> {
        let path = request.path();
        let url = request.url()?;
        let host = url.host_str().unwrap_or_default().to_owned();
        let former = crate::FORMER_HOSTS
            .iter()
            .any(|former| host.eq_ignore_ascii_case(former));
        // A copy of the app from before a move counts itself at the address it knew, so that
        // count is taken at a former address too. A download asked for there is sent on like any
        // other page, and counted once, at the site's own address.
        if (!former || path == "/api/count")
            && let Some(plan) = count::plan(request.method().as_ref(), &path, url.query())
        {
            return counting(plan, &request, &env, COMMON).await;
        }
        let conditional = request.headers().get("If-None-Match")?;
        let plan = crate::respond(
            request.method().as_ref(),
            &host,
            &path,
            conditional.as_deref(),
            ASSETS,
            COMMON,
        );
        let headers = Headers::new();
        for (name, value) in plan.headers {
            headers.set(&name, &value)?;
        }
        let response = if plan.body.is_empty() {
            Response::empty()?
        } else {
            Response::from_bytes(plan.body.to_vec())?
        };
        Ok(response.with_status(plan.status).with_headers(headers))
    }
}

#[cfg(test)]
mod manifest;

#[cfg(test)]
mod tests {
    use super::*;
    static ASSETS: &[Asset] = &[
        Asset {
            path: "/index.html",
            route: Some("/"),
            mime: "text/html; charset=utf-8",
            etag: "\"home\"",
            immutable: false,
            headers: &[],
            bytes: b"home",
        },
        Asset {
            path: "/remote/index.html",
            route: Some("/remote"),
            mime: "text/html; charset=utf-8",
            etag: "\"remote\"",
            immutable: false,
            headers: &[("cache-control", "no-store")],
            bytes: b"remote",
        },
        Asset {
            path: "/assets/app-12345678.js",
            route: None,
            mime: "text/javascript; charset=utf-8",
            etag: "\"js\"",
            immutable: true,
            headers: &[],
            bytes: b"js",
        },
    ];
    fn get(method: &str, path: &str, tag: Option<&str>) -> ResponsePlan {
        at("onghost.dev", method, path, tag)
    }
    fn at(host: &str, method: &str, path: &str, tag: Option<&str>) -> ResponsePlan {
        respond(
            method,
            host,
            path,
            tag,
            ASSETS,
            &[("x-content-type-options", "nosniff")],
        )
    }
    fn value<'a>(response: &'a ResponsePlan, name: &str) -> Option<&'a str> {
        response
            .headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn pretty_index_and_trailing_paths_share_no_store() {
        for path in ["/remote", "/remote/", "/remote/index.html"] {
            let response = get("GET", path, Some("*"));
            assert_eq!(response.status, 200);
            assert_eq!(response.body, b"remote");
            assert_eq!(value(&response, "cache-control"), Some("no-store"));
            assert_eq!(value(&response, "etag"), None);
        }
        assert_eq!(get("GET", "/", None).body, b"home");
        assert_eq!(get("GET", "/index.html", None).body, b"home");
    }

    #[test]
    fn a_former_address_keeps_the_remote_page_and_sends_every_other_page_on() {
        for host in ["o7.codes", "O7.Codes", "auracode.si", "AuraCode.SI"] {
            // Pages, their aliases and anything unknown move, path kept.
            for path in [
                "/",
                "/index.html",
                "/download",
                "/missing",
                "/assets/missing.js",
            ] {
                let response = at(host, "GET", path, None);
                assert_eq!(response.status, 308, "{host}{path}");
                assert_eq!(
                    value(&response, "location"),
                    Some(format!("https://onghost.dev{path}").as_str())
                );
                assert!(response.body.is_empty());
                assert_eq!(value(&response, "x-content-type-options"), Some("nosniff"));
                assert_eq!(at(host, "HEAD", path, None).status, 308);
            }
            // The remote page and the files it loads stay, as they are served at the new address.
            for path in ["/remote", "/remote/", "/remote/index.html"] {
                let response = at(host, "GET", path, None);
                assert_eq!(response.status, 200, "{host}{path}");
                assert_eq!(response.body, b"remote");
                assert_eq!(value(&response, "cache-control"), Some("no-store"));
            }
            assert_eq!(at(host, "GET", "/assets/app-12345678.js", None).body, b"js");
            // Nothing else changes there: no page for an API path, no other method, no odd path.
            assert_eq!(at(host, "GET", "/api/remote/v1/health", None).status, 404);
            assert_eq!(at(host, "POST", "/download", None).status, 405);
            assert_eq!(at(host, "GET", "//onghost.dev.example", None).status, 400);
        }
        // Only an exact former address: a name that merely contains one is not one.
        for host in [
            "onghost.dev",
            "preview.onghost.dev",
            "preview.o7.codes",
            "o7.codes.example",
            "noto7.codes",
            "auracode.si.example",
            "notauracode.si",
        ] {
            assert_eq!(at(host, "GET", "/download", None).status, 404, "{host}");
            assert_eq!(at(host, "GET", "/", None).body, b"home", "{host}");
        }
    }

    /// The association as it will be once the Team ID is known.
    const SAMPLE: &str = r##"{"applinks":{"details":[{"appIDs":["TEAMID.dev.onghost.mobile"],"components":[{"/":"/remote","#":"?*.?*.?*"}]}]}}"##;

    fn with(
        association: Option<&'static str>,
        host: &str,
        method: &str,
        path: &str,
    ) -> ResponsePlan {
        respond_with(
            association,
            method,
            host,
            path,
            None,
            ASSETS,
            &[("x-content-type-options", "nosniff")],
        )
    }

    #[test]
    fn the_app_site_association_is_served_once_there_is_one_and_never_redirected() {
        // As built now, there is none: both addresses are not found, here and at the former
        // address, and nothing is sent elsewhere.
        assert_eq!(ASSOCIATION, None);
        for path in ASSOCIATION_PATHS {
            for host in ["onghost.dev", "o7.codes", "auracode.si"] {
                for method in ["GET", "HEAD"] {
                    let response = at(host, method, path, None);
                    assert_eq!(response.status, 404, "{host}{path}");
                    assert_eq!(value(&response, "location"), None);
                    assert_eq!(value(&response, "cache-control"), Some("no-store"));
                }
            }
        }
        for path in ASSOCIATION_PATHS {
            // Once there is one: JSON, kept an hour, the same bytes at both addresses.
            let response = with(Some(SAMPLE), "onghost.dev", "GET", path);
            assert_eq!(response.status, 200, "{path}");
            assert_eq!(response.body, SAMPLE.as_bytes());
            assert_eq!(value(&response, "content-type"), Some("application/json"));
            assert_eq!(
                value(&response, "cache-control"),
                Some("public, max-age=3600")
            );
            assert_eq!(
                value(&response, "content-length"),
                Some(SAMPLE.len().to_string().as_str())
            );
            assert_eq!(value(&response, "x-content-type-options"), Some("nosniff"));
            assert_eq!(value(&response, "location"), None);
            // HEAD: the same answer with no body.
            let head = with(Some(SAMPLE), "onghost.dev", "HEAD", path);
            assert_eq!(head.status, 200);
            assert!(head.body.is_empty());
            assert_eq!(head.headers, response.headers);
            // Nothing but reading it.
            let post = with(Some(SAMPLE), "onghost.dev", "POST", path);
            assert_eq!(post.status, 405);
            assert_eq!(value(&post, "allow"), Some("GET, HEAD"));
            // A former address answers 404 rather than sending Apple on: it follows no
            // redirect, and the app's links are at the site's address.
            for host in ["o7.codes", "auracode.si", "AuraCode.SI"] {
                let former = with(Some(SAMPLE), host, "GET", path);
                assert_eq!(former.status, 404, "{host}{path}");
                assert_eq!(value(&former, "location"), None);
                assert!(with(Some(SAMPLE), host, "HEAD", path).body.is_empty());
            }
        }
        // Only those two paths.
        for path in [
            "/.well-known/apple-app-site-association/",
            "/.well-known/apple-app-site-association.json",
            "/.well-known/",
            "/.well-known",
            "/Apple-App-Site-Association",
        ] {
            assert_eq!(
                with(Some(SAMPLE), "onghost.dev", "GET", path).status,
                404,
                "{path}"
            );
        }
        // Everything else is answered as before.
        assert_eq!(with(Some(SAMPLE), "onghost.dev", "GET", "/").body, b"home");
        assert_eq!(with(Some(SAMPLE), "auracode.si", "GET", "/").status, 308);
    }

    #[test]
    fn missing_assets_internal_files_and_api_are_not_spa_fallback() {
        for path in [
            "/missing",
            "/assets/missing.js",
            "/_headers",
            "/__spa-fallback.html",
            "/api",
            "/api/remote/v1/health",
            "/API/remote/v1/health",
        ] {
            let response = get("GET", path, None);
            assert_eq!(response.status, 404, "{path}");
            assert_eq!(value(&response, "cache-control"), Some("no-store"));
            assert_ne!(response.body, b"home");
        }
        assert_eq!(get("POST", "/api/remote/v1/mcp/host", None).status, 404);
        assert_eq!(
            value(&get("POST", "/remote", None), "allow"),
            Some("GET, HEAD")
        );
    }

    #[test]
    fn traversal_encodings_and_ambiguous_paths_are_refused() {
        for path in [
            "/../index.html",
            "/assets/./app.js",
            "/%2e%2e/index.html",
            "/a%2fb",
            "/a\\b",
            "/a\0b",
            "//index.html",
            "/remote?x",
            "/remote#x",
        ] {
            assert_eq!(get("GET", path, None).status, 400, "{path}");
        }
    }

    #[test]
    fn head_conditional_and_content_addressed_cache_semantics() {
        let get_response = get("GET", "/assets/app-12345678.js", None);
        let head = get("HEAD", "/assets/app-12345678.js", None);
        assert!(head.body.is_empty());
        assert_eq!(head.headers, get_response.headers);
        assert_eq!(value(&head, "content-length"), Some("2"));
        assert_eq!(
            value(&head, "cache-control"),
            Some("public, max-age=31536000, immutable")
        );
        for tag in ["\"js\"", "\"other\", W/\"js\"", "*"] {
            let response = get("GET", "/assets/app-12345678.js", Some(tag));
            assert_eq!(response.status, 304);
            assert!(response.body.is_empty());
            assert_eq!(value(&response, "content-length"), None);
        }
        assert_eq!(
            value(&get("GET", "/", None), "cache-control"),
            Some("public, max-age=0, must-revalidate")
        );
        assert!(get("HEAD", "/missing", None).body.is_empty());
    }
}
