use std::path::PathBuf;
fn fixture() -> PathBuf {
    static FIXTURE: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let directory = FIXTURE.get_or_init(|| {
        let temp = tempfile::tempdir().unwrap();
        let target = temp
            .path()
            .join(format!("native-fixture{}", std::env::consts::EXE_SUFFIX));
        let result = Command::new("rustc")
            .arg(support::root().join("crates/cli/tests/support/native_fixture.rs"))
            .arg("-o")
            .arg(target)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        temp
    });
    directory
        .path()
        .join(format!("native-fixture{}", std::env::consts::EXE_SUFFIX))
}

mod support;
use serde_json::{Value, json};
use std::{fs, process::Command, time::Duration};
use xtask::wire::{Gateway, metadata};

fn env(dir: &std::path::Path) -> Vec<(String, String)> {
    vec![(
        "RUNE_KIT_DATA_DIR".into(),
        dir.to_string_lossy().into_owned(),
    )]
}
#[test]
fn public_stdio_has_only_json_and_preserves_contract_errors_and_cancellation() {
    let dir = tempfile::tempdir().unwrap();
    let data = env(dir.path());
    let data: Vec<_> = data.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let plugin = fixture();
    let mut gateway = Gateway::start(
        &support::binary(),
        &plugin,
        &["--read-only", "--timeout", "1"],
        &data,
    )
    .unwrap();
    let discovered = gateway.request("server/discover", json!({})).unwrap();
    assert_eq!(discovered["result"]["supportedVersions"][0], "2026-07-28");
    let listed = gateway.request("tools/list", json!({})).unwrap();
    let tools = listed["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 2);
    let read = &tools
        .iter()
        .find(|t| t["name"].as_str().unwrap().ends_with("__read"))
        .unwrap()["name"];
    assert_eq!(tools[0]["annotations"]["readOnlyHint"], true);
    assert_eq!(tools[0]["annotations"]["readOnly"], true);
    let schema = tools.iter().find(|t| t["name"] == *read).unwrap();
    assert_eq!(schema["inputSchema"]["x-rune-rules"]["effects"][0], "read");
    let call = gateway
        .request("tools/call", json!({"name":read,"arguments":{}}))
        .unwrap();
    assert_eq!(call["result"]["content"][0]["text"], "fixture response");
    let namespace = read.as_str().unwrap().rsplit_once("__").unwrap().0;
    let forbidden = gateway
        .request(
            "tools/call",
            json!({"name":format!("{namespace}__write"),"arguments":{}}),
        )
        .unwrap();
    assert_eq!(forbidden["error"]["code"], -32600);
    let slow = format!("{namespace}__slow");
    let timeout = gateway
        .request("tools/call", json!({"name":slow,"arguments":{}}))
        .unwrap();
    assert_eq!(timeout["error"]["code"], -32000);
    assert!(
        timeout["error"]["message"]
            .as_str()
            .unwrap()
            .contains("timed out")
    );
    gateway.send(json!({"jsonrpc":"2.0","id":"cancel-me","method":"tools/call","params":{"name":slow,"arguments":{},"_meta":metadata()}})).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    gateway.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"cancel-me"}})).unwrap();
    let pong = gateway.request("ping", json!({})).unwrap();
    assert_eq!(pong["result"]["resultType"], "complete");
    gateway
        .send(json!({"jsonrpc":"2.0","id":"missing","method":"tools/list"}))
        .unwrap();
    assert_eq!(gateway.receive().unwrap()["error"]["code"], -32602);
    let mut wrong = metadata();
    wrong["io.modelcontextprotocol/protocolVersion"] = json!("2025-03-26");
    gateway
        .send(json!({"jsonrpc":"2.0","id":"wrong","method":"tools/list","params":{"_meta":wrong}}))
        .unwrap();
    assert_eq!(gateway.receive().unwrap()["error"]["code"], -32022);
    let configured = r#"{"tools":[{"name":"read","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":true,"readOnly":false}}]}"#;
    let mut data = data;
    data.push(("RUNE_FIXTURE_TOOLS", configured));
    let mut failed = Gateway::start(&support::binary(), &plugin, &[], &data).unwrap();
    let reply = failed.request("tools/list", json!({})).unwrap();
    assert_eq!(reply["error"]["code"], -32000);
    assert!(
        reply["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Tool discovery failed")
    );
}

