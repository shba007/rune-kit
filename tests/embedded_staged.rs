//! Per-plugin staged mode for the embedded server: originals change only through the
//! host-side `/management/changes` approval token, never through the guest.
use rune_kit::{
    ExecutionKind, McpRouter, PluginManifest,
    configuration::{Configuration, Plugin},
    server::{Options, start_embedded},
};
use serde_json::{Value, json};
use std::{collections::HashMap, path::Path};

fn export_text(name: &str, value: &str) -> String {
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

const PRELUDE: &str = r#"
    (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
    (import "extism:host/env" "store_u8" (func $store (param i64 i32)))
    (import "extism:host/env" "length" (func $length (param i64) (result i64)))
    (import "extism:host/env" "output_set" (func $output (param i64 i64)))
    (import "extism:host/user" "host_file_access" (func $access (param i64) (result i64)))
"#;

fn tools() -> String {
    export_text(
        "mcp_list_tools",
        &json!([{"name":"go","description":"Run fixed requests","inputSchema":{"type":"object","properties":{},"additionalProperties":false}}]).to_string(),
    )
}

/// A guest that runs the given broker requests in order and returns the last response.
fn sequence_guest(requests: &[Value]) -> Vec<u8> {
    let mut body = String::new();
    for request in requests {
        let payload = request.to_string();
        body.push_str(&format!(
            "(local.set $ptr (call $alloc (i64.const {})))\n",
            payload.len()
        ));
        for (i, b) in payload.bytes().enumerate() {
            body.push_str(&format!(
                "(call $store (i64.add (local.get $ptr) (i64.const {i})) (i32.const {b}))\n"
            ));
        }
        body.push_str("(local.set $result (call $access (local.get $ptr)))\n");
    }
    wat::parse_str(format!(
        r#"(module {PRELUDE}
        (memory (export "memory") 1)
        {tools}
        (func (export "mcp_call_tool") (result i32) (local $ptr i64) (local $result i64)
            {body}
            (call $output (local.get $result) (call $length (local.get $result)))
            (i32.const 0)))"#,
        tools = tools()
    ))
    .unwrap()
}

/// A guest from an unrelated plugin: it also imports the broker but never needs it.
fn plain_guest() -> Vec<u8> {
    let reply = r#"{"content":[{"type":"text","text":"unstaged ok"}]}"#;
    wat::parse_str(format!(
        r#"(module {PRELUDE}
        (memory (export "memory") 1)
        {tools}
        {call})"#,
        tools = tools(),
        call = export_text("mcp_call_tool", reply)
    ))
    .unwrap()
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

fn plugin(name: &str, path: &Path, folder: bool) -> Plugin {
    let manifest: PluginManifest = if folder {
        serde_json::from_value(json!({"configuration":{"schema_version":1,"fields":{
            "folder":{"type":"string","description":"Root","required":false,"secret":false,"config_key":"allowed_dir","env":["ALLOWED_DIR"]}
        }}}))
        .unwrap()
    } else {
        PluginManifest::default()
    };
    Plugin {
        name: name.into(),
        path: path.to_owned(),
        kind: ExecutionKind::Wasm,
        manifest,
        params: HashMap::new(),
        workspace: None,
    }
}

async fn changes(client: &reqwest::Client, base: &str, name: &str) -> Value {
    let all: Value = client
        .get(format!("{base}/management/changes"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    all["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["plugin"] == name)
        .unwrap()
        .clone()
}

async fn act(client: &reqwest::Client, base: &str, action: &str, approval: &Value) -> bool {
    client
        .post(format!("{base}/management/changes"))
        .json(&json!({"plugin":"fs","action":action,"approval":approval}))
        .send()
        .await
        .unwrap()
        .status()
        .is_success()
}

fn review<'a>(changes: &'a Value, file: &str) -> &'a Value {
    changes["reviews"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["path"].as_str().unwrap().ends_with(file))
        .unwrap_or_else(|| panic!("no review for {file}: {changes}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn staged_plugin_changes_originals_only_through_host_approval_and_coexists() {
    let dir = tempfile::tempdir().unwrap();
    // Sandbox and approvals live under the data override, never the real home.
    unsafe { std::env::set_var("RUNE_KIT_DATA_DIR", dir.path().join("data")) };
    let real = dir.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let a = real.join("a.txt");
    std::fs::write(&a, "original").unwrap();
    let real_key = std::fs::canonicalize(&real).unwrap();
    let sub = real_key.join("sub");
    let requests = [
        json!({"schema_version":1,"operation":"create_directory","path":sub}),
        json!({"schema_version":1,"operation":"write_file","path":sub.join("b.txt"),"data":b"nested".to_vec()}),
        json!({"schema_version":1,"operation":"write_file","path":real_key.join("a.txt"),"data":b"staged".to_vec()}),
        json!({"schema_version":1,"operation":"read_file","path":real_key.join("a.txt")}),
    ];
    let staged_wasm = dir.path().join("staged.wasm");
    let plain_wasm = dir.path().join("plain.wasm");
    std::fs::write(&staged_wasm, sequence_guest(&requests)).unwrap();
    std::fs::write(&plain_wasm, plain_guest()).unwrap();

    let path = dir.path().join("dashboard.json");
    std::fs::write(&path, br#"{"shared":{},"plugins":{}}"#).unwrap();
    let mut config = Configuration::read(path).unwrap();
    let fs_plugin = plugin("fs", &staged_wasm, true);
    let plain = plugin("plain", &plain_wasm, false);
    let mut router = McpRouter::new();
    router.register("fs".into(), fs_plugin.load(config.settings()).unwrap());
    router.register("plain".into(), plain.load(config.settings()).unwrap());
    config.plugins.push(fs_plugin);
    config.plugins.push(plain);
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
    let base = server.endpoint.trim_end_matches("/mcp").to_owned();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let call = |name: &'static str| {
        let (client, endpoint) = (client.clone(), server.endpoint.clone());
        async move {
            rpc(
                &client,
                &endpoint,
                "tools/call",
                json!({"name":name,"arguments":{}}),
            )
            .await
        }
    };

    // Staged mode needs a folder and a known WASM plugin.
    assert!(
        server
            .set_plugin_staged("fs", true)
            .unwrap_err()
            .contains("allowed_dir")
    );
    assert!(server.set_plugin_staged("nope", true).is_err());
    assert!(!server.is_plugin_staged("fs"));
    server
        .set_plugin_param("fs", "allowed_dir", real.to_str().unwrap())
        .unwrap();
    server.set_plugin_staged("fs", true).unwrap();
    assert!(server.is_plugin_staged("fs"));
    // Changing the folder later keeps the plugin staged.
    server
        .set_plugin_param("fs", "allowed_dir", real.to_str().unwrap())
        .unwrap();
    assert!(server.is_plugin_staged("fs"));

    // The plugin reads back what it wrote, while the original stays untouched.
    let reply = call("fs__go").await;
    let text = reply.to_string();
    // "staged" as the JSON byte array the broker returns.
    assert!(text.contains("115,116,97,103,101,100"), "{reply}");
    assert_ne!(reply["result"]["isError"], true, "{reply}");
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "original");
    assert!(!real.join("sub").exists());

    // An unstaged plugin keeps working beside it and has no reviews.
    let plain_reply = call("plain__go").await;
    assert_eq!(
        plain_reply["result"]["content"][0]["text"], "unstaged ok",
        "{plain_reply}"
    );
    assert!(
        changes(&client, &base, "plain").await["reviews"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let pending = changes(&client, &base, "fs").await;
    assert_eq!(pending["reviews"].as_array().unwrap().len(), 3, "{pending}");
    let token = |c: &Value, f: &str| review(c, f)["approval"].clone();

    // Unknown and stale tokens change nothing.
    assert!(!act(&client, &base, "apply", &json!("0".repeat(64))).await);
    std::fs::write(&a, "external").unwrap();
    assert!(!act(&client, &base, "apply", &token(&pending, "a.txt")).await);
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "external");
    std::fs::write(&a, "original").unwrap();

    // Folders apply before their contents.
    let pending = changes(&client, &base, "fs").await;
    assert!(!act(&client, &base, "apply", &token(&pending, "b.txt")).await);
    assert!(!real.join("sub").exists());
    for file in ["sub", "b.txt", "a.txt"] {
        let current = changes(&client, &base, "fs").await;
        let item = review(&current, file)["approval"].clone();
        assert!(act(&client, &base, "apply", &item).await, "{file}");
    }
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "staged");
    assert_eq!(
        std::fs::read_to_string(real.join("sub/b.txt")).unwrap(),
        "nested"
    );
    // A consumed token cannot be replayed.
    assert!(!act(&client, &base, "apply", &token(&pending, "a.txt")).await);

    // Restore brings the original back.
    let after = changes(&client, &base, "fs").await;
    let recovery = after["recoveries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["path"].as_str().unwrap().ends_with("a.txt"))
        .unwrap()["approval"]
        .clone();
    assert!(act(&client, &base, "restore", &recovery).await);
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "original");

    // Leaving staged mode removes the broker; the guest then fails cleanly.
    server.set_plugin_staged("fs", false).unwrap();
    assert!(!server.is_plugin_staged("fs"));
    let off = call("fs__go").await;
    assert!(
        off["error"].is_object() || off["result"]["isError"] == true,
        "{off}"
    );
    assert_eq!(
        call("plain__go").await["result"]["content"][0]["text"],
        "unstaged ok"
    );
    server.shutdown().await.unwrap();
}
