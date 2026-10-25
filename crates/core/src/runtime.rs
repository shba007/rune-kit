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
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RuntimeError {
    #[error("Remote MCP server returned JSON-RPC error {code}")]
    RemoteRpc { code: i64 },
    #[error("Plugin file not found: {0}")]
    FileNotFound(String),
    #[error("Failed to initialize Extism plugin: {0}")]
    ExtismInit(String),
    #[error("Execution failed: {0}")]
    Execution(String),
    #[error("Request cancelled by client")]
    Cancelled,
    #[error("Tool execution timed out after {0} seconds")]
    Timeout(u64),
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

// Additive cooperative API for Rune Tools. It is intentionally unavailable to
// unrestricted native processes; user approval is never a guest operation.
#[derive(Clone)]
struct BrokerIdentity {
    name: String,
    root: PathBuf,
}
host_fn!(host_file_access(user_data: BrokerIdentity; input: String) -> Result<String, extism::Error> {
    let identity=user_data.get()?.lock().map_err(|_|extism::Error::msg("Cannot access plugin identity"))?.clone();
    let request=serde_json::from_str::<crate::file_access::Request>(&input).map_err(|_|extism::Error::msg("Invalid file-access v1 request"))?;
    let broker=crate::file_access::Broker::open(identity.root,&identity.name)?;
    let response=broker.request(request)?;
    Ok(serde_json::to_string(&response)?)
});

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
        let clean_prog = req.program.to_ascii_lowercase();
        let native_prog = if clean_prog.starts_with("rune-") {
            format!("{}-native", clean_prog)
        } else {
            format!("rune-{}-native", clean_prog)
        };
        let stripped_prog = clean_prog.strip_prefix("rune-").unwrap_or(&clean_prog);
        let stripped_native = format!("{}-native", stripped_prog);

        let candidates = [
            ctx.plugins_dir.join(&clean_prog),
            ctx.plugins_dir.join(format!("{}.exe", clean_prog)),
            ctx.plugins_dir.join(&native_prog),
            ctx.plugins_dir.join(format!("{}.exe", native_prog)),
            ctx.plugins_dir.join(&stripped_native),
            ctx.plugins_dir.join(format!("{}.exe", stripped_native)),
            ctx.plugins_dir.join(stripped_prog),
            ctx.plugins_dir.join(format!("{}.exe", stripped_prog)),
        ];

        if let Some(found) = candidates.iter().find(|p| p.exists()) {
            prog_path = found.clone();
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
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| extism::Error::msg(format!("Execution failed: {}", e)))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let child_arc = Arc::new(Mutex::new(child));
    let child_clone = Arc::clone(&child_arc);

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let mut err = Vec::new();
        if let Some(mut s) = stdout {
            let _ = std::io::Read::read_to_end(&mut s, &mut out);
        }
        if let Some(mut s) = stderr {
            let _ = std::io::Read::read_to_end(&mut s, &mut err);
        }
        let status = child_clone
            .lock()
            .map(|mut c| c.wait())
            .unwrap_or_else(|_| Err(std::io::Error::other("Lock failed")));
        let _ = tx.send((status, out, err));
    });

    match rx.recv_timeout(std::time::Duration::from_secs(60)) {
        Ok((status_res, stdout_bytes, stderr_bytes)) => {
            let (success, exit_code) = match status_res {
                Ok(status) => (status.success(), status.code()),
                Err(_) => (false, None),
            };
            let resp = CmdExecResponse {
                success,
                exit_code,
                stdout: String::from_utf8_lossy(&stdout_bytes).to_string(),
                stderr: String::from_utf8_lossy(&stderr_bytes).to_string(),
            };
            Ok(serde_json::to_string(&resp)?)
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            if let Ok(mut child) = child_arc.lock() {
                let _ = child.kill();
                let _ = child.wait();
            }
            let resp = CmdExecResponse {
                success: false,
                exit_code: Some(124),
                stdout: String::new(),
                stderr: "Execution timed out after 60 seconds".to_string(),
            };
            Ok(serde_json::to_string(&resp)?)
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err(extism::Error::msg("Subprocess output channel closed unexpectedly"))
        }
    }
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
        let clean_name = ctx.plugin_name.to_ascii_lowercase();
        let native_name = if clean_name.starts_with("rune-") {
            format!("{}-native", clean_name)
        } else {
            format!("rune-{}-native", clean_name)
        };
        let stripped_name = clean_name.strip_prefix("rune-").unwrap_or(&clean_name);
        let stripped_native = format!("{}-native", stripped_name);

        let candidates = [
            ctx.plugins_dir.join(&native_name),
            ctx.plugins_dir.join(format!("{}.exe", native_name)),
            ctx.plugins_dir.join(&stripped_native),
            ctx.plugins_dir.join(format!("{}.exe", stripped_native)),
            ctx.plugins_dir.join(&clean_name),
            ctx.plugins_dir.join(format!("{}.exe", clean_name)),
            ctx.plugins_dir.join(stripped_name),
            ctx.plugins_dir.join(format!("{}.exe", stripped_name)),
        ];

        let sidecar_path = candidates.iter().find(|p| p.exists()).cloned().ok_or_else(|| {
            extism::Error::msg(format!(
                "Companion native sidecar for '{}' not found in plugin directory",
                ctx.plugin_name
            ))
        })?;

        let sidecar = NativeSidecar::new(&ctx.plugin_name, &sidecar_path, ctx.params.clone(), None)
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
        Self::load_with_workspace(name, bytes, params, manifest, None, None)
    }

    /// Only the given workspace is mounted, at /workspace. No native host bridges
    /// are registered; modules importing those bridges fail to instantiate.
    pub fn load_in_workspace(
        name: &str,
        bytes: Vec<u8>,
        params: HashMap<String, String>,
        manifest: PluginManifest,
        workspace: &Path,
    ) -> Result<Self, RuntimeError> {
        if !workspace.is_absolute() || !workspace.is_dir() {
            return Err(RuntimeError::Execution(
                "Workspace must be an existing absolute directory".into(),
            ));
        }
        crate::sandbox::check_links(workspace)?;
        let workspace = std::fs::canonicalize(workspace)?;
        Self::load_with_workspace(name, bytes, params, manifest, Some(workspace), None)
    }

    pub fn load_with_file_access(
        name: &str,
        bytes: Vec<u8>,
        params: HashMap<String, String>,
        manifest: PluginManifest,
        sandbox: &crate::sandbox::Sandbox,
    ) -> Result<Self, RuntimeError> {
        let workspace = sandbox.workspace(name)?;
        Self::load_with_workspace(
            name,
            bytes,
            params,
            manifest,
            Some(workspace),
            Some(BrokerIdentity {
                name: name.into(),
                root: sandbox.root().to_owned(),
            }),
        )
    }

    fn load_with_workspace(
        name: &str,
        bytes: Vec<u8>,
        mut params: HashMap<String, String>,
        manifest: PluginManifest,
        workspace: Option<PathBuf>,
        broker: Option<BrokerIdentity>,
    ) -> Result<Self, RuntimeError> {
        let provisioner = BinaryProvisioner::new();
        // Inject provisioned binaries and update PATH for guest execution
        if workspace.is_none() {
            provisioner.inject_env_vars(&manifest.capabilities.exec.allowed_binaries, &mut params);
        }

        if workspace.is_some() {
            let root_key = manifest
                .capabilities
                .filesystem
                .as_ref()
                .and_then(|fs| fs.root_param.as_deref())
                .unwrap_or("allowed_dir");
            params.retain(|key, _| {
                !key.eq_ignore_ascii_case(root_key) && !key.eq_ignore_ascii_case("allowed_dir")
            });
            params.insert(root_key.to_owned(), "/workspace".into());
            params.insert("allowed_dir".into(), "/workspace".into());
        }

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

        let root_key = manifest
            .capabilities
            .filesystem
            .as_ref()
            .and_then(|fs| fs.root_param.as_deref())
            .unwrap_or("allowed_dir");
        if let Some(workspace) = &workspace {
            extism_manifest = extism_manifest
                .with_allowed_path(workspace.to_string_lossy().into_owned(), "/workspace");
        } else if let Some(root) = get_param(root_key) {
            let path = std::path::Path::new(&root);
            if root.trim().is_empty() || !path.is_absolute() || !path.is_dir() {
                return Err(RuntimeError::Execution(format!(
                    "Filesystem root '{root_key}' must be an existing absolute directory"
                )));
            }
            extism_manifest = extism_manifest.with_allowed_path(root.clone(), root);
        }
        // No configured root means no WASI preopen, never the host working directory.

        let extism_manifest = extism_manifest.with_config(params.clone().into_iter());

        let plugins_dir = crate::paths::managed_root()
            .expect("Cannot initialize Kit storage")
            .join("plugins");

        let host_ctx = HostContext {
            plugin_name: name.to_string(),
            allowed_binaries: manifest.capabilities.exec.allowed_binaries.clone(),
            plugins_dir,
            provisioner,
            sidecar: Arc::new(Mutex::new(None)),
            params,
        };

        let builder = PluginBuilder::new(extism_manifest).with_wasi(true);
        let builder = if workspace.is_some() {
            if let Some(identity) = broker {
                builder.with_function(
                    "host_file_access",
                    [PTR],
                    [PTR],
                    UserData::new(identity),
                    host_file_access,
                )
            } else {
                builder
            }
        } else {
            builder
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
        };
        let plugin = builder
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

    pub fn get_cancel_handle(&self) -> extism::CancelHandle {
        self.plugin.cancel_handle()
    }

    pub fn cancel(&mut self) {
        let _ = self.plugin.cancel_handle().cancel();
    }

    pub fn list_tools(&mut self) -> Result<Vec<ToolDefinition>, RuntimeError> {
        let raw = self
            .plugin
            .call::<(), String>("mcp_list_tools", ())
            .map_err(|e| RuntimeError::Execution(e.to_string()))?;

        let mut tools: Vec<ToolDefinition> = serde_json::from_str(&raw)?;
        for tool in &mut tools {
            if let Some(rule) = self.manifest.tools.get(&tool.name) {
                let mut ann = tool.annotations.take().unwrap_or_default();
                if ann.read_only_hint.is_none() {
                    ann.read_only_hint = rule.read_only;
                }
                if ann.destructive_hint.is_none() {
                    ann.destructive_hint = rule.destructive;
                }
                if ann.idempotent_hint.is_none() {
                    ann.idempotent_hint = rule.idempotent;
                }
                if ann.open_world_hint.is_none() {
                    ann.open_world_hint = rule.open_world;
                }
                tool.annotations = Some(ann);
            }
        }
        Ok(tools)
    }

    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, RuntimeError> {
        self.call_tool_with_cancel(
            name,
            arguments,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            None,
        )
    }

    pub fn call_tool_with_cancel(
        &mut self,
        name: &str,
        arguments: Value,
        cancel_flag: Arc<std::sync::atomic::AtomicBool>,
        timeout: Option<std::time::Duration>,
    ) -> Result<Value, RuntimeError> {
        let payload = json!({
            "name": name,
            "arguments": arguments
        })
        .to_string();

        let timed_out = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let timer = timeout.map(|duration| {
            let handle = self.plugin.cancel_handle();
            let timed_out = timed_out.clone();
            std::thread::spawn(move || {
                if matches!(
                    done_rx.recv_timeout(duration),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                ) {
                    timed_out.store(true, std::sync::atomic::Ordering::SeqCst);
                    let _ = handle.cancel();
                }
            })
        });
        let result = self.plugin.call::<&str, String>("mcp_call_tool", &payload);
        let _ = done_tx.send(());
        if let Some(timer) = timer {
            let _ = timer.join();
        }
        if cancel_flag.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(RuntimeError::Cancelled);
        }
        if timed_out.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(RuntimeError::Timeout(timeout.unwrap().as_secs()));
        }
        let raw = result.map_err(|e| RuntimeError::Execution(e.to_string()))?;
        Ok(serde_json::from_str(&raw)?)
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