fn installed(root: &std::path::Path, plugin: &std::path::Path) {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("installed.json"),json!({"version":1,"plugins":{"fixture":{"name":"fixture","version":"0.1.0","binary_path":plugin,"execution_kind":"native","sha256":"fixture","source":"local","default_params":{},"manifest":{"capabilities":{"filesystem":{"mode":"scoped","root_param":"allowed_dir"}},"dependencies":{"binaries":{"missing-helper":{"version":"1","optional":false}}}}}}}).to_string()).unwrap();
}
#[test]
fn tui_plain_inventory_discovers_real_tools_and_reports_missing_payloads() {
    let root = tempfile::tempdir().unwrap();
    installed(root.path(), &fixture());
    let mut command = Command::new(support::binary());
    command
        .args([
            "tui",
            "--json",
            "--read-only",
            "-p",
            "allowed_dir=/explicit/root",
        ])
        .env("RUNE_KIT_DATA_DIR", root.path());
    let result = support::output(&mut command);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let inventory: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(inventory["plugins"][0]["execution"], "Native");
    assert_eq!(
        inventory["plugins"][0]["tools"].as_array().unwrap().len(),
        2
    );
    assert_eq!(inventory["plugins"][0]["filesystem_root"], "/explicit/root");
    assert_eq!(inventory["plugins"][0]["hidden_tools"], 1);
    assert!(
        inventory["plugins"][0]["diagnostics"]
            .to_string()
            .contains("missing-helper")
    );
    installed(root.path(), &root.path().join("missing.exe"));
    let result = support::output(&mut command);
    let missing: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(
        missing["plugins"][0]["state"]
            .as_str()
            .unwrap()
            .contains("failed")
    );
    assert!(
        missing["plugins"][0]["diagnostics"]
            .to_string()
            .contains("reinstall")
    );
}
#[test]
fn tui_empty_and_noninteractive_invocations_do_not_emit_escape_sequences() {
    let root = tempfile::tempdir().unwrap();
    let mut cmd = Command::new(support::binary());
    cmd.arg("tui").env("RUNE_KIT_DATA_DIR", root.path());
    let result = support::output(&mut cmd);
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(String::from_utf8_lossy(&result.stderr).contains("interactive terminal"));
    cmd.arg("--once");
    let result = support::output(&mut cmd);
    assert!(result.status.success());
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(text.contains("No plugins installed"));
    assert!(!text.contains('\u{1b}'));
}

#[test]
fn interactive_dashboard_restores_terminal_after_empty_and_failed_discovery() {
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    use std::io::{Read, Write};
    for missing in [false, true] {
        let root = tempfile::tempdir().unwrap();
        if missing {
            installed(root.path(), &root.path().join("unavailable-plugin.exe"));
        }
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: 28,
                cols: 110,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut command = CommandBuilder::new(support::binary());
        command.arg("tui");
        command.env("RUNE_KIT_DATA_DIR", root.path());
        command.env("TERM", "xterm-256color");
        let mut child = pty.slave.spawn_command(command).unwrap();
        drop(pty.slave);
        let mut reader = pty.master.try_clone_reader().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut bytes = [0; 4096];
            loop {
                match reader.read(&mut bytes) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(bytes[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let mut writer = pty.master.take_writer().unwrap();
        let mut evidence = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        let expected = if missing {
            "Discovery failed"
        } else {
            "No plugins installed"
        };
        while std::time::Instant::now() < deadline {
            if let Ok(bytes) = rx.recv_timeout(Duration::from_millis(100)) {
                evidence.extend(bytes);
                // A PTY is a transport, not a terminal emulator. Answer DSR,
                // which a real terminal answers during Crossterm initialization.
                if evidence.ends_with(b"\x1b[6n") {
                    writer.write_all(b"\x1b[1;1R").unwrap();
                    writer.flush().unwrap();
                }
            }
            if String::from_utf8_lossy(&evidence).contains(expected) {
                break;
            }
        }
        writer.write_all(b"q").unwrap();
        writer.flush().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if std::time::Instant::now() > deadline {
                let _ = child.kill();
                fs::write(support::root().join("target/tui-debug.txt"), &evidence).unwrap();
                panic!(
                    "TUI did not exit on q: {}",
                    String::from_utf8_lossy(&evidence)
                );
            }
            if let Ok(bytes) = rx.recv_timeout(Duration::from_millis(50)) {
                evidence.extend(bytes);
            }
        };
        drop(writer);
        drop(pty.master);
        while let Ok(bytes) = rx.recv_timeout(Duration::from_millis(100)) {
            evidence.extend(bytes);
        }
        let screen = String::from_utf8_lossy(&evidence);
        assert!(status.success(), "TUI process failed: {screen}");
        assert!(
            screen.contains(expected),
            "Missing state {expected}: {screen}"
        );
        assert!(
            screen.contains("\u{1b}[?1049h"),
            "Alternate screen not entered: {screen}"
        );
        assert!(
            screen.contains("\u{1b}[?1049l"),
            "Alternate screen not restored: {screen}"
        );
        let destination = support::root().join("target").join(if missing {
            "tui-error-terminal.txt"
        } else {
            "tui-empty-terminal.txt"
        });
        fs::write(destination, evidence).unwrap();
    }
}

#[test]
fn dashboard_native_discovery_timeout_is_actionable() {
    let root = tempfile::tempdir().unwrap();
    installed(root.path(), &fixture());
    let start = std::time::Instant::now();
    let result = support::output(
        Command::new(support::binary())
            .args(["tui", "--json"])
            .env("RUNE_KIT_DATA_DIR", root.path())
            .env("KIT_FIXTURE_HANG", "1"),
    );
    assert!(result.status.success());
    let inventory: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(
        inventory["plugins"][0]["diagnostics"]
            .to_string()
            .contains("timed out")
    );
    assert!(start.elapsed() < Duration::from_secs(12));
}
