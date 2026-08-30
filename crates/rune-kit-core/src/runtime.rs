// crates/rune-kit-core/src/runtime.rs
use crate::manifest::{PluginInfo, ToolDefinition};
use extism::{Manifest, Plugin, Wasm};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
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

pub struct WasmPluginInstance {
    plugin: Plugin,
    pub name: String,
}

impl WasmPluginInstance {
    /// Load a WASM plugin from a file path on disk
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

    /// Probe the WASM binary for compile-time metadata
    pub fn get_info(&mut self) -> Result<PluginInfo, RuntimeError> {
        let raw = self
            .plugin
            .call::<(), String>("mcp_info", ())
            .map_err(|e| RuntimeError::Execution(e.to_string()))?;

        let info: PluginInfo = serde_json::from_str(&raw)?;
        Ok(info)
    }

    /// Load a WASM plugin directly from raw in-memory bytes
    pub fn load_from_bytes(
        name: &str,
        bytes: Vec<u8>,
        params: HashMap<String, String>,
    ) -> Result<Self, RuntimeError> {
        let mut manifest = Manifest::new([Wasm::data(bytes)]);

        // Grant WASI filesystem access to the allowed directory if specified
        if let Some(allowed_dir) = params.get("allowed_dir") {
            manifest = manifest.with_allowed_path(allowed_dir.clone(), allowed_dir.clone());
        }
        // Also allow access to the relative working directory
        manifest = manifest.with_allowed_path(".".to_string(), ".");

        let manifest = manifest.with_config(params.into_iter());

        // Initialize Extism plugin with WASI enabled
        let plugin = Plugin::new(&manifest, [], true)
            .map_err(|e| RuntimeError::ExtismInit(e.to_string()))?;

        Ok(Self {
            plugin,
            name: name.to_string(),
        })
    }

    /// Invoke `mcp_list_tools` on the WASM plugin
    pub fn list_tools(&mut self) -> Result<Vec<ToolDefinition>, RuntimeError> {
        let raw = self
            .plugin
            .call::<(), String>("mcp_list_tools", ())
            .map_err(|e| RuntimeError::Execution(e.to_string()))?;

        let tools: Vec<ToolDefinition> = serde_json::from_str(&raw)?;
        Ok(tools)
    }

    /// Invoke `mcp_call_tool` with arguments on the WASM plugin
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