pub fn resolve_host_env_params() -> HashMap<String, String> {
    let mut params = HashMap::new();
    const ALLOWED_EXPLICIT_VARS: &[&str] = &[
        "ALLOWED_DIR",
        "OUTPUT_DIR",
        "OUTPUT_DIRECTORY",
        "COOKIES_DIR",
        "COOKIES_FILE",
        "DEFAULT_TIMEZONE",
        "REPO_PATH",
        "MEMORY_FILE",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
    ];

    for (k, v) in std::env::vars() {
        if let Some(suffix) = k.strip_prefix("RUNE_PARAM_") {
            params.insert(suffix.to_ascii_lowercase(), v);
        } else if ALLOWED_EXPLICIT_VARS
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(&k))
        {
            params.insert(k.to_ascii_lowercase(), v);
        }
    }
    params
}

enum NativeProcess {
    Plain(Child),
    Isolated(crate::native_isolation::Process),
}
impl NativeProcess {
    fn kill(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(c) => c.kill(),
            Self::Isolated(c) => c.kill(),
        }
    }
    fn wait(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(c) => c.wait().map(|_| ()),
            Self::Isolated(c) => c.wait(),
        }
    }
}
pub struct NativeSidecar {
    pub name: String,
    binary_path: PathBuf,
    params: HashMap<String, String>,
    #[allow(dead_code)]
    manifest: PluginManifest,
    child: Option<NativeProcess>,
    isolation: Option<crate::native_isolation::Policy>,
    stdin: Option<Box<dyn Write + Send>>,
    rx: Option<std::sync::mpsc::Receiver<Result<String, std::io::Error>>>,
    next_id: u64,
    operation_cancel: Arc<std::sync::atomic::AtomicBool>,
    operation_timeout: Option<std::time::Duration>,
}

