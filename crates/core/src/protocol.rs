use crate::runtime::{PluginInstance, RuntimeError};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct RouterConfig {
    pub read_only: bool,
    pub allowed_tools: Option<Vec<String>>,
    pub tool_timeout_secs: Option<u64>,
}

#[derive(Clone)]
pub struct CancelState {
    pub flag: Arc<AtomicBool>,
    pub wasm_handle: Option<extism::CancelHandle>,
}

// Holds only the currently executing instance, never unrelated idle WASM guests.
struct WasmOperation(Arc<Mutex<Option<extism::CancelHandle>>>);
impl WasmOperation {
    fn track(instance: &PluginInstance, slot: Arc<Mutex<Option<extism::CancelHandle>>>) -> Self {
        *slot.lock().unwrap() = match instance {
            PluginInstance::Wasm(w) => Some(w.get_cancel_handle()),
            _ => None,
        };
        Self(slot)
    }
}
impl Drop for WasmOperation {
    fn drop(&mut self) {
        *self.0.lock().unwrap() = None;
    }
}

pub struct McpRouter {
    instances: HashMap<String, PluginInstance>,
    config: RouterConfig,
    request_cancel: Arc<AtomicBool>,
    wasm_operation: Arc<Mutex<Option<extism::CancelHandle>>>,
    active_cancels: Arc<Mutex<HashMap<String, CancelState>>>,
    cancelled_ids: Arc<Mutex<HashSet<String>>>,
}

pub fn namespace_resource_uri(namespace: &str, local_uri: &str) -> String {
    let clean_local = if let Some(stripped) = local_uri.strip_prefix("rune://") {
        if let Some((_old_ns, subpath)) = stripped.split_once('/') {
            subpath
        } else {
            stripped
        }
    } else {
        local_uri.trim_start_matches('/')
    };
    format!("rune://{}/{}", namespace, clean_local)
}

pub fn parse_resource_uri(uri: &str) -> Option<(&str, &str)> {
    let rest = uri.strip_prefix("rune://")?;
    let (namespace, local_uri) = rest.split_once('/')?;
    Some((namespace, local_uri))
}

fn normalize_id(id: Option<&Value>) -> String {
    match id {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => format!("string:{s}"),
        Some(val) => val.to_string(),
        None => "null".to_string(),
    }
}

fn tool_validator(schema: &Value) -> Result<jsonschema::Validator, String> {
    let mut schema = schema.clone();
    if !schema.is_object() || schema["type"] != "object" {
        return Err("Tool inputSchema must describe an object".to_owned());
    }
    if schema.get("$schema").is_none() {
        schema["$schema"] = json!("https://json-schema.org/draft/2020-12/schema");
    }
    jsonschema::options()
        .offline()
        .build(&schema)
        .map_err(|error| error.to_string())
}

