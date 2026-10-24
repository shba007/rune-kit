//! Small HTTP MCP test client. Private native pipes remain internal only.
use serde_json::{Value, json};
use std::{
    error::Error,
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    time::Duration,
};
type Result<T> = std::result::Result<T, Box<dyn Error>>;
pub struct Gateway {
    child: Child,
    pub endpoint: String,
    client: reqwest::blocking::Client,
    pending: Option<reqwest::blocking::Response>,
    next: u64,
}
pub fn metadata() -> Value {
    json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}})
}
impl Gateway {
    pub fn start(
        binary: &Path,
        plugin: &Path,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> Result<Self> {
        let mut command = Command::new(binary);
        command.env_remove("RUNE_KIT_HTTP_TOKEN");
        command
            .arg("serve")
            .arg(plugin)
            .args(["--headless", "--port", "0"])
            .args(args)
            .envs(env.iter().copied())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn()?;
        let mut stderr = BufReader::new(child.stderr.take().ok_or("Missing stderr")?);
        let mut endpoint = None;
        let mut startup = String::new();
        loop {
            let mut line = String::new();
            if stderr.read_line(&mut line)? == 0 {
                break;
            }
            startup.push_str(&line);
            if let Some(rest) = line.strip_prefix("Kit HTTP listening at ") {
                endpoint = Some(rest.split(';').next().unwrap().to_owned());
                break;
            }
        }
        let Some(endpoint) = endpoint else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("Server failed: {startup}").into());
        };
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut stderr, &mut std::io::sink());
        });
        Ok(Self {
            child,
            endpoint,
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(40))
                .build()?,
            pending: None,
            next: 0,
        })
    }
    pub fn send(&mut self, value: Value) -> Result<()> {
        let mut request = self
            .client
            .post(&self.endpoint)
            .header("Accept", "application/json, text/event-stream")
            .header(
                "MCP-Protocol-Version",
                value["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"]
                    .as_str()
                    .unwrap_or("2026-07-28"),
            )
            .header("Mcp-Method", value["method"].as_str().unwrap_or(""));
        if let Some(name) = value["params"]["name"]
            .as_str()
            .or_else(|| value["params"]["uri"].as_str())
        {
            request = request.header("Mcp-Name", name);
        }
        self.pending = Some(request.json(&value).send()?);
        Ok(())
    }
    pub fn cancel(&mut self) {
        drop(self.pending.take());
    }
    pub fn receive(&mut self) -> Result<Value> {
        let response = self.pending.take().ok_or("No pending response")?;
        let text = response.text()?;
        let json = text
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .unwrap_or(text.trim());
        Ok(serde_json::from_str(json)?)
    }
    pub fn request(&mut self, method: &str, mut params: Value) -> Result<Value> {
        self.next += 1;
        params["_meta"] = metadata();
        self.send(json!({"jsonrpc":"2.0","id":self.next,"method":method,"params":params}))?;
        let response = self.receive()?;
        if response["id"] != self.next {
            return Err(format!("Unexpected or late response: {response}").into());
        }
        Ok(response)
    }
    pub fn private_call(binary: &Path, method: &str) -> Result<Value> {
        let mut child = Command::new(binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut stdin = child.stdin.take().ok_or("Missing private stdin")?;
        writeln!(stdin, "{}", json!({"jsonrpc":"2.0","id":1,"method":method}))?;
        drop(stdin);
        let mut text = String::new();
        child
            .stdout
            .take()
            .ok_or("Missing private stdout")?
            .read_to_string(&mut text)?;
        child.wait()?;
        Ok(serde_json::from_str(text.trim())?)
    }
}
impl Drop for Gateway {
    fn drop(&mut self) {
        self.cancel();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn conformance(binary: &Path, git: &Path, time: &Path, filesystem: &Path) -> Result<Value> {
    let sandbox = tempfile::tempdir()?;
    let root = sandbox.path();
    std::fs::write(root.join("hello.txt"), "Hello from the allowed root")?;
    let data = root.join("tools");
    let data = data.to_str().ok_or("Non-UTF8 test path")?;
    let mut git_gateway = Gateway::start(
        binary,
        git,
        &["--read-only"],
        &[("RUNE_KIT_DATA_DIR", data)],
    )?;
    let listed = git_gateway.request("tools/list", json!({}))?;
    let tools = listed["result"]["tools"]
        .as_array()
        .ok_or_else(|| format!("Git discovery failed: {listed}"))?;
    let validator = tools
        .iter()
        .find(|t| {
            t["name"]
                .as_str()
                .is_some_and(|n| n.ends_with("__validate_commit_message"))
        })
        .ok_or("Missing Git validator")?;
    let call=git_gateway.request("tools/call",json!({"name":validator["name"],"arguments":{"message":"fix: verify gateway annotation compatibility"}}))?;
    let content: Value = serde_json::from_str(
        call["result"]["content"][0]["text"]
            .as_str()
            .ok_or("Missing Git content")?,
    )?;
    if content["valid"] != true {
        return Err(format!("Git call failed: {call}").into());
    }
    let direct = Gateway::private_call(git, "tools/list")?;
    let produced = direct["result"]
        .get("tools")
        .unwrap_or(&direct["result"])
        .as_array()
        .ok_or("Invalid private Git tools")?;
    let mut normal = Gateway::start(binary, git, &[], &[("RUNE_KIT_DATA_DIR", data)])?;
    let all = normal.request("tools/list", json!({}))?;
    for original in produced {
        let name = original["name"].as_str().ok_or("Tool name missing")?;
        let forwarded = all["result"]["tools"]
            .as_array()
            .ok_or("Missing tools")?
            .iter()
            .find(|t| {
                t["name"]
                    .as_str()
                    .is_some_and(|n| n.ends_with(&format!("__{name}")))
            })
            .ok_or("Missing forwarded tool")?;
        if forwarded["inputSchema"] != original["inputSchema"] {
            return Err(format!("Schema changed: {name}").into());
        }
    }
    let write = all["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["annotations"]["readOnlyHint"] != true)
        .ok_or("Missing Git write tool")?;
    let refused =
        git_gateway.request("tools/call", json!({"name":write["name"],"arguments":{}}))?;
    if refused["error"]["code"] != -32600 {
        return Err(format!("Git write not refused: {refused}").into());
    }
    let mut clock = Gateway::start(
        binary,
        time,
        &["--read-only"],
        &[("RUNE_KIT_DATA_DIR", data)],
    )?;
    let clock_tools = clock.request("tools/list", json!({}))?;
    let name = clock_tools["result"]["tools"]
        .as_array()
        .ok_or("Missing time tools")?
        .iter()
        .find(|t| {
            t["name"]
                .as_str()
                .is_some_and(|n| n.ends_with("__get_current_time"))
        })
        .ok_or("No time read tool")?["name"]
        .clone();
    let now = clock.request(
        "tools/call",
        json!({"name":name,"arguments":{"timezone":"UTC"}}),
    )?;
    successful(&now)?;
    let allowed = root.to_str().ok_or("Non-UTF8 root")?.replace('\\', "/");
    let parameter = format!("allowed_dir={allowed}");
    let mut files = Gateway::start(
        binary,
        filesystem,
        &["--read-only", "-p", &parameter],
        &[("RUNE_KIT_DATA_DIR", data)],
    )?;
    let discovery = files.request("server/discover", json!({}))?;
    if discovery["result"]["supportedVersions"][0] != "2026-07-28" {
        return Err("Wrong discovery protocol".into());
    }
    let listing = files.request("tools/list", json!({}))?;
    let mut reads = Vec::new();
    let mut read_name = Value::Null;
    for (suffix, path) in [("__read_text_file", "hello.txt"), ("__list_directory", ".")] {
        let name = listing["result"]["tools"]
            .as_array()
            .ok_or("Missing filesystem tools")?
            .iter()
            .find(|t| t["name"].as_str().is_some_and(|n| n.ends_with(suffix)))
            .ok_or("Required file tool unavailable")?["name"]
            .clone();
        let reply = files.request("tools/call", json!({"name":name,"arguments":{"path":path}}))?;
        successful(&reply)?;
        reads.push(reply);
        if suffix == "__read_text_file" {
            read_name = name;
        }
    }
    let outside = files.request(
        "tools/call",
        json!({"name":read_name,"arguments":{"path":"../outside.txt"},"allowed_dir":root.parent().unwrap().to_string_lossy()}),
    )?;
    if !outside.to_string().contains("Access denied") {
        return Err(format!("Outside-root access not denied: {outside}").into());
    }
    let invalid = files.request(
        "tools/call",
        json!({"name":read_name,"arguments":{"path":42}}),
    )?;
    if invalid["result"]["isError"] != true {
        return Err("Invalid arguments not rejected".into());
    }
    let namespace = read_name.as_str().unwrap().rsplit_once("__").unwrap().0;
    let denied=files.request("tools/call",json!({"name":format!("{namespace}__write_file"),"arguments":{"path":"blocked.txt","content":"bad"}}))?;
    if denied["error"]["code"] != -32600 {
        return Err("File write not refused".into());
    }
    Ok(
        json!({"discovery":discovery,"gitSchemasPreserved":produced.len(),"gitCall":call,"gitWriteRefusal":refused,"timeCall":now,"filesystemReads":reads,"outsideRoot":outside,"invalidArguments":invalid,"fileWriteRefusal":denied}),
    )
}
fn successful(reply: &Value) -> Result<()> {
    let content: Value = serde_json::from_str(
        reply["result"]["content"][0]["text"]
            .as_str()
            .ok_or_else(|| format!("No tool content: {reply}"))?,
    )?;
    if content["status"] != "success" || reply["result"]["isError"] == true {
        return Err(format!("Tool failed: {reply}").into());
    }
    Ok(())
}
