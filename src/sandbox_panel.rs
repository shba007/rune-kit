use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};
use rune_kit_core::sandbox::{PathKind, Sandbox};
use std::path::PathBuf;

pub struct Panel {
    workspace_only: bool,
    store: Result<Sandbox, String>,
    plugins: Vec<String>,
    plugin: usize,
    selected: usize,
    input: Option<String>,
    revoke: bool,
    message: String,
    workspace: String,
}

impl Panel {
    pub fn open(plugins: Vec<String>, workspace_only: bool) -> Self {
        let mut panel = Self::open_at(plugins, rune_kit_core::paths::sandbox_root());
        panel.workspace_only = workspace_only;
        panel
    }

    fn open_at(plugins: Vec<String>, root: std::io::Result<PathBuf>) -> Self {
        let store = root
            .and_then(Sandbox::open)
            .and_then(|store| {
                for plugin in &plugins {
                    store.workspace(plugin)?;
                }
                Ok(store)
            })
            .map_err(|e| e.to_string());
        let mut panel = Self {
            workspace_only: false,
            store,
            plugins,
            plugin: 0,
            selected: 0,
            input: None,
            revoke: false,
            message: String::new(),
            workspace: String::new(),
        };
        panel.update_workspace();
        panel
    }

    fn update_workspace(&mut self) {
        self.workspace = match (self.store.as_ref(), self.plugins.get(self.plugin)) {
            (Ok(store), Some(plugin)) => store
                .workspace(plugin)
                .map(|path| path.display().to_string())
                .unwrap_or_else(|e| e.to_string()),
            _ => String::new(),
        };
    }