impl NativeSidecar {
    pub fn new<P: AsRef<Path>>(
        name: &str,
        path: P,
        params: HashMap<String, String>,
        manifest: Option<PluginManifest>,
    ) -> Result<Self, RuntimeError> {
        Self::new_internal(name, path, params, manifest, None)
    }
    pub fn new_in_workspace(
        name: &str,
        path: &Path,
        mut params: HashMap<String, String>,
        manifest: PluginManifest,
        workspace: &Path,
    ) -> Result<Self, RuntimeError> {
        if !crate::native_isolation::VALIDATED_ON_ALL_PLATFORMS {
            return Err(RuntimeError::Execution("Native plugins are blocked in workspace-only mode pending Windows, macOS and Linux validation".into()));
        }
        let policy = crate::native_isolation::Policy::new(path, workspace)
            .map_err(|_| RuntimeError::Execution("Invalid native isolation paths".into()))?;
        params.insert(
            "allowed_dir".into(),
            policy.workspace.to_string_lossy().into(),
        );
        params.insert(
            "ALLOWED_DIR".into(),
            policy.workspace.to_string_lossy().into(),
        );
        Self::new_internal(name, path, params, Some(manifest), Some(policy))
    }
    fn new_internal<P: AsRef<Path>>(
        name: &str,
        path: P,
        params: HashMap<String, String>,
        manifest: Option<PluginManifest>,
        isolation: Option<crate::native_isolation::Policy>,
    ) -> Result<Self, RuntimeError> {
        let binary_path = path.as_ref().to_path_buf();
        if !binary_path.exists() {
            return Err(RuntimeError::FileNotFound(
                binary_path.display().to_string(),
            ));
        }

        let resolved_manifest = manifest.unwrap_or_else(|| {
            if let Some(parent) = binary_path.parent() {
                let manifest_path = parent.join("plugin.toml");
                if manifest_path.exists() {
                    PluginManifest::from_file(&manifest_path).unwrap_or_default()
                } else {
                    PluginManifest::default()
                }
            } else {
                PluginManifest::default()
            }
        });

        let mut merged_params = params;
        let provisioner = BinaryProvisioner::new();
        // Inject provisioned binaries and update PATH for native process execution
        provisioner.inject_env_vars(
            &resolved_manifest.capabilities.exec.allowed_binaries,
            &mut merged_params,
        );

        let mut sidecar = Self {
            name: name.to_string(),
            isolation,
            binary_path,
            params: merged_params,
            manifest: resolved_manifest,
            child: None,
            stdin: None,
            rx: None,
            next_id: 1,
            operation_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            operation_timeout: None,
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

        let (child, stdin, stdout): (
            NativeProcess,
            Box<dyn Write + Send>,
            Box<dyn std::io::Read + Send>,
        ) = if let Some(policy) = &self.isolation {
            let mut child = crate::native_isolation::spawn(policy, &self.params).map_err(|_| {
                RuntimeError::Execution(
                    "Native isolation launch failed; no unconfined fallback".into(),
                )
            })?;
            let stdin = child
                .stdin
                .take()
                .ok_or_else(|| RuntimeError::Execution("Missing native input pipe".into()))?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| RuntimeError::Execution("Missing native output pipe".into()))?;
            (NativeProcess::Isolated(child), stdin, stdout)
        } else {
            let mut child = cmd.spawn().map_err(|e| {
                RuntimeError::Execution(format!(
                    "Failed to spawn native process '{}': {}",
                    self.binary_path.display(),
                    e
                ))
            })?;
            let stdin = child
                .stdin
                .take()
                .ok_or_else(|| RuntimeError::Execution("Missing native input pipe".into()))?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| RuntimeError::Execution("Missing native output pipe".into()))?;
            (
                NativeProcess::Plain(child),
                Box::new(stdin),
                Box::new(stdout),
            )
        };

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        if tx.send(Ok(line.clone())).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        break;
                    }
                }
            }
        });

        self.child = Some(child);
        self.stdin = Some(stdin);
        self.rx = Some(rx);
        Ok(())
    }

    pub fn cancel(&mut self) {
        self.kill_process();
    }

    pub fn kill_process(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.stdin = None;
        self.rx = None;
    }

    pub(crate) fn set_operation_context(
        &mut self,
        cancel: Arc<std::sync::atomic::AtomicBool>,
        timeout: Option<std::time::Duration>,
    ) {
        self.operation_cancel = cancel;
        self.operation_timeout = timeout;
    }

    pub fn send_request(&mut self, method: &str, params: Value) -> Result<Value, RuntimeError> {
        self.send_request_with_cancel(
            method,
            params,
            self.operation_cancel.clone(),
            self.operation_timeout,
        )
    }

    pub fn send_request_with_cancel(
        &mut self,
        method: &str,
        params: Value,
        cancel_flag: Arc<std::sync::atomic::AtomicBool>,
        timeout: Option<std::time::Duration>,
    ) -> Result<Value, RuntimeError> {
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

        let rx = self.rx.as_ref().ok_or_else(|| {
            RuntimeError::Execution("Child process receiver is not available".to_string())
        })?;

        let timeout_duration = timeout.unwrap_or(std::time::Duration::from_secs(30));
        let start = std::time::Instant::now();

        loop {
            if cancel_flag.load(std::sync::atomic::Ordering::Relaxed) {
                self.kill_process();
                return Err(RuntimeError::Cancelled);
            }

            if start.elapsed() >= timeout_duration {
                self.kill_process();
                return Err(RuntimeError::Timeout(timeout_duration.as_secs()));
            }

            match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(Ok(line)) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    if let Ok(val) = serde_json::from_str::<Value>(trimmed)
                        && val.is_object()
                    {
                        let matches_id = val
                            .get("id")
                            .and_then(|i| i.as_u64())
                            .map(|i| i == req_id)
                            .unwrap_or_else(|| {
                                val.get("id")
                                    .and_then(|i| i.as_str())
                                    .map(|s| s == req_id.to_string())
                                    .unwrap_or(false)
                            });

                        if matches_id {
                            return Ok(val);
                        }
                    }
                }
                Ok(Err(e)) => {
                    self.kill_process();
                    return Err(RuntimeError::Execution(format!(
                        "Failed to read from native process stdout: {}",
                        e
                    )));
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    continue;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    self.kill_process();
                    return Err(RuntimeError::Execution(
                        "Native process closed stdout unexpectedly".to_string(),
                    ));
                }
            }
        }
    }

    pub fn get_info(&mut self) -> Result<PluginInfo, RuntimeError> {
        // Rune Tools exposes info on its private ABI. Retain the legacy probe
        // for older standalone plugins; neither operation is public MCP discovery.
        let mut resp = self.send_request("info", json!({}))?;
        if resp.get("error").is_some() {
            resp = self.send_request("initialize", json!({}))?;
        }
        if let Some(err) = resp.get("error") {
            return Err(RuntimeError::Execution(format!(
                "Native process info error: {err}"
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
        self.list_tools_with_cancel(Arc::new(std::sync::atomic::AtomicBool::new(false)), None)
    }

    pub fn list_tools_with_cancel(
        &mut self,
        cancel: Arc<std::sync::atomic::AtomicBool>,
        timeout: Option<std::time::Duration>,
    ) -> Result<Vec<ToolDefinition>, RuntimeError> {
        let resp = self.send_request_with_cancel("tools/list", json!({}), cancel, timeout)?;
        if let Some(err) = resp.get("error") {
            return Err(RuntimeError::Execution(format!(
                "Native process tools/list error: {}",
                err
            )));
        }

        let res = resp.get("result").unwrap_or(&resp);
        let tools_val = res.get("tools").unwrap_or(res);
        let mut tools: Vec<ToolDefinition> = serde_json::from_value(tools_val.clone())?;
        for tool in &mut tools {
            if let Some(rule) = self.manifest.tools.get(&tool.name) {
                let mut ann = tool.annotations.take().unwrap_or_default();
                if ann.read_only_hint.is_none() {
                    ann.read_only_hint = rule.read_only;
                }
                if ann.destructive_hint.is_none() {
                    ann.destructive_hint = rule.destructive;
                }
                if ann.idempotent_hint.is_none() {
                    ann.idempotent_hint = rule.idempotent;
                }
                if ann.open_world_hint.is_none() {
                    ann.open_world_hint = rule.open_world;
                }
                tool.annotations = Some(ann);
            }
        }
        Ok(tools)
    }

    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, RuntimeError> {
        self.call_tool_with_cancel(
            name,
            arguments,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            None,
        )
    }

    pub fn call_tool_with_cancel(
        &mut self,
        name: &str,
        arguments: Value,
        cancel_flag: Arc<std::sync::atomic::AtomicBool>,
        timeout: Option<std::time::Duration>,
    ) -> Result<Value, RuntimeError> {
        let resp = self.send_request_with_cancel(
            "tools/call",
            json!({
                "name": name,
                "arguments": arguments
            }),
            cancel_flag,
            timeout,
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
    Wasm(Box<WasmPluginInstance>),
    Native(Box<NativeSidecar>),
    Remote(Box<crate::remote::RemoteHttp>),
}

impl PluginInstance {
    pub fn set_operation_context(
        &mut self,
        cancel: Arc<std::sync::atomic::AtomicBool>,
        timeout: Option<std::time::Duration>,
    ) {
        match self {
            Self::Native(n) => n.set_operation_context(cancel, timeout),
            Self::Remote(r) => r.set_operation_context(cancel, timeout),
            Self::Wasm(_) => {}
        }
    }

    pub fn load_wasm_from_file<P: AsRef<Path>>(
        name: &str,
        path: P,
        params: HashMap<String, String>,
    ) -> Result<Self, RuntimeError> {
        WasmPluginInstance::load_from_file(name, path, params)
            .map(Box::new)
            .map(PluginInstance::Wasm)
    }

    pub fn load_native_from_file<P: AsRef<Path>>(
        name: &str,
        path: P,
        params: HashMap<String, String>,
    ) -> Result<Self, RuntimeError> {
        NativeSidecar::new(name, path, params, None)
            .map(Box::new)
            .map(PluginInstance::Native)
    }

    /// A bounded inventory probe; does not keep a gateway or execute tools.
    pub fn discover_tools(
        &mut self,
        cancel: Arc<std::sync::atomic::AtomicBool>,
        timeout: std::time::Duration,
    ) -> Result<Vec<ToolDefinition>, RuntimeError> {
        match self {
            Self::Native(native) => native.list_tools_with_cancel(cancel, Some(timeout)),
            Self::Remote(remote) => {
                remote.set_operation_context(cancel.clone(), Some(timeout));
                let result = remote.list_tools();
                remote.set_operation_context(cancel, None);
                result
            }
            Self::Wasm(wasm) => {
                let handle = wasm.get_cancel_handle();
                let stop = cancel.clone();
                let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
                let timer = std::thread::spawn(move || {
                    let start = std::time::Instant::now();
                    loop {
                        match done_rx.recv_timeout(std::time::Duration::from_millis(50)) {
                            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                                return false;
                            }
                            _ => {}
                        }
                        if stop.load(std::sync::atomic::Ordering::Relaxed)
                            || start.elapsed() >= timeout
                        {
                            let _ = handle.cancel();
                            return true;
                        }
                    }
                });
                let result = wasm.list_tools();
                let _ = done_tx.send(());
                let timed_out = timer.join().unwrap_or(true);
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err(RuntimeError::Cancelled);
                }
                if timed_out {
                    return Err(RuntimeError::Timeout(timeout.as_secs()));
                }
                result
            }
        }
    }
    pub fn get_info(&mut self) -> Result<PluginInfo, RuntimeError> {
        match self {
            PluginInstance::Wasm(w) => w.get_info(),
            PluginInstance::Native(n) => n.get_info(),
            PluginInstance::Remote(r) => r.get_info(),
        }
    }

    pub fn list_tools(&mut self) -> Result<Vec<ToolDefinition>, RuntimeError> {
        match self {
            PluginInstance::Wasm(w) => w.list_tools(),
            PluginInstance::Native(n) => n.list_tools(),
            PluginInstance::Remote(r) => r.list_tools(),
        }
    }

    pub fn cancel(&mut self) {
        match self {
            PluginInstance::Wasm(w) => w.cancel(),
            PluginInstance::Native(n) => n.cancel(),
            PluginInstance::Remote(r) => r.cancel(),
        }
    }

    pub fn manifest(&self) -> &PluginManifest {
        match self {
            PluginInstance::Wasm(w) => &w.manifest,
            PluginInstance::Native(n) => &n.manifest,
            PluginInstance::Remote(r) => &r.manifest,
        }
    }

    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, RuntimeError> {
        match self {
            PluginInstance::Wasm(w) => w.call_tool(name, arguments),
            PluginInstance::Native(n) => n.call_tool(name, arguments),
            PluginInstance::Remote(r) => r.call_tool(name, arguments),
        }
    }

    pub fn call_tool_with_cancel(
        &mut self,
        name: &str,
        arguments: Value,
        cancel_flag: Arc<std::sync::atomic::AtomicBool>,
        timeout: Option<std::time::Duration>,
    ) -> Result<Value, RuntimeError> {
        match self {
            PluginInstance::Wasm(w) => {
                w.call_tool_with_cancel(name, arguments, cancel_flag, timeout)
            }
            PluginInstance::Native(n) => {
                n.call_tool_with_cancel(name, arguments, cancel_flag, timeout)
            }
            PluginInstance::Remote(r) => {
                if let Some(timeout) = timeout {
                    r.set_operation_context(cancel_flag, Some(timeout));
                }
                r.call_tool(name, arguments)
            }
        }
    }

    pub fn list_resources(&mut self) -> Result<Vec<ResourceDefinition>, RuntimeError> {
        match self {
            PluginInstance::Wasm(w) => w.list_resources(),
            PluginInstance::Native(n) => n.list_resources(),
            PluginInstance::Remote(r) => r.list_resources(),
        }
    }

    pub fn read_resource(&mut self, uri: &str) -> Result<Value, RuntimeError> {
        match self {
            PluginInstance::Wasm(w) => w.read_resource(uri),
            PluginInstance::Native(n) => n.read_resource(uri),
            PluginInstance::Remote(r) => r.read_resource(uri),
        }
    }

    pub fn list_prompts(&mut self) -> Result<Vec<PromptDefinition>, RuntimeError> {
        match self {
            PluginInstance::Wasm(w) => w.list_prompts(),
            PluginInstance::Native(n) => n.list_prompts(),
            PluginInstance::Remote(r) => r.list_prompts(),
        }
    }

    pub fn get_prompt(&mut self, name: &str, arguments: Value) -> Result<Value, RuntimeError> {
        match self {
            PluginInstance::Wasm(w) => w.get_prompt(name, arguments),
            PluginInstance::Native(n) => n.get_prompt(name, arguments),
            PluginInstance::Remote(r) => r.get_prompt(name, arguments),
        }
    }
}

#[cfg(test)]
mod workspace_tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("kit-wasi-{}-{nonce}", std::process::id()));
            std::fs::create_dir(&path).unwrap();
            std::fs::create_dir(path.join("workspace")).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn file_access_guest(request: serde_json::Value) -> Vec<u8> {
        let payload = request.to_string();
        let stores: String = payload
            .bytes()
            .enumerate()
            .map(|(i, b)| {
                format!(
                    "(call $store (i64.add (local.get $ptr) (i64.const {i})) (i32.const {b}))\n"
                )
            })
            .collect();
        wat::parse_str(format!(
            r#"(module
            (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
            (import "extism:host/env" "store_u8" (func $store (param i64 i32)))
            (import "extism:host/env" "length" (func $length (param i64) (result i64)))
            (import "extism:host/env" "output_set" (func $output (param i64 i64)))
            (import "extism:host/user" "host_file_access" (func $access (param i64) (result i64)))
            (func (export "mcp_call_tool") (result i32) (local $ptr i64) (local $result i64)
                (local.set $ptr (call $alloc (i64.const {})))
                {stores}
                (local.set $result (call $access (local.get $ptr)))
                (call $output (local.get $result) (call $length (local.get $result)))
                (i32.const 0)))"#,
            payload.len()
        ))
        .unwrap()
    }

    #[test]
    fn cooperative_file_access_is_bound_to_host_identity_and_never_copies_back_from_a_guest() {
        let f = Fixture::new();
        let file = f.0.join("original.txt");
        std::fs::write(&file, b"original").unwrap();
        let mut sandbox = crate::sandbox::Sandbox::open(f.0.join("sandbox")).unwrap();
        sandbox.approve("owner", &file).unwrap();
        let write = serde_json::json!({"schema_version":1,"operation":"write_file","path":file,"data":b"staged".to_vec()});
        let mut owner = WasmPluginInstance::load_with_file_access(
            "owner",
            file_access_guest(write.clone()),
            Default::default(),
            PluginManifest::default(),
            &sandbox,
        )
        .unwrap();
        assert_eq!(
            owner.call_tool("write", serde_json::json!({})).unwrap()["staged"],
            true
        );
        assert_eq!(std::fs::read(&file).unwrap(), b"original");
        let broker = crate::file_access::Broker::open(sandbox.root().to_owned(), "owner").unwrap();
        assert_eq!(broker.reviews().unwrap().len(), 1);
        let mut other = WasmPluginInstance::load_with_file_access(
            "other",
            file_access_guest(write),
            Default::default(),
            PluginManifest::default(),
            &sandbox,
        )
        .unwrap();
        assert!(other.call_tool("write", serde_json::json!({})).is_err());
        let invalid =
            serde_json::json!({"schema_version":1,"operation":"apply_review","path":file});
        let mut guest = WasmPluginInstance::load_with_file_access(
            "owner",
            file_access_guest(invalid),
            Default::default(),
            PluginManifest::default(),
            &sandbox,
        )
        .unwrap();
        assert!(guest.call_tool("apply", serde_json::json!({})).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"original");
    }

    #[test]
    fn workspace_wasi_writes_inside_but_cannot_open_originals_or_parent_paths() {
        let fixture = Fixture::new();
        let workspace = fixture.0.join("workspace");
        let original = fixture.0.join("original.txt");
        std::fs::write(&original, b"original unchanged").unwrap();
        let absolute: String = original
            .to_str()
            .unwrap()
            .bytes()
            .map(|b| format!("\\{b:02x}"))
            .collect();
        let wat = format!(
            r#"(module
            (import "wasi_snapshot_preview1" "path_open" (func $open (param i32 i32 i32 i32 i32 i64 i64 i32 i32) (result i32)))
            (import "wasi_snapshot_preview1" "fd_write" (func $write (param i32 i32 i32 i32) (result i32)))
            (import "wasi_snapshot_preview1" "fd_read" (func $read (param i32 i32 i32 i32) (result i32)))
            (import "wasi_snapshot_preview1" "fd_close" (func $close (param i32) (result i32)))
            (import "wasi_snapshot_preview1" "fd_prestat_get" (func $prestat (param i32 i32) (result i32)))
            (import "wasi_snapshot_preview1" "path_symlink" (func $symlink (param i32 i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 100) "inside.txt")
            (data (i32.const 128) "changed")
            (data (i32.const 160) "../original.txt")
            (data (i32.const 192) "escape")
            (data (i32.const 256) "{absolute}")
            (func (export "probe") (result i32)
                ;; Create/write/read back a file inside the only preopen (fd 3).
                i32.const 3 i32.const 0 i32.const 100 i32.const 10 i32.const 9
                i64.const 66 i64.const 0 i32.const 0 i32.const 0 call $open
                if unreachable end
                i32.const 64 i32.const 128 i32.store
                i32.const 68 i32.const 7 i32.store
                i32.const 0 i32.load i32.const 64 i32.const 1 i32.const 8 call $write
                if unreachable end
                i32.const 0 i32.load call $close drop
                ;; Parent traversal must fail for both read and write.
                i32.const 3 i32.const 0 i32.const 160 i32.const 15 i32.const 0
                i64.const 2 i64.const 0 i32.const 0 i32.const 0 call $open i32.eqz
                if unreachable end
                i32.const 3 i32.const 0 i32.const 160 i32.const 15 i32.const 0
                i64.const 64 i64.const 0 i32.const 0 i32.const 0 call $open i32.eqz
                if unreachable end
                ;; An absolute host path is not a guest capability.
                i32.const 3 i32.const 0 i32.const 256 i32.const {length} i32.const 0
                i64.const 2 i64.const 0 i32.const 0 i32.const 0 call $open i32.eqz
                if unreachable end
                ;; There must not be a second preopened directory.
                i32.const 4 i32.const 16 call $prestat i32.eqz if unreachable end
                ;; If symlink creation is supported, following it must not escape.
                i32.const 256 i32.const {length} i32.const 3 i32.const 192 i32.const 6 call $symlink i32.eqz
                if
                    i32.const 3 i32.const 1 i32.const 192 i32.const 6 i32.const 0
                    i64.const 2 i64.const 0 i32.const 0 i32.const 0 call $open i32.eqz
                    if unreachable end
                    i32.const 3 i32.const 1 i32.const 192 i32.const 6 i32.const 0
                    i64.const 64 i64.const 0 i32.const 0 i32.const 0 call $open i32.eqz
                    if unreachable end
                end
                ;; Read the workspace file directly through WASI.
                i32.const 3 i32.const 0 i32.const 100 i32.const 10 i32.const 0
                i64.const 2 i64.const 0 i32.const 0 i32.const 0 call $open
                if unreachable end
                i32.const 64 i32.const 512 i32.store
                i32.const 68 i32.const 7 i32.store
                i32.const 0 i32.load i32.const 64 i32.const 1 i32.const 8 call $read
                if unreachable end
                i32.const 8 i32.load i32.const 7 i32.ne if unreachable end
                i32.const 512 i32.load8_u i32.const 99 i32.ne if unreachable end
                i32.const 0 i32.load call $close drop
                i32.const 0))"#,
            length = original.to_str().unwrap().len()
        );
        let mut params = HashMap::new();
        params.insert("allowed_dir".into(), fixture.0.to_str().unwrap().into());
        params.insert("ALLOWED_DIR".into(), fixture.0.to_str().unwrap().into());
        let mut plugin = WasmPluginInstance::load_in_workspace(
            "fixture",
            wat::parse_str(wat).unwrap(),
            params,
            PluginManifest::default(),
            &workspace,
        )
        .unwrap();
        plugin.plugin.call::<(), ()>("probe", ()).unwrap();
        assert_eq!(
            std::fs::read(workspace.join("inside.txt")).unwrap(),
            b"changed"
        );
        assert_eq!(std::fs::read(original).unwrap(), b"original unchanged");
        assert!(!fixture.0.join("inside.txt").exists());
    }

    #[test]
    fn workspace_mode_rejects_native_helper_imports_and_invalid_roots() {
        let fixture = Fixture::new();
        for helper in ["host_cmd_exec", "host_sidecar_exec", "host_get_binary_path"] {
            let bytes = wat::parse_str(format!(
                r#"(module (import "extism:host/user" "{helper}" (func (param i64) (result i64))))"#
            ))
            .unwrap();
            assert!(
                WasmPluginInstance::load_in_workspace(
                    "fixture",
                    bytes,
                    Default::default(),
                    PluginManifest::default(),
                    &fixture.0.join("workspace")
                )
                .is_err(),
                "{helper}"
            );
        }
        let bytes = wat::parse_str("(module)").unwrap();
        assert!(
            WasmPluginInstance::load_in_workspace(
                "fixture",
                bytes.clone(),
                Default::default(),
                PluginManifest::default(),
                Path::new("relative")
            )
            .is_err()
        );
        assert!(
            WasmPluginInstance::load_in_workspace(
                "fixture",
                bytes,
                Default::default(),
                PluginManifest::default(),
                &fixture.0.join("missing")
            )
            .is_err()
        );
    }
}
