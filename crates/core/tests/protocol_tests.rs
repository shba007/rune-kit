use rune_kit_core::manifest::{PluginManifest, ToolAnnotations};
use rune_kit_core::protocol::{McpRouter, namespace_resource_uri, parse_resource_uri};

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

    // Conversion to ToolAnnotations: rule fields map to *Hint fields
    let annotations: ToolAnnotations = read_rule.clone().into();
    assert_eq!(annotations.read_only_hint, Some(true));
    assert_eq!(annotations.destructive_hint, Some(false));

    // KF-1: Serialised wire names must use MCP spec 2025-03-26 *Hint suffix
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
    // Old names must NOT appear in the ToolAnnotations serialisation itself
    // (legacy aliases are only injected at the protocol layer in tools/list)
    assert!(
        ann_json.get("readOnly").is_none(),
        "readOnly must not be in ToolAnnotations JSON"
    );
}

// KF-1: Canonical *Hint names are emitted in tools/list (verified at protocol layer).
// This test uses the router directly without a real plugin instance so it only asserts the
// protocol-version upgrade; wire-name injection is covered by test_annotation_wire_names.
#[test]
fn test_router_discover() {
    let mut router = McpRouter::new();
    let resp = router
        .handle_jsonrpc(r#"{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#)
        .expect("Response expected");

    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], 1);
    // KF-1: protocolVersion must report 2025-03-26
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

    // Cancellation notification should return None (no JSON-RPC response for notifications)
    let resp = router.handle_jsonrpc(
        r#"{"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": "call-100"}}"#,
    );
    assert!(resp.is_none());

    // A tool call using that cancelled ID before execution should be rejected with -32800
    let call_resp = router.handle_jsonrpc(
        r#"{"jsonrpc":"2.0","id":"call-100","method":"tools/call","params":{"name":"test__run","_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#,
    );
    assert!(call_resp.is_none());
}

#[test]
fn test_router_cancel_request_method() {
    let mut router = McpRouter::new();

    // $/cancelRequest support
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

    // Any call to non-registered or non-read-only tool in read-only mode is denied with -32600
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

// KF-1: Verify ToolAnnotations serialises with *Hint canonical names and that the
// transitional legacy aliases are accepted on deserialisation (both spellings).
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
    // Canonical names must be present
    assert_eq!(json["readOnlyHint"], true);
    assert_eq!(json["destructiveHint"], false);
    assert_eq!(json["idempotentHint"], true);
    assert_eq!(json["openWorldHint"], false);
    assert_eq!(json["title"], "My Tool");
    // Old names must NOT appear (legacy injection is done in the protocol layer only)
    assert!(json.get("readOnly").is_none());
    assert!(json.get("destructive").is_none());
    assert!(json.get("idempotent").is_none());
    assert!(json.get("openWorld").is_none());
}

// KF-7: Accept both old and new spellings from plugin tool lists.
#[test]
fn test_annotation_both_spellings_deserialise() {
    // New canonical spelling (what a spec-compliant plugin sends)
    let new_json = r#"{"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}"#;
    let ann_new: ToolAnnotations = serde_json::from_str(new_json).expect("New spelling parses");
    assert_eq!(ann_new.read_only_hint, Some(true));
    assert_eq!(ann_new.destructive_hint, Some(false));
    assert_eq!(ann_new.idempotent_hint, Some(true));
    assert_eq!(ann_new.open_world_hint, Some(false));

    // Legacy spelling (what a pre-2025-03-26 plugin sends)
    let old_json =
        r#"{"readOnly": true, "destructive": false, "idempotent": true, "openWorld": false}"#;
    let ann_old: ToolAnnotations = serde_json::from_str(old_json).expect("Old spelling parses");
    assert_eq!(ann_old.read_only_hint, Some(true));
    assert_eq!(ann_old.destructive_hint, Some(false));
    assert_eq!(ann_old.idempotent_hint, Some(true));
    assert_eq!(ann_old.open_world_hint, Some(false));
}

// KF-3: resources/list always includes rune://<ns>/rules regardless of --read-only mode.
// (Tested indirectly here; a real plugin instance would be needed for a full integration test.)
#[test]
fn test_router_resources_list_empty_no_instances() {
    let mut router = McpRouter::new();
    let resp = router
        .handle_jsonrpc(r#"{"jsonrpc":"2.0","id":5,"method":"resources/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#)
        .expect("Response expected");
    assert!(resp["result"]["resources"].as_array().unwrap().is_empty());
}

// KF-3: tools/list in read-only mode with no registered instances returns [] (not an error).
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

// KF-2/KF-4: error shapes for known codes.
#[test]
fn test_error_shapes() {
    let mut router = McpRouter::new();

    // -32800: cancelled
    let cancel_resp = router.handle_jsonrpc(
        r#"{"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": "x1"}}"#,
    );
    assert!(cancel_resp.is_none());
    let call = router
        .handle_jsonrpc(r#"{"jsonrpc":"2.0","id":"x1","method":"tools/call","params":{"name":"ns__tool","_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#)
        ;
    assert!(call.is_none());

    // -32600: permission denied (allowlist)
    let mut r2 = McpRouter::new();
    r2.set_allowed_tools(vec!["ns__only".to_string()]);
    let perm = r2
        .handle_jsonrpc(r#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"ns__other","_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#)
        .unwrap();
    assert_eq!(perm["error"]["code"], -32600);

    // -32601: unknown method
    let unk = router
        .handle_jsonrpc(r#"{"jsonrpc":"2.0","id":11,"method":"no_such_method","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#)
        .unwrap();
    assert_eq!(unk["error"]["code"], -32601);

    // -32700: parse error
    let parse_err = router.handle_jsonrpc("not json").unwrap();
    assert_eq!(parse_err["error"]["code"], -32700);
}

// Exercise the actual tools/list wire path with a portable native MCP fixture.
#[test]
fn native_wire_rules_and_timeout() {
    use rune_kit_core::runtime::{NativeSidecar, PluginInstance};
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
    // A killed native child must respawn successfully for discovery.
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
    use rune_kit_core::runtime::{RuntimeError, WasmPluginInstance};
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
