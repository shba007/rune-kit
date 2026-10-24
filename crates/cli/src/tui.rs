//! Independent inventory and bounded local discovery, never a gateway monitor.
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use rune_kit_core::{
    ExecutionKind, InstalledPlugin, Lockfile, NativeSidecar, PluginInstance, PluginManifest,
    ToolDefinition, WasmPluginInstance,
};
use serde::Serialize;
use std::{
    collections::HashMap,
    fs,
    io::{self, IsTerminal},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Clone, Serialize)]
pub struct PluginRow {
    pub name: String,
    pub version: String,
    pub execution: String,
    pub path: PathBuf,
    pub state: String,
    pub filesystem_root: Option<String>,
    pub network_hosts: Vec<String>,
    pub executable_capabilities: Vec<String>,
    pub tools: Vec<ToolDefinition>,
    pub hidden_tools: usize,
    pub diagnostics: Vec<String>,
    #[serde(skip)]
    entry: InstalledPlugin,
    #[serde(skip)]
    params: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Inventory {
    pub version: String,
    pub executable: PathBuf,
    pub data_root: PathBuf,
    pub read_only: bool,
    pub loading: bool,
    pub plugins: Vec<PluginRow>,
    pub error: Option<String>,
    pub observation: String,
}

impl Inventory {
    pub fn load(root: &Path, read_only: bool, overrides: &[(String, String)]) -> Self {
        let mut inventory = Self {
            version: env!("CARGO_PKG_VERSION").into(), executable: std::env::current_exe().unwrap_or_default(),
            data_root: root.to_owned(), read_only, loading: false, plugins: Vec::new(), error: None,
            observation: "Local inventory only. Studio gateways and actively running requests are not observed.".into(),
        };
        let lock: Lockfile = match fs::read(root.join("installed.json")) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(lock) => lock,
                Err(error) => {
                    inventory.error = Some(format!(
                        "Cannot parse installed.json: {error}. Restore valid metadata from your retained backup."
                    ));
                    return inventory;
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => Lockfile::default(),
            Err(error) => {
                inventory.error = Some(format!(
                    "Cannot read installed.json: {error}. Check storage permissions."
                ));
                return inventory;
            }
        };
        let env = rune_kit_core::resolve_host_env_params();
        let mut entries: Vec<_> = lock.plugins.into_iter().collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, entry) in entries {
            let path = root.join(&entry.binary_path);
            let mut params = entry.default_params.clone();
            params.extend(env.clone());
            params.extend(overrides.iter().cloned());
            let manifest = entry.manifest.clone().unwrap_or_default();
            let root_key = manifest
                .capabilities
                .filesystem
                .as_ref()
                .and_then(|f| f.root_param.as_deref())
                .unwrap_or("allowed_dir");
            let filesystem_root = params
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(root_key))
                .map(|(_, v)| v.clone());
            let mut diagnostics = Vec::new();
            if !path.is_file() {
                diagnostics.push(format!(
                    "Plugin executable missing: {}. Reinstall with rune-kit install {name}.",
                    path.display()
                ));
            }
            for dependency in manifest.dependencies.binaries.keys() {
                let directory = root.join("_bin").join(dependency);
                let found = fs::read_dir(directory).is_ok_and(|entries| {
                    entries.flatten().any(|entry| {
                        entry.path().join(".verified").is_file()
                            && (entry.path().join(dependency).is_file()
                                || entry.path().join(format!("{dependency}.exe")).is_file())
                    })
                });
                if !found {
                    diagnostics.push(format!("Managed dependency {dependency} is not verified locally. Reinstall {name} to provision it; optional dependencies may be unused."));
                }
            }
            inventory.plugins.push(PluginRow {
                name,
                version: entry.version.clone(),
                execution: match entry.execution_kind {
                    ExecutionKind::Native => "Native",
                    ExecutionKind::Wasm => "WASM",
                }
                .into(),
                path,
                state: "Installed; not yet discovered".into(),
                filesystem_root,
                network_hosts: manifest.capabilities.network_hosts,
                executable_capabilities: manifest.capabilities.exec.allowed_binaries,
                tools: Vec::new(),
                hidden_tools: 0,
                diagnostics,
                entry,
                params,
            });
        }
        inventory
    }
}

