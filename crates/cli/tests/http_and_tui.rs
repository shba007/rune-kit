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

#[test]
fn workspace_only_mode_serves_wasm_without_mounting_approved_external_paths() {
    let root = tempfile::tempdir().unwrap();
    let wasm = root.path().join("minimal.wasm");
    fs::write(&wasm, b"\0asm\x01\0\0\0").unwrap();
    let original = root.path().join("original.txt");
    fs::write(&original, b"retained original").unwrap();
    let mut sandbox = rune_kit_core::sandbox::Sandbox::open(root.path().join("sandbox")).unwrap();
    sandbox.approve(wasm.to_str().unwrap(), &original).unwrap();
    let data = env(root.path());
    let data: Vec<_> = data.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let mut gateway =
        Gateway::start(&support::binary(), &wasm, &["--workspace-only"], &data).unwrap();
    let reply = gateway.request("ping", json!({})).unwrap();
    assert!(reply.get("error").is_none(), "{reply}");
    assert_eq!(fs::read(&original).unwrap(), b"retained original");
    let workspace = sandbox.workspace(wasm.to_str().unwrap()).unwrap();
    assert_eq!(fs::read_dir(workspace).unwrap().count(), 0);
}

#[test]
fn workspace_only_mode_rejects_native_plugins_without_starting_a_server() {
    let root = tempfile::tempdir().unwrap();
    installed(root.path(), &fixture());
    for target in [None, Some(fixture())] {
        let mut command = Command::new(support::binary());
        command.args(["serve", "--workspace-only", "--headless", "--port", "0"]);
        if let Some(path) = target {
            command.arg(path);
        }
        command.env("RUNE_KIT_DATA_DIR", root.path());
        let output = support::output(&mut command);
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success());
        assert!(
            error.contains("Workspace-only mode blocks native plugins"),
            "{error}"
        );
        assert!(!error.contains("Kit HTTP listening"));
    }
    let output = support::output(Command::new(support::binary()).args(["serve", "--help"]));
    assert!(String::from_utf8_lossy(&output.stdout).contains("--workspace-only"));
}

