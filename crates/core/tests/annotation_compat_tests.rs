use rune_kit_core::manifest::{PluginManifest, ToolAnnotations, ToolDefinition};
use rune_kit_core::protocol::McpRouter;
use rune_kit_core::runtime::{NativeSidecar, PluginInstance, WasmPluginInstance};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;

const PAIRS: [(&str, &str); 4] = [
    ("readOnlyHint", "readOnly"),
    ("destructiveHint", "destructive"),
    ("idempotentHint", "idempotent"),
    ("openWorldHint", "openWorld"),
];

#[test]
fn accepts_each_spelling_and_equal_pairs_in_either_order() {
    for (canonical, legacy) in PAIRS {
        for value in [true, false] {
            for input in [
                format!(r#"{{"{canonical}":{value}}}"#),
                format!(r#"{{"{legacy}":{value}}}"#),
                format!(r#"{{"{canonical}":{value},"{legacy}":{value}}}"#),
                format!(r#"{{"{legacy}":{value},"{canonical}":{value}}}"#),
            ] {
                let ann: ToolAnnotations = serde_json::from_str(&input).unwrap();
                assert_eq!(serde_json::to_value(ann).unwrap()[canonical], value);
            }
        }
    }
}

#[test]
fn rejects_conflicts_in_either_order_without_choosing_a_safer_fact() {
    for (canonical, legacy) in PAIRS {
        for value in [true, false] {
            for input in [
                format!(r#"{{"{canonical}":{value},"{legacy}":{}}}"#, !value),
                format!(r#"{{"{legacy}":{},"{canonical}":{value}}}"#, !value),
            ] {
                let error = serde_json::from_str::<ToolAnnotations>(&input)
                    .unwrap_err()
                    .to_string();
                assert!(error.contains("conflicting"), "{error}");
                assert!(
                    error.contains(canonical) && error.contains(legacy),
                    "{error}"
                );
            }
        }
    }
}

#[test]
fn rejects_malformed_facts_even_with_a_valid_other_spelling() {
    for (canonical, legacy) in PAIRS {
        for invalid in [json!("true"), json!(1), Value::Null, json!([]), json!({})] {
            for input in [
                json!({canonical: invalid, legacy: true}),
                json!({canonical: true, legacy: invalid}),
            ] {
                assert!(serde_json::from_value::<ToolAnnotations>(input).is_err());
            }
        }
    }
    assert!(
        serde_json::from_str::<ToolAnnotations>(r#"{"readOnlyHint":true,"readOnlyHint":false}"#)
            .is_err()
    );
}

#[test]
fn missing_and_unknown_fields_do_not_grant_read_only() {
    for input in [json!({}), json!({"unrelated": {"readOnlyHint": true}})] {
        let ann: ToolAnnotations = serde_json::from_value(input).unwrap();
        assert_eq!(ann.read_only_hint, None);
        assert_eq!(ann.destructive_hint, None);
        assert_eq!(ann.idempotent_hint, None);
        assert_eq!(ann.open_world_hint, None);
    }
    let tool: ToolDefinition = serde_json::from_value(tool_payload(Value::Null)).unwrap();
    assert!(tool.annotations.is_none());
}

fn tool_payload(annotations: Value) -> Value {
    let mut tool = json!({"name":"read", "description":"Fixture",
        "inputSchema":{"type":"object", "properties":{"message":{"type":"string"}},
        "required":["message"], "x-rune-rules":{"effects":["read"],"custom":[1,true,{"nested":"preserve"}]}}});
    if !annotations.is_null() {
        tool["annotations"] = annotations;
    }
    tool
}

fn dual_annotations() -> Value {
    json!({"readOnlyHint":true,"readOnly":true,"destructiveHint":false,"destructive":false,
        "idempotentHint":true,"idempotent":true,"openWorldHint":false,"openWorld":false,
        "title":"Readable title", "unrelated":"ignored"})
}

fn list(router: &mut McpRouter) -> Value {
    router
        .handle_jsonrpc(r#"{"jsonrpc":"2.0","id":42,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#)
        .unwrap()
}

fn check_discovery(mut router: McpRouter, annotations: &Value) {
    let normal = list(&mut router);
    if annotations.get("readOnly").is_some_and(|v| v == false)
        || annotations
            .get("readOnlyHint")
            .is_some_and(|v| v.is_string())
    {
        assert_eq!(normal["error"]["code"], -32000, "{normal}");
        assert!(
            normal["error"]["message"]
                .as_str()
                .unwrap()
                .contains("fixture")
        );
        assert!(normal.get("result").is_none());
        router.set_read_only(true);
        assert_eq!(list(&mut router)["error"]["code"], -32000);
        let refused = router.handle_jsonrpc(r#"{"jsonrpc":"2.0","id":43,"method":"tools/call","params":{"name":"fixture__read","_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#).unwrap();
        assert_eq!(refused["error"]["code"], -32600);
        return;
    }
    assert_eq!(
        normal["result"]["tools"][0]["inputSchema"],
        tool_payload(Value::Null)["inputSchema"]
    );
    router.set_read_only(true);
    let restricted = list(&mut router);
    let tools = restricted["result"]["tools"].as_array().unwrap();
    if annotations.is_null() {
        assert!(tools.is_empty());
        let refused = router.handle_jsonrpc(r#"{"jsonrpc":"2.0","id":43,"method":"tools/call","params":{"name":"fixture__read","_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#).unwrap();
        assert_eq!(refused["error"]["code"], -32600);
    } else {
        assert_eq!(tools.len(), 1);
        for (canonical, legacy) in PAIRS {
            assert_eq!(tools[0]["annotations"][canonical], annotations[canonical]);
            assert_eq!(tools[0]["annotations"][legacy], annotations[legacy]);
        }
        assert_eq!(tools[0]["annotations"]["title"], "Readable title");
    }
}

struct NativeFixture {
    dir: PathBuf,
    binary: PathBuf,
}
impl NativeFixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "rune-annotations-{}-{}",
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
        Self { dir, binary }
    }
    fn router(&self, annotations: Value) -> McpRouter {
        let params = HashMap::from([(
            "RUNE_FIXTURE_TOOLS".to_string(),
            json!({"tools":[tool_payload(annotations)]}).to_string(),
        )]);
        let native = NativeSidecar::new(
            "fixture",
            &self.binary,
            params,
            Some(PluginManifest::default()),
        )
        .unwrap();
        let mut router = McpRouter::new();
        router.register("fixture".into(), PluginInstance::Native(Box::new(native)));
        router
    }
}
impl Drop for NativeFixture {
    fn drop(&mut self) {
        for path in [&self.binary, &self.binary.with_extension("pdb")] {
            if path.exists() {
                std::fs::remove_file(path).unwrap();
            }
        }
        std::fs::remove_dir(&self.dir).unwrap();
    }
}

#[test]
fn native_discovery_preserves_schema_and_reports_bad_facts() {
    let fixture = NativeFixture::new();
    for ann in [
        dual_annotations(),
        Value::Null,
        json!({"readOnlyHint":true,"readOnly":false}),
        json!({"readOnlyHint":"true"}),
    ] {
        check_discovery(fixture.router(ann.clone()), &ann);
    }
    let mut router = fixture.router(dual_annotations());
    let mut params = HashMap::new();
    params.insert("RUNE_FIXTURE_TOOLS".into(), "{\"tools\":[]}".into());
    let empty = NativeSidecar::new(
        "empty",
        &fixture.binary,
        params,
        Some(PluginManifest::default()),
    )
    .unwrap();
    let mut empty_router = McpRouter::new();
    empty_router.register("empty".into(), PluginInstance::Native(Box::new(empty)));
    assert_eq!(list(&mut empty_router)["result"]["tools"], json!([]));
    // A healthy plugin must not hide a second plugin's discovery failure.
    let params = HashMap::from([(
        "RUNE_FIXTURE_TOOLS".into(),
        json!({"tools":[tool_payload(json!({"readOnlyHint":true,"readOnly":false}))]}).to_string(),
    )]);
    let bad = NativeSidecar::new(
        "bad",
        &fixture.binary,
        params,
        Some(PluginManifest::default()),
    )
    .unwrap();
    router.register("bad".into(), PluginInstance::Native(Box::new(bad)));
    assert_eq!(list(&mut router)["error"]["code"], -32000);
}

fn wasm_router(annotations: Value) -> McpRouter {
    let payload = json!([tool_payload(annotations)]).to_string();
    let stores: String = payload
        .bytes()
        .enumerate()
        .map(|(i, b)| {
            format!("(call $store (i64.add (local.get $ptr) (i64.const {i})) (i32.const {b}))\n")
        })
        .collect();
    let wat = format!(
        r#"(module
      (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
      (import "extism:host/env" "store_u8" (func $store (param i64 i32)))
      (import "extism:host/env" "output_set" (func $output (param i64 i64)))
      (func (export "mcp_list_tools") (result i32) (local $ptr i64)
        (local.set $ptr (call $alloc (i64.const {})))
        {stores}
        (call $output (local.get $ptr) (i64.const {})) (i32.const 0)))"#,
        payload.len(),
        payload.len()
    );
    let wasm = WasmPluginInstance::load_from_bytes(
        "fixture",
        wat::parse_str(wat).unwrap(),
        HashMap::new(),
        PluginManifest::default(),
    )
    .unwrap();
    let mut router = McpRouter::new();
    router.register("fixture".into(), PluginInstance::Wasm(Box::new(wasm)));
    router
}

#[test]
fn wasm_discovery_uses_the_same_annotation_policy() {
    for ann in [
        dual_annotations(),
        Value::Null,
        json!({"readOnlyHint":true,"readOnly":false}),
        json!({"readOnlyHint":"true"}),
    ] {
        check_discovery(wasm_router(ann.clone()), &ann);
    }
}
