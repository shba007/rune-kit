use rune_kit::manifest::{PluginManifest, ToolAnnotations};
use rune_kit::protocol::{McpRouter, namespace_resource_uri, parse_resource_uri};

#[test]
fn resource_uri_round_trip() {
    let uri = namespace_resource_uri("my-plugin", "config.json");
    assert_eq!(uri, "rune://my-plugin/config.json");
    let (ns, local) = parse_resource_uri(&uri).unwrap();
    assert_eq!(ns, "my-plugin");
    assert_eq!(local, "config.json");
}

#[test]
fn resource_uri_round_trip_with_nested_path() {
    let uri = namespace_resource_uri("my-plugin", "sub/dir/data.json");
    let (ns, local) = parse_resource_uri(&uri).unwrap();
    assert_eq!(ns, "my-plugin");
    assert_eq!(local, "sub/dir/data.json");
}

#[test]
fn rules_resource_routes_absolute_artifact_namespaces_with_slashes() {
    use rune_kit::{PluginInstance, WasmPluginInstance};
    let mut router = McpRouter::new();
    for namespace in [
        "/tmp/local-plugin",
        "/tmp/local-plugin/nested",
        r"\\?\D:\plugins\local plugin",
    ] {
        let wasm = WasmPluginInstance::load_from_bytes(
            namespace,
            wat::parse_str("(module)").unwrap(),
            Default::default(),
            PluginManifest::default(),
        )
        .unwrap();
        router.register(namespace.into(), PluginInstance::Wasm(Box::new(wasm)));
    }
    for namespace in [
        "/tmp/local-plugin",
        "/tmp/local-plugin/nested",
        r"\\?\D:\plugins\local plugin",
    ] {
        let uri = namespace_resource_uri(namespace, "rules");
        let request = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"resources/read","params":{"uri":uri,"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}});
        let response = router.handle_jsonrpc(&request.to_string()).unwrap();
        assert_eq!(response["result"]["contents"][0]["uri"], uri, "{response}");
    }
}

#[test]
fn artifact_namespace_is_encoded_as_one_uri_component() {
    assert_eq!(
        namespace_resource_uri("/tmp/local plugin", "rules"),
        "rune://%2Ftmp%2Flocal%20plugin/rules"
    );
    assert_eq!(
        namespace_resource_uri(r"D:\plugins\local", "rules"),
        "rune://D%3A%5Cplugins%5Clocal/rules"
    );
}

#[test]
fn parse_resource_uri_rejects_unnamespaced() {
    assert_eq!(parse_resource_uri("config.json"), None);
    assert_eq!(parse_resource_uri("rune://only-namespace"), None);
}

#[test]
fn test_manifest_tool_rules_parsing() {
    let toml_str = r#"
        name = "test-plugin"
        version = "0.1.0"

        [tools.read_doc]
        read_only = true
        idempotent = true
        destructive = false
        open_world = false
        description = "Read document"

        [tools.delete_doc]
        read_only = false
        destructive = true
        idempotent = false
        open_world = false
        description = "Delete document"
    "#;

    let manifest = PluginManifest::from_toml(toml_str).expect("Valid manifest toml");
    assert_eq!(manifest.tools.len(), 2);

    let read_rule = manifest
        .tools
        .get("read_doc")
        .expect("read_doc rule exists");
    assert_eq!(read_rule.read_only, Some(true));
    assert_eq!(read_rule.idempotent, Some(true));
    assert_eq!(read_rule.destructive, Some(false));

    let annotations: ToolAnnotations = read_rule.clone().into();
    assert_eq!(annotations.read_only_hint, Some(true));
    assert_eq!(annotations.destructive_hint, Some(false));

    let ann_json = serde_json::to_value(&annotations).unwrap();
    assert_eq!(
        ann_json["readOnlyHint"], true,
        "canonical readOnlyHint must be present"
    );
    assert_eq!(
        ann_json["destructiveHint"], false,
        "canonical destructiveHint must be present"
    );
    assert_eq!(
        ann_json["idempotentHint"], true,
        "canonical idempotentHint must be present"
    );
    assert_eq!(
        ann_json["openWorldHint"], false,
        "canonical openWorldHint must be present"
    );
    assert!(
        ann_json.get("readOnly").is_none(),
        "readOnly must not be in ToolAnnotations JSON"
    );
}

#[test]
fn test_router_discover() {
    let mut router = McpRouter::new();
    let resp = router
        .handle_jsonrpc(r#"{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#)
        .expect("Response expected");

    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"]["supportedVersions"][0], "2026-07-28");
    assert_eq!(
        resp["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "rune-kit"
    );
}

#[test]
fn test_router_notifications_ignored() {
    let mut router = McpRouter::new();
    let resp = router
        .handle_jsonrpc(r#"{"jsonrpc": "2.0", "method": "notifications/progress", "params": {}}"#);
    assert!(resp.is_none());
}

#[test]
fn test_router_cancellation_tracking() {
    let mut router = McpRouter::new();

    let resp = router.handle_jsonrpc(
        r#"{"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": "call-100"}}"#,
    );
    assert!(resp.is_none());

    let call_resp = router.handle_jsonrpc(
        r#"{"jsonrpc":"2.0","id":"call-100","method":"tools/call","params":{"name":"test__run","_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#,
    );
    assert!(call_resp.is_none());
}

#[test]
fn test_router_cancel_request_method() {
    let mut router = McpRouter::new();

    let resp = router.handle_jsonrpc(
        r#"{"jsonrpc": "2.0", "method": "$/cancelRequest", "params": {"id": "req-42"}}"#,
    );
    assert!(resp.is_none());

    let call_resp = router.handle_jsonrpc(
        r#"{"jsonrpc":"2.0","id":"req-42","method":"tools/call","params":{"name":"test__run","_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#,
    );
    assert!(call_resp.is_none());
}

#[test]
fn test_router_invalid_tool_namespacing() {
    let mut router = McpRouter::new();
    let resp = router.handle_jsonrpc(
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"invalid_no_namespace","_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#,
    );
    let val = resp.expect("Response expected");
    assert_eq!(val["error"]["code"], -32602);
}

#[test]
fn test_router_allowlist_enforcement() {
    let mut router = McpRouter::new();
    router.set_allowed_tools(vec!["ns__allowed_tool".to_string()]);

    let resp = router.handle_jsonrpc(
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"ns__forbidden_tool","_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#,
    );
    let val = resp.expect("Response expected");
    assert_eq!(val["error"]["code"], -32600);
    assert!(
        val["error"]["message"]
            .as_str()
            .unwrap()
            .contains("allowlist")
    );
}

#[test]
fn test_router_read_only_enforcement() {
    let mut router = McpRouter::new();
    router.set_read_only(true);

    let resp = router.handle_jsonrpc(
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"ns__write_file","_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#,
    );
    let val = resp.expect("Response expected");
    assert_eq!(val["error"]["code"], -32600);
    assert!(
        val["error"]["message"]
            .as_str()
            .unwrap()
            .contains("read-only")
    );
}

#[test]
fn test_annotation_wire_names_canonical() {
    let ann = ToolAnnotations {
        read_only_hint: Some(true),
        destructive_hint: Some(false),
        idempotent_hint: Some(true),
        open_world_hint: Some(false),
        title: Some("My Tool".to_string()),
    };

    let json = serde_json::to_value(&ann).unwrap();
    assert_eq!(json["readOnlyHint"], true);
    assert_eq!(json["destructiveHint"], false);
    assert_eq!(json["idempotentHint"], true);
    assert_eq!(json["openWorldHint"], false);
    assert_eq!(json["title"], "My Tool");
    assert!(json.get("readOnly").is_none());
    assert!(json.get("destructive").is_none());
    assert!(json.get("idempotent").is_none());
    assert!(json.get("openWorld").is_none());
}

#[test]
fn test_annotation_both_spellings_deserialise() {
    let new_json = r#"{"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}"#;
    let ann_new: ToolAnnotations = serde_json::from_str(new_json).expect("New spelling parses");
    assert_eq!(ann_new.read_only_hint, Some(true));
    assert_eq!(ann_new.destructive_hint, Some(false));
    assert_eq!(ann_new.idempotent_hint, Some(true));
    assert_eq!(ann_new.open_world_hint, Some(false));

    let old_json =
        r#"{"readOnly": true, "destructive": false, "idempotent": true, "openWorld": false}"#;
    let ann_old: ToolAnnotations = serde_json::from_str(old_json).expect("Old spelling parses");
    assert_eq!(ann_old.read_only_hint, Some(true));
    assert_eq!(ann_old.destructive_hint, Some(false));
    assert_eq!(ann_old.idempotent_hint, Some(true));
    assert_eq!(ann_old.open_world_hint, Some(false));
}

#[test]
fn test_router_resources_list_empty_no_instances() {
    let mut router = McpRouter::new();
    let resp = router
        .handle_jsonrpc(r#"{"jsonrpc":"2.0","id":5,"method":"resources/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#)
        .expect("Response expected");
    assert!(resp["result"]["resources"].as_array().unwrap().is_empty());
}

#[test]
fn test_router_tools_list_read_only_empty() {
    let mut router = McpRouter::new();
    router.set_read_only(true);
    let resp = router
        .handle_jsonrpc(r#"{"jsonrpc":"2.0","id":6,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#)
        .expect("Response expected");
    let tools = resp["result"]["tools"].as_array().unwrap();
    assert!(
        tools.is_empty(),
        "Read-only mode with no read-only tools yields empty list"
    );
}

#[test]
fn test_error_shapes() {
    let mut router = McpRouter::new();

    let cancel_resp = router.handle_jsonrpc(
        r#"{"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": "x1"}}"#,
    );
    assert!(cancel_resp.is_none());
    let call = router
        .handle_jsonrpc(r#"{"jsonrpc":"2.0","id":"x1","method":"tools/call","params":{"name":"ns__tool","_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#)
        ;
    assert!(call.is_none());

    let mut r2 = McpRouter::new();
    r2.set_allowed_tools(vec!["ns__only".to_string()]);
    let perm = r2
        .handle_jsonrpc(r#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"ns__other","_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#)
        .unwrap();
    assert_eq!(perm["error"]["code"], -32600);

    let unk = router
        .handle_jsonrpc(r#"{"jsonrpc":"2.0","id":11,"method":"no_such_method","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#)
        .unwrap();
    assert_eq!(unk["error"]["code"], -32601);

    let parse_err = router.handle_jsonrpc("not json").unwrap();
    assert_eq!(parse_err["error"]["code"], -32700);
}

#[test]
fn native_wire_rules_and_timeout() {
    use rune_kit::runtime::{NativeSidecar, PluginInstance};
    use std::collections::HashMap;
    let dir = std::env::temp_dir().join(format!(
        "rune-kf-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let binary = dir.join(format!("fixture{}", std::env::consts::EXE_SUFFIX));
    assert!(
        std::process::Command::new("rustc")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/mcp_native.rs"
            ))
            .arg("-o")
            .arg(&binary)
            .status()
            .unwrap()
            .success()
    );
    let manifest = PluginManifest::from_toml(
        r#"
        name = "fixture"
        version = "0.1.0"
        [tools.read]
        read_only = true
        destructive = false
        idempotent = true
        open_world = false
    "#,
    )
    .unwrap();
    let mut native =
        NativeSidecar::new("fixture", &binary, HashMap::new(), Some(manifest)).unwrap();
    assert_eq!(native.get_info().unwrap().version, "0.1.0");
    let mut router = McpRouter::new();
    router.register("fixture".into(), PluginInstance::Native(Box::new(native)));
    router.set_read_only(true);
    let listed = router
        .handle_jsonrpc(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#)
        .unwrap();
    let tools = listed["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "fixture__read");
    let ann = &tools[0]["annotations"];
    for (canonical, legacy, expected) in [
        ("readOnlyHint", "readOnly", true),
        ("destructiveHint", "destructive", false),
        ("idempotentHint", "idempotent", true),
        ("openWorldHint", "openWorld", false),
    ] {
        assert_eq!(ann[canonical], expected);
        assert_eq!(ann[legacy], expected);
    }
    let resources = router
        .handle_jsonrpc(r#"{"jsonrpc":"2.0","id":2,"method":"resources/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#)
        .unwrap();
    assert!(
        resources["result"]["resources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["uri"] == "rune://fixture/rules")
    );
    let rules = router.handle_jsonrpc(r#"{"jsonrpc":"2.0","id":3,"method":"resources/read","params":{"uri":"rune://fixture/rules","_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#).unwrap();
    let text = rules["result"]["contents"][0]["text"].as_str().unwrap();
    let rules: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(rules["manifest"]["tools"]["read"]["read_only"], true);
    let refused = router
        .handle_jsonrpc(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"fixture__write","_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#,
        )
        .unwrap();
    assert_eq!(refused["error"]["code"], -32600);
    router.set_tool_timeout(1);
    let timed_out = router
        .handle_jsonrpc(
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"fixture__read","_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#,
        )
        .unwrap();
    assert_eq!(
        timed_out["error"],
        serde_json::json!({"code":-32000,"message":"Tool execution timed out after 1 seconds"})
    );
    assert_eq!(
        router
            .handle_jsonrpc(r#"{"jsonrpc":"2.0","id":6,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#)
            .unwrap()["result"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    drop(router);
    std::fs::remove_file(&binary).unwrap();
    let symbols = binary.with_extension("pdb");
    if symbols.exists() {
        std::fs::remove_file(symbols).unwrap();
    }
    std::fs::remove_dir(&dir).unwrap();
}

#[test]
fn wasm_explicit_timeout_interrupts_guest() {
    use rune_kit::runtime::{RuntimeError, WasmPluginInstance};
    let bytes = wat::parse_str(
        r#"(module
        (func (export "mcp_call_tool") (result i32)
            (loop $spin (br $spin)) i32.const 0))"#,
    )
    .unwrap();
    let mut wasm = WasmPluginInstance::load_from_bytes(
        "timeout-fixture",
        bytes,
        Default::default(),
        PluginManifest::default(),
    )
    .unwrap();
    let start = std::time::Instant::now();
    let result = wasm.call_tool_with_cancel(
        "spin",
        serde_json::json!({}),
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Some(std::time::Duration::from_secs(1)),
    );
    assert!(
        matches!(result, Err(RuntimeError::Timeout(1))),
        "{result:?}"
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn protocol_rejects_batches_and_missing_metadata() {
    let mut router = McpRouter::new();
    assert_eq!(
        router.handle_jsonrpc("[]").unwrap()["error"]["code"],
        -32600
    );
    assert_eq!(
        router
            .handle_jsonrpc(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
            .unwrap()["error"]["code"],
        -32602
    );
}

#[test]
fn modern_requests_validate_version_identity_and_legacy_handshake() {
    let mut router = McpRouter::new();
    let mut request = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{
        "io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}});
    let reply = router.handle_jsonrpc(&request.to_string()).unwrap();
    assert_eq!(reply["result"]["resultType"], "complete");
    assert_eq!(reply["result"]["cacheScope"], "private");
    request["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] =
        serde_json::json!("2025-03-26");
    let reply = router.handle_jsonrpc(&request.to_string()).unwrap();
    assert_eq!(reply["error"]["code"], -32022);
    assert_eq!(reply["error"]["data"]["supported"][0], "2026-07-28");
    request["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] =
        serde_json::json!("2026-07-28");
    request["method"] = serde_json::json!("initialize");
    assert_eq!(
        router.handle_jsonrpc(&request.to_string()).unwrap()["error"]["code"],
        -32601
    );
    request["params"]["_meta"]["io.modelcontextprotocol/clientInfo"] =
        serde_json::json!({"name":"incomplete"});
    assert_eq!(
        router.handle_jsonrpc(&request.to_string()).unwrap()["error"]["code"],
        -32602
    );
}

#[test]
fn dashboard_wasm_discovery_budget_interrupts_guest() {
    use rune_kit::runtime::{PluginInstance, RuntimeError, WasmPluginInstance};
    let bytes = wat::parse_str(r#"(module (func (export "mcp_list_tools") (result i32) (loop $spin (br $spin)) i32.const 0))"#).unwrap();
    let wasm = WasmPluginInstance::load_from_bytes(
        "inventory-timeout",
        bytes,
        Default::default(),
        PluginManifest::default(),
    )
    .unwrap();
    let mut instance = PluginInstance::Wasm(Box::new(wasm));
    let start = std::time::Instant::now();
    let result = instance.discover_tools(
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        std::time::Duration::from_secs(1),
    );
    assert!(
        matches!(result, Err(RuntimeError::Timeout(1))),
        "{result:?}"
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn cancelling_one_wasm_call_leaves_other_plugin_and_reused_id_usable() {
    use rune_kit::{PluginInstance, WasmPluginInstance};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    fn output(name: &str, value: &str) -> String {
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
    fn instance(spin: bool) -> PluginInstance {
        let list = r#"[{"name":"read","description":"Read fixture","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":true}}]"#;
        let call = if spin {
            "(func (export \"mcp_call_tool\") (result i32) (loop $spin (br $spin)) i32.const 0)"
                .into()
        } else {
            output("mcp_call_tool", r#""healthy""#)
        };
        let module = format!(
            r#"(module
            (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
            (import "extism:host/env" "store_u8" (func $store (param i64 i32)))
            (import "extism:host/env" "output_set" (func $output (param i64 i64)))
            {} {call})"#,
            output("mcp_list_tools", list)
        );
        PluginInstance::Wasm(Box::new(
            WasmPluginInstance::load_from_bytes(
                "fixture",
                wat::parse_str(module).unwrap(),
                Default::default(),
                PluginManifest::default(),
            )
            .unwrap(),
        ))
    }
    let mut router = McpRouter::new();
    router.set_tool_timeout(3);
    router.register("busy".into(), instance(true));
    router.register("healthy".into(), instance(false));
    let cancel = Arc::new(AtomicBool::new(false));
    let signal = cancel.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(150));
        signal.store(true, Ordering::Relaxed);
    });
    let request = |name: &str| {
        serde_json::json!({"jsonrpc":"2.0","id":"same","method":"tools/call","params":{"name":name,"arguments":{},"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}).to_string()
    };
    let cancelled = router
        .handle_cancellable(&request("busy__read"), cancel)
        .unwrap();
    canceller.join().unwrap();
    assert_eq!(cancelled["error"]["code"], -32800, "{cancelled}");
    let healthy = router
        .handle_cancellable(&request("healthy__read"), Arc::new(AtomicBool::new(false)))
        .unwrap();
    assert_eq!(healthy["id"], "same");
    assert_eq!(
        healthy["result"]["content"][0]["text"], "healthy",
        "{healthy}"
    );
}