fn probe(mut row: PluginRow, read_only: bool, cancel: Arc<AtomicBool>) -> PluginRow {
    let result = (|| -> Result<Vec<ToolDefinition>> {
        if !row.path.is_file() {
            return Err("Executable unavailable; reinstall this plugin".into());
        }
        let manifest = if let Some(manifest) = row.entry.manifest.clone() {
            manifest
        } else {
            let path = row
                .path
                .parent()
                .unwrap_or(Path::new("."))
                .join("plugin.toml");
            if path.exists() {
                PluginManifest::from_file(path)?
            } else {
                PluginManifest::default()
            }
        };
        let mut instance = match row.entry.execution_kind {
            ExecutionKind::Native => PluginInstance::Native(Box::new(NativeSidecar::new(
                &row.name,
                &row.path,
                row.params.clone(),
                Some(manifest),
            )?)),
            ExecutionKind::Wasm => {
                PluginInstance::Wasm(Box::new(WasmPluginInstance::load_from_bytes(
                    &row.name,
                    fs::read(&row.path)?,
                    row.params.clone(),
                    manifest,
                )?))
            }
        };
        // Report the manifest actually used by this probe, including sidecar-adjacent manifests.
        row.network_hosts = instance.manifest().capabilities.network_hosts.clone();
        row.executable_capabilities = instance
            .manifest()
            .capabilities
            .exec
            .allowed_binaries
            .clone();
        let key = instance
            .manifest()
            .capabilities
            .filesystem
            .as_ref()
            .and_then(|f| f.root_param.as_deref())
            .unwrap_or("allowed_dir");
        row.filesystem_root = row
            .params
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.clone());
        let tools = instance.discover_tools(cancel, Duration::from_secs(5))?;
        Ok(tools) // instance is dropped before the completed probe is shown.
    })();
    match result {
        Ok(mut tools) => {
            let count = tools.len();
            if read_only {
                tools.retain(|tool| {
                    tool.annotations.as_ref().and_then(|a| a.read_only_hint) == Some(true)
                });
            }
            tools.sort_by(|a, b| a.name.cmp(&b.name));
            row.hidden_tools = count - tools.len();
            row.tools = tools;
            row.state = format!("Discovered {} tools; probe stopped", row.tools.len());
        }
        Err(error) => {
            row.state = "Discovery failed; probe stopped".into();
            row.diagnostics.push(format!("{error}. Check the plugin path, configured root and dependencies; then press r to retry."));
        }
    }
    row
}

struct Worker {
    cancel: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<()>,
}
impl Worker {
    fn stop(self) {
        self.cancel.store(true, Ordering::Relaxed);
        let _ = self.thread.join();
    }
}

fn start(inventory: &Inventory) -> (Worker, mpsc::Receiver<(usize, PluginRow)>) {
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let rows = inventory.plugins.clone();
    let read_only = inventory.read_only;
    let (tx, rx) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        for (i, row) in rows.into_iter().enumerate() {
            if flag.load(Ordering::Relaxed) {
                break;
            }
            if tx.send((i, probe(row, read_only, flag.clone()))).is_err() {
                break;
            }
        }
    });
    (Worker { cancel, thread }, rx)
}

#[derive(Default)]
struct View {
    selected: usize,
    scroll: u16,
    details_focus: bool,
}
impl View {
    fn key(&mut self, code: KeyCode, len: usize) {
        match code {
            KeyCode::Tab => self.details_focus = !self.details_focus,
            KeyCode::Down | KeyCode::Char('j') if !self.details_focus => {
                self.selected = (self.selected + 1).min(len.saturating_sub(1));
                self.scroll = 0;
            }
            KeyCode::Up | KeyCode::Char('k') if !self.details_focus => {
                self.selected = self.selected.saturating_sub(1);
                self.scroll = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => self.scroll = self.scroll.saturating_add(1),
            KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(8),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(8),
            KeyCode::Home => {
                self.selected = 0;
                self.scroll = 0;
            }
            _ => {}
        }
    }
}

fn safe(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).collect()
}
fn panel(title: &'static str, focus: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(title)
        .border_style(Style::new().fg(if focus { Color::Cyan } else { Color::DarkGray }))
}

