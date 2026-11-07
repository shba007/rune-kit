use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use rune_kit_core::file_access::{Broker, RecoveryInfo, Review};
use std::{cell::Cell, path::PathBuf};

pub struct Panel {
    root: PathBuf,
    plugins: Vec<String>,
    plugin: usize,
    selected: usize,
    backups: bool,
    reviews: Vec<Review>,
    recoveries: Vec<RecoveryInfo>,
    confirm: bool,
    message: String,
    visible: Cell<bool>,
    scroll: u16,
}
impl Panel {
    pub fn new(root: PathBuf, plugins: Vec<String>) -> Self {
        let mut panel = Self {
            root,
            plugins,
            plugin: 0,
            selected: 0,
            backups: false,
            reviews: vec![],
            recoveries: vec![],
            confirm: false,
            message: String::new(),
            visible: Cell::new(false),
            scroll: 0,
        };
        panel.refresh();
        panel
    }
    fn broker(&self) -> std::io::Result<Broker> {
        Broker::open(
            self.root.clone(),
            self.plugins
                .get(self.plugin)
                .ok_or_else(|| std::io::Error::other("No loaded plugins"))?,
        )
    }
    fn refresh(&mut self) {
        self.confirm = false;
        self.scroll = 0;
        let result = (|| {
            let broker = self.broker()?;
            broker.prune_completed()?;
            Ok::<_, std::io::Error>((broker.reviews()?, broker.recoveries()?))
        })();
        match result {
            Ok((reviews, recoveries)) => {
                self.reviews = reviews;
                self.recoveries = recoveries;
                self.message.clear();
            }
            Err(e) => self.message = e.to_string(),
        }
        self.selected = self.selected.min(self.len().saturating_sub(1));
    }
    fn len(&self) -> usize {
        if self.backups {
            self.recoveries.len()
        } else {
            self.reviews.len()
        }
    }
    pub fn key(&mut self, key: KeyCode) -> bool {
        if self.confirm {
            if key == KeyCode::Char('y') && self.visible.get() {
                let result = self.broker().and_then(|broker| {
                    if self.backups {
                        broker.restore(&self.recoveries[self.selected])
                    } else {
                        broker.apply_review(&self.reviews[self.selected])
                    }
                });
                let message = match result {
                    Ok(()) => "Completed. Recovery backups are retained for seven days.".into(),
                    Err(e) => e.to_string(),
                };
                self.refresh();
                self.message = message;
            }
            self.confirm = false;
            return false;
        }
        match key {
            KeyCode::Esc | KeyCode::Char('q') => return true,
            KeyCode::Tab if !self.plugins.is_empty() => {
                self.plugin = (self.plugin + 1) % self.plugins.len();
                self.selected = 0;
                self.refresh();
            }
            KeyCode::Char('b') => {
                self.backups = !self.backups;
                self.selected = 0;
                self.refresh();
            }
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Down => {
                self.selected = (self.selected + 1).min(self.len().saturating_sub(1));
                self.scroll = 0;
            }
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                self.scroll = 0;
            }
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(3),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(3),
            KeyCode::Enter if self.len() > 0 && self.visible.get() => self.confirm = true,
            _ => {}
        }
        false
    }
    pub fn draw(&self, frame: &mut Frame) {
        let area = frame.area();
        frame.render_widget(Clear, area);
        self.visible.set(area.width >= 80 && area.height >= 24);
        if !self.visible.get() {
            frame.render_widget(
                Paragraph::new("Enlarge to 80 x 24 to review changes. Esc returns."),
                area,
            );
            return;
        }
        let [header, list, detail, footer] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(11),
            Constraint::Length(3),
        ])
        .areas(area);
        let plugin = self
            .plugins
            .get(self.plugin)
            .map(String::as_str)
            .unwrap_or("No loaded plugins");
        frame.render_widget(Paragraph::new(vec![Line::from(crate::tui::safe(&format!("{} · {plugin}",if self.backups{"Recovery backups"}else{"Review staged file changes"}))),Line::from("Inspect the staged file or backup before approving. File contents and secrets are not displayed.")]),header);
        let rows: Vec<_> = if self.backups {
            self.recoveries
                .iter()
                .map(|info| {
                    ListItem::new(crate::tui::safe(&format!(
                        "{}  {}",
                        if info.completed.is_some() {
                            "Complete"
                        } else {
                            "Unresolved"
                        },
                        info.path.display()
                    )))
                })
                .collect()
        } else {
            self.reviews
                .iter()
                .map(|review| {
                    ListItem::new(crate::tui::safe(&format!(
                        "{}  {}",
                        if review.deleted {
                            "Delete"
                        } else if review.baseline.is_none() {
                            "Create"
                        } else {
                            "Modify"
                        },
                        review.path.display()
                    )))
                })
                .collect()
        };
        frame.render_stateful_widget(
            List::new(rows)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(if self.backups {
                            " Backups "
                        } else {
                            " Changes "
                        }),
                )
                .highlight_style(Style::new().fg(Color::Black).bg(Color::White)),
            list,
            &mut ListState::default().with_selected(Some(self.selected)),
        );
        let details = if self.backups {
            self.recoveries.get(self.selected).map(|info|format!("Original: {}\nBackup: {}\nRecovery ID: {}\nUnresolved backups never expire. Restore refuses newer external edits.",info.path.display(),info.backup_path.display(),info.id))
        } else {
            self.reviews.get(self.selected).map(|review|format!("Original: {}\nStaged file: {}\nOriginal SHA-256: {}\nStaged SHA-256: {}\nCopy-back refuses changed originals or changed staged content.",review.path.display(),review.staged_path.display(),review.baseline.as_deref().unwrap_or("new file"),review.staged_digest.as_deref().unwrap_or("delete")))
        };
        frame.render_widget(
            Paragraph::new(crate::tui::safe_multiline(
                details
                    .as_deref()
                    .unwrap_or("No pending changes or backups."),
            ))
            .wrap(Wrap { trim: false })
            .scroll((self.scroll, 0))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Review details · PgUp/PgDown "),
            ),
            detail,
        );
        let prompt = if self.confirm {
            if self.backups {
                "Restore this exact backup? y confirms; any other key cancels"
            } else {
                "Copy these reviewed changes back to the original? y confirms; any other key cancels"
            }
        } else {
            &self.message
        };
        frame.render_widget(Paragraph::new(vec![Line::styled(crate::tui::safe(prompt),Style::new().fg(Color::Yellow)),Line::from("Tab plugin · ↑↓ select · b changes/backups · r refresh · Enter review · Esc back")]),footer);
        crate::tui::compatibility(frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune_kit_core::{
        file_access::{Operation, Request},
        sandbox::Sandbox,
    };
    #[test]
    fn tui_requires_visible_review_and_confirmation_and_can_restore_the_selected_backup() {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let source = root.path().join("original.txt");
        std::fs::write(&source, b"original").unwrap();
        let storage = root.path().join("sandbox");
        let mut sandbox = Sandbox::open(storage.clone()).unwrap();
        sandbox.approve("fixture", &source).unwrap();
        let broker = Broker::open(storage.clone(), "fixture").unwrap();
        broker
            .request(Request {
                schema_version: 1,
                operation: Operation::WriteFile,
                path: source.clone(),
                data: b"updated".to_vec(),
            })
            .unwrap();
        let mut panel = Panel::new(storage, vec!["fixture".into()]);
        panel.key(KeyCode::Enter);
        panel.key(KeyCode::Char('y'));
        assert_eq!(std::fs::read(&source).unwrap(), b"original");
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 28)).unwrap();
        terminal.draw(|frame| panel.draw(frame)).unwrap();
        panel.key(KeyCode::Enter);
        panel.key(KeyCode::Char('n'));
        assert_eq!(std::fs::read(&source).unwrap(), b"original");
        panel.key(KeyCode::Enter);
        panel.key(KeyCode::Char('y'));
        assert_eq!(std::fs::read(&source).unwrap(), b"updated");
        panel.key(KeyCode::Char('b'));
        terminal.draw(|frame| panel.draw(frame)).unwrap();
        panel.key(KeyCode::Enter);
        panel.key(KeyCode::Char('y'));
        assert_eq!(std::fs::read(&source).unwrap(), b"original");
    }
}
