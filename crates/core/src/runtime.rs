use crate::manifest::{
    PluginInfo, PluginManifest, PromptDefinition, ResourceDefinition, ToolDefinition,
};
use crate::provision::{BinaryProvisioner, ProvisionError};
use extism::{Manifest, PTR, Plugin, PluginBuilder, UserData, Wasm, host_fn};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
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
    #[error("Provision error: {0}")]
    Provision(#[from] ProvisionError),
}

#[derive(Clone)]
pub struct HostContext {
    pub plugin_name: String,
    pub allowed_binaries: Vec<String>,
    pub plugins_dir: PathBuf,
    pub provisioner: BinaryProvisioner,
    pub sidecar: Arc<Mutex<Option<NativeSidecar>>>,
    pub params: HashMap<String, String>,
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

#[derive(Serialize, Deserialize)]
pub struct SidecarExecRequest {
    pub method: String,
    pub params: Value,
}

host_fn!(host_cmd_exec(user_data: HostContext; input: String) -> Result<String, extism::Error> {
    let data = user_data.get()?;
    let ctx = data
        .lock()
        .map_err(|_| extism::Error::msg("Failed to lock host context"))?;

    let req: CmdExecRequest = serde_json::from_str(&input)
        .map_err(|e| extism::Error::msg(format!("Invalid JSON in host_cmd_exec: {}", e)))?;

    if !ctx.allowed_binaries.iter().any(|b| b == &req.program) {
        let resp = CmdExecResponse {
            success: false,
            exit_code: Some(1),
            stdout: String::new(),
            stderr: format!("Unauthorized execution: '{}' is not in capabilities.exec.allowed_binaries", req.program),
        };
        return Ok(serde_json::to_string(&resp)?);
    }

    let mut prog_path = PathBuf::from(&req.program);
    if !prog_path.is_absolute() {
        let candidate_sidecar = ctx.plugins_dir.join(format!("rune-{}-native", req.program));
        let candidate_sidecar_exe = ctx.plugins_dir.join(format!("rune-{}-native.exe", req.program));
        let candidate = ctx.plugins_dir.join(&req.program);
        let candidate_exe = ctx.plugins_dir.join(format!("{}.exe", req.program));

        if candidate_sidecar.exists() {
            prog_path = candidate_sidecar;
        } else if candidate_sidecar_exe.exists() {
            prog_path = candidate_sidecar_exe;
        } else if candidate.exists() {
            prog_path = candidate;
        } else if candidate_exe.exists() {
            prog_path = candidate_exe;
        } else if let Ok(prov_path) = ctx.provisioner.resolve_binary(&req.program, &ctx.allowed_binaries) {
            prog_path = prov_path;
        }
    }

    let mut cmd = Command::new(&prog_path);
    cmd.args(&req.args);
    cmd.envs(&ctx.params);
    if let Some(ref cwd) = req.cwd {
        cmd.current_dir(cwd);
    }

    let output = cmd.output().map_err(|e| extism::Error::msg(format!("Execution failed: {}", e)))?;

    let resp = CmdExecResponse {
        success: output.status.success(),
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    };
    Ok(serde_json::to_string(&resp)?)
});

host_fn!(host_sidecar_exec(user_data: HostContext; input: String) -> Result<String, extism::Error> {
    let data = user_data.get()?;
    let ctx = data
        .lock()
        .map_err(|_| extism::Error::msg("Failed to lock host context"))?;

    let req: SidecarExecRequest = serde_json::from_str(&input)
        .map_err(|e| extism::Error::msg(format!("Invalid JSON in host_sidecar_exec: {}", e)))?;

    let mut sidecar_lock = ctx
        .sidecar
        .lock()
        .map_err(|_| extism::Error::msg("Failed to lock sidecar mutex"))?;

    if sidecar_lock.is_none() {
        let sidecar_name = format!("rune-{}-native", ctx.plugin_name);
        let mut sidecar_path = ctx.plugins_dir.join(&sidecar_name);
        if !sidecar_path.exists() {
            sidecar_path = ctx.plugins_dir.join(format!("{}.exe", sidecar_name));
        }
        if !sidecar_path.exists() {
            sidecar_path = ctx.plugins_dir.join(&ctx.plugin_name);
        }

        if !sidecar_path.exists() {
            return Err(extism::Error::msg(format!(
                "Companion native sidecar '{}' not found in plugin directory",
                sidecar_name
            )));
        }

        let sidecar = NativeSidecar::new(&ctx.plugin_name, &sidecar_path, ctx.params.clone())
            .map_err(|e| extism::Error::msg(format!("Failed to spawn companion sidecar: {}", e)))?;
        *sidecar_lock = Some(sidecar);
    }

    let sidecar = sidecar_lock.as_mut().unwrap();
    let res = sidecar
        .send_request(&req.method, req.params)
        .map_err(|e| extism::Error::msg(format!("Companion sidecar IPC error: {}", e)))?;

    Ok(serde_json::to_string(&res)?)
});

host_fn!(host_get_binary_path(user_data: HostContext; input: String) -> Result<String, extism::Error> {
    let data = user_data.get()?;
    let ctx = data
        .lock()
        .map_err(|_| extism::Error::msg("Failed to lock host context"))?;

    let bin_name = input.trim().trim_matches('"');

    match ctx.provisioner.resolve_binary(bin_name, &ctx.allowed_binaries) {
        Ok(path) => {
            let res = json!({
                "status": "ok",
                "path": path.to_string_lossy().to_string()
            });
            Ok(serde_json::to_string(&res)?)
        }
        Err(ProvisionError::InProgress { progress_percent, eta_seconds, .. }) => {
            let res = json!({
                "status": "error",
                "error": format!("{} is currently being provisioned ({}% downloaded). Please retry in {} seconds.", bin_name, progress_percent, eta_seconds)
            });
            Ok(serde_json::to_string(&res)?)
        }
        Err(err) => {
            let res = json!({
                "status": "error",
                "error": err.to_string()
            });
            Ok(serde_json::to_string(&res)?)
        }
    }
});

pub struct WasmPluginInstance {
    plugin: Plugin,
    pub name: String,
    pub manifest: PluginManifest,
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

