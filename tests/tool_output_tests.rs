use rune_kit::{
    manifest::{PluginManifest, ToolDefinition},
    protocol::McpRouter,
    runtime::{NativeSidecar, PluginInstance, WasmPluginInstance},
};
use serde_json::{Value, json};
use std::{collections::HashMap, path::PathBuf, sync::OnceLock};

fn native_binary() -> &'static PathBuf {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY.get_or_init(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!(
                "structured-fixture{}",
                std::env::consts::EXE_SUFFIX
            ));
        assert!(
            std::process::Command::new("rustc")
                .arg("tests/fixtures/structured_native.rs")
                .arg("-o")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        path
    })
}
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
fn router(native: bool, tool: Value, result: Value) -> McpRouter {
    let instance = if native {
        let params = HashMap::from([
            (
                "RUNE_FIXTURE_TOOLS".into(),
                json!({"tools":[tool]}).to_string(),
            ),
            ("RUNE_FIXTURE_RESULT".into(), result.to_string()),
        ]);
        PluginInstance::Native(Box::new(
            NativeSidecar::new(
                "fixture",
                native_binary(),
                params,
                Some(PluginManifest::default()),
            )
            .unwrap(),
        ))
    } else {
        let module = format!(
            r#"(module
            (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
            (import "extism:host/env" "store_u8" (func $store (param i64 i32)))
            (import "extism:host/env" "output_set" (func $output (param i64 i64))) {} {})"#,
            output("mcp_list_tools", &json!([tool]).to_string()),
            output("mcp_call_tool", &result.to_string())
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
    };
    let mut router = McpRouter::new();
    router.register("fixture".into(), instance);
    router
}
fn request(router: &mut McpRouter, method: &str) -> Value {
    let mut reply = router.handle_jsonrpc(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":{"name":"fixture__count","arguments":{},"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}).to_string()).unwrap();
    if let Some(result) = reply.get_mut("result").and_then(Value::as_object_mut) {
        result.remove("_meta");
        result.remove("resultType");
    }
    reply
}
fn tool() -> Value {
    json!({"name":"count","description":"Count fixture","inputSchema":{"type":"object"},"outputSchema":{"type":"object","properties":{"count":{"type":"integer"}},"required":["count"]}})
}
fn result() -> Value {
    json!({"content":[{"type":"text","text":"Found 3 items"},{"type":"text","text":"Second block"}],"structuredContent":{"count":3},"isError":false})
}
#[test]
fn structured_results_and_legacy_behavior_on_both_boundaries() {
    for native in [false, true] {
        let mut r = router(native, tool(), result());
        assert_eq!(
            request(&mut r, "tools/list")["result"]["tools"][0]["outputSchema"],
            tool()["outputSchema"]
        );
        assert_eq!(request(&mut r, "tools/call")["result"], result());
        let mut legacy_tool = tool();
        legacy_tool.as_object_mut().unwrap().remove("outputSchema");
        let legacy = json!({"content":[{"type":"text","text":"legacy"}],"isError":false});
        let mut r = router(native, legacy_tool.clone(), legacy.clone());
        assert!(
            request(&mut r, "tools/list")["result"]["tools"][0]
                .get("outputSchema")
                .is_none()
        );
        assert_eq!(request(&mut r, "tools/call")["result"], legacy);
        let mut r = router(native, legacy_tool, result());
        assert_eq!(request(&mut r, "tools/call")["result"], result());
    }
}
#[test]
fn invalid_results_are_tool_errors_and_invalid_schemas_fail_discovery() {
    for native in [false, true] {
        for structured in [json!({"count":"wrong"}), json!({}), Value::Null, json!([])] {
            let mut bad = result();
            bad["structuredContent"] = structured;
            let mut r = router(native, tool(), bad);
            let reply = request(&mut r, "tools/call");
            assert_eq!(reply["result"]["isError"], true, "{reply}");
            assert!(
                reply["result"]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("fixture__count")
            );
        }
        for schema in [
            json!({"type":"array"}),
            json!({"type":"object","properties":{"count":{"type":"invalid"}}}),
            json!(false),
        ] {
            let mut bad = tool();
            bad["outputSchema"] = schema;
            let mut r = router(native, bad, result());
            let reply = request(&mut r, "tools/list");
            assert!(reply.get("error").is_some(), "{reply}");
            assert!(reply.to_string().contains("count"));
        }
        let mut r = router(
            native,
            tool(),
            json!({"content":[{"type":"text","text":"legacy"}]}),
        );
        let reply = request(&mut r, "tools/call");
        assert_eq!(reply["result"]["isError"], true);
        assert!(reply.to_string().contains("fixture__count"));
    }
}
#[test]
fn manifest_alias_and_absence_round_trip() {
    let mut value = tool();
    let schema = value
        .as_object_mut()
        .unwrap()
        .remove("outputSchema")
        .unwrap();
    value["output_schema"] = schema.clone();
    let parsed: ToolDefinition = serde_json::from_value(value).unwrap();
    assert_eq!(
        serde_json::to_value(parsed).unwrap()["outputSchema"],
        schema
    );
    let parsed: ToolDefinition = serde_json::from_value(
        json!({"name":"old","description":"old","inputSchema":{"type":"object"}}),
    )
    .unwrap();
    assert!(
        serde_json::to_value(parsed)
            .unwrap()
            .get("outputSchema")
            .is_none()
    );
}