#[test]
fn dashboard_configuration_applies_to_live_native_instance_and_preserves_failed_edits() {
    use rune_kit_cli::configuration::{Configuration, Plugin};
    use rune_kit_core::{ExecutionKind, McpRouter, PluginManifest};
    use std::sync::{Arc, Mutex};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("dashboard.json");
    let manifest: PluginManifest = serde_json::from_value(serde_json::json!({"configuration": {
        "schema_version":1,"fields": {
            "tools_payload":{"type":"string","description":"Test tool inventory","required":true,"secret":false,"env":["RUNE_FIXTURE_TOOLS"]},
            "api_key":{"type":"string","description":"Test credential","required":false,"secret":true,"env":["FIXTURE_SECRET"]},
            "allowed_dir":{"type":"string","description":"Filesystem root","required":false,"secret":false,"env":["ALLOWED_DIR"]}
        }
    }})).unwrap();
    let plugin = Plugin {
        name: "fixture".into(),
        path: fixture(),
        kind: ExecutionKind::Native,
        manifest,
        params: Default::default(),
        workspace: None,
    };
    let mut config = Configuration::read(path.clone()).unwrap();
    let router = Arc::new(Mutex::new(McpRouter::new()));
    router
        .lock()
        .unwrap()
        .register("fixture".into(), plugin.load(config.settings()).unwrap());
    config.plugins.push(plugin);
    let field = config
        .fields()
        .into_iter()
        .find(|f| f.key == "tools_payload")
        .unwrap();
    let payload =
        serde_json::json!({"tools":[{"name":"configured","description":"Configured fixture","inputSchema":{"type":"object"}}]})
            .to_string();
    config
        .apply(&router, &field, payload.clone(), false)
        .unwrap();
    let reply = router
        .lock()
        .unwrap()
        .handle_jsonrpc(&rpc("tools/list", serde_json::json!(1), serde_json::json!({})).to_string())
        .unwrap();
    assert!(reply.to_string().contains("fixture__configured"), "{reply}");
    let saved = std::fs::read(&path).unwrap();
    let stage = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&stage, b"retained staging file").unwrap();
    assert!(
        config
            .apply(
                &router,
                &field,
                payload.replace("configured", "should_not_apply"),
                false
            )
            .is_err()
    );
    assert_eq!(std::fs::read(&stage).unwrap(), b"retained staging file");
    std::fs::remove_file(stage).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), saved);
    let reply = router
        .lock()
        .unwrap()
        .handle_jsonrpc(&rpc("tools/list", serde_json::json!(2), serde_json::json!({})).to_string())
        .unwrap();
    assert!(reply.to_string().contains("fixture__configured"));
    assert!(!reply.to_string().contains("should_not_apply"));
    let secret = config
        .fields()
        .into_iter()
        .find(|f| f.key == "api_key")
        .unwrap();
    config
        .apply(&router, &secret, "private-test-credential".into(), false)
        .unwrap();
    assert!(
        !String::from_utf8(std::fs::read(&path).unwrap())
            .unwrap()
            .contains("private-test-credential")
    );
    assert!(
        config
            .fields()
            .into_iter()
            .find(|f| f.key == "api_key")
            .unwrap()
            .value
            .is_empty()
    );
    let fs_field = config
        .fields()
        .into_iter()
        .find(|f| f.key == "allowed_dir")
        .unwrap();
    assert!(
        config
            .apply(&router, &fs_field, "/external".into(), false)
            .is_err()
    );
    let guard = router.lock().unwrap();
    assert!(config.apply(&router, &field, "busy".into(), false).is_err());
    drop(guard);
    std::fs::write(&path, b"external edit").unwrap();
    assert!(config.apply(&router, &field, payload, false).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"external edit");
    std::fs::write(&path, saved).unwrap();
    let restored = Configuration::read(path).unwrap();
    assert!(restored.settings().plugins["fixture"].contains_key("tools_payload"));
    assert!(!restored.settings().plugins["fixture"].contains_key("api_key"));
}
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
fn public_http_preserves_contract_errors_and_stream_cancellation() {
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
    gateway.cancel();
    let pong = gateway.request("ping", json!({})).unwrap();
    assert_eq!(pong["result"]["resultType"], "complete");
    gateway
        .send(json!({"jsonrpc":"2.0","id":"missing","method":"tools/list"}))
        .unwrap();
    assert_eq!(gateway.receive().unwrap()["error"]["code"], -32020);
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
fn serve_dashboard_discovers_tools_and_restores_terminal() {
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    use std::io::{Read, Write};
    for with_plugin in [false, true] {
        let attached = true;
        let missing = false;
        let root = tempfile::tempdir().unwrap();
        if with_plugin {
            installed(root.path(), &fixture());
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
        command.args(["serve", "--port", "0"]);
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
        let expected = if attached {
            "Rune MCP Engine & Package Manager"
        } else if missing {
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
        if attached {
            let text = String::from_utf8_lossy(&evidence);
            let start = text
                .find("http://127.0.0.1:")
                .expect("Attached endpoint missing");
            let end = start + text[start..].find("/mcp").unwrap() + 4;
            let endpoint = &text[start..end];
            let client = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap();
            let result = http(
                &client,
                endpoint,
                &rpc("ping", json!("dashboard-check"), json!({})),
            )
            .send()
            .unwrap();
            assert_eq!(result.status(), 200);
            assert!(result.text().unwrap().contains("dashboard-check"));
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while std::time::Instant::now() < deadline
                && !String::from_utf8_lossy(&evidence).contains("ping")
            {
                if let Ok(bytes) = rx.recv_timeout(Duration::from_millis(100)) {
                    evidence.extend(bytes);
                }
            }
            assert!(
                String::from_utf8_lossy(&evidence).contains("ping"),
                "Server call was not shown in attached TUI"
            );
        }
        writer.write_all(b"d\t").unwrap();
        writer.flush().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        let expected_tools = if with_plugin { "fixture__" } else { "Tools" };
        while std::time::Instant::now() < deadline
            && !String::from_utf8_lossy(&evidence).contains(expected_tools)
        {
            if let Ok(bytes) = rx.recv_timeout(Duration::from_millis(100)) {
                evidence.extend(bytes);
            }
        }
        assert!(
            String::from_utf8_lossy(&evidence).contains(expected_tools),
            "Discovery was not shown: {}",
            String::from_utf8_lossy(&evidence)
        );
        writer.write_all(b"s").unwrap();
        writer.flush().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline
            && !String::from_utf8_lossy(&evidence).contains("NOT ACTIVE")
        {
            if let Ok(bytes) = rx.recv_timeout(Duration::from_millis(100)) {
                evidence.extend(bytes);
            }
        }
        assert!(
            String::from_utf8_lossy(&evidence).contains("NOT ACTIVE"),
            "Sandbox disclosure missing"
        );
        assert!(root.path().join("sandbox").is_dir());
        assert!(!root.path().join("sandbox/approvals.json").exists());
        writer.write_all(b"qi").unwrap();
        writer.flush().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline
            && !String::from_utf8_lossy(&evidence).contains("Executable:")
        {
            if let Ok(bytes) = rx.recv_timeout(Duration::from_millis(100)) {
                evidence.extend(bytes);
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
        if attached {
            assert!(
                screen.contains("Executable:"),
                "Executable path missing: {screen}"
            );
            assert!(
                screen.contains("Data:"),
                "Resolved data path missing: {screen}"
            );
        }
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
        let destination = support::root().join("target").join(if attached {
            "tui-http-terminal.txt"
        } else if missing {
            "tui-error-terminal.txt"
        } else {
            "tui-empty-terminal.txt"
        });
        fs::write(destination, evidence).unwrap();
    }
}

fn rpc(method: &str, id: Value, params: Value) -> Value {
    let mut params = params;
    params["_meta"] = metadata();
    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
}
fn http(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    value: &Value,
) -> reqwest::blocking::RequestBuilder {
    let mut request = client
        .post(endpoint)
        .header("Accept", "application/json, text/event-stream")
        .header(
            "MCP-Protocol-Version",
            value["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"]
                .as_str()
                .unwrap_or("2026-07-28"),
        )
        .header("Mcp-Method", value["method"].as_str().unwrap_or(""));
    if let Some(name) = value["params"]["name"]
        .as_str()
        .or_else(|| value["params"]["uri"].as_str())
    {
        request = request.header("Mcp-Name", name);
    }
    request.json(value)
}
#[test]
fn http_security_negotiation_and_protocol_validation() {
    let root = tempfile::tempdir().unwrap();
    let plugin = fixture();
    let data = env(root.path());
    let data: Vec<_> = data.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let gateway = Gateway::start(&support::binary(), &plugin, &["--read-only"], &data).unwrap();
    let client = reqwest::blocking::Client::new();
    let ping = rpc("ping", json!(1), json!({}));
    for method in [reqwest::Method::GET, reqwest::Method::DELETE] {
        assert_eq!(
            client
                .request(method, &gateway.endpoint)
                .send()
                .unwrap()
                .status(),
            405
        );
    }
    assert_eq!(
        client
            .get(&gateway.endpoint)
            .header("Origin", "https://evil.example")
            .send()
            .unwrap()
            .status(),
        403
    );
    for (header, value, expected) in [
        ("Origin", "https://evil.example", 403),
        ("Host", "evil.example", 403),
        ("Accept", "application/json", 406),
        ("Mcp-Method", "tools/call", 400),
        ("MCP-Protocol-Version", "2025-03-26", 400),
    ] {
        let mut built = http(&client, &gateway.endpoint, &ping).build().unwrap();
        built.headers_mut().insert(
            reqwest::header::HeaderName::from_bytes(header.as_bytes()).unwrap(),
            value.parse().unwrap(),
        );
        let response = client.execute(built).unwrap();
        assert_eq!(response.status(), expected, "{header}");
        let error: Value = response.json().unwrap();
        if expected == 400 {
            assert_eq!(error["error"]["code"], -32020);
        }
    }
    let unknown = rpc("initialize", json!(1), json!({}));
    let response = http(&client, &gateway.endpoint, &unknown).send().unwrap();
    assert_eq!(response.status(), 404);
    assert_eq!(response.json::<Value>().unwrap()["error"]["code"], -32601);
    let invalid = client
        .post(&gateway.endpoint)
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json")
        .body("[")
        .send()
        .unwrap();
    assert_eq!(invalid.status(), 400);
    assert_eq!(invalid.json::<Value>().unwrap()["error"]["code"], -32700);
    let response = http(&client, &gateway.endpoint, &ping)
        .header("Mcp-Session-Id", "ignored")
        .header("Last-Event-ID", "ignored")
        .send()
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.headers().get("mcp-session-id").is_none());
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    assert!(
        response
            .text()
            .unwrap()
            .contains("\"resultType\":\"complete\"")
    );
    let notification =
        json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}});
    assert_eq!(
        http(&client, &gateway.endpoint, &notification)
            .send()
            .unwrap()
            .status(),
        400
    );
    let oversized = client
        .post(&gateway.endpoint)
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json")
        .body(" ".repeat(256 * 1024 + 1))
        .send()
        .unwrap();
    assert_eq!(oversized.status(), 413);
}
#[test]
fn server_permissions_are_ceilings_and_reused_ids_are_isolated() {
    let root = tempfile::tempdir().unwrap();
    let plugin = fixture();
    let data = env(root.path());
    let data: Vec<_> = data.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let mut gateway = Gateway::start(
        &support::binary(),
        &plugin,
        &["--read-only", "--timeout", "5"],
        &data,
    )
    .unwrap();
    let listing = gateway.request("tools/list", json!({})).unwrap();
    let tools = listing["result"]["tools"].as_array().unwrap();
    let read = tools
        .iter()
        .find(|t| t["name"].as_str().unwrap().ends_with("__read"))
        .unwrap()["name"]
        .clone();
    let slow = tools
        .iter()
        .find(|t| t["name"].as_str().unwrap().ends_with("__slow"))
        .unwrap()["name"]
        .clone();
    let mut selected = rpc("tools/list", json!("same"), json!({}));
    selected["params"]["_meta"]["io.rune-kit/allowedTools"] = json!([read]);
    gateway.send(selected).unwrap();
    let list = gateway.receive().unwrap();
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 1);
    let mut refused = rpc(
        "tools/call",
        json!("same"),
        json!({"name":read,"arguments":{}}),
    );
    refused["params"]["_meta"]["io.rune-kit/allowedTools"] = json!([]);
    gateway.send(refused).unwrap();
    assert_eq!(gateway.receive().unwrap()["error"]["code"], -32600);
    gateway
        .send(rpc(
            "tools/call",
            json!("same"),
            json!({"name":slow,"arguments":{}}),
        ))
        .unwrap();
    let endpoint = gateway.endpoint.clone();
    let other = std::thread::spawn(move || {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        http(
            &client,
            &endpoint,
            &rpc(
                "tools/call",
                json!("same"),
                json!({"name":read,"arguments":{}}),
            ),
        )
        .send()
        .unwrap()
        .text()
        .unwrap()
    });
    std::thread::sleep(Duration::from_millis(150));
    gateway.cancel();
    let result = other.join().unwrap();
    assert!(result.contains("fixture response"), "{result}");
    assert!(!result.contains("-32800"));
}
#[test]
fn occupied_port_non_loopback_and_legacy_transport_are_actionable() {
    let root = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port().to_string();
    for (args, message) in [
        (
            vec!["serve", "--headless", "--port", port.as_str()],
            "Cannot bind",
        ),
        (
            vec!["serve", "--headless", "--bind", "0.0.0.0"],
            "Only loopback",
        ),
        (vec!["run", "--all"], "Public stdio serving was removed"),
    ] {
        let mut command = Command::new(support::binary());
        command.args(args).env("RUNE_KIT_DATA_DIR", root.path());
        let output = support::output(&mut command);
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(message),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!output.stdout.contains(&27));
    }
}