        let manifest = if let Some(parent) = path_ref.parent() {
            let manifest_path = parent.join("plugin.toml");
            if manifest_path.exists() {
                PluginManifest::from_file(&manifest_path).unwrap_or_default()
            } else {
                PluginManifest::default()
            }
        } else {
            PluginManifest::default()
        };

        Self::load_from_bytes(name, wasm_bytes, params, manifest)
    }

    pub fn load_from_bytes(
        name: &str,
        bytes: Vec<u8>,
        params: HashMap<String, String>,
        manifest: PluginManifest,
    ) -> Result<Self, RuntimeError> {
        let mut extism_manifest = Manifest::new([Wasm::data(bytes)]);

        let get_param = |key: &str| -> Option<String> {
            params.get(key).cloned().or_else(|| {
                params
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(key))
                    .map(|(_, v)| v.clone())
            })
        };

        let mut allowed_hosts: Vec<String> = manifest.capabilities.network_hosts.clone();
        if let Some(param_hosts) = get_param("allowed_hosts") {
            for h in param_hosts
                .split(',')
                .map(str::trim)
                .filter(|h| !h.is_empty())
            {
                allowed_hosts.push(h.to_string());
            }
        }

        if allowed_hosts.is_empty() {
            extism_manifest = extism_manifest.with_allowed_host("*".to_string());
        } else {
            for host in allowed_hosts {
                extism_manifest = extism_manifest.with_allowed_host(host);
            }
        }

        if let Some(printer_ip) = get_param("printer_ip").filter(|s| !s.is_empty()) {
            extism_manifest = extism_manifest.with_allowed_host(printer_ip);
        }

        if let Some(ref fs) = manifest.capabilities.filesystem
            && let Some(ref param_key) = fs.root_param
            && let Some(root_path) = get_param(param_key)
        {
            extism_manifest = extism_manifest.with_allowed_path(root_path.clone(), root_path);
        } else if let Some(allowed_dir) = get_param("allowed_dir") {
            extism_manifest = extism_manifest.with_allowed_path(allowed_dir.clone(), allowed_dir);
        } else {
            extism_manifest = extism_manifest.with_allowed_path(".".to_string(), ".");
        }

        let extism_manifest = extism_manifest.with_config(params.clone().into_iter());

        let plugins_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("rune-kit")
            .join("plugins");

        let host_ctx = HostContext {
            plugin_name: name.to_string(),
            allowed_binaries: manifest.capabilities.exec.allowed_binaries.clone(),
            plugins_dir,
            provisioner: BinaryProvisioner::new(),
            sidecar: Arc::new(Mutex::new(None)),
            params,
        };

        let plugin = PluginBuilder::new(extism_manifest)
            .with_wasi(true)
            .with_function(
                "host_cmd_exec",
                [PTR],
                [PTR],
                UserData::new(host_ctx.clone()),
                host_cmd_exec,
            )
            .with_function(
                "host_sidecar_exec",
                [PTR],
                [PTR],
                UserData::new(host_ctx.clone()),
                host_sidecar_exec,
            )
            .with_function(
                "host_get_binary_path",
                [PTR],
                [PTR],
                UserData::new(host_ctx),
                host_get_binary_path,
            )
            .build()
            .map_err(|e| RuntimeError::ExtismInit(e.to_string()))?;