fn details(inventory: &Inventory, selected: usize) -> Vec<Line<'static>> {
    if let Some(error) = &inventory.error {
        return vec![Line::from(safe(error))];
    }
    let Some(row) = inventory.plugins.get(selected) else {
        return vec![
            Line::from("No plugins installed."),
            Line::from("Install one with rune-kit install <plugin>, then press r."),
            Line::from("No discovery or tool execution has taken place."),
        ];
    };
    let mut lines = vec![
        Line::from(format!(
            "{}  {}  {}",
            safe(&row.name),
            safe(&row.version),
            row.execution
        )),
        Line::from(safe(&row.state)),
        Line::from(format!(
            "Executable: {}",
            safe(&row.path.display().to_string())
        )),
        Line::from(format!(
            "Read-only filter: {} (this dashboard only)",
            inventory.read_only
        )),
        Line::from(format!(
            "Configured filesystem root: {}",
            row.filesystem_root
                .as_ref()
                .map(|v| safe(v))
                .unwrap_or_else(|| "Not supplied; no default preopen".into())
        )),
        Line::from(format!(
            "Declared network hosts: {}",
            safe(&row.network_hosts.join(", "))
        )),
        Line::from(format!(
            "Declared executables: {}",
            safe(&row.executable_capabilities.join(", "))
        )),
        Line::from("No tools are called by this dashboard."),
        Line::from(""),
    ];
    for error in &row.diagnostics {
        lines.push(Line::styled(safe(error), Style::new().fg(Color::Red)));
    }
    if row.tools.is_empty() {
        lines.push(Line::from(if inventory.loading {
            "Discovery pending..."
        } else {
            "No tools available from this probe."
        }));
    }
    for tool in &row.tools {
        let read_only = tool.annotations.as_ref().and_then(|a| a.read_only_hint) == Some(true);
        lines.push(Line::styled(
            format!(
                "{} [{}]",
                safe(&tool.name),
                if read_only {
                    "read-only"
                } else {
                    "write-capable or undeclared"
                }
            ),
            Style::new().fg(Color::Cyan),
        ));
        lines.push(Line::from(safe(&tool.description)));
    }
    if row.hidden_tools > 0 {
        lines.push(Line::from(format!(
            "{} tools hidden by the read-only filter.",
            row.hidden_tools
        )));
    }
    lines
}

fn draw(frame: &mut Frame, inventory: &Inventory, view: &mut View) {
    let area = frame.area();
    if area.width < 42 || area.height < 18 {
        frame.render_widget(
            Paragraph::new("Rune Kit\nEnlarge terminal to at least 42 x 18.\nr refresh | q quit")
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(5),
        Constraint::Min(5),
        Constraint::Length(3),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    " RUNE KIT ",
                    Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(
                    "{}  {}",
                    inventory.version,
                    if inventory.loading {
                        "Discovering..."
                    } else {
                        "Inventory"
                    }
                )),
            ]),
            Line::from(format!(
                " Exe: {}",
                safe(&inventory.executable.display().to_string())
            )),
            Line::from(format!(
                " Data: {}",
                safe(&inventory.data_root.display().to_string())
            )),
        ])
        .block(panel("Local dashboard", false)),
        header,
    );
    let parts = if body.width >= 92 {
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).split(body)
    } else {
        Layout::vertical([Constraint::Length(6), Constraint::Min(3)]).split(body)
    };
    let rows: Vec<ListItem> = if inventory.plugins.is_empty() {
        vec![ListItem::new(if inventory.error.is_some() {
            "Inventory unavailable"
        } else {
            "No plugins installed"
        })]
    } else {
        inventory
            .plugins
            .iter()
            .map(|row| {
                ListItem::new(vec![
                    Line::from(format!(
                        "{}  {} ({})",
                        safe(&row.name),
                        safe(&row.version),
                        row.execution
                    )),
                    Line::from(safe(&row.state)),
                ])
            })
            .collect()
    };
    let mut state = ListState::default().with_selected(Some(view.selected));
    frame.render_stateful_widget(
        List::new(rows)
            .block(panel("Installed plugins", !view.details_focus))
            .highlight_style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            .highlight_symbol("> "),
        parts[0],
        &mut state,
    );
    frame.render_widget(
        Paragraph::new(details(inventory, view.selected))
            .block(panel("Discovery / diagnostics", view.details_focus))
            .wrap(Wrap { trim: false })
            .scroll((view.scroll, 0)),
        parts[1],
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(if area.width < 90 {
                "j/k select  Tab focus  r refresh  q quit"
            } else {
                "Up/Down or j/k select | Tab focus | PgUp/PgDn scroll | r refresh | q quit"
            }),
            Line::styled(
                if area.width < 90 {
                    "PgUp/PgDn scroll | Gateways: not observed"
                } else {
                    "Local probes only. Studio gateways / active requests: not observed."
                },
                Style::new().fg(Color::DarkGray),
            ),
        ]),
        footer,
    );
    if std::env::var_os("NO_COLOR").is_some() {
        for cell in &mut frame.buffer_mut().content {
            cell.set_fg(Color::Reset);
            cell.set_bg(Color::Reset);
        }
    }
}

