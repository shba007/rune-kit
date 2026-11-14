//! Local, sessionless MCP 2026-07-28 Streamable HTTP transport.
use axum::{
    Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use rune_kit_core::McpRouter;
use rune_kit_core::headers::annotations;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    convert::Infallible,
    net::{IpAddr, SocketAddr},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Semaphore, mpsc};

pub const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:47601/mcp";
const HISTORY: usize = 64;
const BODY_LIMIT: usize = 256 * 1024;

pub struct Options {
    pub bind: IpAddr,
    pub port: u16,
    pub headless: bool,
    pub workspace_only: bool,
    pub read_only: bool,
    pub allowed_tools: Vec<String>,
    pub timeout: u64,
    pub plugins: Vec<String>,
    pub roots: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct Call {
    pub sequence: u64,
    pub method: String,
    pub name: String,
    pub status: String,
    pub millis: u128,
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub bind_address: String,
    pub workspace_only: bool,
    pub endpoint: String,
    pub status: String,
    pub started: Instant,
    pub plugins: Vec<String>,
    pub read_only: bool,
    pub allowed_tools: Vec<String>,
    pub roots: Vec<String>,
    pub timeout: u64,
    pub executable: std::path::PathBuf,
    pub data_root: std::path::PathBuf,
    pub active: Vec<Call>,
    pub recent: VecDeque<Call>,
    pub tools: Vec<String>,
    pub discovery_note: String,
}
struct Shared {
    router: Arc<Mutex<McpRouter>>,
    configuration: Arc<Mutex<crate::configuration::Configuration>>,
    snapshot: Arc<Mutex<Snapshot>>,
    address: SocketAddr,
    sequence: AtomicU64,
    permits: Arc<Semaphore>,
    flags: Mutex<HashMap<u64, Arc<AtomicBool>>>,
    token: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigurationUpdate {
    plugin: String,
    key: String,
    value: String,
    #[serde(default)]
    shared: bool,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoteUpdate {
    name: String,
    config: Option<rune_kit_core::remote::RemoteConfig>,
}

async fn remote_servers(State(state): State<Arc<Shared>>) -> Response {
    if !state.address.ip().is_loopback() {
        return (
            StatusCode::FORBIDDEN,
            "Configuration management requires a loopback listener",
        )
            .into_response();
    }
    let root = state.snapshot.lock().unwrap().data_root.clone();
    match crate::remote_panel::Panel::open(root).api_list() {
        Ok(servers) => axum::Json(json!({"servers": servers})).into_response(),
        Err(message) => (StatusCode::CONFLICT, message).into_response(),
    }
}

fn installed_packages(root: &std::path::Path) -> Result<Value, String> {
    fn read<T: serde::de::DeserializeOwned + Default>(path: &std::path::Path) -> Result<T, String> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| "Installed package list is malformed".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
            Err(_) => Err("Cannot read installed package list".into()),
        }
    }
    let plugins: rune_kit_core::Lockfile = read(&root.join("installed.json"))?;
    let skills: rune_kit_core::manifest::SkillLockfile = read(&root.join("installed_skills.json"))?;
    let mut mcps: Vec<_> = plugins
        .mcps
        .values()
        .map(|p| json!({"name":p.name,"version":p.version,"description":p.description}))
        .collect();
    let mut skills: Vec<_> = skills.skills.values().map(|s| json!({"name":s.name,"version":s.version,"description":s.description,"author":s.author})).collect();
    mcps.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    skills.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(json!({"mcps":mcps,"skills":skills,"plugins":mcps}))
}

async fn package_icon(Path(name): Path<String>) -> Response {
    match crate::icons::package_icon(&name).await {
        Ok(Some(icon)) => (
            [
                (header::CONTENT_TYPE, "image/svg+xml"),
                (header::CACHE_CONTROL, "private, max-age=3600"),
            ],
            icon.as_ref().clone(),
        )
            .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::BAD_GATEWAY.into_response(),
    }
}

async fn registry_list() -> Response {
    match crate::icons::registry().await {
        Ok(list) => axum::Json(list).into_response(),
        Err(_) => (StatusCode::BAD_GATEWAY, "Cannot load the tools registry").into_response(),
    }
}

async fn packages(State(state): State<Arc<Shared>>) -> Response {
    if !state.address.ip().is_loopback() {
        return (
            StatusCode::FORBIDDEN,
            "Package management requires a loopback listener",
        )
            .into_response();
    }
    let root = state.snapshot.lock().unwrap().data_root.clone();
    match installed_packages(&root) {
        Ok(packages) => axum::Json(packages).into_response(),
        Err(message) => (StatusCode::CONFLICT, message).into_response(),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PackageInstall {
    kind: String,
    target: String,
    version: Option<String>,
    #[serde(default)]
    prefer_native: bool,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PackageOperation {
    kind: String,
    name: String,
}

fn is_mcp_kind(kind: &str) -> bool {
    matches!(kind, "mcp" | "plugin")
}

async fn dispatch_package_update<M, S>(is_mcp: bool, mcp: M, skill: S) -> Response
where
    M: std::future::Future<Output = Response>,
    S: std::future::Future<Output = Response>,
{
    if is_mcp { mcp.await } else { skill.await }
}

async fn package_operation(
    State(state): State<Arc<Shared>>,
    axum::extract::Path(action): axum::extract::Path<String>,
    axum::Json(request): axum::Json<PackageOperation>,
) -> Response {
    if !state.address.ip().is_loopback() {
        return (
            StatusCode::FORBIDDEN,
            "Package management requires a loopback listener",
        )
            .into_response();
    }
    let is_mcp = is_mcp_kind(&request.kind);
    if (!is_mcp && request.kind != "skill")
        || request.name.is_empty()
        || request.name.len() > 128
        || request.name.chars().any(char::is_control)
    {
        return (StatusCode::BAD_REQUEST, "Invalid package selection").into_response();
    }
    let root = state.snapshot.lock().unwrap().data_root.clone();
    let packages = match installed_packages(&root) {
        Ok(value) => value,
        Err(message) => return (StatusCode::CONFLICT, message).into_response(),
    };
    let list = if is_mcp {
        packages
            .get("mcps")
            .or_else(|| packages.get("plugins"))
            .unwrap()
    } else {
        &packages["skills"]
    };
    if !list
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item["name"] == request.name))
    {
        return (StatusCode::NOT_FOUND, "Package is not installed").into_response();
    }
    if action == "remove" {
        let result = if is_mcp {
            rune_kit_core::PackageManager::new().uninstall(&request.name)
        } else {
            rune_kit_core::SkillManager::new().uninstall_skill(&request.name)
        };
        return match result {
            Ok(true) => axum::Json(json!({"restart_required": is_mcp})).into_response(),
            Ok(false) => (StatusCode::NOT_FOUND, "Package is not installed").into_response(),
            Err(error) => (StatusCode::CONFLICT, error.to_string()).into_response(),
        };
    }
    if !matches!(action.as_str(), "check" | "update") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let names = vec![request.name.clone()];
    let statuses = if is_mcp {
        rune_kit_core::PackageManager::new()
            .check_plugin_updates(Some(&names))
            .await
    } else {
        rune_kit_core::SkillManager::new()
            .check_skill_updates(Some(&names))
            .await
    };
    let status = match statuses {
        Ok(mut statuses) if !statuses.is_empty() => statuses.remove(0),
        Ok(_) => {
            return (
                StatusCode::NOT_FOUND,
                "No registry update information for this package",
            )
                .into_response();
        }
        Err(error) => return (StatusCode::BAD_GATEWAY, error.to_string()).into_response(),
    };
    if action == "check" || !status.has_update {
        return axum::Json(json!({"has_update":status.has_update,"installed_version":status.installed_version,"latest_version":status.latest_version})).into_response();
    }
    dispatch_package_update(is_mcp, async {
        match rune_kit_core::PackageManager::new()
            .update_installed(&request.name, Some(status.latest_version.clone()))
            .await
        {
            Ok(plugin) => axum::Json(
                json!({"name":plugin.name,"version":plugin.version,"restart_required":true}),
            )
            .into_response(),
            Err(error) => (StatusCode::CONFLICT, error.to_string()).into_response(),
        }
    }, async {
        match rune_kit_core::SkillManager::new().update_skill_with_backup(&request.name, Some(status.latest_version.clone())).await {
            Ok((skill, backup)) => axum::Json(json!({"name":skill.name,"version":skill.version,"restart_required":false,"backup":backup})).into_response(),
            Err(error) => (StatusCode::CONFLICT, error.to_string()).into_response(),
        }
    }).await
}

async fn install_package(
    State(state): State<Arc<Shared>>,
    axum::Json(request): axum::Json<PackageInstall>,
) -> Response {
    if !state.address.ip().is_loopback() {
        return (
            StatusCode::FORBIDDEN,
            "Package management requires a loopback listener",
        )
            .into_response();
    }
    let target = request.target.trim();
    if target.is_empty() || target.chars().any(char::is_control) || target.len() > 4096 {
        return (StatusCode::BAD_REQUEST, "Enter a valid package target").into_response();
    }
    if (target.contains('/') || target.contains('\\'))
        && !target.starts_with("https://")
        && !std::path::Path::new(target).is_absolute()
    {
        return (
            StatusCode::BAD_REQUEST,
            "Local package paths must be absolute",
        )
            .into_response();
    }
    let result = match request.kind.as_str() {
        "mcp" | "plugin" => rune_kit_core::PackageManager::new().install_new(target, request.version, request.prefer_native).await.map(|p| json!({"kind":"mcp","name":p.name,"version":p.version,"restart_required":true})),
        "skill" => rune_kit_core::SkillManager::new().install_new_skill(target, request.version).await.map(|s| json!({"kind":"skill","name":s.name,"version":s.version,"restart_required":false})),
        _ => return (StatusCode::BAD_REQUEST, "Choose mcp or skill").into_response(),
    };
    match result {
        Ok(result) => axum::Json(result).into_response(),
        Err(error) => (StatusCode::CONFLICT, error.to_string()).into_response(),
    }
}

async fn update_remote_server(
    State(state): State<Arc<Shared>>,
    axum::Json(update): axum::Json<RemoteUpdate>,
) -> Response {
    if !state.address.ip().is_loopback() {
        return (
            StatusCode::FORBIDDEN,
            "Configuration management requires a loopback listener",
        )
            .into_response();
    }
    match tokio::task::spawn_blocking(move || {
        let root = state.snapshot.lock().unwrap().data_root.clone();
        crate::remote_panel::Panel::open(root).api_update(
            &state.router,
            &state.snapshot,
            update.name,
            update.config,
        )
    })
    .await
    {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(message)) => (StatusCode::CONFLICT, message).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Remote update failed").into_response(),
    }
}

async fn configuration_fields(State(state): State<Arc<Shared>>) -> Response {
    if !state.address.ip().is_loopback() {
        return (
            StatusCode::FORBIDDEN,
            "Configuration management requires a loopback listener",
        )
            .into_response();
    }
    let Ok(configuration) = state.configuration.try_lock() else {
        return (StatusCode::CONFLICT, "Configuration is busy; retry").into_response();
    };
    let fields: Vec<_> = configuration
        .fields()
        .into_iter()
        .map(|field| {
            json!({
                "plugin": field.plugin, "key": field.key, "kind": field.schema.kind,
                "description": field.schema.description, "required": field.schema.required,
                "secret": field.schema.secret, "locked": field.locked, "value": field.value,
            })
        })
        .collect();
    axum::Json(json!({ "fields": fields })).into_response()
}

async fn update_configuration(
    State(state): State<Arc<Shared>>,
    axum::Json(update): axum::Json<ConfigurationUpdate>,
) -> Response {
    if !state.address.ip().is_loopback() {
        return (
            StatusCode::FORBIDDEN,
            "Configuration management requires a loopback listener",
        )
            .into_response();
    }
    match tokio::task::spawn_blocking(move || {
        let mut configuration = state
            .configuration
            .try_lock()
            .map_err(|_| "Configuration is busy; retry".to_string())?;
        let field = configuration
            .fields()
            .into_iter()
            .find(|field| field.plugin == update.plugin && field.key == update.key)
            .ok_or_else(|| "Configuration field is unavailable".to_string())?;
        configuration.apply(&state.router, &field, update.value, update.shared)
    })
    .await
    {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(message)) => (StatusCode::CONFLICT, message).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Configuration update failed",
        )
            .into_response(),
    }
}
struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