    pub fn key(&mut self, key: KeyCode) -> bool {
        if let Some(input) = &mut self.input {
            match key {
                KeyCode::Esc => self.input = None,
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) if !c.is_control() => input.push(c),
                KeyCode::Enter => {
                    let path = PathBuf::from(input.as_str());
                    let result = match (&mut self.store, self.plugins.get(self.plugin)) {
                        (Ok(store), Some(plugin)) => {
                            store.approve(plugin, &path).map_err(|e| e.to_string())
                        }
                        _ => Err("Select an available plugin first".into()),
                    };
                    match result {
                        Ok(()) => {
                            self.input = None;
                            self.message =
                                "Approval recorded only; runtime permissions are unchanged.".into();
                        }
                        Err(error) => self.message = error,
                    }
                }
                _ => {}
            }
            return false;
        }
        if self.revoke {
            if key == KeyCode::Char('y')
                && let (Ok(store), Some(plugin)) = (&mut self.store, self.plugins.get(self.plugin))
            {
                if let Some(approval) = store.approvals(plugin).get(self.selected).cloned() {
                    self.message = match store.revoke(plugin, &approval.path) {
                        Ok(()) => "Approval removed. Workspace and original data retained.".into(),
                        Err(e) => e.to_string(),
                    };
                }
                self.selected = self
                    .selected
                    .min(store.approvals(plugin).len().saturating_sub(1));
            }
            self.revoke = false;
            return false;
        }
        match key {
            KeyCode::Esc | KeyCode::Char('q') => return true,
            KeyCode::Tab if !self.plugins.is_empty() => {
                self.plugin = (self.plugin + 1) % self.plugins.len();
                self.selected = 0;
                self.message.clear();
                self.update_workspace();
            }
            KeyCode::Char('a') if self.store.is_ok() && !self.plugins.is_empty() => {
                self.input = Some(String::new());
                self.message.clear();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let (Ok(store), Some(plugin)) = (&self.store, self.plugins.get(self.plugin)) {
                    self.selected =
                        (self.selected + 1).min(store.approvals(plugin).len().saturating_sub(1));
                }
            }
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Delete
                if self
                    .store
                    .as_ref()
                    .ok()
                    .zip(self.plugins.get(self.plugin))
                    .is_some_and(|(s, p)| !s.approvals(p).is_empty()) =>
            {
                self.revoke = true
            }
            _ => {}
        }
        false
    }

    pub fn draw(&self, frame: &mut Frame) {
        let full = frame.area();
        if full.width < 48 || full.height < 16 {
            return;
        }
        let header = crate::tui::header_height(full.height);
        let area = Rect::new(full.x, full.y + header, full.width, full.height - header);
        frame.render_widget(Clear, area);
        let [notice, list, footer] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .areas(area);
        let plugin = self
            .plugins
            .get(self.plugin)
            .map(String::as_str)
            .unwrap_or("No loaded plugins");
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(
                    if self.workspace_only {
                        "Workspace-only isolation ACTIVE. Copy-back requires approval."
                    } else {
                        "Write isolation NOT ACTIVE. Copy-back requires approval."
                    },
                    Style::new().fg(Color::Yellow),
                ),
                Line::from(if self.workspace_only {
                    "Broker-aware plugins can read approved originals and stage file edits."
                } else {
                    "Approvals record intent only; no files are copied or permissions changed."
                }),
                Line::from(crate::tui::safe(&format!(
                    "Plugin: {plugin} · Workspace: {}",
                    self.workspace
                ))),
            ]),
            notice,
        );
        let rows: Vec<_> = match &self.store {
            Err(error) => vec![ListItem::new(crate::tui::safe(error))],
            Ok(store) => {
                let approvals = store.approvals(plugin);
                if approvals.is_empty() {
                    vec![ListItem::new(
                        "No external paths approved · a adds a file or folder",
                    )]
                } else {
                    approvals
                        .iter()
                        .map(|a| {
                            ListItem::new(crate::tui::safe(&format!(
                                "{}  {}",
                                match a.kind {
                                    PathKind::File => "File",
                                    PathKind::Directory => "Folder",
                                },
                                a.path.display()
                            )))
                        })
                        .collect()
                }
            }
        };
        frame.render_stateful_widget(
            List::new(rows)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Sandbox · recorded approvals "),
                )
                .highlight_style(Style::new().fg(Color::Black).bg(Color::White)),
            list,
            &mut ListState::default().with_selected(Some(self.selected)),
        );
        let prompt = if let Some(input) = &self.input {
            format!("Approve absolute path: {}", crate::tui::safe(input))
        } else if self.revoke {
            "Remove selected approval? y confirms; any other key cancels".into()
        } else {
            crate::tui::safe(&self.message)
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(prompt),
                Line::from(if self.input.is_some() {
                    "Enter records approval · Esc cancels"
                } else {
                    "Tab plugin · ↑↓ path · a approve · Delete revoke · Esc back"
                }),
            ]),
            footer,
        );
        crate::tui::compatibility(frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    fn approve(panel: &mut Panel, path: &Path) {
        panel.key(KeyCode::Char('a'));
        for c in path.to_str().unwrap().chars() {
            panel.key(KeyCode::Char(c));
        }
        panel.key(KeyCode::Enter);
        assert!(panel.input.is_none(), "{}", panel.message);
    }

    #[test]
    fn dashboard_records_multiple_paths_per_plugin_and_confirms_revocation() {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let file = root.path().join("original.txt");
        let folder = root.path().join("external");
        std::fs::write(&file, b"original").unwrap();
        std::fs::create_dir(&folder).unwrap();
        let mut panel = Panel::open_at(
            vec!["one".into(), "two".into()],
            Ok(root.path().join("sandbox")),
        );
        assert_eq!(
            std::fs::read_dir(root.path().join("sandbox/mcps"))
                .unwrap()
                .count(),
            2
        );
        approve(&mut panel, &file);
        approve(&mut panel, &folder);
        assert_eq!(panel.store.as_ref().unwrap().approvals("one").len(), 2);
        panel.key(KeyCode::Tab);
        assert!(panel.store.as_ref().unwrap().approvals("two").is_empty());
        approve(&mut panel, &file);
        panel.key(KeyCode::Delete);
        panel.key(KeyCode::Char('n'));
        assert_eq!(panel.store.as_ref().unwrap().approvals("two").len(), 1);
        panel.key(KeyCode::Delete);
        panel.key(KeyCode::Char('y'));
        assert!(panel.store.as_ref().unwrap().approvals("two").is_empty());
        assert_eq!(std::fs::read(&file).unwrap(), b"original");
        let restored = Panel::open_at(vec!["one".into()], Ok(root.path().join("sandbox")));
        assert_eq!(restored.store.as_ref().unwrap().approvals("one").len(), 2);
    }

    #[test]
    fn panel_discloses_inactive_isolation_and_handles_empty_inventory_and_small_terminals() {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let mut panel = Panel::open_at(vec![], Ok(root.path().join("sandbox")));
        for size in [(110, 28), (48, 16), (30, 8)] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(size.0, size.1)).unwrap();
            terminal.draw(|frame| panel.draw(frame)).unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect();
            if size.0 >= 48 {
                // RUNE_ASCII replaces the Unicode middle dot, not this disclosure.
                assert!(text.contains("NOT ACTIVE"));
                assert!(text.contains("No loaded plugins"));
            }
        }
        panel.workspace_only = true;
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 28)).unwrap();
        terminal.draw(|frame| panel.draw(frame)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("Workspace-only isolation ACTIVE"));
        assert!(text.contains("Broker-aware plugins"));
        let mut failed = Panel::open_at(vec!["test".into()], Ok(PathBuf::from("relative")));
        assert!(failed.store.is_err());
        failed.key(KeyCode::Char('a'));
        assert!(failed.input.is_none());
        assert!(failed.key(KeyCode::Esc));
    }
}
