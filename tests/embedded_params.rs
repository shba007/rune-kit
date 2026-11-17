use rune_kit::{
    ExecutionKind, McpRouter, PluginManifest,
    configuration::{Configuration, Plugin},
    server::{Options, start_embedded},
};
use serde_json::{Value, json};
use std::collections::HashMap;
fn export(name: &str, value: &str) -> String {
    let stores: String = value
        .bytes()
        .enumerate()
        .map(|(i, b)| {
            format!("(call $store (i64.add (local.get $ptr) (i64.const {i})) (i32.const {b}))")
        })
        .collect();
    format!(
        "(func (export \"{name}\") (result i32) (local $ptr i64) (local.set $ptr (call $alloc (i64.const {}))) {stores} (call $output (local.get $ptr) (i64.const {})) i32.const 0)",
        value.len(),
        value.len()
    )
}
fn fixture() -> Vec<u8> {
    let list = json!([{"name":"read","description":"Read fixture","inputSchema":{"type":"object","properties":{},"additionalProperties":false}}]);
    let prefix = r#"{"content":[{"type":"text","text":""#;
    let suffix = r#""}]}"#;
    let stores: String = prefix
        .bytes()
        .chain(std::iter::once(b'?'))
        .chain(suffix.bytes())
        .enumerate()
        .map(|(i, b)| {
            format!("(call $store (i64.add (local.get $ptr) (i64.const {i})) (i32.const {b}))")
        })
        .collect();
    wat::parse_str(format!(r#"(module
    (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
    (import "extism:host/env" "store_u8" (func $store (param i64 i32)))
    (import "extism:host/env" "output_set" (func $output (param i64 i64)))
    (import "wasi_snapshot_preview1" "path_open" (func $open (param i32 i32 i32 i32 i32 i64 i64 i32 i32) (result i32)))
    (import "wasi_snapshot_preview1" "fd_read" (func $read (param i32 i32 i32 i32) (result i32)))
    (import "wasi_snapshot_preview1" "fd_close" (func $close (param i32) (result i32)))
    (memory (export "memory") 1)
    (data (i32.const 100) "inside.txt") (data (i32.const 160) "../outside.txt")
    {list_export}
    (func (export "mcp_call_tool") (result i32) (local $ptr i64)
      ;; The sibling file outside the root must never be readable.
      i32.const 3 i32.const 0 i32.const 160 i32.const 14 i32.const 0
      i64.const 2 i64.const 0 i32.const 0 i32.const 0 call $open i32.eqz if unreachable end
      i32.const 3 i32.const 0 i32.const 100 i32.const 10 i32.const 0
      i64.const 2 i64.const 0 i32.const 0 i32.const 0 call $open if unreachable end
      i32.const 64 i32.const 512 i32.store i32.const 68 i32.const 1 i32.store
      i32.const 0 i32.load i32.const 64 i32.const 1 i32.const 8 call $read if unreachable end
      i32.const 8 i32.load i32.const 1 i32.ne if unreachable end
      i32.const 0 i32.load call $close drop
      (local.set $ptr (call $alloc (i64.const {length}))) {stores}
      (call $store (i64.add (local.get $ptr) (i64.const {offset})) (i32.load8_u (i32.const 512)))
      (call $output (local.get $ptr) (i64.const {length})) i32.const 0))"#,list_export=export("mcp_list_tools",&list.to_string()),length=prefix.len()+1+suffix.len(),offset=prefix.len())).unwrap()
}
async fn rpc(client: &reqwest::Client, endpoint: &str, method: &str, mut params: Value) -> Value {
    params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}});
    let mut request = client
        .post(endpoint)
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", method);
    if let Some(name) = params["name"].as_str() {
        request = request.header("Mcp-Name", name);
    }
    let body = request
        .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let data = body
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .unwrap_or(&body);
    serde_json::from_str(data).unwrap()
}
#[tokio::test(flavor = "multi_thread")]
async fn embedded_folder_updates_are_atomic_live_and_host_only() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first");
    let second = dir.path().join("second");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    std::fs::create_dir(first.join("nested")).unwrap();
    std::fs::write(first.join("inside.txt"), "A").unwrap();
    std::fs::write(second.join("inside.txt"), "B").unwrap();
    std::fs::write(dir.path().join("outside.txt"), "outside").unwrap();
    let payload = dir.path().join("fixture.wasm");
    std::fs::write(&payload, fixture()).unwrap();
    let path = dir.path().join("dashboard.json");
    std::fs::write(&path, br#"{"shared":{"retained":"yes"},"plugins":{}}"#).unwrap();
    let mut config = Configuration::read(path.clone()).unwrap();
    let manifest:PluginManifest=serde_json::from_value(json!({"configuration":{"schema_version":1,"fields":{
      "folder":{"type":"string","description":"Root","required":false,"secret":false,"config_key":"allowed_dir","env":["ALLOWED_DIR"]},
      "secret":{"type":"string","description":"Secret","required":false,"secret":true,"config_key":"secret"}
    }}})).unwrap();
    let plugin = Plugin {
        name: "fs".into(),
        path: payload.clone(),
        kind: ExecutionKind::Wasm,
        manifest,
        params: HashMap::from([("secret".into(), "private-value".into())]),
        workspace: None,
    };
    let mut router = McpRouter::new();
    router.register("fs".into(), plugin.load(config.settings()).unwrap());
    let mut other = plugin.clone();
    other.name = "other".into();
    other.path = dir.path().join("other.wasm");
    std::fs::copy(&payload, &other.path).unwrap();
    router.register("other".into(), other.load(config.settings()).unwrap());
    // Reloading any plugin except fs would now fail.
    std::fs::remove_file(&other.path).unwrap();
    config.plugins.push(other);
    config.plugins.push(plugin);
    let server = start_embedded(
        router,
        Options {
            bind: "127.0.0.1".parse().unwrap(),
            port: 0,
            headless: true,
            workspace_only: false,
            read_only: false,
            allowed_tools: vec![],
            timeout: 30,
            plugins: vec![],
            roots: vec![],
        },
        config,
    )
    .await
    .unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let original = std::fs::read(&path).unwrap();
    for value in [
        dir.path().join("missing"),
        dir.path().join("outside.txt"),
        "relative".into(),
    ] {
        assert!(
            server
                .set_plugin_param("fs", "allowed_dir", value.to_str().unwrap())
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
    assert!(
        server
            .set_plugin_param("unknown", "allowed_dir", first.to_str().unwrap())
            .unwrap_err()
            .contains("Unknown plugin")
    );
    assert!(
        server
            .set_plugin_param("fs", "secret", first.to_str().unwrap())
            .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    server
        .set_plugin_param(
            "fs",
            "allowed_dir",
            first.join("nested/..").to_str().unwrap(),
        )
        .unwrap();
    let saved = std::fs::read(&path).unwrap();
    let values = server.plugin_params("fs");
    assert!(!values.contains_key("secret"));
    assert!(!values["allowed_dir"].starts_with(r"\\?\"));
    assert_eq!(values["folder"], values["allowed_dir"]);
    let reply = rpc(
        &client,
        &server.endpoint,
        "tools/call",
        json!({"name":"fs__read","arguments":{}}),
    )
    .await;
    assert_eq!(reply["result"]["content"][0]["text"], "A", "{reply}");
    // A lock held by setup blocks saving without changing the running plugin or file.
    std::fs::write(dir.path().join("dashboard.lock"), b"").unwrap();
    assert!(
        server
            .set_plugin_param("fs", "folder", second.to_str().unwrap())
            .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), saved);
    std::fs::remove_file(dir.path().join("dashboard.lock")).unwrap();
    // Failed plugin construction also leaves disk and live configuration alone.
    std::fs::rename(&payload, payload.with_extension("away")).unwrap();
    assert!(
        server
            .set_plugin_param("fs", "folder", second.to_str().unwrap())
            .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), saved);
    std::fs::rename(payload.with_extension("away"), &payload).unwrap();
    server
        .set_plugin_param("fs", "folder", second.to_str().unwrap())
        .unwrap();
    let reply = rpc(
        &client,
        &server.endpoint,
        "tools/call",
        json!({"name":"fs__read","arguments":{}}),
    )
    .await;
    assert_eq!(reply["result"]["content"][0]["text"], "B", "{reply}");
    let saved = std::fs::read(&path).unwrap();
    let settings: Value = serde_json::from_slice(&saved).unwrap();
    assert_eq!(settings["shared"]["retained"], "yes");
    assert_eq!(
        settings["plugins"]["fs"]["folder"],
        server.plugin_params("fs")["allowed_dir"]
    );
    // MCP arguments and method names cannot reach the host configuration API.
    let denied = rpc(
        &client,
        &server.endpoint,
        "tools/call",
        json!({"name":"fs__read","arguments":{"allowed_dir":first}}),
    )
    .await;
    assert_eq!(denied["result"]["isError"], true, "{denied}");
    let denied = rpc(
        &client,
        &server.endpoint,
        "set_plugin_param",
        json!({"plugin":"fs","key":"allowed_dir","value":first}),
    )
    .await;
    assert_eq!(denied["error"]["code"], -32601);
    let denied = client
        .put(server.endpoint.replace("/mcp", "/management/config"))
        .json(&json!({"plugin":"fs","key":"folder","value":first}))
        .send()
        .await
        .unwrap();
    assert!(!denied.status().is_success());
    assert_eq!(std::fs::read(&path).unwrap(), saved);
    assert_eq!(
        rpc(
            &client,
            &server.endpoint,
            "tools/call",
            json!({"name":"fs__read","arguments":{}})
        )
        .await["result"]["content"][0]["text"],
        "B"
    );
    assert!(server.plugin_params("unknown").is_empty());
    server.shutdown().await.unwrap();
}