fn error(status: StatusCode, id: Option<&Value>, code: i64, message: &str) -> Response {
    let mut value = json!({"jsonrpc":"2.0","error":{"code":code,"message":message}});
    if let Some(id) = id.filter(|v| v.is_string() || v.is_number()) {
        value["id"] = id.clone();
    }
    (status, axum::Json(value)).into_response()
}
fn mismatch(id: Option<&Value>) -> Response {
    error(
        StatusCode::BAD_REQUEST,
        id,
        -32020,
        "Header mismatch: required header missing, malformed or different from body",
    )
}
fn single<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let first = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        None
    } else {
        Some(first)
    }
}
fn decode(value: &str) -> Option<String> {
    if let Some(encoded) = value
        .strip_prefix("=?base64?")
        .and_then(|v| v.strip_suffix("?="))
    {
        return String::from_utf8(STANDARD.decode(encoded).ok()?).ok();
    }
    if value.trim() != value || !value.bytes().all(|b| b == b'\t' || (32..=126).contains(&b)) {
        return None;
    }
    Some(value.to_owned())
}
fn equal(headers: &HeaderMap, name: &str, value: &str, encoded: bool) -> bool {
    single(headers, name)
        .and_then(|v| {
            if encoded {
                decode(v)
            } else {
                Some(v.to_owned())
            }
        })
        .as_deref()
        == Some(value)
}
fn accepts(headers: &HeaderMap) -> bool {
    let values: Vec<_> = headers
        .get_all(header::ACCEPT)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|v| v.trim().to_ascii_lowercase())
        .collect();
    ["application/json", "text/event-stream"]
        .iter()
        .all(|mime| {
            values.iter().any(|v| {
                let mut pieces = v.split(';');
                pieces.next().is_some_and(|v| v.trim() == *mime)
                    && !pieces.any(|p| {
                        p.trim().starts_with("q=")
                            && p.trim()[2..]
                                .parse::<f32>()
                                .map_or(true, |q| !(0.0..=1.0).contains(&q) || q == 0.0)
                    })
            })
        })
}
fn valid_host(headers: &HeaderMap, address: SocketAddr) -> bool {
    let host = single(headers, "host");
    if host == Some(format!("localhost:{}", address.port()).as_str()) {
        return address.ip().is_loopback() || address.ip().is_unspecified();
    }
    let Some(host) = host.and_then(|value| value.parse::<SocketAddr>().ok()) else {
        return false;
    };
    if host.port() != address.port() || host.ip().is_unspecified() || host.ip().is_multicast() {
        return false;
    }
    host == address
        || (address.ip().is_unspecified()
            && host.is_ipv4() == address.is_ipv4()
            && std::net::UdpSocket::bind((host.ip(), 0)).is_ok())
}