impl Default for McpRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl McpRouter {
    pub fn new() -> Self {
        Self {
            instances: HashMap::new(),
            config: RouterConfig::default(),
            request_cancel: Arc::new(AtomicBool::new(false)),
            wasm_operation: Arc::new(Mutex::new(None)),
            active_cancels: Arc::new(Mutex::new(HashMap::new())),
            cancelled_ids: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn with_config(config: RouterConfig) -> Self {
        Self {
            instances: HashMap::new(),
            config,
            request_cancel: Arc::new(AtomicBool::new(false)),
            wasm_operation: Arc::new(Mutex::new(None)),
            active_cancels: Arc::new(Mutex::new(HashMap::new())),
            cancelled_ids: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn set_read_only(&mut self, read_only: bool) {
        self.config.read_only = read_only;
    }

    pub fn set_allowed_tools(&mut self, allowed_tools: Vec<String>) {
        self.config.allowed_tools = Some(allowed_tools);
    }

    pub fn set_tool_timeout(&mut self, timeout_secs: u64) {
        self.config.tool_timeout_secs = Some(timeout_secs);
    }

    pub fn register(&mut self, namespace: String, instance: PluginInstance) {
        self.instances.insert(namespace, instance);
    }

    pub fn cancel_request(&self, req_id: &str) {
        self.cancelled_ids
            .lock()
            .unwrap()
            .insert(req_id.to_string());
        if let Some(state) = self.active_cancels.lock().unwrap().get(req_id) {
            state.flag.store(true, Ordering::Relaxed);
            if let Some(ref wasm) = state.wasm_handle {
                let _ = wasm.cancel();
            }
        }
    }

    /// The HTTP worker serializes instances. Only this operation's disconnect flag
    /// can interrupt its native process or WASM execution; client IDs are not keys.
    pub fn handle_cancellable(&mut self, request: &str, cancel: Arc<AtomicBool>) -> Option<Value> {
        let operation_cancel = Arc::new(AtomicBool::new(false));
        self.request_cancel = operation_cancel.clone();
        let expired = Arc::new(AtomicBool::new(false));
        let timed_out = expired.clone();
        let timeout = self.config.tool_timeout_secs.unwrap_or(30);
        let started = std::time::Instant::now();
        for instance in self.instances.values_mut() {
            if let PluginInstance::Native(native) = instance {
                native.set_operation_context(
                    operation_cancel.clone(),
                    Some(Duration::from_secs(timeout)),
                );
            }
        }
        let active_wasm = self.wasm_operation.clone();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let watcher = std::thread::spawn(move || {
            loop {
                if cancel.load(Ordering::Relaxed)
                    || started.elapsed() >= Duration::from_secs(timeout)
                {
                    timed_out.store(!cancel.load(Ordering::Relaxed), Ordering::Relaxed);
                    operation_cancel.store(true, Ordering::Relaxed);
                    if let Some(handle) = active_wasm.lock().unwrap().as_ref() {
                        let _ = handle.cancel();
                    }
                    break;
                }
                if !matches!(
                    done_rx.recv_timeout(Duration::from_millis(20)),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                ) {
                    break;
                }
            }
        });
        let result = self.handle_jsonrpc(request);
        let _ = done_tx.send(());
        let _ = watcher.join();
        self.request_cancel = Arc::new(AtomicBool::new(false));
        for instance in self.instances.values_mut() {
            if let PluginInstance::Native(native) = instance {
                native.set_operation_context(self.request_cancel.clone(), None);
            }
        }
        if expired.load(Ordering::Relaxed) {
            let request: Value = serde_json::from_str(request).unwrap_or(Value::Null);
            return Some(
                json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32000,"message":format!("Request execution timed out after {timeout} seconds")}}),
            );
        }
        result
    }

    /// Transport-independent public MCP dispatch. Plugin calls retain their private ABI.
    pub fn handle_jsonrpc(&mut self, request_str: &str) -> Option<Value> {
        let req: Value = match serde_json::from_str(request_str) {
            Ok(value) => value,
            Err(_) => {
                return Some(
                    json!({"jsonrpc":"2.0","error":{"code":-32700,"message":"Parse error"}}),
                );
            }
        };
        let id = req.get("id");
        let error = |code: i64, message: &str| {
            let mut value = json!({"jsonrpc":"2.0","error":{"code":code,"message":message}});
            if let Some(id) = id.filter(|id| id.is_string() || id.is_number()) {
                value["id"] = id.clone();
            }
            Some(value)
        };
        if !req.is_object()
            || req["jsonrpc"] != "2.0"
            || !req["method"].is_string()
            || id.is_some_and(|v| !v.is_string() && !v.is_number())
            || req.get("params").is_some_and(|v| !v.is_object())
        {
            return error(-32600, "Invalid request");
        }
        if id.is_none() {
            if req["method"] == "notifications/cancelled" || req["method"] == "$/cancelRequest" {
                return self.dispatch(request_str);
            }
            return None;
        }
        let meta = &req["params"]["_meta"];
        let version = meta["io.modelcontextprotocol/protocolVersion"].as_str();
        if version.is_none()
            || !meta["io.modelcontextprotocol/clientCapabilities"].is_object()
            || meta
                .get("io.modelcontextprotocol/clientInfo")
                .is_some_and(|info| !info["name"].is_string() || !info["version"].is_string())
            || meta
                .get("progressToken")
                .is_some_and(|v| !v.is_string() && !v.is_number())
        {
            return error(-32602, "Invalid or missing request metadata");
        }
        let capabilities = &meta["io.modelcontextprotocol/clientCapabilities"];
        for key in [
            "roots",
            "sampling",
            "elicitation",
            "experimental",
            "extensions",
        ] {
            if capabilities.get(key).is_some_and(|v| !v.is_object()) {
                return error(-32602, "Invalid client capability");
            }
        }
        for (key, children) in [
            ("sampling", &["context", "tools"][..]),
            ("elicitation", &["form", "url"][..]),
        ] {
            for child in children {
                if capabilities[key].get(child).is_some_and(|v| !v.is_object()) {
                    return error(-32602, "Invalid client capability");
                }
            }
        }
        if meta
            .get("io.modelcontextprotocol/logLevel")
            .is_some_and(|level| {
                ![
                    "debug",
                    "info",
                    "notice",
                    "warning",
                    "error",
                    "critical",
                    "alert",
                    "emergency",
                ]
                .iter()
                .any(|v| level.as_str() == Some(v))
            })
        {
            return error(-32602, "Invalid logging level");
        }
        if version != Some("2026-07-28") {
            let mut response = error(-32022, "Unsupported protocol version").unwrap();
            response["error"]["data"] = json!({"supported":["2026-07-28"],"requested":version});
            return Some(response);
        }
        let params = &req["params"];
        match req["method"].as_str().unwrap() {
            "tools/call" | "prompts/get"
                if !params["name"].is_string()
                    || params.get("arguments").is_some_and(|v| !v.is_object()) =>
            {
                return error(-32602, "Invalid call parameters");
            }
            "resources/read" if !params["uri"].is_string() => {
                return error(-32602, "Missing resource URI");
            }
            "tools/list" | "resources/list" | "resources/templates/list" | "prompts/list"
                if params.get("cursor").is_some() =>
            {
                return error(-32602, "Invalid cursor: this gateway returns a single page");
            }
            _ => {}
        }
        let response = self.dispatch(request_str);
        if self.cancelled_ids.lock().unwrap().remove(&normalize_id(id)) {
            return None;
        }
        response.map(|mut response| {
            if let Some(result) = response.get_mut("result") {
                result["resultType"] = json!("complete");
                if matches!(
                    req["method"].as_str(),
                    Some(
                        "server/discover"
                            | "tools/list"
                            | "resources/list"
                            | "resources/read"
                            | "resources/templates/list"
                            | "prompts/list"
                            | "prompts/get"
                    )
                ) {
                    result["ttlMs"] = json!(0);
                    result["cacheScope"] = json!("private");
                }
                result["_meta"] = json!({"io.modelcontextprotocol/serverInfo":{
                    "name":"rune-kit","version":env!("CARGO_PKG_VERSION")}});
            }
            response
        })
    }

    fn dispatch(&mut self, request_str: &str) -> Option<Value> {
        let req: Value = match serde_json::from_str(request_str) {
            Ok(val) => val,
            Err(_) => {
                return Some(json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": { "code": -32700, "message": "Parse error" }
                }));
            }
        };

