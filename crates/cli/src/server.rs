//! Local, sessionless MCP 2026-07-28 Streamable HTTP transport.
use axum::{
    Router,
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use rune_kit_core::McpRouter;
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
    router: Mutex<McpRouter>,
    snapshot: Arc<Mutex<Snapshot>>,
    address: SocketAddr,
    sequence: AtomicU64,
    permits: Arc<Semaphore>,
    flags: Mutex<HashMap<u64, Arc<AtomicBool>>>,
    token: Option<String>,
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
    host == Some(address.to_string().as_str())
        || host == Some(format!("localhost:{}", address.port()).as_str())
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
            "Only local non-browser clients with the configured Host are supported",
        );
    }
    if let Some(token) = &state.token
        && single(&headers, "authorization") != Some(format!("Bearer {token}").as_str())
    {
        return error(
            StatusCode::UNAUTHORIZED,
            None,
            -32600,
            "A valid local bearer token is required",
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

// Validate definitions as well as values; annotations below arrays, refs or
// composition keywords are not statically reachable and cannot be trusted.
fn annotations(schema: &Value) -> Result<Vec<(Vec<String>, String)>, String> {
    fn walk(
        node: &Value,
        path: Vec<String>,
        reachable: bool,
        property: bool,
        out: &mut Vec<(Vec<String>, String)>,
    ) -> Result<(), String> {
        if let Some(name) = node.get("x-mcp-header") {
            let name = name.as_str().ok_or("Invalid x-mcp-header")?;
            if !reachable
                || !property
                || !matches!(
                    node["type"].as_str(),
                    Some("string" | "integer" | "boolean")
                )
                || name.is_empty()
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
                || out.iter().any(|(_, seen)| seen.eq_ignore_ascii_case(name))
            {
                return Err("Invalid or duplicate x-mcp-header".into());
            }
            out.push((path.clone(), name.to_owned()));
        }
        if let Some(object) = node.as_object() {
            for (key, value) in object {
                if key == "properties" {
                    if let Some(properties) = value.as_object() {
                        for (name, child) in properties {
                            let mut path = path.clone();
                            path.push(name.clone());
                            walk(child, path, reachable, true, out)?;
                        }
                    }
                } else {
                    walk(value, path.clone(), false, false, out)?;
                }
            }
        } else if let Some(array) = node.as_array() {
            for child in array {
                walk(child, path.clone(), false, false, out)?;
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(schema, Vec::new(), true, false, &mut out)?;
    Ok(out)
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

pub async fn serve(router: McpRouter, options: Options) -> Result<(), Box<dyn std::error::Error>> {
    if !options.bind.is_loopback() {
        return Err("Only loopback binding is supported".into());
    }
    let listener=tokio::net::TcpListener::bind((options.bind,options.port)).await.map_err(|e|format!("Cannot bind Kit HTTP server to {}:{}: {e}. Choose another --port or stop the existing server.",options.bind,options.port))?;
    let address = listener.local_addr()?;
    let endpoint = format!("http://{address}/mcp");
    let snapshot = Arc::new(Mutex::new(Snapshot {
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
    let state = Arc::new(Shared {
        router: Mutex::new(router),
        snapshot: snapshot.clone(),
        address,
        sequence: AtomicU64::new(1),
        permits: Arc::new(Semaphore::new(32)),
        flags: Mutex::new(HashMap::new()),
        token: std::env::var("RUNE_KIT_HTTP_TOKEN")
            .ok()
            .filter(|v| !v.is_empty()),
    });
    eprintln!("Kit HTTP listening at {endpoint}; Ctrl+C to stop");
    let (stop_tx, mut stop_rx) = tokio::sync::mpsc::channel::<()>(1);
    use std::io::IsTerminal;
    let dashboard_stop = stop_tx.clone();
    let _keep_stop_sender = stop_tx;
    let dashboard =
        if !options.headless && std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            Some(tokio::task::spawn_blocking(move || {
                let result = crate::tui::server_dashboard(snapshot);
                let _ = dashboard_stop.blocking_send(());
                result.map_err(|e| e.to_string())
            }))
        } else {
            None
        };
    let app = Router::new()
        .route("/mcp", post(handle))
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