pub fn validate_bind(bind: IpAddr) -> Result<(), Box<dyn std::error::Error>> {
    if !bind.is_loopback() && http_token()?.is_none_or(|token| token.trim().is_empty()) {
        return Err("LAN serving requires RUNE_KIT_HTTP_TOKEN; configure a bearer token before binding outside loopback".into());
    }
    Ok(())
}
pub(crate) fn http_token() -> Result<Option<String>, Box<dyn std::error::Error>> {
    match std::env::var("RUNE_KIT_HTTP_TOKEN") {
        Ok(token) => Ok(Some(token).filter(|s| !s.is_empty())),
        Err(std::env::VarError::NotPresent) => crate::setup::saved_token(),
        Err(e) => Err(e.into()),
    }
}

async fn security(
    State(state): State<Arc<Shared>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let headers = request.headers().clone();
    if headers.contains_key(header::ORIGIN) || !valid_host(&headers, state.address) {
        return error(
            StatusCode::FORBIDDEN,
            None,
            -32600,
            "Only non-browser clients with a configured local interface Host are supported",
        );
    }
    if let Some(token) = &state.token
        && single(&headers, "authorization") != Some(format!("Bearer {token}").as_str())
    {
        return error(
            StatusCode::UNAUTHORIZED,
            None,
            -32600,
            "A valid bearer token is required",
        );
    }
    next.run(request).await
}