        Ok(Self {
            plugin,
            name: name.to_string(),
            manifest,
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
        match self.plugin.call::<(), String>("mcp_list_resources", ()) {
            Ok(raw) => {
                let resources: Vec<ResourceDefinition> = serde_json::from_str(&raw)?;
                Ok(resources)
            }
            Err(_) => Ok(Vec::new()),
        }
    }

    pub fn read_resource(&mut self, uri: &str) -> Result<Value, RuntimeError> {
        match self.plugin.call::<&str, String>("mcp_read_resource", uri) {
            Ok(raw) => {
                let val: Value = serde_json::from_str(&raw)?;
                Ok(val)
            }
            Err(_) => Ok(Value::Null),
        }
    }

    pub fn list_prompts(&mut self) -> Result<Vec<PromptDefinition>, RuntimeError> {
        match self.plugin.call::<(), String>("mcp_list_prompts", ()) {
            Ok(raw) => {
                let prompts: Vec<PromptDefinition> = serde_json::from_str(&raw)?;
                Ok(prompts)
            }
            Err(_) => Ok(Vec::new()),
        }
    }

    pub fn get_prompt(&mut self, name: &str, arguments: Value) -> Result<Value, RuntimeError> {
        let payload = json!({
            "name": name,
            "arguments": arguments
        })
        .to_string();

        match self.plugin.call::<&str, String>("mcp_get_prompt", &payload) {
            Ok(raw) => {
                let val: Value = serde_json::from_str(&raw)?;
                Ok(val)
            }
            Err(_) => Ok(Value::Null),
        }
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
                "Failed to spawn native process '{}': {}",
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

    pub fn send_request(&mut self, method: &str, params: Value) -> Result<Value, RuntimeError> {
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
            RuntimeError::Execution(format!("Failed to write to native process stdin: {}", e))
        })?;
        stdin.flush().map_err(|e| {
            RuntimeError::Execution(format!("Failed to flush native process stdin: {}", e))
        })?;

        let reader = self.reader.as_mut().ok_or_else(|| {
            RuntimeError::Execution("Child process stdout is not available".to_string())
        })?;

        let mut line = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line).map_err(|e| {
                RuntimeError::Execution(format!("Failed to read from native process stdout: {}", e))
            })?;
            if n == 0 {
                return Err(RuntimeError::Execution(
                    "Native process closed stdout unexpectedly".to_string(),
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
                "Native process initialize error: {}",
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
                "Native process tools/list error: {}",
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
                .unwrap_or("Unknown native execution error");
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
        match self.send_request("resources/list", json!({})) {
            Ok(resp) if resp.get("error").is_none() => {
                let res = resp.get("result").unwrap_or(&resp);
                let resources_val = res.get("resources").unwrap_or(res);
                let resources: Vec<ResourceDefinition> =
                    serde_json::from_value(resources_val.clone()).unwrap_or_default();
                Ok(resources)
            }
            _ => Ok(Vec::new()),
        }
    }

    pub fn read_resource(&mut self, uri: &str) -> Result<Value, RuntimeError> {
        match self.send_request("resources/read", json!({ "uri": uri })) {
            Ok(resp) if resp.get("error").is_none() => {
                let res = resp.get("result").unwrap_or(&resp);
                Ok(res.clone())
            }
            _ => Ok(Value::Null),
        }
    }

    pub fn list_prompts(&mut self) -> Result<Vec<PromptDefinition>, RuntimeError> {
        match self.send_request("prompts/list", json!({})) {
            Ok(resp) if resp.get("error").is_none() => {
                let res = resp.get("result").unwrap_or(&resp);
                let prompts_val = res.get("prompts").unwrap_or(res);
                let prompts: Vec<PromptDefinition> =
                    serde_json::from_value(prompts_val.clone()).unwrap_or_default();
                Ok(prompts)
            }
            _ => Ok(Vec::new()),
        }
    }

    pub fn get_prompt(&mut self, name: &str, arguments: Value) -> Result<Value, RuntimeError> {
        match self.send_request(
            "prompts/get",
            json!({
                "name": name,
                "arguments": arguments
            }),
        ) {
            Ok(resp) if resp.get("error").is_none() => {
                let res = resp.get("result").unwrap_or(&resp);
                Ok(res.clone())
            }
            _ => Ok(Value::Null),
        }
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
