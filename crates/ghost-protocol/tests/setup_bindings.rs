#![allow(clippy::expect_used)]

#[test]
fn export_setup_action_bindings() {
    let types = specta::Types::default()
        .register::<ghost_protocol::setup_actions::AgentInstallReview>()
        .register::<ghost_protocol::setup_actions::CloneInput>()
        .register::<ghost_protocol::setup_actions::CloneCheck>()
        .register::<ghost_protocol::setup_actions::CloneReview>()
        .register::<ghost_protocol::setup_actions::CloneRecord>()
        .register::<ghost_protocol::setup_actions::CloneRecovery>();
    let generated = specta_typescript::Typescript::default()
        .header("// Generated from ghost-protocol. Run cargo test -p ghost-protocol export_setup_action_bindings. Do not edit.")
        .export(&types, specta_serde::Format).expect("setup action DTOs export");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/src/lib/setup-actions/bindings.ts");
    if std::env::var_os("CI").is_some() {
        assert_eq!(
            std::fs::read_to_string(path).expect("committed DTOs"),
            generated
        );
    } else if std::fs::read_to_string(&path).ok().as_deref() != Some(&generated) {
        std::fs::create_dir_all(path.parent().expect("DTO folder")).expect("create DTO folder");
        std::fs::write(path, generated).expect("write setup action DTOs");
    }
}
