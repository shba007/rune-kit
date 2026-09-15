use crate::runtime::PluginInstance;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};

pub struct McpRouter {
    instances: HashMap<String, PluginInstance>,
}

pub fn namespace_resource_uri(namespace: &str, local_uri: &str) -> String {
    format!("rune://{}/{}", namespace, local_uri)
}

pub fn parse_resource_uri(uri: &str) -> Option<(&str, &str)> {
    let rest = uri.strip_prefix("rune://")?;
    let (namespace, local_uri) = rest.split_once('/')?;
    Some((namespace, local_uri))
}

impl McpRouter {
    pub fn new() -> Self {
        Self {
            instances: HashMap::new(),
        }
    }

    pub fn register(&mut self, namespace: String, instance: PluginInstance) {
        self.instances.insert(namespace, instance);
    }

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

        if method.starts_with("notifications/") || method == "initialized" {
            return None;
        }

        match method {
            "initialize" => Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": { "listChanged": false },
                        "resources": {},
                        "prompts": {}
                    },
                    "serverInfo": { "name": "rune-kit", "version": "0.1.0" }
                }
            })),
            "ping" => Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {}
            })),
            "tools/list" => {
                let mut all_tools = Vec::new();
                for (ns, instance) in self.instances.iter_mut() {
                    match instance.list_tools() {
                        Ok(tools) => {
                            for tool in tools {
                                all_tools.push(json!({
                                    "name": format!("{}__{}", ns, tool.name),
                                    "description": tool.description,
                                    "inputSchema": tool.input_schema
                                }));
                            }
                        }
                        Err(err) => {
                            eprintln!("[Rune Error] Failed to list tools from '{}': {}", ns, err);
                        }
                    }
                }
                Some(json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": all_tools } }))
            }
            "resources/list" => {
                let mut all_resources = Vec::new();
                for (ns, instance) in self.instances.iter_mut() {
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
                            eprintln!("[Rune Error] Failed to list resources from '{}': {}", ns, err);
                        }
                    }
                }
                Some(json!({ "jsonrpc": "2.0", "id": id, "result": { "resources": all_resources } }))
            }
            "prompts/list" => {
                let mut all_prompts = Vec::new();
                for (ns, instance) in self.instances.iter_mut() {
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
                            eprintln!("[Rune Error] Failed to list prompts from '{}': {}", ns, err);
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
                if let Some(instance) = self.instances.get_mut(ns) {
                    match instance.call_tool(tool_name, arguments) {
                        Ok(res) => {
                            let content = if let Some(content_arr) =
                                res.get("content").and_then(|c| c.as_array())
                            {
                                json!(content_arr)
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
                        Some(instance) => match instance.read_resource(local_uri) {
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
                        },
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
                    Some(instance) => match instance.get_prompt(prompt_name, arguments) {
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
                    },
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
