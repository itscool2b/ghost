#![allow(clippy::expect_used)]
#[test]
fn export_config_vendor_bindings() {
    let types = specta::Types::default().register::<ghost_protocol::config_vendor::VendorReview>();
    let generated = specta_typescript::Typescript::default()
        .header("// Generated from ghost-protocol. Run cargo test -p ghost-protocol export_config_vendor_bindings. Do not edit.")
        .export(&types, specta_serde::Format).expect("vendor DTO export");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/src/lib/config-vendor/bindings.ts");
    if std::env::var_os("CI").is_some() {
        assert_eq!(
            std::fs::read_to_string(path).expect("committed vendor DTOs"),
            generated
        );
    } else if std::fs::read_to_string(&path).ok().as_deref() != Some(&generated) {
        std::fs::create_dir_all(path.parent().expect("DTO folder")).expect("DTO folder");
        std::fs::write(path, generated).expect("write vendor DTOs");
    }
}
