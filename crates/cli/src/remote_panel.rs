use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Block, Borders, Clear, Paragraph},
};
use rune_kit_core::{
    McpRouter, PluginInstance,
    remote::{RemoteConfig, RemoteHttp},
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

pub struct Panel {
    root: PathBuf,
    original: Option<Vec<u8>>,
    servers: BTreeMap<String, RemoteConfig>,
    selected: usize,
    draft: Option<[String; 4]>,
    editing: bool,
    field: usize,
    remove: bool,
    message: String,
    writable: bool,
    visible: bool,
}

impl Panel {
    pub fn open(root: PathBuf) -> Self {
        let loaded = crate::remotes::bytes(&root).and_then(|bytes| {
            crate::remotes::read(&root.join("remotes.json"))
                .map(|servers| (bytes, servers))
                .map_err(|e| e.to_string())
        });
        let writable = loaded.is_ok();
        let (original, servers, message) = match loaded {
            Ok((bytes, servers)) => (bytes, servers, String::new()),
            Err(e) => (None, BTreeMap::new(), e),
        };
        Self {
            root,
            original,
            servers,
            selected: 0,
            draft: None,
            editing: false,
            field: 0,
            remove: false,
            message,
            writable,
            visible: false,
        }
    }
    fn name(&self) -> Option<String> {
        self.servers.keys().nth(self.selected).cloned()
    }
    fn apply(
        &mut self,
        router: &Arc<Mutex<McpRouter>>,
        state: &Arc<Mutex<crate::server::Snapshot>>,
        remove: bool,
    ) -> Result<(), String> {
        let mut router = router
            .try_lock()
            .map_err(|_| "A request is active; retry when it finishes")?;
        let workspace_only = state.lock().unwrap().workspace_only;
        let mut next = self.servers.clone();
        let (name, instance) = if remove {
            let name = self.name().ok_or("Select a remote first")?;
            next.remove(&name);
            (name, None)
        } else {
            let fields = self.draft.as_ref().ok_or("No remote changes to save")?;
            let name = fields[0].clone();
            if (!self.editing && next.contains_key(&name))
                || (router.contains_namespace(&name) && !router.is_remote(&name))
            {
                return Err("Namespace already exists".into());
            }
            if workspace_only {
                return Err("This server uses workspace-only mode. External MCP filesystems cannot be enforced; restart without --workspace-only to enable remotes".into());
            }
            let config = RemoteConfig {
                endpoint: fields[1].clone(),
                protocol: fields[2].clone(),
                token_env: if fields[3].is_empty() {
                    None
                } else {
                    Some(fields[3].clone())
                },
            };
            config.validate().map_err(|e| e.to_string())?;
            let token = config
                .token_env
                .as_ref()
                .map(|key| {
                    std::env::var(key)
                        .map_err(|_| "Configured token environment variable is missing")
                })
                .transpose()?;
            let instance =
                RemoteHttp::new(name.clone(), config.clone(), token).map_err(|e| e.to_string())?;
            next.insert(name.clone(), config);
            (name, Some(PluginInstance::Remote(Box::new(instance))))
        };
        let persisted = crate::remotes::save_dashboard(&self.root, &self.original, next.clone())?;
        self.original = Some(persisted);
        self.servers = next;
        router.remove_remote(&name);
        if let Some(instance) = instance {
            router.register(name.clone(), instance);
        }
        let mut snapshot = state.lock().unwrap();
        snapshot
            .plugins
            .retain(|line| !line.starts_with(&format!("{name}: external HTTP")));
        if !remove {
            snapshot.plugins.push(format!(
                "{name}: external HTTP server loaded; discovery pending; filesystem not managed"
            ));
        }
        snapshot.tools.clear();
        snapshot.discovery_note =
            "Remote configuration changed; press d to rediscover tools".into();
        self.selected = self.selected.min(self.servers.len().saturating_sub(1));
        Ok(())
    }
    pub fn key(
        &mut self,
        key: KeyCode,
        router: &Arc<Mutex<McpRouter>>,
        state: &Arc<Mutex<crate::server::Snapshot>>,
    ) -> bool {
        if self.remove {
            if !self.visible {
                self.message = "Enlarge terminal before confirming removal".into();
                return false;
            }
            if key == KeyCode::Char('y') {
                self.message = self
                    .apply(router, state, true)
                    .err()
                    .unwrap_or_else(|| "Removed from this server and saved configuration".into());
            }
            self.remove = false;
            return false;
        }
        if let Some(draft) = &mut self.draft {
            match key {
                KeyCode::Esc => self.draft = None,
                KeyCode::Tab | KeyCode::Down => {
                    self.field = (self.field + 1) % 4;
                    if self.editing && self.field == 0 {
                        self.field = 1;
                    }
                }
                KeyCode::BackTab | KeyCode::Up => {
                    self.field = (self.field + 3) % 4;
                    if self.editing && self.field == 0 {
                        self.field = 3;
                    }
                }
                KeyCode::Backspace => {
                    draft[self.field].pop();
                }
                KeyCode::Char(c) if !c.is_control() => draft[self.field].push(c),
                KeyCode::Enter if !self.visible => {
                    self.message = "Enlarge terminal before saving".into()
                }
                KeyCode::Enter => match self.apply(router, state, false) {
                    Ok(()) => {
                        self.draft = None;
                        self.message =
                            "Saved and applied immediately; press d to discover tools".into();
                    }
                    Err(e) => self.message = e,
                },
                _ => {}
            }
            return false;
        }
        match key {
            KeyCode::Esc | KeyCode::Char('q') => return true,
            KeyCode::Down => {
                self.selected = self
                    .selected
                    .saturating_add(1)
                    .min(self.servers.len().saturating_sub(1))
            }
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Char('a') if self.writable => {
                self.draft = Some([
                    String::new(),
                    String::new(),
                    "2026-07-28".into(),
                    String::new(),
                ]);
                self.field = 0;
                self.editing = false;
                self.message.clear();
            }
            KeyCode::Char('e') if self.writable => {
                if let Some(name) = self.name() {
                    let config = &self.servers[&name];
                    self.draft = Some([
                        name,
                        config.endpoint.clone(),
                        config.protocol.clone(),
                        config.token_env.clone().unwrap_or_default(),
                    ]);
                    self.field = 1;
                    self.editing = true;
                    self.message.clear();
                }
            }
            KeyCode::Delete if self.writable && self.name().is_some() => self.remove = true,
            _ => {}
        }
        false
    }
    pub fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        frame.render_widget(Clear, area);
        self.visible = area.width >= 76 && area.height >= 20;
        if !self.visible {
            frame.render_widget(
                Paragraph::new("Enlarge terminal to at least 76 × 20 · Esc back"),
                area,
            );
            return;
        }
        let mut lines = vec![
            "Third-party MCP servers · Streamable HTTP".into(),
            "External filesystems are not managed by Kit. Changes apply between requests.".into(),
            String::new(),
        ];
        let count =
            usize::from(
                area.height
                    .saturating_sub(if self.draft.is_some() { 14 } else { 9 }),
            );
        let start = self.selected.saturating_sub(count.saturating_sub(1));
        for (i, (name, config)) in self.servers.iter().enumerate().skip(start).take(count) {
            lines.push(format!(
                "{} {name} · {} · {}",
                if i == self.selected { ">" } else { " " },
                config.redacted_endpoint(),
                config.protocol
            ));
        }
        if self.servers.is_empty() {
            lines.push("No remote servers configured".into());
        }
        if let Some(draft) = &self.draft {
            lines.push(String::new());
            for (i, label) in [
                "Namespace",
                "Endpoint",
                "Protocol version",
                "Token environment variable (optional)",
            ]
            .iter()
            .enumerate()
            {
                let value = if i == 1 {
                    RemoteConfig {
                        endpoint: draft[1].clone(),
                        protocol: draft[2].clone(),
                        token_env: None,
                    }
                    .redacted_endpoint()
                } else {
                    draft[i].clone()
                };
                lines.push(format!(
                    "{} {label}: {value}",
                    if i == self.field { ">" } else { " " }
                ));
            }
            lines.push("Tab/↑↓ field · type/backspace edit · Enter save · Esc cancel".into());
            lines.push(
                "Endpoint query values are hidden. Enter a token variable name, never its value."
                    .into(),
            );
        } else {
            lines.push("a add · e edit · Delete remove · Esc back".into());
        }
        if self.remove {
            lines.push(format!("Remove: {}", self.name().unwrap_or_default()));
            lines.push(
                "Remove from this server and saved configuration? y confirm · Esc cancel".into(),
            );
        }
        lines.push(self.message.clone());
        let text = crate::tui::safe_multiline(&lines.join("\n"));
        frame.render_widget(
            Paragraph::new(text).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Remote servers"),
            ),
            Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: area.height,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state(root: &std::path::Path) -> Arc<Mutex<crate::server::Snapshot>> {
        Arc::new(Mutex::new(crate::server::Snapshot {
            bind_address: "127.0.0.1:47601".into(),
            workspace_only: false,
            endpoint: "http://127.0.0.1:47601/mcp".into(),
            status: "listening".into(),
            started: std::time::Instant::now(),
            plugins: vec![],
            read_only: false,
            allowed_tools: vec![],
            roots: vec![],
            timeout: 30,
            executable: "rune-kit".into(),
            data_root: root.into(),
            active: vec![],
            recent: Default::default(),
            tools: vec!["old__tool".into()],
            discovery_note: String::new(),
        }))
    }
    fn draw(panel: &mut Panel, width: u16, height: u16) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| panel.draw(frame)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }
    #[test]
    fn add_edit_remove_apply_immediately_and_require_visible_confirmation() {
        let root = tempfile::tempdir().unwrap();
        let router = Arc::new(Mutex::new(McpRouter::new()));
        let state = state(root.path());
        let mut panel = Panel::open(root.path().into());
        panel.key(KeyCode::Char('a'), &router, &state);
        panel.draft = Some([
            "external".into(),
            "http://127.0.0.1:1/mcp?key=private-secret".into(),
            "2026-07-28".into(),
            String::new(),
        ]);
        let screen = draw(&mut panel, 110, 28);
        assert!(!screen.contains("private-secret"));
        panel.key(KeyCode::Enter, &router, &state);
        assert!(router.lock().unwrap().is_remote("external"));
        assert!(state.lock().unwrap().tools.is_empty());
        panel.key(KeyCode::Char('e'), &router, &state);
        panel.field = 2;
        panel.draft.as_mut().unwrap()[2] = "2025-11-25".into();
        panel.key(KeyCode::Enter, &router, &state);
        assert_eq!(
            crate::remotes::read(&root.path().join("remotes.json")).unwrap()["external"].protocol,
            "2025-11-25"
        );
        panel.key(KeyCode::Delete, &router, &state);
        draw(&mut panel, 40, 10);
        panel.key(KeyCode::Char('y'), &router, &state);
        assert!(router.lock().unwrap().is_remote("external"));
        assert!(draw(&mut panel, 110, 28).contains("Remove: external"));
        panel.key(KeyCode::Char('y'), &router, &state);
        assert!(!router.lock().unwrap().contains_namespace("external"));
        assert!(
            crate::remotes::read(&root.path().join("remotes.json"))
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn active_requests_stale_files_and_workspace_mode_preserve_the_previous_configuration() {
        let root = tempfile::tempdir().unwrap();
        let router = Arc::new(Mutex::new(McpRouter::new()));
        let state = state(root.path());
        let mut panel = Panel::open(root.path().into());
        panel.key(KeyCode::Char('a'), &router, &state);
        panel.draft = Some([
            "external".into(),
            "http://127.0.0.1:1/mcp".into(),
            "2026-07-28".into(),
            String::new(),
        ]);
        draw(&mut panel, 110, 28);
        let guard = router.lock().unwrap();
        panel.key(KeyCode::Enter, &router, &state);
        assert!(panel.message.contains("active"));
        drop(guard);
        state.lock().unwrap().workspace_only = true;
        panel.key(KeyCode::Enter, &router, &state);
        assert!(panel.message.contains("workspace-only"));
        state.lock().unwrap().workspace_only = false;
        std::fs::write(root.path().join("remotes.json"), b"external-edit").unwrap();
        panel.key(KeyCode::Enter, &router, &state);
        assert!(panel.message.contains("outside"));
        assert!(!router.lock().unwrap().is_remote("external"));
        assert_eq!(
            std::fs::read(root.path().join("remotes.json")).unwrap(),
            b"external-edit"
        );
    }
}
