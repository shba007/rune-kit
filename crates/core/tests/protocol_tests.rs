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

    let annotations: ToolAnnotations = read_rule.clone().into();
    assert_eq!(annotations.read_only, Some(true));
    assert_eq!(annotations.destructive, Some(false));

    let ann_json = serde_json::to_value(&annotations).unwrap();
    assert_eq!(ann_json["readOnly"], true);
    assert_eq!(ann_json["destructive"], false);
    assert_eq!(ann_json["idempotent"], true);
    assert_eq!(ann_json["openWorld"], false);
}

#[test]
fn test_router_initialize() {
    let mut router = McpRouter::new();
    let resp = router
        .handle_jsonrpc(r#"{"jsonrpc": "2.0", "id": 1, "method": "initialize"}"#)
        .expect("Response expected");

    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(resp["result"]["serverInfo"]["name"], "rune-kit");
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
        r#"{"jsonrpc": "2.0", "id": "call-100", "method": "tools/call", "params": {"name": "test__run"}}"#,
    );
    let val = call_resp.expect("Error response");
    assert_eq!(val["error"]["code"], -32800);
    assert_eq!(val["error"]["message"], "Request cancelled by client");
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
        r#"{"jsonrpc": "2.0", "id": "req-42", "method": "tools/call", "params": {"name": "test__run"}}"#,
    );
    let val = call_resp.expect("Error response");
    assert_eq!(val["error"]["code"], -32800);
}

#[test]
fn test_router_invalid_tool_namespacing() {
    let mut router = McpRouter::new();
    let resp = router.handle_jsonrpc(
        r#"{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "invalid_no_namespace"}}"#,
    );
    let val = resp.expect("Response expected");
    assert_eq!(val["error"]["code"], -32602);
}

#[test]
fn test_router_allowlist_enforcement() {
    let mut router = McpRouter::new();
    router.set_allowed_tools(vec!["ns__allowed_tool".to_string()]);

    let resp = router.handle_jsonrpc(
        r#"{"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "ns__forbidden_tool"}}"#,
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
        r#"{"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "ns__write_file"}}"#,
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