#[test]
fn mirrored_headers_and_optional_local_auth_are_enforced_before_tool_execution() {
    let root = tempfile::tempdir().unwrap();
    let plugin = fixture();
    let data_root = root.path().to_str().unwrap();
    let tools = r#"{"tools":[{"name":"read","description":"Header fixture","inputSchema":{"type":"object","properties":{"tenant":{"type":"string","x-mcp-header":"Tenant"}},"required":["tenant"]},"annotations":{"readOnlyHint":true}}]}"#;
    let mut gateway = Gateway::start(
        &support::binary(),
        &plugin,
        &["--read-only"],
        &[
            ("RUNE_KIT_DATA_DIR", data_root),
            ("RUNE_FIXTURE_TOOLS", tools),
        ],
    )
    .unwrap();
    let listed = gateway.request("tools/list", json!({})).unwrap();
    assert!(listed.get("error").is_none(), "{listed}");
    let name = &listed["result"]["tools"][0]["name"];
    let call = rpc(
        "tools/call",
        json!("header"),
        json!({"name":name,"arguments":{"tenant":"sample"}}),
    );
    let client = reqwest::blocking::Client::new();
    let rejected = http(&client, &gateway.endpoint, &call).send().unwrap();
    assert_eq!(rejected.status(), 400);
    assert_eq!(rejected.json::<Value>().unwrap()["error"]["code"], -32020);
    let accepted = http(&client, &gateway.endpoint, &call)
        .header("Mcp-Param-Tenant", "sample")
        .send()
        .unwrap();
    assert_eq!(accepted.status(), 200);
    assert!(accepted.text().unwrap().contains("fixture response"));
    let protected = Gateway::start(
        &support::binary(),
        &plugin,
        &[],
        &[
            ("RUNE_KIT_DATA_DIR", data_root),
            ("RUNE_KIT_HTTP_TOKEN", "test-only-token"),
        ],
    )
    .unwrap();
    let ping = rpc("ping", json!(1), json!({}));
    assert_eq!(
        http(&client, &protected.endpoint, &ping)
            .send()
            .unwrap()
            .status(),
        401
    );
    let authenticated = http(&client, &protected.endpoint, &ping)
        .bearer_auth("test-only-token")
        .send()
        .unwrap();
    assert_eq!(authenticated.status(), 200);
    assert!(!authenticated.text().unwrap().contains("test-only-token"));
}