async fn handle(State(state): State<Arc<Shared>>, request: axum::extract::Request) -> Response {
    let headers = request.headers().clone();
    if single(&headers, "content-type").is_none_or(|v| {
        !v.split(';')
            .next()
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"))
    }) {
        return error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            None,
            -32600,
            "Content-Type must be application/json",
        );
    }
    if !accepts(&headers) {
        return error(
            StatusCode::NOT_ACCEPTABLE,
            None,
            -32600,
            "Accept must include application/json and text/event-stream",
        );
    }
    let bytes = match tokio::time::timeout(
        Duration::from_secs(10),
        axum::body::to_bytes(request.into_body(), BODY_LIMIT),
    )
    .await
    {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) => {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                None,
                -32600,
                "Request body limit is 256 KiB",
            );
        }
        Err(_) => {
            return error(
                StatusCode::REQUEST_TIMEOUT,
                None,
                -32000,
                "Request body deadline exceeded",
            );
        }
    };
    let request: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return error(StatusCode::BAD_REQUEST, None, -32700, "Parse error"),
    };
    let id = request.get("id");
    if !request.is_object()
        || request["jsonrpc"] != "2.0"
        || !request["method"].is_string()
        || id.is_some_and(|v| !v.is_string() && !v.is_number())
        || request.get("params").is_some_and(|v| !v.is_object())
    {
        return error(
            StatusCode::BAD_REQUEST,
            id,
            -32600,
            "Expected one JSON-RPC request or notification",
        );
    }
    // No core HTTP notifications exist in this revision. Cancellation is stream closure.
    if id.is_none() {
        return error(
            StatusCode::BAD_REQUEST,
            None,
            -32600,
            "No client notifications are implemented over HTTP; close the request SSE stream to cancel",
        );
    }
    let method = request["method"].as_str().unwrap();
    let meta = &request["params"]["_meta"];
    let Some(version) = meta["io.modelcontextprotocol/protocolVersion"].as_str() else {
        return mismatch(id);
    };
    if !equal(&headers, "mcp-protocol-version", version, false)
        || !equal(&headers, "mcp-method", method, false)
    {
        return mismatch(id);
    }
    if version != "2026-07-28" {
        let mut response = error(
            StatusCode::BAD_REQUEST,
            id,
            -32022,
            "Unsupported protocol version",
        );
        let body = json!({"jsonrpc":"2.0","id":id,"error":{"code":-32022,"message":"Unsupported protocol version","data":{"supported":["2026-07-28"],"requested":version}}});
        *response.body_mut() = Body::from(body.to_string());
        return response;
    }
    if !meta["io.modelcontextprotocol/clientCapabilities"].is_object() {
        return error(
            StatusCode::BAD_REQUEST,
            id,
            -32602,
            "Missing or invalid client capabilities",
        );
    }
    if matches!(method, "tools/call" | "prompts/get" | "resources/read") {
        let key = if method == "resources/read" {
            "uri"
        } else {
            "name"
        };
        let Some(name) = request["params"][key].as_str() else {
            return error(StatusCode::BAD_REQUEST, id, -32602, "Missing name or URI");
        };
        if !equal(&headers, "mcp-name", name, true) {
            return mismatch(id);
        }
    }
    if !matches!(
        method,
        "server/discover"
            | "ping"
            | "tools/list"
            | "tools/call"
            | "resources/list"
            | "resources/read"
            | "resources/templates/list"
            | "prompts/list"
            | "prompts/get"
    ) {
        return error(StatusCode::NOT_FOUND, id, -32601, "Method not found");
    }
    let selected = meta.get("io.rune-kit/allowedTools");
    if selected.is_some_and(|v| {
        v.as_array()
            .is_none_or(|a| a.iter().any(|v| !v.is_string()))
    }) {
        return error(
            StatusCode::BAD_REQUEST,
            id,
            -32602,
            "io.rune-kit/allowedTools must be an array of tool names",
        );
    }
    if method == "tools/call"
        && selected.is_some_and(|a| !a.as_array().unwrap().contains(&request["params"]["name"]))
    {
        return error(
            StatusCode::OK,
            id,
            -32600,
            "Tool is outside the client's selected tool subset",
        );
    }
    let permit = match state.permits.clone().try_acquire_owned() {
        Ok(p) => p,
        Err(_) => {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                id,
                -32000,
                "Server queue is full; retry later",
            );
        }
    };
    let sequence = state.sequence.fetch_add(1, Ordering::Relaxed);
    let cancel = Arc::new(AtomicBool::new(false));
    let record = Call {
        sequence,
        method: method.chars().take(80).collect(),
        name: request["params"]["name"]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(120)
            .collect(),
        status: "queued".into(),
        millis: 0,
    };
    state.snapshot.lock().unwrap().active.push(record.clone());
    state.flags.lock().unwrap().insert(sequence, cancel.clone());
    let guard = CancelOnDrop(cancel.clone());
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Option<Value>>();
    let (tx, rx) = mpsc::channel::<Value>(1);
    let shared = state.clone();
    let flag = cancel.clone();
    let response_id = request["id"].clone();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let started = Instant::now();
        let mut router = shared.router.lock().unwrap_or_else(|p| p.into_inner());
        let mut record = record;
        let response = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if flag.load(Ordering::Relaxed) {
            None
        } else {
            if let Some(active) = shared
                .snapshot
                .lock()
                .unwrap()
                .active
                .iter_mut()
                .find(|r| r.sequence == sequence)
            {
                active.status = "running".into();
            }
            let preflight = preflight(&mut router, &request, &headers, flag.clone());
            let _ = ready_tx.send(preflight.clone());
            let mut response = if let Some(error) = preflight {
                Some(error)
            } else {
                router.handle_cancellable(&request.to_string(), flag.clone())
            };
            if request["method"] == "tools/list"
                && response
                    .as_ref()
                    .and_then(|r| r["result"]["tools"].as_array())
                    .is_some_and(|tools| {
                        tools
                            .iter()
                            .any(|tool| annotations(&tool["inputSchema"]).is_err())
                    })
            {
                response = Some(
                    json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32000,"message":"Tool discovery failed: invalid x-mcp-header annotation"}}),
                );
            }
            if request["method"] == "tools/list"
                && let Some(selected) = selected_tools(&request)
                && let Some(tools) = response
                    .as_mut()
                    .and_then(|r| r["result"]["tools"].as_array_mut())
            {
                tools.retain(|t| selected.contains(&t["name"]));
            }
            response
        }
        })).unwrap_or_else(|_| Some(json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32603,"message":"Plugin operation failed internally"}})));
        record.millis = started.elapsed().as_millis();
        record.status = if flag.load(Ordering::Relaxed) {
            "cancelled".into()
        } else if let Some(code) = response.as_ref().and_then(|r| r["error"]["code"].as_i64()) {
            let label = match code {
                -32600 => "forbidden: check server permissions",
                -32602 => "invalid request: check tool schema",
                -32020 => "header mismatch: fix client headers",
                -32022 => "version mismatch: update client",
                -32000 => "execution/discovery failed: inspect client error",
                -32800 => "cancelled",
                _ => "request failed: inspect client error",
            };
            format!("{label} ({code})")
        } else if response
            .as_ref()
            .is_some_and(|r| r["result"]["isError"] == true)
        {
            "tool failure".into()
        } else {
            "complete".into()
        };
        let mut snapshot = shared.snapshot.lock().unwrap();
        if record.method == "tools/list" {
            if !flag.load(Ordering::Relaxed)
                && let Some(tools) = response
                    .as_ref()
                    .and_then(|r| r["result"]["tools"].as_array())
            {
                snapshot.discovery_note =
                    format!("{} tools returned by the last discovery", tools.len());
                snapshot.tools = tools
                    .iter()
                    .take(256)
                    .filter_map(|tool| tool["name"].as_str())
                    .map(|name| name.chars().take(160).collect())
                    .collect();
            }
            let state = if response.as_ref().is_some_and(|r| r.get("error").is_some()) {
                "discovery failed (see client error)"
            } else {
                "discovered"
            };
            for plugin in &mut snapshot.plugins {
                if let Some((prefix, _)) = plugin.split_once("; ") {
                    *plugin = format!("{prefix}; {state}");
                }
            }
        }
        snapshot.active.retain(|r| r.sequence != sequence);
        snapshot.recent.push_front(record);
        snapshot.recent.truncate(HISTORY);
        drop(snapshot);
        shared.flags.lock().unwrap().remove(&sequence);
        if !flag.load(Ordering::Relaxed)
            && let Some(response) = response
        {
            let _ = tx.blocking_send(response);
        }
    });
    // The body owns the cancellation guard. Dropping a single stream only affects
    // this internal sequence, even when unrelated clients reuse JSON-RPC IDs.
    let ready = match tokio::time::timeout(Duration::from_secs(35), ready_rx).await {
        Ok(Ok(value)) => value,
        Ok(Err(_)) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                Some(&response_id),
                -32603,
                "Server worker failed",
            );
        }
        Err(_) => {
            return error(
                StatusCode::REQUEST_TIMEOUT,
                Some(&response_id),
                -32000,
                "Server queue or discovery deadline exceeded",
            );
        }
    };
    if let Some(value) = ready {
        let status = if value["error"]["code"] == -32020 {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::OK
        };
        return (status, axum::Json(value)).into_response();
    }
    let stream = futures_util::stream::unfold((rx, guard), |(mut rx, guard)| async move {
        rx.recv().await.map(|value| {
            (
                Ok::<_, Infallible>(format!("event: message\ndata: {value}\n\n")),
                (rx, guard),
            )
        })
    });
    let mut response = Response::new(Body::from_stream(stream));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, "text/event-stream".parse().unwrap());
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert("x-accel-buffering", "no".parse().unwrap());
    response
}
fn selected_tools(request: &Value) -> Option<&Vec<Value>> {
    request["params"]["_meta"]["io.rune-kit/allowedTools"].as_array()
}

