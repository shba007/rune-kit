use rune_kit_cli::output::{render_list_table, render_registry_table, render_update_status_table};
use rune_kit_core::{ExecutionKind, InstalledPlugin, Lockfile};
use std::collections::HashMap;

fn sample_lockfile() -> Lockfile {
    let mut plugins = HashMap::new();
    plugins.insert(
        "alpha".to_string(),
        InstalledPlugin {
            name: "alpha".to_string(),
            description: Some("A sample plugin".to_string()),
            version: "1.2.3".to_string(),
            binary_path: "alpha.wasm".to_string(),
            native_binary_path: Some("alpha-bin".to_string()),
            execution_kind: ExecutionKind::Wasm,
            sha256: "abc".to_string(),
            source: "file:///alpha.wasm".to_string(),
            default_params: HashMap::new(),
        },
    );
    Lockfile { version: 1, plugins }
}

#[test]
fn registry_table_header_matches_layout() {
    let rows = render_registry_table(&[], &sample_lockfile()).format_rows();
    assert_eq!(
        rows[0],
        format!("{:<20} {:<10} {:<10} {:<14} {:<30}", "NAME", "CURRENT", "LATEST", "BUILDS", "DESCRIPTION")
    );
}

#[test]
fn update_table_header_matches_layout() {
    let rows = render_update_status_table(&[]).format_rows();
    assert_eq!(
        rows[0],
        format!("{:<16} {:<12} {:<12} {:<14} {:<12}", "PLUGIN", "CURRENT", "LATEST", "STATUS", "BUILDS")
    );
}

#[test]
fn list_table_header_matches_layout() {
    let rows = render_list_table(&sample_lockfile()).format_rows();
    assert_eq!(
        rows[0],
        format!("{:<14} {:<10} {:<8} {:<34} {:<14}", "NAME", "VERSION", "BUILDS", "DESCRIPTION", "SOURCE")
    );
}