struct Restore;
impl Drop for Restore {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

pub fn run(once: bool, json: bool, read_only: bool, params: Vec<(String, String)>) -> Result<()> {
    if !once && (!io::stdin().is_terminal() || !io::stdout().is_terminal()) {
        return Err("rune-kit tui needs an interactive terminal. Use rune-kit tui --once or --json for a plain inventory snapshot.".into());
    }
    let root = rune_kit_core::paths::managed_root()?;
    let mut inventory = Inventory::load(&root, read_only, &params);
    if once {
        let cancel = Arc::new(AtomicBool::new(false));
        inventory.plugins = inventory
            .plugins
            .into_iter()
            .map(|row| probe(row, read_only, cancel.clone()))
            .collect();
        if json {
            println!("{}", serde_json::to_string_pretty(&inventory)?);
        } else {
            println!(
                "RUNE KIT {}\nExecutable: {}\nData: {}\n{}",
                inventory.version,
                safe(&inventory.executable.display().to_string()),
                safe(&root.display().to_string()),
                inventory.observation
            );
            if let Some(error) = &inventory.error {
                println!("{}", safe(error));
            }
            if inventory.plugins.is_empty() && inventory.error.is_none() {
                println!("No plugins installed. Run rune-kit install <plugin>.");
            }
            for i in 0..inventory.plugins.len() {
                for line in details(&inventory, i) {
                    println!("{line}");
                }
            }
        }
        return Ok(());
    }
    let (initial, mut rx) = start(&inventory);
    let mut worker = Some(initial);
    inventory.loading = !inventory.plugins.is_empty();
    let result = (|| -> Result<()> {
        let _restore = Restore;
        let mut terminal: DefaultTerminal = ratatui::try_init()?;
        let mut view = View::default();
        loop {
            while let Ok((i, row)) = rx.try_recv() {
                inventory.plugins[i] = row;
            }
            if worker.as_ref().is_some_and(|w| w.thread.is_finished()) {
                inventory.loading = false;
            }
            terminal.draw(|frame| draw(frame, &inventory, &mut view))?;
            if event::poll(Duration::from_millis(100))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                    || key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    break;
                }
                if key.code == KeyCode::Char('r') && !inventory.loading {
                    if let Some(old) = worker.take() {
                        old.stop();
                    }
                    inventory = Inventory::load(&root, read_only, &params);
                    let (next, next_rx) = start(&inventory);
                    worker = Some(next);
                    rx = next_rx;
                    inventory.loading = !inventory.plugins.is_empty();
                    view.selected = view.selected.min(inventory.plugins.len().saturating_sub(1));
                    view.scroll = 0;
                } else {
                    view.key(key.code, inventory.plugins.len());
                }
            }
        }
        Ok(())
    })();
    // Terminal cleanup happens before waiting for bounded discovery cancellation.
    if let Some(worker) = worker {
        worker.stop();
    }
    result
}

/// Attached to this server's observed state; never probes separate plugin processes.
pub fn server_dashboard(state: Arc<std::sync::Mutex<crate::server::Snapshot>>) -> Result<()> {
    let mut terminal = ratatui::try_init()?;
    let _restore = Restore;
    let mut selected = 0usize;
    loop {
        let snapshot = state.lock().unwrap().clone();
        if snapshot.status != "listening" {
            break;
        }
        terminal.draw(|frame| draw_server(frame, &snapshot, selected))?;
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                KeyCode::Down | KeyCode::Char('j') => {
                    selected = selected
                        .saturating_add(1)
                        .min(snapshot.recent.len().saturating_sub(1))
                }
                KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
                KeyCode::Char('r') => { /* next frame reads fresh server state */ }
                _ => {}
            }
        }
    }
    Ok(())
}

