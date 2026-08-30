// crates/rune-kit-core/src/protocol.rs
use crate::runtime::WasmPluginInstance;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};

pub struct McpRouter {
    instances: HashMap<String, WasmPluginInstance>,
}

impl McpRouter {
    pub fn new() -> Self {
        Self {
            instances: HashMap::new(),
        }
    }

    /// Register a WASM plugin instance under a specific namespace
    pub fn register(&mut self, namespace: String, instance: WasmPluginInstance) {
        self.instances.insert(namespace, instance);
    }

    /// Route and handle incoming JSON-RPC 2.0 requests
    pub fn handle_jsonrpc(&mut self, request_str: &str) -> Option<Value> {
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

        match method {
            "initialize" => Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "rune-kit", "version": "0.1.0" }
                }
            })),
            "tools/list" => {
                let mut all_tools = Vec::new();
                for (ns, instance) in self.instances.iter_mut() {
                    if let Ok(tools) = instance.list_tools() {
                        for tool in tools {
                            all_tools.push(json!({
                                "name": format!("{}__{}", ns, tool.name),
                                "description": tool.description,
                                "inputSchema": tool.input_schema
                            }));
                        }
                    }
                }
                Some(json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": all_tools } }))
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
                if let Some(instance) = self.instances.get_mut(ns) {
                    match instance.call_tool(tool_name, arguments) {
                        Ok(res) => Some(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "content": [{ "type": "text", "text": res.to_string() }]
                            }
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
            _ => Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": "Method not found" }
            })),
        }
    }

    /// Run the standard input/output transport loop
    pub fn run_stdio(&mut self) -> Result<(), io::Error> {
        let stdin = io::stdin();
        let mut stdout = io::stdout();

        for line in stdin.lock().lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            if let Some(res) = self.handle_jsonrpc(&line) {
                writeln!(stdout, "{}", serde_json::to_string(&res)?)?;
                stdout.flush()?;
            }
        }
        Ok(())
    }
}
