use rune_kit_cli::output::{render_list_table, render_update_status_table};
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
            manifest: None,
        },
    );
    Lockfile {
        version: 1,
        plugins,
    }
}

#[test]
fn list_table_header_matches_layout() {
    let rows = render_list_table(&sample_lockfile()).format_rows();
    assert_eq!(
        rows[0],
        format!(
            "{:<14} {:<10} {:<8} {:<34} {:<14}",
            "NAME", "VERSION", "BUILDS", "DESCRIPTION", "SOURCE"
        )
    );
}

#[test]
fn update_table_header_matches_layout() {
    let rows = render_update_status_table(&[]).format_rows();
    assert_eq!(
        rows[0],
        format!(
            "{:<16} {:<12} {:<12} {:<14} {:<12}",
            "PLUGIN", "CURRENT", "LATEST", "STATUS", "BUILDS"
        )
    );
}

#[test]
fn test_cli_binary_version() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rune"))
        .arg("--version")
        .output()
        .expect("Failed to execute rune binary");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("rune "));
}

#[test]
fn test_cli_list_json() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rune"))
        .args(["list", "--json"])
        .output()
        .expect("Failed to execute rune list --json");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("Output must be valid JSON");
    assert!(parsed.get("plugins").is_some());
    assert!(parsed.get("skills").is_some());
    assert!(parsed["plugins"].is_array());
    assert!(parsed["skills"].is_array());
}

#[test]
fn test_cli_global_json_flag() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rune"))
        .args(["--json", "list"])
        .output()
        .expect("Failed to execute rune --json list");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("Output must be valid JSON");
    assert!(parsed.get("plugins").is_some());
}

#[test]
fn test_cli_search_json() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rune"))
        .args(["search", "nonexistent_query_xyz", "--json"])
        .output()
        .expect("Failed to execute rune search --json");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("Output must be valid JSON");
    assert_eq!(parsed["keyword"], "nonexistent_query_xyz");
    assert!(parsed["results"].is_array());
    assert!(parsed["results"].as_array().unwrap().is_empty());
}

#[test]
fn test_cli_run_help_flags() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rune"))
        .args(["run", "--help"])
        .output()
        .expect("Failed to execute rune run --help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--read-only"));
    assert!(stdout.contains("--allow-tool"));
    assert!(stdout.contains("--timeout"));
}
