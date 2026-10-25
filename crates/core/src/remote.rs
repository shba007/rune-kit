//! External Streamable HTTP MCP client. Owns no remote process or filesystem.
use crate::{
    headers,
    manifest::{PluginInfo, PluginManifest, PromptDefinition, ResourceDefinition, ToolDefinition},
    runtime::RuntimeError,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

const CURRENT: &str = "2026-07-28";
const LIMIT: usize = 8 * 1024 * 1024;
fn fail(message: &str) -> RuntimeError {
    RuntimeError::Execution(message.into())
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteConfig {
    pub endpoint: String,
    pub protocol: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_env: Option<String>,
}
impl RemoteConfig {
    pub fn validate(&self) -> Result<(), RuntimeError> {
        let url = reqwest::Url::parse(&self.endpoint)
            .map_err(|_| fail("Remote endpoint must be a valid HTTP or HTTPS URL"))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(fail(
                "Remote endpoint must use HTTP/S without embedded credentials or a fragment",
            ));
        }
        if !matches!(
            self.protocol.as_str(),
            CURRENT | "2025-11-25" | "2025-06-18" | "2025-03-26"
        ) {
            return Err(fail("Unsupported remote MCP protocol version"));
        }
        if self.token_env.as_ref().is_some_and(|key| {
            key.is_empty()
                || !key
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        }) {
            return Err(fail(
                "Bearer-token environment variable must use uppercase letters, digits, and underscores",
            ));
        }
        Ok(())
    }
    pub fn redacted_endpoint(&self) -> String {
        reqwest::Url::parse(&self.endpoint)
            .map(|mut url| {
                let _ = url.set_username("");
                let _ = url.set_password(None);
                url.set_query(None);
                url.set_fragment(None);
                url.to_string()
            })
            .unwrap_or_else(|_| "invalid endpoint".into())
    }
}

#[derive(Clone)]
struct Context {
    cancel: Arc<AtomicBool>,
    deadline: Instant,
    seconds: u64,
}
impl Context {
    fn new(cancel: Arc<AtomicBool>, timeout: Duration) -> Self {
        Self {
            cancel,
            deadline: Instant::now() + timeout,
            seconds: timeout.as_secs(),
        }
    }
}
struct Message {
    method: String,
    params: Value,
    headers: Vec<(String, String)>,
    context: Context,
    reply: mpsc::Sender<Result<Value, RuntimeError>>,
}

pub struct RemoteHttp {
    pub manifest: PluginManifest,
    name: String,
    sender: Option<mpsc::Sender<Message>>,
    worker: Option<JoinHandle<()>>,
    context: Option<Context>,
    pub(crate) tools: BTreeMap<String, Value>,
    pub(crate) resources: BTreeMap<String, Value>,
    pub(crate) prompts: BTreeMap<String, Value>,
}
impl RemoteHttp {
    pub fn new(
        name: String,
        config: RemoteConfig,
        token: Option<String>,
    ) -> Result<Self, RuntimeError> {
        config.validate()?;
        if token
            .as_ref()
            .is_some_and(|value| value.trim().is_empty() || value.chars().any(char::is_control))
        {
            return Err(fail("Invalid remote bearer token"));
        }
        let (sender, rx) = mpsc::channel::<Message>();
        let worker = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            let mut connection = Connection::new(config, token);
            while let Ok(message) = rx.recv() {
                let result = match (&runtime, &mut connection) {
                    (Ok(runtime), Ok(connection)) => runtime.block_on(async {
                        let operation =
                            connection.request(&message.method, message.params, message.headers);
                        tokio::pin!(operation);
                        loop {
                            if message.context.cancel.load(Ordering::Relaxed) {
                                return Err(RuntimeError::Cancelled);
                            }
                            if Instant::now() >= message.context.deadline {
                                return Err(RuntimeError::Timeout(message.context.seconds));
                            }
                            tokio::select! {
                                result=&mut operation=>return result,
                                _=tokio::time::sleep(Duration::from_millis(20))=>{}
                            }
                        }
                    }),
                    _ => Err(fail("Cannot initialize remote HTTP transport")),
                };
                let _ = message.reply.send(result);
            }
        });
        Ok(Self {
            manifest: PluginManifest::default(),
            name,
            sender: Some(sender),
            worker: Some(worker),
            context: None,
            tools: BTreeMap::new(),
            resources: BTreeMap::new(),
            prompts: BTreeMap::new(),
        })
    }
    pub fn set_operation_context(&mut self, cancel: Arc<AtomicBool>, timeout: Option<Duration>) {
        self.context = timeout.map(|timeout| Context::new(cancel, timeout));
    }
    pub fn cancel(&self) {
        if let Some(context) = &self.context {
            context.cancel.store(true, Ordering::Relaxed);
        }
    }
    fn request(
        &mut self,
        method: &str,
        params: Value,
        headers: Vec<(String, String)>,
    ) -> Result<Value, RuntimeError> {
        let context = self.context.clone().unwrap_or_else(|| {
            Context::new(Arc::new(AtomicBool::new(false)), Duration::from_secs(30))
        });
        if context.cancel.load(Ordering::Relaxed) {
            return Err(RuntimeError::Cancelled);
        }
        let remaining = context.deadline.saturating_duration_since(Instant::now());
        let (reply, rx) = mpsc::channel();
        self.sender
            .as_ref()
            .ok_or_else(|| fail("Remote transport is closed"))?
            .send(Message {
                method: method.into(),
                params,
                headers,
                context,
                reply,
            })
            .map_err(|_| fail("Remote transport worker stopped"))?;
        rx.recv_timeout(remaining + Duration::from_secs(1))
            .map_err(|_| fail("Remote transport worker did not return before the deadline"))?
    }
    fn paged(&mut self, method: &str, key: &str) -> Result<Vec<Value>, RuntimeError> {
        let mut values = Vec::new();
        let mut cursor = None;
        let mut seen = HashSet::new();
        for _ in 0..20 {
            let params = cursor
                .as_ref()
                .map_or_else(|| json!({}), |value| json!({"cursor":value}));
            let result = match self.request(method, params, vec![]) {
                Err(RuntimeError::RemoteRpc { code: -32601 }) if values.is_empty() => {
                    return Ok(vec![]);
                }
                other => other?,
            };
            let page = result[key]
                .as_array()
                .ok_or_else(|| fail("Malformed remote discovery response"))?;
            if values.len() + page.len() > 10_000 {
                return Err(fail(
                    "Remote discovery inventory exceeds the supported limit",
                ));
            }
            values.extend(page.iter().cloned());
            match result.get("nextCursor") {
                None | Some(Value::Null) => return Ok(values),
                Some(Value::String(value)) if !value.is_empty() && seen.insert(value.clone()) => {
                    cursor = Some(value.clone())
                }
                _ => return Err(fail("Invalid or repeated remote discovery cursor")),
            }
        }
        Err(fail("Remote discovery exceeds the supported page limit"))
    }
    pub fn get_info(&mut self) -> Result<PluginInfo, RuntimeError> {
        Ok(PluginInfo {
            name: self.name.clone(),
            version: "external HTTP server".into(),
            description: Some("External MCP server; its filesystem is not managed by Kit".into()),
        })
    }
    pub fn list_tools(&mut self) -> Result<Vec<ToolDefinition>, RuntimeError> {
        let mut tools = Vec::new();
        let mut raw = BTreeMap::new();
        for mut value in self.paged("tools/list", "tools")? {
            if !value.is_object() {
                return Err(fail("Invalid remote tool definition"));
            }
            if value.get("description").is_none() {
                value["description"] = json!("");
            }
            let tool: ToolDefinition = serde_json::from_value(value.clone())
                .map_err(|_| fail("Invalid remote tool definition"))?;
            headers::annotations(&tool.input_schema)
                .map_err(|_| fail("Invalid remote x-mcp-header annotation"))?;
            if tool.name.is_empty() || raw.insert(tool.name.clone(), value).is_some() {
                return Err(fail("Empty or duplicate remote tool name"));
            }
            tools.push(tool);
        }
        self.tools = raw;
        Ok(tools)
    }
    pub fn call_tool(&mut self, name: &str, args: Value) -> Result<Value, RuntimeError> {
        let schema = self
            .tools
            .get(name)
            .map(|tool| &tool["inputSchema"])
            .ok_or_else(|| fail("Discover remote tools before calling them"))?;
        let headers = headers::mirror_values(schema, &args)
            .map_err(|_| fail("Cannot mirror remote tool parameters"))?;
        let result = self.request("tools/call", json!({"name":name,"arguments":args}), headers)?;
        if !result["content"].is_array() {
            return Err(fail("Malformed remote tool result"));
        }
        Ok(result)
    }
    pub fn list_resources(&mut self) -> Result<Vec<ResourceDefinition>, RuntimeError> {
        let mut resources = Vec::new();
        let mut raw = BTreeMap::new();
        for mut value in self.paged("resources/list", "resources")? {
            if !value.is_object() {
                return Err(fail("Invalid remote resource definition"));
            }
            if value.get("description").is_none() {
                value["description"] = json!("");
            }
            let resource: ResourceDefinition = serde_json::from_value(value.clone())
                .map_err(|_| fail("Invalid remote resource definition"))?;
            if reqwest::Url::parse(&resource.uri).is_err()
                || raw.insert(resource.uri.clone(), value).is_some()
            {
                return Err(fail("Invalid or duplicate remote resource URI"));
            }
            resources.push(resource);
        }
        self.resources = raw;
        Ok(resources)
    }
    pub fn read_resource(&mut self, uri: &str) -> Result<Value, RuntimeError> {
        let mut result = self.request("resources/read", json!({"uri":uri}), vec![])?;
        let contents = result["contents"]
            .as_array_mut()
            .ok_or_else(|| fail("Malformed remote resource result"))?;
        for content in contents {
            let uri = content["uri"]
                .as_str()
                .ok_or_else(|| fail("Malformed remote resource content URI"))?;
            content["uri"] = json!(format!("rune://{}/{uri}", self.name));
        }
        Ok(result)
    }
    pub fn list_prompts(&mut self) -> Result<Vec<PromptDefinition>, RuntimeError> {
        let mut prompts = Vec::new();
        let mut raw = BTreeMap::new();
        for mut value in self.paged("prompts/list", "prompts")? {
            if !value.is_object() {
                return Err(fail("Invalid remote prompt definition"));
            }
            if value.get("description").is_none() {
                value["description"] = json!("");
            }
            if let Some(args) = value.get_mut("arguments").and_then(Value::as_array_mut) {
                for arg in args {
                    if !arg.is_object() {
                        return Err(fail("Invalid remote prompt argument"));
                    }
                    if arg.get("description").is_none() {
                        arg["description"] = json!("");
                    }
                }
            }
            let prompt: PromptDefinition = serde_json::from_value(value.clone())
                .map_err(|_| fail("Invalid remote prompt definition"))?;
            if prompt.name.is_empty() || raw.insert(prompt.name.clone(), value).is_some() {
                return Err(fail("Empty or duplicate remote prompt name"));
            }
            prompts.push(prompt);
        }
        self.prompts = raw;
        Ok(prompts)
    }
    pub fn get_prompt(&mut self, name: &str, args: Value) -> Result<Value, RuntimeError> {
        self.request("prompts/get", json!({"name":name,"arguments":args}), vec![])
    }
}
impl Drop for RemoteHttp {
    fn drop(&mut self) {
        self.cancel();
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Connection {
    config: RemoteConfig,
    client: reqwest::Client,
    token: Option<String>,
    session: Option<String>,
    initialized: bool,
    next_id: u64,
}
impl Connection {
    fn new(config: RemoteConfig, token: Option<String>) -> Result<Self, RuntimeError> {
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(10));
        let url =
            reqwest::Url::parse(&config.endpoint).map_err(|_| fail("Invalid remote endpoint"))?;
        if url.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        }) {
            builder = builder.no_proxy();
        }
        Ok(Self {
            config,
            client: builder
                .build()
                .map_err(|_| fail("Cannot create remote HTTP client"))?,
            token,
            session: None,
            initialized: false,
            next_id: 1,
        })
    }
    async fn request(
        &mut self,
        method: &str,
        params: Value,
        headers: Vec<(String, String)>,
    ) -> Result<Value, RuntimeError> {
        if self.config.protocol != CURRENT && !self.initialized {
            self.session = None;
            let init=self.exchange("initialize",json!({"protocolVersion":self.config.protocol,"capabilities":{},"clientInfo":{"name":"rune-kit","version":env!("CARGO_PKG_VERSION")}}),vec![],false).await?;
            if init["protocolVersion"] != self.config.protocol {
                return Err(fail(
                    "Remote server negotiated a different protocol; configure its supported version explicitly",
                ));
            }
            self.exchange("notifications/initialized", json!({}), vec![], true)
                .await?;
            self.initialized = true;
        }
        self.exchange(method, params, headers, false).await
    }
    async fn exchange(
        &mut self,
        method: &str,
        mut params: Value,
        headers: Vec<(String, String)>,
        notification: bool,
    ) -> Result<Value, RuntimeError> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| fail("Remote request identity exhausted"))?;
        if self.config.protocol == CURRENT {
            params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":CURRENT,"io.modelcontextprotocol/clientCapabilities":{},"io.modelcontextprotocol/clientInfo":{"name":"rune-kit","version":env!("CARGO_PKG_VERSION")}});
        }
        let mut request = self
            .client
            .post(&self.config.endpoint)
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", &self.config.protocol);
        if self.config.protocol == CURRENT {
            request = request.header("Mcp-Method", method);
            if let Some(name) = params
                .get("name")
                .or_else(|| params.get("uri"))
                .and_then(Value::as_str)
            {
                request = request.header("Mcp-Name", headers::encode(name));
            }
        }
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if let Some(session) = &self.session {
            request = request.header("Mcp-Session-Id", session);
        }
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let mut body = json!({"jsonrpc":"2.0","method":method,"params":params});
        if !notification {
            body["id"] = json!(id);
        }
        let mut response = request
            .json(&body)
            .send()
            .await
            .map_err(|_| fail("Remote HTTP request failed; the operation was not retried"))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND && self.session.is_some() {
            self.session = None;
            self.initialized = false;
            return Err(fail(
                "Remote session expired; the operation was not retried",
            ));
        }
        if !response.status().is_success() {
            return Err(fail(&format!(
                "Remote HTTP request failed with status {}; the operation was not retried",
                response.status().as_u16()
            )));
        }
        if let Some(session) = response.headers().get("Mcp-Session-Id") {
            if self.config.protocol == CURRENT {
                return Err(fail(
                    "Session headers are not supported by the current MCP revision",
                ));
            }
            let session = session
                .to_str()
                .map_err(|_| fail("Invalid remote session identifier"))?;
            if session.is_empty() || !session.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
                return Err(fail("Invalid remote session identifier"));
            }
            if method != "initialize" && self.session.as_deref() != Some(session) {
                return Err(fail("Remote changed its session identifier unexpectedly"));
            }
            self.session = Some(session.to_owned());
        }
        if notification {
            return if response.status() == reqwest::StatusCode::ACCEPTED {
                Ok(Value::Null)
            } else {
                Err(fail(
                    "Remote did not acknowledge initialization notification",
                ))
            };
        }
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.split(';').next())
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if !matches!(
            content_type.as_str(),
            "application/json" | "text/event-stream"
        ) {
            return Err(fail("Remote response must be JSON or an SSE stream"));
        }
        let mut data = Vec::new();
        let mut event = String::new();
        let mut first_line = true;
        let mut skip_lf = false;
        let mut total = 0usize;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| fail("Remote response was interrupted; the operation was not retried"))?
        {
            total = total.saturating_add(chunk.len());
            if total > LIMIT {
                return Err(fail("Remote response exceeds the 8 MiB limit"));
            }
            data.extend_from_slice(&chunk);
            if content_type == "text/event-stream" {
                loop {
                    if skip_lf && !data.is_empty() {
                        if data[0] == b'\n' {
                            data.remove(0);
                        }
                        skip_lf = false;
                    }
                    let Some(end) = data.iter().position(|b| matches!(*b, b'\n' | b'\r')) else {
                        break;
                    };
                    skip_lf = data[end] == b'\r' && end + 1 == data.len();
                    let delimiter = if data[end] == b'\r' && data.get(end + 1) == Some(&b'\n') {
                        2
                    } else {
                        1
                    };
                    let line = String::from_utf8(data.drain(..end).collect())
                        .map_err(|_| fail("Remote SSE response is not valid UTF-8"))?;
                    data.drain(..delimiter);
                    let line = if first_line {
                        first_line = false;
                        line.trim_start_matches('\u{feff}')
                    } else {
                        &line
                    };
                    if line.is_empty() {
                        if !event.is_empty() {
                            let value: Value = serde_json::from_str(event.trim_end_matches('\n'))
                                .map_err(|_| fail("Malformed remote SSE event"))?;
                            event.clear();
                            if let Some(result) = result(value, id)? {
                                return Ok(result);
                            }
                        }
                    } else if let Some(value) = line.strip_prefix("data:") {
                        event.push_str(value.strip_prefix(' ').unwrap_or(value));
                        event.push('\n');
                    }
                }
            }
        }
        if content_type == "application/json" {
            let value = serde_json::from_slice(&data)
                .map_err(|_| fail("Malformed remote JSON response"))?;
            return result(value, id)?
                .ok_or_else(|| fail("Remote returned no response for this request"));
        }
        Err(fail(
            "Remote SSE stream ended without a matching response; the operation was not retried",
        ))
    }
}
fn result(value: Value, id: u64) -> Result<Option<Value>, RuntimeError> {
    if value["jsonrpc"] != "2.0" || !value.is_object() {
        return Err(fail("Invalid remote JSON-RPC response"));
    }
    if value.get("method").is_some() {
        if value.get("id").is_none()
            && value["method"]
                .as_str()
                .is_some_and(|m| m.starts_with("notifications/"))
        {
            return Ok(None);
        }
        return Err(fail("Remote requested an unsupported client capability"));
    }
    if value["id"] != json!(id) || value.get("result").is_some() == value.get("error").is_some() {
        return Err(fail(
            "Remote response identity or result envelope does not match the request",
        ));
    }
    if let Some(error) = value.get("error") {
        return Err(RuntimeError::RemoteRpc {
            code: error["code"]
                .as_i64()
                .ok_or_else(|| fail("Invalid remote JSON-RPC error"))?,
        });
    }
    let result = value["result"].clone();
    if !result.is_object() {
        return Err(fail("Remote result must be an object"));
    }
    Ok(Some(result))
}
