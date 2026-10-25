mod support;
use serde_json::{Value, json};
use std::{fs, process::Command};

fn command(root: &std::path::Path, args: &[&str]) -> std::process::Output {
    support::output(
        Command::new(support::binary())
            .current_dir(support::root())
            .env("RUNE_KIT_DATA_DIR", root)
            .env_remove("RUNE_KIT_HTTP_TOKEN")
            .args(args),
    )
}
#[test]
fn registry_preserves_entries_redacts_query_and_blocks_workspace_mode() {
    let root = tempfile::tempdir().unwrap();
    let added = command(
        root.path(),
        &[
            "remote",
            "add",
            "external",
            "http://127.0.0.1:1/mcp?key=private",
            "--token-env",
            "EXTERNAL_TOKEN",
        ],
    );
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let path = root.path().join("remotes.json");
    let original = fs::read(&path).unwrap();
    let saved: Value = serde_json::from_slice(&original).unwrap();
    assert_eq!(saved["servers"]["external"]["token_env"], "EXTERNAL_TOKEN");
    let listed = command(root.path(), &["--json", "remote", "list"]);
    assert!(listed.status.success());
    assert!(!String::from_utf8_lossy(&listed.stdout).contains("private"));
    assert!(
        !command(
            root.path(),
            &["remote", "add", "external", "http://127.0.0.1:2/mcp"]
        )
        .status
        .success()
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(
        command(
            root.path(),
            &[
                "remote",
                "add",
                "second",
                "http://127.0.0.1:2/mcp",
                "--protocol",
                "2025-11-25"
            ]
        )
        .status
        .success()
    );
    let blocked = command(
        root.path(),
        &["serve", "external", "--headless", "--workspace-only"],
    );
    assert!(!blocked.status.success());
    assert!(String::from_utf8_lossy(&blocked.stderr).contains("cannot enforce"));
    assert!(
        command(root.path(), &["remote", "remove", "external"])
            .status
            .success()
    );
    let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["servers"].as_object().unwrap().len(), 1);
    assert_eq!(saved["servers"]["second"]["protocol"], "2025-11-25");
}

#[test]
fn invalid_corrupt_and_colliding_configurations_are_never_overwritten() {
    let root = tempfile::tempdir().unwrap();
    for args in [
        vec!["remote", "add", "bad__name", "http://localhost/mcp"],
        vec!["remote", "add", "valid", "http://user:secret@localhost/mcp"],
        vec![
            "remote",
            "add",
            "valid",
            "https://example.com/mcp",
            "--protocol",
            "invalid",
        ],
    ] {
        assert!(!command(root.path(), &args).status.success());
        assert!(!root.path().join("remotes.json").exists());
    }
    let path = root.path().join("remotes.json");
    fs::write(&path, b"invalid").unwrap();
    assert!(
        !command(
            root.path(),
            &["remote", "add", "valid", "http://localhost/mcp"]
        )
        .status
        .success()
    );
    assert_eq!(fs::read(&path).unwrap(), b"invalid");
    fs::write(&path, json!({"schema_version":1,"servers":{}}).to_string()).unwrap();
    fs::write(root.path().join("installed.json"),json!({"version":1,"plugins":{"valid":{"name":"valid","version":"1.0.0","binary_path":"fixture.wasm","execution_kind":"wasm","sha256":"fixture","source":"local","default_params":{}}}}).to_string()).unwrap();
    let result = command(
        root.path(),
        &["remote", "add", "valid", "http://localhost/mcp"],
    );
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("Namespace"));
}