        let id = req.get("id");
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");

        if method == "notifications/cancelled" || method == "$/cancelRequest" {
            let req_id = req
                .get("params")
                .and_then(|p| p.get("requestId").or_else(|| p.get("id")))
                .map(|v| normalize_id(Some(v)))
                .unwrap_or_default();
            if !req_id.is_empty() {
                self.cancel_request(&req_id);
            }
            return None;
        }

        if method.starts_with("notifications/") || method == "initialized" {
            return None;
        }

        match method {
            "server/discover" => Some(json!({
                "jsonrpc":"2.0", "id":id,
                "result": {"supportedVersions":["2026-07-28"],
                    "capabilities":{"tools":{},"resources":{},"prompts":{}},
                    "ttlMs":0,"cacheScope":"private"}
            })),
            "ping" => Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {}
            })),
            "tools/list" => {
                let mut all_tools = Vec::new();
                let mut discovery_errors = Vec::new();
                for (ns, instance) in self.instances.iter_mut() {
                    let _operation = WasmOperation::track(instance, self.wasm_operation.clone());
                    match instance.discover_tools(
                        self.request_cancel.clone(),
                        Duration::from_secs(self.config.tool_timeout_secs.unwrap_or(30)),
                    ) {
                        Ok(tools) => {
                            for tool in tools {
                                let full_name = format!("{}__{}", ns, tool.name);

                                // Allowlist filter
                                if let Some(ref allowlist) = self.config.allowed_tools
                                    && !allowlist.iter().any(|allowed| {
                                        allowed == &full_name || allowed == &tool.name
                                    })
                                {
                                    continue;
                                }

                                // Read-only filter
                                if self.config.read_only {
                                    let is_read_only = tool
                                        .annotations
                                        .as_ref()
                                        .and_then(|a| a.read_only_hint)
                                        .unwrap_or(false);
                                    if !is_read_only {
                                        continue;
                                    }
                                }

                                if let Err(error) = tool_validator(&tool.input_schema) {
                                    discovery_errors.push(format!(
                                        "plugin '{ns}', tool '{}': {error}",
                                        tool.name
                                    ));
                                    continue;
                                }
                                let mut tool_obj = json!({
                                    "name": full_name,
                                    "description": tool.description,
                                    "inputSchema": tool.input_schema,
                                });
                                if let Some(ref ann) = tool.annotations {
                                    // Emit canonical MCP spec 2025-03-26 names (readOnlyHint, etc.)
                                    // AND legacy names (readOnly, etc.) throughout 0.3.x.
                                    // Removal requires a minor release and a migration note.
                                    let mut ann_json =
                                        serde_json::to_value(ann).unwrap_or_default();
                                    if let Some(v) = ann.read_only_hint {
                                        ann_json["readOnly"] = json!(v);
                                    }
                                    if let Some(v) = ann.destructive_hint {
                                        ann_json["destructive"] = json!(v);
                                    }
                                    if let Some(v) = ann.idempotent_hint {
                                        ann_json["idempotent"] = json!(v);
                                    }
                                    if let Some(v) = ann.open_world_hint {
                                        ann_json["openWorld"] = json!(v);
                                    }
                                    tool_obj["annotations"] = ann_json;
                                }
                                all_tools.push(tool_obj);
                            }
                        }
                        Err(err) => {
                            let diagnostic = format!("plugin '{ns}': {err}");
                            eprintln!("[Rune Error] Failed to list tools from {diagnostic}");
                            discovery_errors.push(diagnostic);
                        }
                    }
                }
                // Never turn failed discovery into a successful empty or partial list.
                if !discovery_errors.is_empty() {
                    discovery_errors.sort();
                    return Some(json!({"jsonrpc":"2.0", "id":id,
                        "error":{"code":-32000,"message":format!("Tool discovery failed: {}", discovery_errors.join("; "))}}));
                }
                // KF-3: Warn on stderr when read-only mode yields no tools (all tools are
                // write-capable), so operators can detect a misconfigured --read-only deployment.
                if self.config.read_only && all_tools.is_empty() {
                    eprintln!(
                        "[Rune Warning] --read-only mode is active but no tools are declared \
                         readOnlyHint=true. The tools list exposed to clients is empty. \
                         Declare read_only = true in plugin.toml [tools.<name>] to expose tools \
                         in read-only mode."
                    );
                }
                all_tools.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
                Some(json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": all_tools } }))
            }
            "resources/templates/list" => {
                Some(json!({"jsonrpc":"2.0","id":id,"result":{"resourceTemplates":[]}}))
            }
            "resources/list" => {
                let mut all_resources = Vec::new();
                for (ns, instance) in self.instances.iter_mut() {
                    let _operation = WasmOperation::track(instance, self.wasm_operation.clone());
                    match instance.list_resources() {
                        Ok(resources) => {
                            for res in resources {
                                all_resources.push(json!({
                                    "uri": namespace_resource_uri(ns, &res.uri),
                                    "name": res.name,
                                    "description": res.description,
                                    "mimeType": res.mime_type
                                }));
                            }
                        }
                        Err(err) => {
                            return Some(
                                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":format!("Resource discovery failed: plugin '{ns}': {err}")}}),
                            );
                        }
                    }

                    // Expose declared rules resource for each namespace
                    all_resources.push(json!({
                        "uri": format!("rune://{}/rules", ns),
                        "name": format!("{} Rules & Capabilities", ns),
                        "description": format!("Declared capabilities, tool annotations, and execution rules for plugin '{}'", ns),
                        "mimeType": "application/json"
                    }));
                }
                Some(
                    json!({ "jsonrpc": "2.0", "id": id, "result": { "resources": all_resources } }),
                )
            }
            "prompts/list" => {
                let mut all_prompts = Vec::new();
                for (ns, instance) in self.instances.iter_mut() {
                    let _operation = WasmOperation::track(instance, self.wasm_operation.clone());
                    match instance.list_prompts() {
                        Ok(prompts) => {
                            for prompt in prompts {
                                all_prompts.push(json!({
                                    "name": format!("{}__{}", ns, prompt.name),
                                    "description": prompt.description,
                                    "arguments": prompt.arguments
                                }));
                            }
                        }
                        Err(err) => {
                            return Some(
                                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":format!("Prompt discovery failed: plugin '{ns}': {err}")}}),
                            );
                        }
                    }
                }
                Some(json!({ "jsonrpc": "2.0", "id": id, "result": { "prompts": all_prompts } }))
            }
            "tools/call" => {
                let params = req.get("params").cloned().unwrap_or(json!({}));
                let full_name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

                let parts: Vec<&str> = full_name.splitn(2, "__").collect();
                if parts.len() != 2 {
                    return Some(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32602, "message": "Invalid tool namespacing (expected namespace__tool)" }
                    }));
                }

                let (ns, tool_name) = (parts[0], parts[1]);

                // 1. Allowlist enforcement
                if let Some(ref allowlist) = self.config.allowed_tools
                    && !allowlist
                        .iter()
                        .any(|allowed| allowed == full_name || allowed == tool_name)
                {
                    return Some(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": -32600,
                            "message": format!("Permission denied: tool '{}' is not in the allowlist", full_name)
                        }
                    }));
                }

                // 2. Read-only profile enforcement
                if self.config.read_only {
                    let is_read_only = if let Some(instance) = self.instances.get_mut(ns) {
                        let _operation =
                            WasmOperation::track(instance, self.wasm_operation.clone());
                        if let Ok(tools) = instance.discover_tools(
                            self.request_cancel.clone(),
                            Duration::from_secs(self.config.tool_timeout_secs.unwrap_or(30)),
                        ) {
                            tools
                                .iter()
                                .find(|t| t.name == tool_name)
                                .and_then(|t| t.annotations.as_ref())
                                .and_then(|a| a.read_only_hint)
                                .unwrap_or(false)
                        } else {
                            false
                        }
                    } else {
                        false
                    };

                    if !is_read_only {
                        return Some(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {
                                "code": -32600,
                                "message": format!("Permission denied: gateway is running in read-only mode and tool '{}' is not declared read-only", full_name)
                            }
                        }));
                    }
                }

                let req_id_str = normalize_id(id);

                // 3. Pre-call cancellation check
                if self.cancelled_ids.lock().unwrap().contains(&req_id_str) {
                    return Some(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32800, "message": "Request cancelled by client" }
                    }));
                }

                if let Some(instance) = self.instances.get_mut(ns) {
                    let _operation = WasmOperation::track(instance, self.wasm_operation.clone());
                    let tools = match instance.discover_tools(
                        self.request_cancel.clone(),
                        Duration::from_secs(self.config.tool_timeout_secs.unwrap_or(30)),
                    ) {
                        Ok(tools) => tools,
                        Err(error) => {
                            return Some(
                                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":format!("Tool discovery failed: plugin '{ns}': {error}")}}),
                            );
                        }
                    };
                    let Some(tool) = tools.iter().find(|t| t.name == tool_name) else {
                        return Some(
                            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":format!("Unknown tool: {full_name}")}}),
                        );
                    };
                    let validator = match tool_validator(&tool.input_schema) {
                        Ok(validator) => validator,
                        Err(error) => {
                            return Some(
                                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":format!("Invalid tool schema: {error}")}}),
                            );
                        }
                    };
                    if let Err(error) = validator.validate(&arguments) {
                        return Some(
                            json!({"jsonrpc":"2.0","id":id,"result":{"isError":true,"content":[{"type":"text","text":format!("Invalid tool arguments: {error}")}]}}),
                        );
                    }
                    let cancel_flag = self.request_cancel.clone();
                    let wasm_handle = if let PluginInstance::Wasm(w) = instance {
                        Some(w.get_cancel_handle())
                    } else {
                        None
                    };

                    let cancelled = self.cancelled_ids.lock().unwrap();
                    if cancelled.contains(&req_id_str) {
                        return None;
                    }
                    self.active_cancels.lock().unwrap().insert(
                        req_id_str.clone(),
                        CancelState {
                            flag: cancel_flag.clone(),
                            wasm_handle,
                        },
                    );

                    drop(cancelled);
                    let timeout = self.config.tool_timeout_secs.map(Duration::from_secs);
                    let call_res =
                        instance.call_tool_with_cancel(tool_name, arguments, cancel_flag, timeout);

                    self.active_cancels.lock().unwrap().remove(&req_id_str);

                    match call_res {
                        Ok(res) => {
                            let content = if let Some(content_arr) =
                                res.get("content").and_then(|c| c.as_array())
                            {
                                json!(content_arr)
                            } else if let Some(content_str) =
                                res.get("content").and_then(|c| c.as_str())
                            {
                                json!([{ "type": "text", "text": content_str }])
                            } else if let Some(text) = res.as_str() {
                                json!([{ "type": "text", "text": text }])
                            } else {
                                json!([{ "type": "text", "text": res.to_string() }])
                            };
                            let is_error = res
                                .get("isError")
                                .and_then(|e| e.as_bool())
                                .unwrap_or(false);
                            Some(json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": {
                                   "content": content,
                                   "isError": is_error
                                }
                            }))
                        }
                        Err(RuntimeError::Cancelled) => Some(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": { "code": -32800, "message": "Request cancelled by client" }
                        })),
                        Err(RuntimeError::Timeout(secs)) => Some(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": { "code": -32000, "message": format!("Tool execution timed out after {} seconds", secs) }
                        })),
                        Err(e) => Some(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": { "code": -32000, "message": e.to_string() }
                        })),
                    }
                } else {
                    Some(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32601, "message": format!("Namespace '{}' not found", ns) }
                    }))
                }
            }
            "resources/read" => {
                let params = req.get("params").cloned().unwrap_or(json!({}));
                let uri = params.get("uri").and_then(|u| u.as_str()).unwrap_or("");
                match parse_resource_uri(uri) {
                    Some((ns, local_uri)) => match self.instances.get_mut(ns) {
                        Some(instance) => {
                            let _operation =
                                WasmOperation::track(instance, self.wasm_operation.clone());
                            if local_uri == "rules" {
                                let rules_json = serde_json::to_string_pretty(&json!({
                                    "namespace": ns,
                                    "manifest": instance.manifest(),
                                }))
                                .unwrap_or_default();

                                Some(json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "result": {
                                        "contents": [{
                                            "uri": format!("rune://{}/rules", ns),
                                            "mimeType": "application/json",
                                            "text": rules_json
                                        }]
                                    }
                                }))
                            } else {
                                match instance.read_resource(local_uri) {
                                    Ok(res) => Some(json!({
                                        "jsonrpc": "2.0",
                                        "id": id,
                                        "result": res
                                    })),
                                    Err(e) => Some(json!({
                                        "jsonrpc": "2.0",
                                        "id": id,
                                        "error": { "code": -32000, "message": e.to_string() }
                                    })),
                                }
                            }
                        }
                        None => Some(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": { "code": -32601, "message": format!("Namespace '{}' not found", ns) }
                        })),
                    },
                    None => Some(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": -32602,
                            "message": "Invalid resource URI (expected rune://<namespace>/<local-uri>)"
                        }
                    })),
                }
            }
            "prompts/get" => {
                let params = req.get("params").cloned().unwrap_or(json!({}));
                let full_name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

                let parts: Vec<&str> = full_name.splitn(2, "__").collect();
                if parts.len() != 2 {
                    return Some(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32602, "message": "Invalid prompt namespacing (expected namespace__prompt)" }
                    }));
                }

                let (ns, prompt_name) = (parts[0], parts[1]);
                match self.instances.get_mut(ns) {
                    Some(instance) => {
                        let _operation =
                            WasmOperation::track(instance, self.wasm_operation.clone());
                        match instance.get_prompt(prompt_name, arguments) {
                            Ok(res) => Some(json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": res
                            })),
                            Err(e) => Some(json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "error": { "code": -32000, "message": e.to_string() }
                            })),
                        }
                    }
                    None => Some(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32601, "message": format!("Namespace '{}' not found", ns) }
                    })),
                }
            }
            _ => Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("Method '{}' not found", method) }
            })),
        }
    }
}

#[cfg(test)]
mod schema_tests {
    use super::*;
    #[test]
    fn default_draft_and_local_references_validate_without_network() {
        let validator = tool_validator(&json!({"type":"object","$defs":{"path":{"type":"string"}},"properties":{"path":{"$ref":"#/$defs/path"}},"required":["path"],"unevaluatedProperties":false})).unwrap();
        assert!(validator.is_valid(&json!({"path":"ok"})));
        assert!(!validator.is_valid(&json!({"path":1})));
        assert!(!validator.is_valid(&json!({"path":"ok","extra":1})));
        assert!(
            tool_validator(&json!({"type":"object","$ref":"https://example.invalid/schema"}))
                .is_err()
        );
        assert!(
            tool_validator(&json!({"type":"object","$schema":"https://example.invalid/dialect"}))
                .is_err()
        );
    }
}
