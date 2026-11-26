use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The Windows application manifest, next to this file.
const WINDOWS_MANIFEST: &str = "windows-app-manifest.xml";

fn main() {
    println!("cargo:rerun-if-env-changed=GHOST_UPDATER_CHANNEL");
    match env::var("GHOST_UPDATER_CHANNEL").as_deref() {
        Err(env::VarError::NotPresent) | Ok("stable" | "staging") => {}
        _ => panic!("GHOST_UPDATER_CHANNEL must be stable or staging"),
    }
    // tauri-build embeds its Windows manifest in binaries only, so this crate's own test binaries
    // started without Common Controls v6 and Windows refused to load them (exit 0xc0000139,
    // STATUS_ENTRYPOINT_NOT_FOUND, on an import only v6 exports). Tauri's manifest is switched off
    // here and the same manifest is embedded in every linked target instead: the app and its test
    // binaries alike. Embedding both would give the app two manifest resources.
    let attributes = tauri_build::Attributes::new()
        .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
    if let Err(error) = tauri_build::try_build(attributes) {
        panic!("tauri-build failed: {error:#}");
    }
    match env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("windows") => embed_windows_manifest(),
        Ok("macos") => link_clang_runtime(),
        _ => {}
    }
}

/// Links clang's own runtime library on macOS. The speech engine's Metal code is Objective-C that
/// asks which macOS it runs on (`@available`), which the compiler turns into a call to
/// `__isPlatformVersionAtLeast`. That function lives in clang's runtime (`libclang_rt.osx.a`,
/// one archive for both architectures), which clang adds when it links a program itself. Rust
/// links with `-nodefaultlibs`, so nothing added it and the Apple silicon app failed to link with
/// the symbol undefined.
fn link_clang_runtime() {
    match clang_runtime_dir() {
        Some(dir) => {
            println!("cargo:rustc-link-search=native={}", dir.display());
            // Named for the linker to find, not `static=`: rustc would read the archive itself to
            // bundle it, and it does not read archives that hold two architectures.
            println!("cargo:rustc-link-lib=clang_rt.osx");
        }
        // Without it the link fails on its own, naming the symbol; this says where to look.
        None => println!(
            "cargo:warning=clang's runtime library (libclang_rt.osx.a) was not found: \
             the app may fail to link with __isPlatformVersionAtLeast undefined"
        ),
    }
}

/// The folder holding `libclang_rt.osx.a`: `lib/darwin` under the resource folder clang reports.
fn clang_runtime_dir() -> Option<PathBuf> {
    let output = Command::new("clang")
        .arg("--print-search-dirs")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let listing = String::from_utf8_lossy(&output.stdout);
    let libraries = listing
        .lines()
        .find_map(|line| line.strip_prefix("libraries: ="))?;
    // The line may list several folders; the first that has the archive is the one.
    libraries
        .split(':')
        .map(|root| Path::new(root).join("lib").join("darwin"))
        .find(|dir| dir.join("libclang_rt.osx.a").is_file())
}

fn embed_windows_manifest() {
    println!("cargo:rerun-if-changed={WINDOWS_MANIFEST}");
    let crate_dir = env_path("CARGO_MANIFEST_DIR");
    let manifest = crate_dir.join(WINDOWS_MANIFEST);
    // Resource 1 of type 24 (RT_MANIFEST) is the manifest Windows reads when it starts an exe. The
    // path is absolute so the resource compiler finds it from any working directory.
    let rc = format!("1 24 \"{}\"\n", rc_string(&manifest));
    let rc_file = env_path("OUT_DIR").join("app-manifest.rc");
    if let Err(error) = fs::write(&rc_file, rc) {
        panic!("could not write {}: {error}", rc_file.display());
    }
    if let Err(result) =
        embed_resource::compile_for_everything(&rc_file, embed_resource::NONE).manifest_required()
    {
        panic!("could not embed the Windows application manifest: {result}");
    }
}

fn env_path(name: &str) -> PathBuf {
    match env::var_os(name) {
        Some(value) => PathBuf::from(value),
        None => panic!("Cargo did not set {name} for the build script"),
    }
}

/// `path` as the body of a resource-script string: backslashes doubled, quotes doubled.
fn rc_string(path: &Path) -> String {
    path.display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\"\"")
}