#[test]
fn studio_http_fixtures_match_real_gateway_responses() {
    let root = tempfile::tempdir().unwrap();
    let plugin = fixture();
    let data = env(root.path());
    let data: Vec<_> = data.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let gateway = Gateway::start(&support::binary(), &plugin, &["--read-only"], &data).unwrap();
    let fixtures: Value =
        serde_json::from_str(include_str!("../../../fixtures/mcp-http-2026-07-28.json")).unwrap();
    let client = reqwest::blocking::Client::new();
    for example in fixtures["examples"].as_array().unwrap() {
        if example["body"].to_string().contains("{{") {
            continue;
        }
        let mut request = client.post(&gateway.endpoint);
        for (name, value) in example["headers"].as_object().unwrap() {
            request = request.header(name, value.as_str().unwrap());
        }
        let response = request.json(&example["body"]).send().unwrap();
        assert_eq!(
            u64::from(response.status().as_u16()),
            example["expected"]["http"].as_u64().unwrap(),
            "{}",
            example["name"]
        );
        if let Some(code) = example["expected"].get("errorCode") {
            assert_eq!(response.json::<Value>().unwrap()["error"]["code"], *code);
        } else {
            assert!(
                response
                    .text()
                    .unwrap()
                    .contains("\"resultType\":\"complete\"")
            );
        }
    }
}
#[test]
fn client_selection_cannot_expand_server_allowlist() {
    let root = tempfile::tempdir().unwrap();
    let plugin = fixture();
    let data = env(root.path());
    let data: Vec<_> = data.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let read = format!("{}__read", plugin.display());
    let write = format!("{}__write", plugin.display());
    let mut gateway =
        Gateway::start(&support::binary(), &plugin, &["--allow-tool", &read], &data).unwrap();
    let mut request = rpc("tools/list", json!(1), json!({}));
    request["params"]["_meta"]["io.rune-kit/allowedTools"] = json!([write]);
    gateway.send(request).unwrap();
    assert_eq!(gateway.receive().unwrap()["result"]["tools"], json!([]));
    let mut request = rpc("tools/call", json!(1), json!({"name":write,"arguments":{}}));
    request["params"]["_meta"]["io.rune-kit/allowedTools"] = json!([write]);
    gateway.send(request).unwrap();
    assert_eq!(gateway.receive().unwrap()["error"]["code"], -32600);
    assert!(
        gateway
            .request("tools/call", json!({"name":read,"arguments":{}}))
            .unwrap()["result"]
            .is_object()
    );
}