#[test]
fn native_structured_content_counts_toward_result_limit() {
    let params = HashMap::from([("RUNE_FIXTURE_RESULT".into(), "oversized".into())]);
    let mut native = NativeSidecar::new(
        "fixture",
        native_binary(),
        params,
        Some(PluginManifest::default()),
    )
    .unwrap();
    let error = native.call_tool("count", json!({})).unwrap_err();
    assert!(error.to_string().contains("8 MiB"), "{error}");
}

#[test]
fn intentional_errors_bypass_output_validation() {
    for native in [false, true] {
        for structured in [None, Some(json!({"count":"wrong"})), Some(Value::Null)] {
            let mut failure =
                json!({"content":[{"type":"text","text":"Intentional failure"}],"isError":true});
            if let Some(value) = structured {
                failure["structuredContent"] = value;
            }
            let mut r = router(native, tool(), failure.clone());
            assert_eq!(request(&mut r, "tools/call")["result"], failure);
        }
    }
}
#[test]
fn large_legacy_native_results_are_not_rejected() {
    let params = HashMap::from([("RUNE_FIXTURE_RESULT".into(), "large-legacy".into())]);
    let mut native = NativeSidecar::new(
        "fixture",
        native_binary(),
        params,
        Some(PluginManifest::default()),
    )
    .unwrap();
    let value = native.call_tool("count", json!({})).unwrap();
    assert_eq!(value.as_str().unwrap().len(), 8 * 1024 * 1024 + 1);
}
#[test]
fn large_wasm_results_only_limit_structured_envelopes() {
    for structured in [false, true] {
        let prefix = if structured {
            r#"{"content":[],"structuredContent":{"payload":""#
        } else {
            "\""
        };
        let suffix = if structured { "\"}}" } else { "\"" };
        let size = 8 * 1024 * 1024 + 1;
        let prefix_stores: String = prefix
            .bytes()
            .enumerate()
            .map(|(i, b)| {
                format!("(call $store (i64.add (local.get $ptr) (i64.const {i})) (i32.const {b}))")
            })
            .collect();
        let suffix_stores: String = suffix
            .bytes()
            .enumerate()
            .map(|(i, b)| {
                format!(
                    "(call $store (i64.add (local.get $ptr) (i64.const {})) (i32.const {b}))",
                    prefix.len() + size + i
                )
            })
            .collect();
        let module = format!(
            r#"(module
          (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
          (import "extism:host/env" "store_u8" (func $store (param i64 i32)))
          (import "extism:host/env" "output_set" (func $output (param i64 i64)))
          (func (export "mcp_call_tool") (result i32) (local $ptr i64) (local $i i64)
            (local.set $ptr (call $alloc (i64.const {total}))) {prefix_stores}
            (local.set $i (i64.const {start}))
            (loop $fill
              (call $store (i64.add (local.get $ptr) (local.get $i)) (i32.const 120))
              (local.set $i (i64.add (local.get $i) (i64.const 1)))
              (br_if $fill (i64.lt_u (local.get $i) (i64.const {end}))))
            {suffix_stores} (call $output (local.get $ptr) (i64.const {total})) i32.const 0))"#,
            total = prefix.len() + size + suffix.len(),
            start = prefix.len(),
            end = prefix.len() + size
        );
        let mut wasm = WasmPluginInstance::load_from_bytes(
            "fixture",
            wat::parse_str(module).unwrap(),
            Default::default(),
            PluginManifest::default(),
        )
        .unwrap();
        let result = wasm.call_tool("count", json!({}));
        if structured {
            assert!(result.unwrap_err().to_string().contains("8 MiB"));
        } else {
            assert_eq!(result.unwrap().as_str().unwrap().len(), size);
        }
    }
}
