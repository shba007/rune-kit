use crate::{
    Commands,
    configuration::{Settings, validate},
};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use rune_kit_core::{
    PackageManager,
    registry::{ConfigurationField, VersionMetadata},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::{self, IsTerminal, Write},
    net::IpAddr,
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const REGISTRY: &str =
    "https://raw.githubusercontent.com/shba007/rune-tools/refs/heads/main/registry/index.json";
// Initial compatibility set, not a second dependency catalog. New releases must
// publish dependency metadata or be reviewed before the wizard enables them.
const SUPPORTED: [(&str, &str); 5] = [
    ("rune-time", "0.3.0"),
    ("rune-fetch", "0.4.0"),
    ("rune-filesystem", "0.4.1"),
    ("rune-memory", "0.3.0"),
    ("rune-sequential-thinking", "0.3.0"),
];

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    schema_version: u32,
    bind: IpAddr,
    port: u16,
    plugins: Vec<String>,
    workspace_only: bool,
}
impl Saved {
    fn command(self) -> Commands {
        Commands::Serve {
            plugin: None,
            all: false,
            bind: self.bind,
            port: self.port,
            headless: false,
            workspace_only: self.workspace_only,
            selected: self.plugins,
            param: vec![],
            read_only: false,
            allow_tool: vec![],
            timeout: 30,
        }
    }
}
fn root() -> io::Result<PathBuf> {
    rune_kit_core::paths::managed_root()
}
pub fn saved_command() -> Result<Option<Commands>> {
    let path = root()?.join("setup.json");
    match fs::read(path) {
        Ok(data) => {
            let saved: Saved = serde_json::from_slice(&data)?;
            if saved.schema_version != 1 || saved.plugins.is_empty() {
                return Err("Invalid setup state; rerun rune-kit setup".into());
            }
            Ok(Some(saved.command()))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn env_values(base: &Path) -> Result<BTreeMap<String, String>> {
    let data = match fs::read_to_string(base.join(".env")) {
        Ok(data) => data,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e.into()),
    };
    let mut values = BTreeMap::new();
    for line in data.lines() {
        if let Some((key, value)) = line.split_once('=')
            && matches!(key, "RUNE_KIT_HTTP_TOKEN" | "RUNE_KIT_PLUGIN_SECRETS")
        {
            let value = value.trim();
            let value = if value.starts_with('"') {
                serde_json::from_str(value)?
            } else {
                value.to_owned()
            };
            values.insert(key.to_owned(), value);
        }
    }
    Ok(values)
}
pub fn saved_token() -> Result<Option<String>> {
    Ok(env_values(&root()?)?
        .remove("RUNE_KIT_HTTP_TOKEN")
        .filter(|s| !s.is_empty()))
}
pub fn plugin_secrets(plugin: &str) -> Result<HashMap<String, String>> {
    let values = env_values(&root()?)?;
    let all: BTreeMap<String, HashMap<String, String>> = values
        .get("RUNE_KIT_PLUGIN_SECRETS")
        .map(|value| serde_json::from_str(value))
        .transpose()?
        .unwrap_or_default();
    Ok(all.get(plugin).cloned().unwrap_or_default())
}
fn atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let stage = path.with_extension(format!("setup-{}.tmp", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&stage)?;
    let result = (|| {
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&stage, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(stage);
    }
    result
}
fn credentials(base: &Path, values: BTreeMap<String, String>) -> Result<()> {
    let path = base.join(".env");
    let old = match fs::read_to_string(&path) {
        Ok(data) => data,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    let mut lines: Vec<_> = old
        .lines()
        .filter(|line| {
            !line.split_once('=').is_some_and(|(key, _)| {
                matches!(key, "RUNE_KIT_HTTP_TOKEN" | "RUNE_KIT_PLUGIN_SECRETS")
            })
        })
        .map(str::to_owned)
        .collect();
    for (key, value) in values {
        lines.push(format!("{key}={}", serde_json::to_string(&value)?));
    }
    atomic(&path, format!("{}\n", lines.join("\n")).as_bytes())?;
    Ok(())
}

#[derive(Clone)]
struct Candidate {
    name: String,
    version: String,
    metadata: Option<VersionMetadata>,
    reason: Option<String>,
    selected: bool,
    expected_sha: Option<String>,
}
impl Candidate {
    fn accepts(&self, installed: &rune_kit_core::InstalledPlugin) -> bool {
        if self
            .expected_sha
            .as_ref()
            .is_none_or(|sha| !installed.sha256.eq_ignore_ascii_case(sha))
        {
            return false;
        }
        match (
            self.metadata.as_ref(),
            installed
                .manifest
                .as_ref()
                .and_then(|m| m.configuration.as_ref()),
        ) {
            (Some(expected), Some(actual)) => match (
                serde_json::to_value(&expected.configuration),
                serde_json::to_value(actual),
            ) {
                (Ok(expected), Ok(actual)) => expected == actual,
                _ => false,
            },
            _ => false,
        }
    }
}
fn catalog(index: serde_json::Value) -> Result<Vec<Candidate>> {
    let map = index
        .as_object()
        .ok_or("Registry index must be an object")?;
    let mut candidates = Vec::new();
    for (name, pkg) in map {
        let Some(version) = pkg["latest"].as_str() else {
            continue;
        };
        let entry = &pkg["versions"][version];
        let metadata = VersionMetadata::from_value(entry).ok();
        let expected_sha = metadata
            .as_ref()
            .zip(entry["url"].as_str())
            .and_then(|(metadata, url)| metadata.checksum(url).ok())
            .map(|checksum| checksum.sha256.clone());
        let reason = if !SUPPORTED.contains(&(name.as_str(), version)) {
            Some(
                "Not in the initial supported set; complete dependency metadata is unavailable"
                    .into(),
            )
        } else if !metadata
            .as_ref()
            .zip(entry["url"].as_str())
            .is_some_and(|(metadata, url)| metadata.checksum(url).is_ok())
        {
            Some("Versioned configuration or verified WASM download metadata is missing".into())
        } else {
            None
        };
        candidates.push(Candidate {
            name: name.clone(),
            version: version.into(),
            metadata,
            reason,
            selected: false,
            expected_sha,
        });
    }
    candidates.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(candidates)
}
#[derive(Clone, Copy, PartialEq)]
enum Step {
    Network,
    Plugins,
    Dependencies,
    Downloading,
    Configure,
    Review,
}
struct Wizard {
    step: Step,
    bind: String,
    port: String,
    require_token: bool,
    workspace_only: bool,
    focus: usize,
    candidates: Vec<Candidate>,
    cursor: usize,
    fields: Vec<(String, String, ConfigurationField)>,
    field: usize,
    input: String,
    values: BTreeMap<String, BTreeMap<String, String>>,
    message: String,
    pending: Option<mpsc::Receiver<std::result::Result<String, String>>>,
    downloads_complete: bool,
}
impl Wizard {
    fn new(candidates: Vec<Candidate>) -> Self {
        Self {
            step: Step::Network,
            bind: "127.0.0.1".into(),
            port: "47601".into(),
            require_token: true,
            workspace_only: true,
            focus: 0,
            candidates,
            cursor: 0,
            fields: vec![],
            field: 0,
            input: String::new(),
            values: BTreeMap::new(),
            message: String::new(),
            pending: None,
            downloads_complete: false,
        }
    }
    fn selected(&self) -> Vec<(String, String)> {
        self.candidates
            .iter()
            .filter(|c| c.selected)
            .map(|c| (c.name.clone(), c.version.clone()))
            .collect()
    }
    fn network(&self) -> std::result::Result<(IpAddr, u16), String> {
        let bind = self
            .bind
            .parse::<IpAddr>()
            .map_err(|_| "Enter a valid IP address")?;
        let port = self
            .port
            .parse::<u16>()
            .map_err(|_| "Enter a port between 0 and 65535")?;
        if !bind.is_loopback() && !self.require_token {
            return Err("LAN access requires an API key".into());
        }
        Ok((bind, port))
    }
    fn prepare_fields(&mut self) {
        self.fields = self
            .candidates
            .iter()
            .filter(|c| c.selected)
            .flat_map(|c| {
                c.metadata
                    .as_ref()
                    .unwrap()
                    .configuration
                    .fields
                    .iter()
                    .filter(|(key, field)| {
                        key.as_str() != "allowed_dir"
                            && field.config_key.as_deref() != Some("allowed_dir")
                            && !field.env.iter().any(|key| key == "ALLOWED_DIR")
                    })
                    .map(|(key, field)| (c.name.clone(), key.clone(), field.clone()))
            })
            .collect();
        self.field = 0;
        self.next_input();
        self.step = if self.fields.is_empty() {
            Step::Review
        } else {
            Step::Configure
        };
    }
    fn next_input(&mut self) {
        self.input = self
            .fields
            .get(self.field)
            .filter(|(_, _, field)| !field.secret)
            .and_then(|(plugin, key, field)| {
                self.values
                    .get(plugin)
                    .and_then(|values| values.get(key))
                    .map(|value| serde_json::Value::String(value.clone()))
                    .or_else(|| field.default.clone())
            })
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string())
            })
            .unwrap_or_default();
    }
    fn download(&mut self, handle: &tokio::runtime::Handle) {
        let selected: Vec<_> = self
            .candidates
            .iter()
            .filter(|c| c.selected)
            .cloned()
            .collect();
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.step = Step::Downloading;
        handle.spawn(async move {
            let pm = PackageManager::new();
            for candidate in selected {
                let name = &candidate.name;
                let version = &candidate.version;
                let _ = tx.send(Ok(format!("Downloading and verifying {name} {version}")));
                let installed = match pm.install(name, Some(version.clone()), false).await {
                    Ok(installed) => installed,
                    Err(_) => {
                        let _ = tx.send(Err(format!(
                        "Installation failed for {name}; check the registry and network, then retry"
                    )));
                        return;
                    }
                };
                if !candidate.accepts(&installed) {
                    let _ = tx.send(Err(
                        "Registry metadata changed during setup; restart the wizard before saving"
                            .into(),
                    ));
                    return;
                }
            }
            let _ = tx.send(Ok("complete".into()));
        });
    }
    fn poll(&mut self) {
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(Ok(value)) if value == "complete" => {
                    self.pending = None;
                    self.downloads_complete = true;
                    self.prepare_fields();
                    self.message.clear();
                }
                Ok(Ok(value)) => self.message = value,
                Ok(Err(error)) => {
                    self.pending = None;
                    self.step = Step::Dependencies;
                    self.message = error;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.step = Step::Dependencies;
                    self.message = "Download worker stopped; retry installation".into();
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }
    // true completes setup. Cancellation is handled by the terminal owner.
    fn key(&mut self, key: KeyCode, handle: &tokio::runtime::Handle) -> bool {
        match self.step {
            Step::Network=>match key {
                KeyCode::Up=>self.focus=self.focus.saturating_sub(1),KeyCode::Down|KeyCode::Tab=>self.focus=(self.focus+1)%5,
                KeyCode::Char(' ')=>match self.focus {0=>{self.bind=if self.bind=="127.0.0.1"{"0.0.0.0"}else{"127.0.0.1"}.into();if self.bind!="127.0.0.1"{self.require_token=true;}},3=>self.require_token= !self.require_token,4=>self.workspace_only= !self.workspace_only,_=>{}},
                KeyCode::Backspace=>{match self.focus {1=>{self.bind.pop();},2=>{self.port.pop();},_=>{}}},
                KeyCode::Char(c) if !c.is_control()=>match self.focus {1=>self.bind.push(c),2 if c.is_ascii_digit()=>self.port.push(c),_=>{}},
                KeyCode::Enter=>match self.network(){Ok((bind,port))=>match std::net::TcpListener::bind((bind,port)){Ok(_)=>{self.step=Step::Plugins;self.message.clear();},Err(_)=>self.message="Cannot bind this address and port; choose a local interface or another port".into()},Err(error)=>self.message=error},_=>{}
            },
            Step::Plugins=>match key {
                KeyCode::Down=>self.cursor=(self.cursor+1).min(self.candidates.len().saturating_sub(1)),KeyCode::Up=>self.cursor=self.cursor.saturating_sub(1),
                KeyCode::Char(' ')=>if let Some(candidate)=self.candidates.get_mut(self.cursor){if candidate.reason.is_none(){candidate.selected= !candidate.selected;self.downloads_complete=false;}else{self.message=candidate.reason.clone().unwrap();}},
                KeyCode::Enter=>if self.selected().is_empty(){self.message="Select at least one supported MCP".into();}else{self.step=Step::Dependencies;self.message.clear();},
                KeyCode::Esc=>self.step=Step::Network,_=>{}
            },
            Step::Dependencies=>match key {KeyCode::Enter=>if self.downloads_complete {self.prepare_fields();}else{self.download(handle)},KeyCode::Esc=>self.step=Step::Plugins,_=>{}},
            Step::Downloading=>{},
            Step::Configure=>match key {
                KeyCode::Esc=>{if self.field>0{self.field-=1;self.next_input();}else{self.step=Step::Dependencies;}},
                KeyCode::Backspace=>{self.input.pop();},KeyCode::Char(c) if !c.is_control()=>self.input.push(c),
                KeyCode::Enter=>{
                    let (plugin,key,field)=&self.fields[self.field];
                    if self.input.is_empty() && !field.required {} else if let Err(error)=validate(field,&self.input){self.message=error;return false;}
                    else {self.values.entry(plugin.clone()).or_default().insert(key.clone(),self.input.clone());}
                    self.field+=1;self.message.clear();
                    if self.field==self.fields.len(){self.step=Step::Review;}else{self.next_input();}
                },_=>{}
            },
            Step::Review=>match key {KeyCode::Enter=>return true,KeyCode::Esc=>{if self.fields.is_empty(){self.step=Step::Dependencies;}else{self.field=self.fields.len()-1;self.next_input();self.step=Step::Configure;}},_=>{}}
        }
        false
    }
    fn save(&self, base: &Path) -> Result<Saved> {
        let (bind, port) = self.network().map_err(io::Error::other)?;
        let saved = Saved {
            schema_version: 1,
            bind,
            port,
            plugins: self.selected().into_iter().map(|(name, _)| name).collect(),
            workspace_only: self.workspace_only,
        };
        if saved.plugins.is_empty() || !self.downloads_complete {
            return Err("Setup is incomplete".into());
        }
        for (plugin, key, field) in &self.fields {
            if field.required
                && self
                    .values
                    .get(plugin)
                    .and_then(|values| values.get(key))
                    .is_none_or(|value| validate(field, value).is_err())
            {
                return Err(
                    format!("Required configuration is incomplete for {plugin} / {key}").into(),
                );
            }
        }
        let lock_path = base.join("dashboard.lock");
        let lock = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)?;
        struct Lock(PathBuf);
        impl Drop for Lock {
            fn drop(&mut self) {
                let _ = fs::remove_file(&self.0);
            }
        }
        let _guard = Lock(lock_path);
        drop(lock);
        let settings_path = base.join("dashboard.json");
        let mut settings: Settings = match fs::read(&settings_path) {
            Ok(data) => serde_json::from_slice(&data)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Settings::default(),
            Err(e) => return Err(e.into()),
        };
        let mut secrets = env_values(base)?;
        let mut plugin_secrets: BTreeMap<String, BTreeMap<String, String>> = secrets
            .get("RUNE_KIT_PLUGIN_SECRETS")
            .map(|value| serde_json::from_str(value))
            .transpose()?
            .unwrap_or_default();
        if self.require_token {
            if secrets
                .get("RUNE_KIT_HTTP_TOKEN")
                .is_none_or(|token| token.trim().is_empty())
            {
                let mut bytes = [0u8; 32];
                getrandom::fill(&mut bytes).map_err(io::Error::other)?;
                secrets.insert(
                    "RUNE_KIT_HTTP_TOKEN".into(),
                    bytes.iter().map(|b| format!("{b:02x}")).collect(),
                );
            }
        } else {
            secrets.remove("RUNE_KIT_HTTP_TOKEN");
        }
        for (plugin, key, field) in &self.fields {
            if let Some(value) = self.values.get(plugin).and_then(|values| values.get(key)) {
                if field.secret {
                    if let Some(values) = settings.plugins.get_mut(plugin) {
                        values.remove(key);
                    }
                    plugin_secrets
                        .entry(plugin.clone())
                        .or_default()
                        .insert(key.clone(), value.clone());
                } else {
                    settings
                        .plugins
                        .entry(plugin.clone())
                        .or_default()
                        .insert(key.clone(), value.clone());
                }
            }
        }
        secrets.insert(
            "RUNE_KIT_PLUGIN_SECRETS".into(),
            serde_json::to_string(&plugin_secrets)?,
        );
        credentials(base, secrets)?;
        atomic(&settings_path, &serde_json::to_vec_pretty(&settings)?)?;
        atomic(
            &base.join("setup.json"),
            &serde_json::to_vec_pretty(&saved)?,
        )?;
        Ok(saved)
    }
    fn draw(&self, frame: &mut Frame) {
        let area = frame.area();
        if area.width < 64 || area.height < 20 {
            frame.render_widget(
                Paragraph::new("Enlarge terminal to at least 64 x 20. Ctrl+C cancels setup."),
                area,
            );
            return;
        }
        let [brand, body, footer] = Layout::vertical([
            Constraint::Length(6),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .areas(area);
        frame.render_widget(
            Paragraph::new(vec![
                Line::from("rune-kit"),
                Line::from("  · • ·"),
                Line::from("· ● ● ● ·"),
                Line::from("  · • ·"),
                Line::from("Rune MCP Engine & Package Manager"),
            ]),
            brand,
        );
        let title = match self.step {
            Step::Network => "Network & security",
            Step::Plugins => "Select MCPs · choose at least one",
            Step::Dependencies => "Downloads & binary requirements",
            Step::Downloading => "Downloading",
            Step::Configure => "Plugin configuration",
            Step::Review => "Review · Enter saves and starts",
        };
        let block = Block::default().borders(Borders::ALL).title(title);
        match self.step {
            Step::Plugins => {
                let rows: Vec<_> = self
                    .candidates
                    .iter()
                    .map(|c| {
                        ListItem::new(crate::tui::safe(&format!(
                            "{} {} {}{}",
                            if c.selected { "●" } else { "○" },
                            c.name,
                            c.version,
                            if c.reason.is_some() {
                                " [unavailable]"
                            } else {
                                ""
                            }
                        )))
                    })
                    .collect();
                frame.render_stateful_widget(
                    List::new(rows)
                        .block(block)
                        .highlight_style(Style::new().fg(Color::Black).bg(Color::White)),
                    body,
                    &mut ListState::default().with_selected(Some(self.cursor)),
                );
            }
            _ => {
                let lines: Vec<Line> = match self.step {
                    Step::Network => {
                        let rows = [
                            format!(
                                "Access: {}",
                                if self.bind == "127.0.0.1" {
                                    "this computer"
                                } else {
                                    "network"
                                }
                            ),
                            format!("Bind address: {}", self.bind),
                            format!("Port: {}", self.port),
                            format!("Require API key: {}", self.require_token),
                            format!("Workspace-only WASM: {}", self.workspace_only),
                        ];
                        let mut lines: Vec<_> = rows
                            .into_iter()
                            .enumerate()
                            .map(|(i, row)| {
                                Line::from(format!(
                                    "{} {row}",
                                    if i == self.focus { ">" } else { " " }
                                ))
                            })
                            .collect();
                        lines.push(Line::from("LAN requires a generated API key. Workspace-only blocks native helpers."));
                        lines.push(Line::from("External reads/staging require broker-aware plugins; ordinary WASM sees /workspace."));
                        lines
                    }
                    Step::Dependencies => {
                        let mut lines: Vec<_> = self
                            .selected()
                            .iter()
                            .map(|(name, version)| {
                                Line::from(format!("{name} {version}: WASM plugin download"))
                            })
                            .collect();
                        lines.push(Line::from(
                            "No external CLI downloads are required by these reviewed releases.",
                        ));
                        lines.push(Line::from(
                            "Enter downloads and verifies the selected plugins.",
                        ));
                        lines
                    }
                    Step::Downloading => vec![Line::from(
                        "Please wait. Downloads are verified before activation.",
                    )],
                    Step::Configure => {
                        let (plugin, key, field) = &self.fields[self.field];
                        vec![
                            Line::from(format!("{plugin} / {key} ({})", field.kind)),
                            Line::from(crate::tui::safe(&field.description)),
                            Line::from(if field.secret {
                                "Secret input is masked and saved in Kit's .env"
                            } else {
                                "Enter a value; blank skips an optional field"
                            }),
                            Line::from(if field.secret {
                                "********".into()
                            } else {
                                crate::tui::safe(&self.input)
                            }),
                            Line::from(
                                "Use the sandbox dashboard to approve multiple external files/folders.",
                            ),
                        ]
                    }
                    Step::Review => vec![
                        Line::from(format!("Bind {}:{}", self.bind, self.port)),
                        Line::from(format!(
                            "Plugins: {}",
                            self.selected()
                                .iter()
                                .map(|(n, _)| n.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )),
                        Line::from(format!(
                            "API key: {} · Workspace-only: {}",
                            self.require_token, self.workspace_only
                        )),
                        Line::from(
                            "Enter saves setup and starts this Kit server. Secrets are never displayed.",
                        ),
                    ],
                    Step::Plugins => unreachable!(),
                };
                frame.render_widget(
                    Paragraph::new(lines)
                        .wrap(Wrap { trim: false })
                        .block(block),
                    body,
                );
            }
        }
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(
                    crate::tui::safe(&self.message),
                    Style::new().fg(Color::Yellow),
                ),
                Line::from("↑↓ move · Space select/toggle · Enter next · Esc back · Ctrl+C cancel"),
            ]),
            footer,
        );
        crate::tui::compatibility(frame);
    }
}

fn request_catalog(
    handle: &tokio::runtime::Handle,
) -> mpsc::Receiver<std::result::Result<Vec<Candidate>, String>> {
    let (tx, rx) = mpsc::channel();
    handle.spawn(async move {
        let result = async {
            let index = reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()?
                .get(REGISTRY)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            catalog(index)
        }
        .await
        .map_err(|_| {
            "Cannot load the Rune registry; check the network and press r to retry".to_owned()
        });
        let _ = tx.send(result);
    });
    rx
}

pub async fn run() -> Result<Commands> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("First-run setup requires an interactive terminal. Run rune-kit in a terminal, or use explicit commands such as serve --headless.".into());
    }
    let wizard = Wizard::new(vec![]);
    let handle = tokio::runtime::Handle::current();
    tokio::task::block_in_place(move || {
        let mut wizard = wizard;
        let mut catalog_request = Some(request_catalog(&handle));
        wizard.message = "Loading Rune registry in the background".into();
        let mut terminal = ratatui::try_init()?;
        struct Restore;
        impl Drop for Restore {
            fn drop(&mut self) {
                ratatui::restore();
            }
        }
        let _restore = Restore;
        loop {
            if let Some(rx) = &catalog_request {
                match rx.try_recv() {
                    Ok(Ok(candidates)) => {
                        wizard.candidates = candidates;
                        catalog_request = None;
                        wizard.message.clear();
                    }
                    Ok(Err(error)) => {
                        wizard.message = error;
                        catalog_request = None;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        wizard.message = "Registry worker stopped; press r to retry".into();
                        catalog_request = None;
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                }
            }
            wizard.poll();
            terminal.draw(|frame| wizard.draw(frame))?;
            if event::poll(Duration::from_millis(100))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    return Err("Setup cancelled; completed plugin downloads are retained".into());
                }
                if key.code == KeyCode::Char('r')
                    && wizard.candidates.is_empty()
                    && catalog_request.is_none()
                {
                    catalog_request = Some(request_catalog(&handle));
                    wizard.message = "Loading Rune registry in the background".into();
                    continue;
                }
                if wizard.key(key.code, &handle) {
                    match wizard.save(&root()?) {
                        Ok(saved) => return Ok(saved.command()),
                        Err(e) => wizard.message = format!("Cannot save setup: {e}"),
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn index() -> serde_json::Value {
        serde_json::json!({"rune-time":{"latest":"0.3.0","versions":{"0.3.0":{
            "url":"https://example.test/time.wasm","checksums":{"time.wasm":{"sha256":"00".repeat(32),"size":1}},
            "configuration":{"schema_version":1,"fields":{
                "timezone":{"type":"string","description":"Default timezone","required":false,"secret":false,"config_key":"default_timezone","default":"UTC"},
                "zz_secret":{"type":"string","description":"Secret fixture","required":true,"secret":true,"config_key":"api_key"}
            }}
        }}},"rune-browser":{"latest":"0.3.0","versions":{"0.3.0":{}}}})
    }
    #[test]
    fn catalog_requires_reviewed_version_complete_metadata_and_a_matching_checksum() {
        let entries = catalog(index()).unwrap();
        assert!(
            entries
                .iter()
                .find(|c| c.name == "rune-time")
                .unwrap()
                .reason
                .is_none()
        );
        assert!(
            entries
                .iter()
                .find(|c| c.name == "rune-browser")
                .unwrap()
                .reason
                .is_some()
        );
        let mut broken = index();
        broken["rune-time"]["versions"]["0.3.0"]["checksums"] = serde_json::json!({});
        assert!(catalog(broken).unwrap().iter().all(|c| c.reason.is_some()));
        let mut newer = index();
        newer["rune-time"]["latest"] = serde_json::json!("0.4.0");
        assert!(catalog(newer).unwrap().iter().all(|c| c.reason.is_some()));
    }

    #[test]
    fn downloaded_artifacts_must_match_the_configuration_and_hash_reviewed_in_the_wizard() {
        let candidate = catalog(index())
            .unwrap()
            .into_iter()
            .find(|c| c.name == "rune-time")
            .unwrap();
        let config =
            serde_json::to_value(&candidate.metadata.as_ref().unwrap().configuration).unwrap();
        let mut installed:rune_kit_core::InstalledPlugin=serde_json::from_value(serde_json::json!({"name":"rune-time","version":"0.3.0","binary_path":"fixture.wasm","sha256":"00".repeat(32),"source":"registry","default_params":{},"manifest":{"configuration":config}})).unwrap();
        assert!(candidate.accepts(&installed));
        installed.sha256 = "11".repeat(32);
        assert!(!candidate.accepts(&installed));
        installed.sha256 = "00".repeat(32);
        installed
            .manifest
            .as_mut()
            .unwrap()
            .configuration
            .as_mut()
            .unwrap()
            .fields
            .get_mut("zz_secret")
            .unwrap()
            .secret = false;
        assert!(!candidate.accepts(&installed));
    }
    #[test]
    fn wizard_requires_selection_and_lan_auth_and_preserves_fields_when_going_back() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut w = Wizard::new(catalog(index()).unwrap());
        w.bind = "0.0.0.0".into();
        w.require_token = false;
        assert!(w.network().is_err());
        w.require_token = true;
        w.port = "70000".into();
        assert!(w.network().is_err());
        w.port = "0".into();
        w.step = Step::Plugins;
        w.key(KeyCode::Enter, runtime.handle());
        assert!(w.step == Step::Plugins);
        w.cursor = w
            .candidates
            .iter()
            .position(|c| c.name == "rune-browser")
            .unwrap();
        w.key(KeyCode::Char(' '), runtime.handle());
        assert!(w.selected().is_empty());
        w.cursor = w
            .candidates
            .iter()
            .position(|c| c.name == "rune-time")
            .unwrap();
        w.key(KeyCode::Char(' '), runtime.handle());
        w.downloads_complete = true;
        w.key(KeyCode::Enter, runtime.handle());
        w.key(KeyCode::Enter, runtime.handle());
        assert!(w.step == Step::Configure);
        assert_eq!(w.input, "UTC");
        w.input = "Asia/Kolkata".into();
        w.key(KeyCode::Enter, runtime.handle());
        assert_eq!(w.field, 1);
        w.key(KeyCode::Enter, runtime.handle());
        assert_eq!(w.field, 1);
        w.input = "secret".into();
        w.key(KeyCode::Enter, runtime.handle());
        assert!(w.step == Step::Review);
        w.key(KeyCode::Esc, runtime.handle());
        assert!(w.step == Step::Configure);
        w.key(KeyCode::Esc, runtime.handle());
        assert_eq!(w.input, "Asia/Kolkata");
    }
    #[test]
    fn setup_save_preserves_unrelated_settings_and_keeps_secrets_out_of_dashboard_json() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join(".env"),
            "# retained\nKEEP=value\nRUNE_KIT_HTTP_TOKEN=\"\"\n",
        )
        .unwrap();
        fs::write(
            root.path().join("dashboard.json"),
            r#"{"shared":{},"plugins":{"other":{"value":"retained"}}}"#,
        )
        .unwrap();
        let mut w = Wizard::new(catalog(index()).unwrap());
        w.candidates
            .iter_mut()
            .find(|c| c.name == "rune-time")
            .unwrap()
            .selected = true;
        w.downloads_complete = true;
        w.prepare_fields();
        w.values.insert(
            "rune-time".into(),
            BTreeMap::from([
                ("timezone".into(), "UTC".into()),
                ("zz_secret".into(), "private-test-value".into()),
            ]),
        );
        w.save(root.path()).unwrap();
        let env = env_values(root.path()).unwrap();
        assert_eq!(env["RUNE_KIT_HTTP_TOKEN"].len(), 64);
        assert!(env["RUNE_KIT_PLUGIN_SECRETS"].contains("private-test-value"));
        assert!(
            fs::read_to_string(root.path().join(".env"))
                .unwrap()
                .contains("KEEP=value")
        );
        let dashboard = fs::read_to_string(root.path().join("dashboard.json")).unwrap();
        assert!(!dashboard.contains("private-test-value"));
        assert!(dashboard.contains("retained"));
        let saved: Saved =
            serde_json::from_slice(&fs::read(root.path().join("setup.json")).unwrap()).unwrap();
        assert_eq!(saved.plugins, vec!["rune-time"]);
        let token = env["RUNE_KIT_HTTP_TOKEN"].clone();
        w.save(root.path()).unwrap();
        assert_eq!(
            env_values(root.path()).unwrap()["RUNE_KIT_HTTP_TOKEN"],
            token
        );
    }
    #[test]
    fn secret_prompt_and_review_never_render_secret_contents() {
        let mut w = Wizard::new(catalog(index()).unwrap());
        w.candidates
            .iter_mut()
            .find(|c| c.name == "rune-time")
            .unwrap()
            .selected = true;
        w.prepare_fields();
        w.field = 1;
        w.input = "private-test-value".into();
        for size in [(80, 24), (64, 20), (30, 8)] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(size.0, size.1)).unwrap();
            terminal.draw(|frame| w.draw(frame)).unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect();
            assert!(!text.contains("private-test-value"));
            if size.0 >= 64 {
                assert!(text.contains("********"));
            }
        }
    }
}