#[test]
fn noninteractive_serve_remains_available_after_stdin_is_closed() {
    use std::io::{BufRead, BufReader, Read};
    use std::process::Stdio;
    let root = tempfile::tempdir().unwrap();
    let mut command = Command::new(support::binary());
    command
        .args(["serve", "--port", "0"])
        .env("RUNE_KIT_DATA_DIR", root.path())
        .env_remove("RUNE_KIT_HTTP_TOKEN")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().unwrap();
    let mut line = String::new();
    BufReader::new(child.stderr.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let endpoint = line
        .strip_prefix("Kit HTTP listening at ")
        .expect("Server did not start")
        .split(';')
        .next()
        .unwrap();
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let response = http(
        &client,
        endpoint,
        &rpc("server/discover", json!(1), json!({})),
    )
    .send()
    .unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.text().unwrap().contains("supportedVersions"));
    child.kill().unwrap();
    child.wait().unwrap();
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    assert!(stdout.is_empty());
    fs::write(root.path().join("installed.json"), "not JSON").unwrap();
    let mut broken = Command::new(support::binary());
    broken
        .args(["serve", "--headless"])
        .env("RUNE_KIT_DATA_DIR", root.path());
    let result = support::output(&mut broken);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("Cannot parse"));
}

#[test]
fn help_exposes_serve_without_standalone_tui() {
    let result = support::output(Command::new(support::binary()).arg("--help"));
    assert!(result.status.success());
    let help = String::from_utf8(result.stdout).unwrap();
    assert!(help.contains("serve"));
    assert!(
        !help
            .lines()
            .any(|line| line.trim_start().starts_with("tui "))
    );
    let removed = support::output(Command::new(support::binary()).arg("tui"));
    assert!(!removed.status.success());
}