fn draw_server(frame: &mut Frame, snapshot: &crate::server::Snapshot, selected: usize) {
    let areas = Layout::vertical([
        Constraint::Length(5),
        Constraint::Percentage(35),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(frame.area());
    let header = format!(
        "Kit {} · {} · {}\nUptime {}s · read-only {} · timeout {}s · {} active\nAllowlist: {} · roots: {}",
        env!("CARGO_PKG_VERSION"),
        safe(&snapshot.endpoint),
        snapshot.status,
        snapshot.started.elapsed().as_secs(),
        snapshot.read_only,
        snapshot.timeout,
        snapshot.active.len(),
        if snapshot.allowed_tools.is_empty() {
            "all server-permitted tools".into()
        } else {
            snapshot.allowed_tools.join(", ")
        },
        if snapshot.roots.is_empty() {
            "no filesystem root configured".into()
        } else {
            snapshot.roots.join(", ")
        }
    );
    frame.render_widget(
        Paragraph::new(
            header
                .lines()
                .map(|line| Line::from(safe(line)))
                .collect::<Vec<_>>(),
        )
        .block(panel(" SERVER ", true))
        .wrap(Wrap { trim: false }),
        areas[0],
    );
    let mut lines: Vec<Line> = snapshot
        .plugins
        .iter()
        .map(|p| Line::from(safe(p)))
        .collect();
    if lines.is_empty() {
        lines.push(Line::from(
            "No plugins installed. Install a plugin and restart the server.",
        ));
    }
    lines.extend(snapshot.active.iter().map(|call| {
        Line::from(format!(
            "#{} {} {} — {}",
            call.sequence,
            safe(&call.method),
            safe(&call.name),
            call.status
        ))
    }));
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(" PLUGINS / ACTIVE CALLS ", false))
            .wrap(Wrap { trim: false }),
        areas[1],
    );
    let rows: Vec<ListItem> = snapshot
        .recent
        .iter()
        .map(|call| {
            ListItem::new(format!(
                "#{} {} {} · {} · {}ms",
                call.sequence,
                safe(&call.method),
                safe(&call.name),
                call.status,
                call.millis
            ))
        })
        .collect();
    let rows = if rows.is_empty() {
        vec![ListItem::new(
            "No completed calls yet. Waiting for an HTTP client.",
        )]
    } else {
        rows
    };
    frame.render_stateful_widget(
        List::new(rows)
            .block(panel(" RECENT CALLS (64 max; no arguments) ", true))
            .highlight_style(Style::default().fg(Color::Red)),
        areas[2],
        &mut ListState::default().with_selected(Some(selected)),
    );
    frame.render_widget(
        Paragraph::new("↑/↓ select · r refresh · q quit server · Ctrl+C stop"),
        areas[3],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    fn render(inventory: &Inventory, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| draw(frame, inventory, &mut View::default()))
            .unwrap();
        let mut text = String::new();
        for y in 0..height {
            for x in 0..width {
                text.push_str(terminal.backend().buffer()[(x, y)].symbol());
            }
            text.push('\n');
        }
        text
    }
    #[test]
    fn empty_corrupt_inventory_and_small_resizing_are_clear() {
        let root = std::env::temp_dir().join(format!("kit-tui-empty-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let inventory = Inventory::load(&root, true, &[]);
        let screen = render(&inventory, 110, 26);
        assert!(screen.contains("No plugins installed"));
        assert!(screen.contains("r refresh"));
        assert!(screen.contains("not observed"));
        assert!(render(&inventory, 68, 24).contains("q quit"));
        let preview = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("target/tui-preview.txt");
        fs::write(preview, &screen).unwrap();
        assert!(render(&inventory, 30, 8).contains("Enlarge terminal"));
        fs::write(root.join("installed.json"), "invalid").unwrap();
        assert!(
            Inventory::load(&root, false, &[])
                .error
                .unwrap()
                .contains("Cannot parse")
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn navigation_stays_within_inventory_and_scrolls_details() {
        let mut view = View::default();
        view.key(KeyCode::Down, 0);
        assert_eq!(view.selected, 0);
        view.key(KeyCode::Down, 2);
        assert_eq!(view.selected, 1);
        view.key(KeyCode::Down, 2);
        assert_eq!(view.selected, 1);
        view.key(KeyCode::Tab, 2);
        view.key(KeyCode::Down, 2);
        assert_eq!(view.scroll, 1);
        view.key(KeyCode::PageUp, 2);
        assert_eq!(view.scroll, 0);
    }
}
