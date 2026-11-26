//! Checks on the configuration that ships with the app: what the page may call (the capability
//! file) and what Tauri embeds from `tauri.conf.json`. These files are data, so a mistake in them
//! compiles fine; these tests are what catches it.

use serde_json::Value;

const CAPABILITY: &str = include_str!("../capabilities/default.json");
const TAURI_CONF: &str = include_str!("../tauri.conf.json");

fn json(text: &str) -> Value {
    serde_json::from_str(text).expect("valid JSON")
}

fn permissions() -> Vec<Value> {
    json(CAPABILITY)["permissions"]
        .as_array()
        .expect("a permissions list")
        .clone()
}

/// A permission entry's identifier, whether it is a plain string or a scoped object.
fn identifier(entry: &Value) -> &str {
    entry
        .as_str()
        .or_else(|| entry["identifier"].as_str())
        .expect("every permission has an identifier")
}

/// Tauri's `app_local_data_dir()` is the local app data folder joined with the bundle identifier,
/// and ghost-core's default data root (`DataRoot::resolve`) is the same folder joined with
/// `APP_IDENTIFIER`. Equal identifiers make them one folder, so the WebView data Tauri keeps there
/// on Windows and Linux sits beside `device.json` and `profiles/` instead of in a second folder of
/// its own. `app.appDirectoriesOverride` would move Tauri's folders and not ghost-core's.
#[test]
fn tauri_keeps_its_app_folders_where_aura_core_keeps_the_data() {
    let conf = json(TAURI_CONF);
    assert_eq!(
        conf["identifier"].as_str(),
        Some(ghost_core::paths::APP_IDENTIFIER)
    );
    let app = conf["app"].as_object().expect("an app section");
    for key in ["appDirectoriesOverride", "app-directories-override"] {
        assert!(!app.contains_key(key), "tauri.conf.json sets app.{key}");
    }
}

#[test]
fn the_capability_covers_only_the_main_window() {
    assert_eq!(json(CAPABILITY)["windows"], serde_json::json!(["main"]));
}

#[test]
fn the_page_cannot_reveal_or_open_paths() {
    // The opener plugin's reveal and open_path commands take any path from the page and check no
    // scope: reveal_data_dir is the vetted replacement for the one folder the UI shows.
    for entry in permissions() {
        let id = identifier(&entry);
        assert!(
            !matches!(
                id,
                "opener:default"
                    | "opener:allow-reveal-item-in-dir"
                    | "opener:allow-open-path"
                    | "opener:allow-default-urls"
            ),
            "the capability grants {id}"
        );
    }
}

#[test]
fn updates_require_a_signed_version_and_only_rust_can_install() {
    let config = json(TAURI_CONF);
    let updater = &config["plugins"]["updater"];
    assert_eq!(updater["requireSignedVersion"], true);
    assert_eq!(updater["allowDowngrades"], false);
    for entry in permissions() {
        assert!(!identifier(&entry).starts_with("updater:"));
    }
}

/// Ghost registers no link scheme with the system (`auracode://` existed only for the removed
/// sign-in page), and the page frames nothing.
#[test]
fn no_link_scheme_is_packaged_and_the_page_frames_nothing() {
    let config = json(TAURI_CONF);
    assert!(config["plugins"].get("deep-link").is_none());
    assert_eq!(config["app"]["security"]["csp"]["frame-src"], "'none'");
    for entry in permissions() {
        assert!(!identifier(&entry).starts_with("deep-link:"));
    }
}

#[test]
fn http_and_https_are_the_only_urls_the_page_can_open() {
    let open_url: Vec<Value> = permissions()
        .into_iter()
        .filter(|entry| identifier(entry) == "opener:allow-open-url")
        .collect();
    assert_eq!(open_url.len(), 1, "one open_url grant: {open_url:?}");
    let entry = &open_url[0];
    assert!(
        entry.is_object(),
        "a plain string would be unscoped, so `allow-default-urls` would be needed (which also lets \
         the page hand mailto: and tel: links to the OS)"
    );
    let mut allowed: Vec<&str> = entry["allow"]
        .as_array()
        .expect("an allow list")
        .iter()
        .map(|scope| scope["url"].as_str().expect("a url pattern"))
        .collect();
    allowed.sort_unstable();
    assert_eq!(allowed, ["http://*", "https://*"]);
    assert!(entry.get("deny").is_none());
}

/// Width, height, bit depth and colour type from a PNG's IHDR chunk.
fn png_header(bytes: &[u8]) -> (u32, u32, u8, u8) {
    assert!(
        bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.get(12..16) == Some(b"IHDR"),
        "not a PNG"
    );
    let word = |at: usize| {
        u32::from_be_bytes(
            bytes[at..at + 4]
                .try_into()
                .expect("four bytes for a 32-bit field"),
        )
    };
    (word(16), word(20), bytes[24], bytes[25])
}

#[test]
fn the_window_icon_tauri_embeds_is_large() {
    // tauri-codegen embeds the first PNG in `bundle.icon` as the default window icon on macOS and
    // Linux, where GTK shows it in the taskbar and window switchers at up to 256 px. Anything
    // smaller is scaled up and blurry. It must also be 8-bit RGBA, or codegen refuses it.
    let conf = json(TAURI_CONF);
    let icons = conf["bundle"]["icon"].as_array().expect("an icon list");
    let first_png = icons
        .iter()
        .filter_map(Value::as_str)
        .find(|path| path.ends_with(".png"))
        .expect("a PNG icon");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(first_png);
    let bytes = std::fs::read(&path).expect("read the icon");
    let (width, height, depth, color) = png_header(&bytes);
    assert!(
        width >= 256 && height >= 256,
        "{first_png} is {width}x{height}"
    );
    assert_eq!((depth, color), (8, 6), "{first_png} is not 8-bit RGBA");
}

#[test]
fn every_listed_icon_exists() {
    let conf = json(TAURI_CONF);
    for icon in conf["bundle"]["icon"].as_array().expect("an icon list") {
        let icon = icon.as_str().expect("a path");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(icon);
        assert!(path.is_file(), "{icon} is listed but missing");
    }
}
