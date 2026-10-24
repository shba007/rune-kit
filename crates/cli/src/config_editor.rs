use crate::configuration::{Configuration, Field};
use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use rune_kit_core::McpRouter;
use std::sync::{Arc, Mutex, mpsc};

#[derive(Default)]
pub struct Editor {
    selected: usize,
    input: Option<String>,
    shared: bool,
    message: String,
    pending: Option<mpsc::Receiver<Result<(), String>>>,
}

impl Editor {
    pub fn poll(&mut self) {
        if let Some(receiver) = &self.pending {
            match receiver.try_recv() {
                Ok(result) => {
                    self.message = result.err().unwrap_or_else(|| {
                        "Applied immediately. Secret values last only for this session.".into()
                    });
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.message =
                        "Configuration worker stopped; check settings before retrying".into();
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }

    pub fn key(
        &mut self,
        code: KeyCode,
        config: Arc<Mutex<Configuration>>,
        router: Arc<Mutex<McpRouter>>,
        snapshot: Arc<Mutex<crate::server::Snapshot>>,
    ) -> bool {
        if self.pending.is_some() {
            return code == KeyCode::Esc;
        }
        let fields = match config.try_lock() {
            Ok(config) => config.fields(),
            Err(_) => {
                self.message = "Configuration update in progress".into();
                return false;
            }
        };
        if let Some(input) = &mut self.input {
            match code {
                KeyCode::Esc => self.input = None,
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) if !c.is_control() => input.push(c),
                KeyCode::Enter => {
                    if let Some(field) = fields.get(self.selected).cloned() {
                        let value = self.input.take().unwrap();
                        let shared = self.shared;
                        let (tx, rx) = mpsc::channel();
                        self.pending = Some(rx);
                        self.message = "Applying…".into();
                        std::thread::spawn(move || {
                            let result =
                                config.lock().unwrap().apply(&router, &field, value, shared);
                            if result.is_ok() {
                                let mut state = snapshot.lock().unwrap();
                                state.tools.clear();
                                state.discovery_note =
                                    "Configuration changed; press d to rediscover tools".into();
                            }
                            let _ = tx.send(result);
                        });
                    }
                }
                _ => {}
            }
        } else {
            match code {
                KeyCode::Esc | KeyCode::Char('q') => return true,
                KeyCode::Down | KeyCode::Char('j') => {
                    self.selected = (self.selected + 1).min(fields.len().saturating_sub(1))
                }
                KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
                KeyCode::Char('s') => self.shared = !self.shared,
                KeyCode::Enter => {
                    if let Some(field) = fields.get(self.selected) {
                        if field.locked {
                            self.message = "Filesystem roots await sandbox support".into();
                        } else {
                            self.input = Some(field.value.clone());
                            self.message.clear();
                        }
                    }
                }
                _ => {}
            }
        }
        false
    }

    pub fn draw(&self, frame: &mut Frame, fields: &[Field]) {
        let area = frame.area();
        if area.width < 48 || area.height < 16 {
            return;
        }
        let header = super::tui::header_height(area.height);
        let area = Rect::new(
            area.x,
            area.y + header,
            area.width,
            area.height.saturating_sub(header),
        );
        frame.render_widget(Clear, area);
        let [body, footer] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).areas(area);
        let [list, detail] =
            Layout::horizontal([Constraint::Percentage(42), Constraint::Percentage(58)])
                .areas(body);
        let rows: Vec<_> = fields
            .iter()
            .map(|field| {
                ListItem::new(super::tui::safe(&format!(
                    "{} / {}{}",
                    field.plugin,
                    field.key,
                    if field.locked { " [locked]" } else { "" }
                )))
            })
            .collect();
        frame.render_stateful_widget(
            List::new(rows)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Plugin settings "),
                )
                .highlight_style(Style::new().fg(Color::Black).bg(Color::White)),
            list,
            &mut ListState::default().with_selected(Some(self.selected)),
        );
        let lines = if let Some(field) = fields.get(self.selected) {
            vec![
                Line::from(super::tui::safe(&field.schema.description)),
                Line::from(format!(
                    "Type: {} · {}",
                    field.schema.kind,
                    if field.schema.required {
                        "required"
                    } else {
                        "optional"
                    }
                )),
                Line::from(if self.shared {
                    "Save as shared default (per-plugin overrides take precedence)"
                } else {
                    "Save for this plugin"
                }),
                Line::from(if field.schema.secret {
                    "Secret · masked · session only"
                } else {
                    "Saved for future launches"
                }),
            ]
        } else {
            vec![
                Line::from("No configuration metadata available."),
                Line::from("Install a registry version with configuration metadata."),
            ]
        };
        let [detail, input_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(3)]).areas(detail);
        let value = fields
            .get(self.selected)
            .map(|field| {
                if field.schema.secret {
                    "********".into()
                } else {
                    super::tui::safe(self.input.as_ref().unwrap_or(&field.value))
                }
            })
            .unwrap_or_default();
        frame.render_widget(
            Paragraph::new(value).block(Block::default().borders(Borders::ALL).title(
                if self.input.is_some() {
                    " Editing · Enter applies "
                } else {
                    " Value · Enter edits "
                },
            )),
            input_area,
        );
        frame.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: false }).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Configuration "),
            ),
            detail,
        );
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(super::tui::safe(&self.message)),
                Line::from("↑↓ select · Enter edit/save · s shared default · Esc back"),
            ]),
            footer,
        );
        super::tui::compatibility(frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secret_input_never_reaches_the_terminal_buffer() {
        let editor = Editor {
            input: Some("private-value".into()),
            ..Default::default()
        };
        let schema = serde_json::from_value(serde_json::json!({"type":"string", "description":"Credential", "required":true, "secret":true, "env":["API_KEY"]})).unwrap();
        let field = Field {
            plugin: "fixture".into(),
            key: "api_key".into(),
            schema,
            value: "private-value".into(),
            locked: false,
        };
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 28)).unwrap();
        terminal
            .draw(|frame| editor.draw(frame, std::slice::from_ref(&field)))
            .unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(rendered.contains("********"));
        assert!(!rendered.contains("private-value"));
        assert!(rendered.contains("session only"));
        for size in [(48, 16), (30, 8)] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(size.0, size.1)).unwrap();
            terminal
                .draw(|frame| editor.draw(frame, std::slice::from_ref(&field)))
                .unwrap();
        }
    }
}
