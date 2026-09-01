// crates/rune-kit-core/src/runtime.rs
use crate::manifest::{PluginInfo, ToolDefinition};
use extism::{Error as ExtismError, Manifest, PTR, Plugin, PluginBuilder, UserData, Wasm, host_fn};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RuntimeError {
    #[error("Plugin file not found: {0}")]
    FileNotFound(String),
    #[error("Failed to initialize Extism plugin: {0}")]
    ExtismInit(String),
    #[error("WASM execution failed: {0}")]
    Execution(String),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Serialize, Deserialize)]
pub struct CmdExecRequest {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct CmdExecResponse {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

// Declarative host_fn! macro generates the callback for Extism
host_fn!(host_cmd_exec(input: String) -> String {
    let req: CmdExecRequest = serde_json::from_str(&input)
        .map_err(|e| ExtismError::msg(format!("Invalid command request payload: {}", e)))?;

    let mut cmd = Command::new(&req.program);
    cmd.args(&req.args);

    if let Some(cwd) = &req.cwd {
        cmd.current_dir(cwd);
    }

    match cmd.output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let resp = CmdExecResponse {
                success: output.status.success(),
                exit_code: output.status.code(),
                stdout,
                stderr,
            };
            let json_str = serde_json::to_string(&resp)
                .map_err(|e| ExtismError::msg(e.to_string()))?;
            Ok(json_str)
        }
        Err(e) => {
            let resp = CmdExecResponse {
                success: false,
                exit_code: None,
                stdout: String::new(),
                stderr: format!("Host binary '{}' failed or not found in PATH: {}", req.program, e),
            };
            let json_str = serde_json::to_string(&resp)
                .map_err(|e| ExtismError::msg(e.to_string()))?;
            Ok(json_str)
        }
    }
});

pub struct WasmPluginInstance {
    plugin: Plugin,
    pub name: String,
}

impl WasmPluginInstance {
    pub fn load_from_file<P: AsRef<Path>>(
        name: &str,
        path: P,
        params: HashMap<String, String>,
    ) -> Result<Self, RuntimeError> {
        let path_ref = path.as_ref();
        let wasm_bytes = std::fs::read(path_ref)
            .map_err(|_| RuntimeError::FileNotFound(path_ref.display().to_string()))?;

        Self::load_from_bytes(name, wasm_bytes, params)
    }

    pub fn load_from_bytes(
        name: &str,
        bytes: Vec<u8>,
        params: HashMap<String, String>,
    ) -> Result<Self, RuntimeError> {
        let mut manifest = Manifest::new([Wasm::data(bytes)]);

        if let Some(allowed_hosts) = params.get("allowed_hosts") {
            for host in allowed_hosts.split(',') {
                manifest = manifest.with_allowed_host(host.trim().to_string());
            }
        } else {
            manifest = manifest.with_allowed_host("*".to_string());
        }

        if let Some(printer_ip) = params.get("printer_ip").filter(|s| !s.is_empty()) {
            manifest = manifest.with_allowed_host(printer_ip.clone());
        }

        if let Some(allowed_dir) = params.get("allowed_dir") {
            manifest = manifest.with_allowed_path(allowed_dir.clone(), allowed_dir.clone());
        }
        manifest = manifest.with_allowed_path(".".to_string(), ".");

        let manifest = manifest.with_config(params.into_iter());

        let plugin = PluginBuilder::new(manifest)
            .with_wasi(true)
            .with_function(
                "host_cmd_exec",
                [PTR],
                [PTR],
                UserData::new(()),
                host_cmd_exec,
            )
            .build()
            .map_err(|e| RuntimeError::ExtismInit(e.to_string()))?;

        Ok(Self {
            plugin,
            name: name.to_string(),
        })
    }

    pub fn get_info(&mut self) -> Result<PluginInfo, RuntimeError> {
        let raw = self
            .plugin
            .call::<(), String>("mcp_info", ())
            .map_err(|e| RuntimeError::Execution(e.to_string()))?;

        let info: PluginInfo = serde_json::from_str(&raw)?;
        Ok(info)
    }

    pub fn list_tools(&mut self) -> Result<Vec<ToolDefinition>, RuntimeError> {
        let raw = self
            .plugin
            .call::<(), String>("mcp_list_tools", ())
            .map_err(|e| RuntimeError::Execution(e.to_string()))?;

        let tools: Vec<ToolDefinition> = serde_json::from_str(&raw)?;
        Ok(tools)
    }

    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, RuntimeError> {
        let payload = json!({
            "name": name,
            "arguments": arguments
        })
        .to_string();

        let raw = self
            .plugin
            .call::<&str, String>("mcp_call_tool", &payload)
            .map_err(|e| RuntimeError::Execution(e.to_string()))?;

        let parsed: Value = serde_json::from_str(&raw)?;
        Ok(parsed)
    }
}