fn mirrors(schema: &Value, args: &Value, headers: &HeaderMap) -> Result<(), String> {
    for (path, name) in annotations(schema)? {
        let mut value = args;
        for part in path {
            value = &value[&part];
        }
        let header = format!("mcp-param-{name}");
        if !value.is_null() {
            let matches = match value {
                Value::String(text) => equal(headers, &header, text, true),
                Value::Bool(flag) => equal(headers, &header, &flag.to_string(), true),
                Value::Number(number) => {
                    let numeric = number
                        .as_f64()
                        .filter(|n| {
                            n.is_finite() && n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_991.0
                        })
                        .ok_or("Invalid mirrored integer range")?;
                    single(headers, &header)
                        .and_then(decode)
                        .and_then(|s| s.parse::<f64>().ok())
                        == Some(numeric)
                }
                _ => return Err("Invalid mirrored parameter type".into()),
            };
            if !matches {
                return Err("Header mismatch".into());
            }
        } else if headers.contains_key(&header) {
            return Err("Unexpected mirrored header".into());
        }
    }
    Ok(())
}
fn preflight(
    router: &mut McpRouter,
    request: &Value,
    headers: &HeaderMap,
    cancel: Arc<AtomicBool>,
) -> Option<Value> {
    if request["method"] == "tools/call" {
        let list = json!({"jsonrpc":"2.0","id":"internal-header-check","method":"tools/list","params":{"_meta":request["params"]["_meta"]}});
        if let Some(list) = router.handle_cancellable(&list.to_string(), cancel.clone()) {
            if list.get("error").is_some() {
                let mut list = list;
                list["id"] = request["id"].clone();
                return Some(list);
            }
            if list["result"]["tools"].as_array().is_some_and(|tools| {
                tools
                    .iter()
                    .any(|tool| annotations(&tool["inputSchema"]).is_err())
            }) {
                return Some(
                    json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32000,"message":"Tool discovery failed: invalid x-mcp-header annotation"}}),
                );
            }
            if let Some(tool) = list["result"]["tools"]
                .as_array()
                .and_then(|t| t.iter().find(|t| t["name"] == request["params"]["name"]))
                && mirrors(
                    &tool["inputSchema"],
                    &request["params"]["arguments"],
                    headers,
                )
                .is_err()
            {
                return Some(
                    json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32020,"message":"Header mismatch in mirrored tool parameters"}}),
                );
            }
        }
    }
    None
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let terminate = async {
            if let Ok(mut signal) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            {
                signal.recv().await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        tokio::select! { _=tokio::signal::ctrl_c()=>{}, _=terminate=>{} }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

pub struct EmbeddedServer {
    pub endpoint: String,
    pub address: SocketAddr,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<Result<(), String>>,
}

impl EmbeddedServer {
    pub async fn shutdown(mut self) -> Result<(), String> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        match tokio::time::timeout(std::time::Duration::from_secs(5), &mut self.task).await {
            Ok(Ok(res)) => res,
            Ok(Err(err)) => Err(format!("Embedded Kit server task failed: {err}")),
            Err(_) => {
                self.task.abort();
                let _ = self.task.await;
                Err("Embedded Kit server did not stop within timeout".into())
            }
        }
    }

    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
}

pub async fn start_embedded(
    router: McpRouter,
    options: Options,
    configuration: crate::configuration::Configuration,
) -> Result<EmbeddedServer, String> {
    validate_bind(options.bind).map_err(|e| e.to_string())?;
    let listener = tokio::net::TcpListener::bind((options.bind, options.port))
        .await
        .map_err(|e| {
            format!(
                "Cannot bind Kit HTTP server to {}:{}: {e}",
                options.bind, options.port
            )
        })?;
    let address = listener.local_addr().map_err(|e| e.to_string())?;
    let connect_address = if address.ip().is_unspecified() {
        SocketAddr::new(
            if address.is_ipv4() {
                "127.0.0.1".parse().unwrap()
            } else {
                "::1".parse().unwrap()
            },
            address.port(),
        )
    } else {
        address
    };
    let endpoint = format!("http://{connect_address}/mcp");
    let snapshot = Arc::new(Mutex::new(Snapshot {
        bind_address: address.to_string(),
        workspace_only: options.workspace_only,
        endpoint: endpoint.clone(),
        status: "listening".into(),
        started: Instant::now(),
        plugins: options.plugins,
        read_only: options.read_only,
        allowed_tools: options.allowed_tools,
        roots: options.roots,
        timeout: options.timeout,
        executable: std::env::current_exe().unwrap_or_default(),
        data_root: rune_kit_core::paths::managed_root().unwrap_or_default(),
        active: Vec::new(),
        recent: VecDeque::new(),
        tools: Vec::new(),
        discovery_note: "Press d to discover tools in this server".into(),
    }));
    let configuration = Arc::new(Mutex::new(configuration));
    let state = Arc::new(Shared {
        router: Arc::new(Mutex::new(router)),
        configuration: configuration.clone(),
        snapshot: snapshot.clone(),
        address,
        sequence: AtomicU64::new(1),
        permits: Arc::new(Semaphore::new(32)),
        flags: Mutex::new(HashMap::new()),
        token: http_token().map_err(|e| e.to_string())?,
    });
    let app = Router::new()
        .route("/mcp", post(handle))
        .route(
            "/management/config",
            get(configuration_fields).put(update_configuration),
        )
        .route(
            "/management/remotes",
            get(remote_servers).put(update_remote_server),
        )
        .route("/management/packages", get(packages).post(install_package))
        .route("/management/packages/{action}", post(package_operation))
        .route("/management/icons/{name}", get(package_icon))
        .route("/management/registry", get(registry_list))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            security,
        ))
        .with_state(state.clone());

    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown_state = state.clone();
    let shutdown = async move {
        let _ = stop_rx.await;
        shutdown_state.snapshot.lock().unwrap().status = "stopping".into();
        for flag in shutdown_state.flags.lock().unwrap().values() {
            flag.store(true, Ordering::Relaxed);
        }
    };

    let task = tokio::spawn(async move {
        let res = axum::serve(listener, app)
            .with_graceful_shutdown(shutdown)
            .await;
        state.snapshot.lock().unwrap().status = "stopped".into();
        for flag in state.flags.lock().unwrap().values() {
            flag.store(true, Ordering::Relaxed);
        }
        res.map_err(|e| e.to_string())
    });

    Ok(EmbeddedServer {
        endpoint,
        address,
        stop: Some(stop_tx),
        task,
    })
}

