//! DTO generation deliberately does not depend on a desktop runtime or platform plugins.
#![allow(clippy::expect_used)]

#[test]
fn export_library_bindings() {
    let types = specta::Types::default()
        .register::<ghost_config::Inventory>()
        .register::<ghost_config::ItemPreview>()
        .register::<ghost_config::CopyPreview>()
        .register::<ghost_config::CopyResult>()
        .register::<ghost_config::CopyReceipt>();
    let generated = specta_typescript::Typescript::default()
        .header("// Generated from ghost-config. Run cargo test -p ghost-config export_library_bindings. Do not edit.")
        .export(&types, specta_serde::Format)
        .expect("Library DTOs export");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/src/lib/library/bindings.ts");
    if std::env::var_os("CI").is_some() {
        assert_eq!(
            std::fs::read_to_string(path).expect("committed bindings"),
            generated,
            "Library DTOs drifted. Run cargo test -p ghost-config export_library_bindings locally."
        );
    } else if std::fs::read_to_string(&path).ok().as_deref() != Some(&generated) {
        std::fs::create_dir_all(path.parent().expect("bindings folder"))
            .expect("create bindings folder");
        std::fs::write(path, generated).expect("write Library DTOs");
    }
}
