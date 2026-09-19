use crate::manifest::{PluginInfo, PromptDefinition, ResourceDefinition, ToolDefinition};
use extism::{Manifest, PTR, Plugin, PluginBuilder, UserData, Wasm, host_fn};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RuntimeError {
    #[error("Plugin file not found: {0}")]
    FileNotFound(String),
    #[error("Failed to initialize Extism plugin: {0}")]
    ExtismInit(String),
    #[error("Execution failed: {0}")]
    Execution(String),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
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

host_fn!(host_cmd_exec(input: String) -> Result<String, extism::Error> {
    let req: CmdExecRequest = serde_json::from_str(&input).map_err(|e| extism::Error::msg(format!("Invalid JSON: {}", e)))?;
    
    let mut prog_path = PathBuf::from(&req.program);
    if !prog_path.is_absolute() && !prog_path.exists() {
        if let Some(data_dir) = dirs::data_dir() {
            let plugins_dir = data_dir.join("rune-kit").join("plugins");
            let candidate = plugins_dir.join(&req.program);
            let candidate_exe = plugins_dir.join(format!("{}.exe", req.program));
            if candidate.exists() { prog_path = candidate; }
            else if candidate_exe.exists() { prog_path = candidate_exe; }
        }
    }

    let output = Command::new(&prog_path).args(&req.args).output().map_err(|e| extism::Error::msg(e.to_string()))?;

    let resp = CmdExecResponse {
        success: output.status.success(),
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    };
    Ok(serde_json::to_string(&resp).unwrap_or_else(|_| "{}".to_string()))
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
            for host in allowed_hosts
                .split(',')
                .map(str::trim)
                .filter(|h| !h.is_empty())
            {
                manifest = manifest.with_allowed_host(host.to_string());
            }
        } else {
            manifest = manifest.with_allowed_host("*".to_string());
        }

        if let Some(printer_ip) = params.get("printer_ip").filter(|s| !s.is_empty()) {
            manifest = manifest.with_allowed_host(printer_ip.clone());
        }

        if let Some(allowed_dir) = params.get("allowed_dir") {
            manifest = manifest.with_allowed_path(allowed_dir.clone(), allowed_dir.clone());
        } else {
            manifest = manifest.with_allowed_path(".".to_string(), ".");
        }

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

    pub fn list_resources(&mut self) -> Result<Vec<ResourceDefinition>, RuntimeError> {
        // MCP plugins don't implement resources; always empty
        Ok(Vec::new())
    }

    pub fn read_resource(&mut self, _uri: &str) -> Result<Value, RuntimeError> {
        // MCP plugins don't implement resources; always empty
        Ok(Value::Null)
    }

    pub fn list_prompts(&mut self) -> Result<Vec<PromptDefinition>, RuntimeError> {
        // MCP plugins don't implement prompts; always empty
        Ok(Vec::new())
    }

    pub fn get_prompt(&mut self, _name: &str, _arguments: Value) -> Result<Value, RuntimeError> {
        // MCP plugins don't implement prompts; always empty
        Ok(Value::Null)
    }
}

pub struct NativeSidecar {
    pub name: String,
    binary_path: PathBuf,
    params: HashMap<String, String>,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    reader: Option<BufReader<ChildStdout>>,
    next_id: u64,
}

impl NativeSidecar {
    pub fn new<P: AsRef<Path>>(
        name: &str,
        path: P,
        params: HashMap<String, String>,
    ) -> Result<Self, RuntimeError> {
        let binary_path = path.as_ref().to_path_buf();
        if !binary_path.exists() {
            return Err(RuntimeError::FileNotFound(
                binary_path.display().to_string(),
            ));
        }

        let mut sidecar = Self {
            name: name.to_string(),
            binary_path,
            params,
            child: None,
            stdin: None,
            reader: None,
            next_id: 1,
        };

        sidecar.spawn_process()?;
        Ok(sidecar)
    }

    fn spawn_process(&mut self) -> Result<(), RuntimeError> {
        let mut cmd = Command::new(&self.binary_path);
        cmd.envs(&self.params);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::inherit());

        let mut child = cmd.spawn().map_err(|e| {
            RuntimeError::Execution(format!(
                "Failed to spawn native sidecar '{}': {}",
                self.binary_path.display(),
                e
            ))
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            RuntimeError::Execution("Failed to open child process stdin".to_string())
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            RuntimeError::Execution("Failed to open child process stdout".to_string())
        })?;

        self.child = Some(child);
        self.stdin = Some(stdin);
        self.reader = Some(BufReader::new(stdout));
        Ok(())
    }

    fn send_request(&mut self, method: &str, params: Value) -> Result<Value, RuntimeError> {
        if self.child.is_none() {
            self.spawn_process()?;
        }

        let req_id = self.next_id;
        self.next_id += 1;

        let req = json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": method,
            "params": params
        });

        let mut req_str = serde_json::to_string(&req)?;
        req_str.push('\n');

        let stdin = self.stdin.as_mut().ok_or_else(|| {
            RuntimeError::Execution("Child process stdin is not available".to_string())
        })?;

        stdin.write_all(req_str.as_bytes()).map_err(|e| {
            RuntimeError::Execution(format!("Failed to write to native sidecar stdin: {}", e))
        })?;
        stdin.flush().map_err(|e| {
            RuntimeError::Execution(format!("Failed to flush native sidecar stdin: {}", e))
        })?;

        let reader = self.reader.as_mut().ok_or_else(|| {
            RuntimeError::Execution("Child process stdout is not available".to_string())
        })?;

        let mut line = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line).map_err(|e| {
                RuntimeError::Execution(format!("Failed to read from native sidecar stdout: {}", e))
            })?;
            if n == 0 {
                return Err(RuntimeError::Execution(
                    "Native sidecar process closed stdout unexpectedly".to_string(),
                ));
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            if let Ok(val) = serde_json::from_str::<Value>(trimmed)
                && val.is_object()
            {
                return Ok(val);
            }
        }
    }

    pub fn get_info(&mut self) -> Result<PluginInfo, RuntimeError> {
        let resp = self.send_request("initialize", json!({}))?;
        if let Some(err) = resp.get("error") {
            return Err(RuntimeError::Execution(format!(
                "Sidecar initialize error: {}",
                err
            )));
        }

        let res = resp.get("result").unwrap_or(&resp);
        let info_val = res.get("serverInfo").unwrap_or(res);

        let name = info_val
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&self.name)
            .to_string();
        let version = info_val
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or("0.1.0")
            .to_string();
        let description = info_val
            .get("description")
            .and_then(Value::as_str)
            .map(ToString::to_string);

        Ok(PluginInfo {
            name,
            version,
            description,
        })
    }

    pub fn list_tools(&mut self) -> Result<Vec<ToolDefinition>, RuntimeError> {
        let resp = self.send_request("tools/list", json!({}))?;
        if let Some(err) = resp.get("error") {
            return Err(RuntimeError::Execution(format!(
                "Sidecar tools/list error: {}",
                err
            )));
        }

        let res = resp.get("result").unwrap_or(&resp);
        let tools_val = res.get("tools").unwrap_or(res);
        let tools: Vec<ToolDefinition> = serde_json::from_value(tools_val.clone())?;
        Ok(tools)
    }

    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, RuntimeError> {
        let resp = self.send_request(
            "tools/call",
            json!({
                "name": name,
                "arguments": arguments
            }),
        )?;

        if let Some(err) = resp.get("error") {
            let msg = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Unknown sidecar execution error");
            return Err(RuntimeError::Execution(msg.to_string()));
        }

        let res = resp.get("result").unwrap_or(&resp);
        if let Some(content_arr) = res.get("content").and_then(Value::as_array)
            && let Some(first) = content_arr.first()
            && let Some(text) = first.get("text").and_then(Value::as_str)
        {
            if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                return Ok(parsed);
            }
            return Ok(Value::String(text.to_string()));
        }

        Ok(res.clone())
    }

    pub fn list_resources(&mut self) -> Result<Vec<ResourceDefinition>, RuntimeError> {
        // Native sidecars don't implement resources; always empty
        Ok(Vec::new())
    }

    pub fn read_resource(&mut self, _uri: &str) -> Result<Value, RuntimeError> {
        // Native sidecars don't implement resources; always empty
        Ok(Value::Null)
    }

    pub fn list_prompts(&mut self) -> Result<Vec<PromptDefinition>, RuntimeError> {
        // Native sidecars don't implement prompts; always empty
        Ok(Vec::new())
    }

    pub fn get_prompt(&mut self, _name: &str, _arguments: Value) -> Result<Value, RuntimeError> {
        // Native sidecars don't implement prompts; always empty
        Ok(Value::Null)
    }
}