pub async fn start_default_embedded(
    bind: std::net::IpAddr,
    port: u16,
) -> Result<EmbeddedServer, String> {
    let (router, options, configuration) =
        crate::load_installed_environment(&[], false, 30, false, &[], &[], bind, port, true)
            .map_err(|e| e.to_string())?;
    start_embedded(router, options, configuration).await
}

pub async fn serve(
    router: McpRouter,
    options: Options,
    configuration: crate::configuration::Configuration,
) -> Result<(), Box<dyn std::error::Error>> {
    validate_bind(options.bind)?;
    let listener=tokio::net::TcpListener::bind((options.bind,options.port)).await.map_err(|e|format!("Cannot bind Kit HTTP server to {}:{}: {e}. Choose another --port or stop the existing server.",options.bind,options.port))?;
    let address = listener.local_addr()?;
    let connect_address = if address.ip().is_unspecified() {
        SocketAddr::new(
            if address.is_ipv4() {
                "127.0.0.1".parse()?
            } else {
                "::1".parse()?
            },
            address.port(),
        )
    } else {
        address
    };
    let endpoint = format!("http://{connect_address}/mcp");
    let snapshot = Arc::new(Mutex::new(Snapshot {
        bind_address: address.to_string(),
        workspace_only: options.workspace_only,
        endpoint: endpoint.clone(),
        status: "listening".into(),
        started: Instant::now(),
        plugins: options.plugins,
        read_only: options.read_only,
        allowed_tools: options.allowed_tools,
        roots: options.roots,
        timeout: options.timeout,
        executable: std::env::current_exe()?,
        data_root: rune_kit_core::paths::managed_root()?,
        active: Vec::new(),
        recent: VecDeque::new(),
        tools: Vec::new(),
        discovery_note: "Press d to discover tools in this server".into(),
    }));
    let configuration = Arc::new(Mutex::new(configuration));
    let state = Arc::new(Shared {
        router: Arc::new(Mutex::new(router)),
        configuration: configuration.clone(),
        snapshot: snapshot.clone(),
        address,
        sequence: AtomicU64::new(1),
        permits: Arc::new(Semaphore::new(32)),
        flags: Mutex::new(HashMap::new()),
        token: http_token()?,
    });
    eprintln!(
        "Kit HTTP listening at {endpoint}; bound {address}; filesystem isolation: {}; Ctrl+C to stop",
        if options.workspace_only {
            "workspace-only WASM; external originals and native helpers blocked"
        } else {
            "not enforced"
        }
    );
    let (stop_tx, mut stop_rx) = tokio::sync::mpsc::channel::<()>(1);
    use std::io::IsTerminal;
    let dashboard_stop = stop_tx.clone();
    let dashboard_router = state.router.clone();
    let _keep_stop_sender = stop_tx;
    let dashboard =
        if !options.headless && std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            Some(tokio::task::spawn_blocking(move || {
                let result =
                    crate::tui::server_dashboard(snapshot, dashboard_router, configuration);
                let _ = dashboard_stop.blocking_send(());
                result.map_err(|e| e.to_string())
            }))
        } else {
            None
        };
    let app = Router::new()
        .route("/mcp", post(handle))
        .route(
            "/management/config",
            get(configuration_fields).put(update_configuration),
        )
        .route(
            "/management/remotes",
            get(remote_servers).put(update_remote_server),
        )
        .route("/management/packages", get(packages).post(install_package))
        .route("/management/packages/{action}", post(package_operation))
        .route("/management/icons/{name}", get(package_icon))
        .route("/management/registry", get(registry_list))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            security,
        ))
        .with_state(state.clone());
    let shutdown_state = state.clone();
    let shutdown = async move {
        tokio::select! { _=shutdown_signal()=>{}, _=stop_rx.recv()=>{} }
        shutdown_state.snapshot.lock().unwrap().status = "stopping".into();
        for flag in shutdown_state.flags.lock().unwrap().values() {
            flag.store(true, Ordering::Relaxed);
        }
    };
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await;
    state.snapshot.lock().unwrap().status = "stopped".into();
    for flag in state.flags.lock().unwrap().values() {
        flag.store(true, Ordering::Relaxed);
    }
    if let Some(dashboard) = dashboard {
        dashboard.await?.map_err(std::io::Error::other)?;
    }
    result?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn package_updates_dispatch_mcp_and_plugin_to_mcp_updater() {
        for (kind, expected) in [
            ("mcp", StatusCode::ACCEPTED),
            ("plugin", StatusCode::ACCEPTED),
            ("skill", StatusCode::IM_A_TEAPOT),
        ] {
            let response = dispatch_package_update(
                is_mcp_kind(kind),
                async { StatusCode::ACCEPTED.into_response() },
                async { StatusCode::IM_A_TEAPOT.into_response() },
            )
            .await;
            assert_eq!(
                response.status(),
                expected,
                "wrong update handler for {kind}"
            );
        }
    }
    #[test]
    fn package_inventory_reports_installed_entries_and_rejects_bad_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("installed_skills.json"), json!({"version":1,"skills":{"example":{"name":"example","version":"1.0","author":"author"}}}).to_string()).unwrap();
        let packages = installed_packages(dir.path()).unwrap();
        assert!(packages["plugins"].as_array().unwrap().is_empty());
        assert_eq!(packages["skills"][0]["name"], "example");
        std::fs::write(dir.path().join("installed.json"), b"invalid").unwrap();
        assert!(installed_packages(dir.path()).is_err());
    }
    #[tokio::test]
    async fn management_hides_secrets_and_refuses_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let mut config =
            crate::configuration::Configuration::read(dir.path().join("config.json")).unwrap();
        let manifest = serde_json::from_value(json!({"configuration":{"schema_version":1,"fields":{
            "api_key":{"type":"string","description":"Key","required":false,"secret":true,"default":"private-value"}
        }}})).unwrap();
        config.plugins.push(crate::configuration::Plugin {
            name: "test".into(),
            path: Default::default(),
            kind: rune_kit_core::ExecutionKind::Native,
            manifest,
            params: Default::default(),
            workspace: None,
        });
        let address: SocketAddr = "127.0.0.1:47601".parse().unwrap();
        let state = Arc::new(Shared {
            router: Arc::new(Mutex::new(McpRouter::new())),
            configuration: Arc::new(Mutex::new(config)),
            address,
            snapshot: Arc::new(Mutex::new(Snapshot {
                bind_address: address.to_string(),
                workspace_only: false,
                endpoint: String::new(),
                status: String::new(),
                started: Instant::now(),
                plugins: vec![],
                read_only: false,
                allowed_tools: vec![],
                roots: vec![],
                timeout: 1,
                executable: Default::default(),
                data_root: Default::default(),
                active: vec![],
                recent: VecDeque::new(),
                tools: vec![],
                discovery_note: String::new(),
            })),
            sequence: AtomicU64::new(1),
            permits: Arc::new(Semaphore::new(1)),
            flags: Mutex::new(HashMap::new()),
            token: None,
        });
        let response = configuration_fields(State(state.clone())).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), BODY_LIMIT)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["fields"][0]["value"], "");
        assert!(!body.to_string().contains("private-value"));
        let response = update_configuration(
            State(state),
            axum::Json(ConfigurationUpdate {
                plugin: "test".into(),
                key: "missing".into(),
                value: "x".into(),
                shared: false,
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(!dir.path().join("config.json").exists());
    }
    #[test]
    fn encoded_names_and_header_annotations_follow_static_paths() {
        assert_eq!(
            decode("=?base64?SGVsbG8sIOS4lueVjA==?=").unwrap(),
            "Hello, 世界"
        );
        assert_eq!(decode(" padded "), None);
        assert_eq!(decode("=?base64?not valid?="), None);
        let schema = json!({"type":"object","properties":{"nested":{"type":"object","properties":{"tenant":{"type":"string","x-mcp-header":"Tenant"}}}}});
        let args = json!({"nested":{"tenant":"東京"}});
        let mut headers = HeaderMap::new();
        headers.insert(
            "mcp-param-tenant",
            format!("=?base64?{}?=", STANDARD.encode("東京"))
                .parse()
                .unwrap(),
        );
        assert!(mirrors(&schema, &args, &headers).is_ok());
        assert!(mirrors(&schema, &args, &HeaderMap::new()).is_err());
        let integer = json!({"properties":{"count":{"type":"integer","x-mcp-header":"Count"}}});
        let mut numeric = HeaderMap::new();
        numeric.insert("mcp-param-count", "42".parse().unwrap());
        assert!(mirrors(&integer, &json!({"count":42.0}), &numeric).is_ok());
        assert!(mirrors(&integer, &json!({"count":42.5}), &numeric).is_err());
        for invalid in [
            json!({"items":{"type":"string","x-mcp-header":"A"}}),
            json!({"properties":{"x":{"type":"number","x-mcp-header":"A"}}}),
            json!({"properties":{"x":{"type":"string","x-mcp-header":"A"},"y":{"type":"string","x-mcp-header":"a"}}}),
        ] {
            assert!(annotations(&invalid).is_err());
        }
    }
}