impl Drop for NativeSidecar {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub enum PluginInstance {
    Wasm(WasmPluginInstance),
    Native(NativeSidecar),
}

impl PluginInstance {
    pub fn load_wasm_from_file<P: AsRef<Path>>(
        name: &str,
        path: P,
        params: HashMap<String, String>,
    ) -> Result<Self, RuntimeError> {
        WasmPluginInstance::load_from_file(name, path, params).map(PluginInstance::Wasm)
    }

    pub fn load_native_from_file<P: AsRef<Path>>(
        name: &str,
        path: P,
        params: HashMap<String, String>,
    ) -> Result<Self, RuntimeError> {
        NativeSidecar::new(name, path, params).map(PluginInstance::Native)
    }

    pub fn get_info(&mut self) -> Result<PluginInfo, RuntimeError> {
        match self {
            PluginInstance::Wasm(w) => w.get_info(),
            PluginInstance::Native(n) => n.get_info(),
        }
    }

    pub fn list_tools(&mut self) -> Result<Vec<ToolDefinition>, RuntimeError> {
        match self {
            PluginInstance::Wasm(w) => w.list_tools(),
            PluginInstance::Native(n) => n.list_tools(),
        }
    }

    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, RuntimeError> {
        match self {
            PluginInstance::Wasm(w) => w.call_tool(name, arguments),
            PluginInstance::Native(n) => n.call_tool(name, arguments),
        }
    }

    pub fn list_resources(&mut self) -> Result<Vec<ResourceDefinition>, RuntimeError> {
        match self {
            PluginInstance::Wasm(w) => w.list_resources(),
            PluginInstance::Native(n) => n.list_resources(),
        }
    }

    pub fn read_resource(&mut self, uri: &str) -> Result<Value, RuntimeError> {
        match self {
            PluginInstance::Wasm(w) => w.read_resource(uri),
            PluginInstance::Native(n) => n.read_resource(uri),
        }
    }

    pub fn list_prompts(&mut self) -> Result<Vec<PromptDefinition>, RuntimeError> {
        match self {
            PluginInstance::Wasm(w) => w.list_prompts(),
            PluginInstance::Native(n) => n.list_prompts(),
        }
    }

    pub fn get_prompt(&mut self, name: &str, arguments: Value) -> Result<Value, RuntimeError> {
        match self {
            PluginInstance::Wasm(w) => w.get_prompt(name, arguments),
            PluginInstance::Native(n) => n.get_prompt(name, arguments),
        }
    }
}
