//! Dashboard attached to the local HTTP server; no independent plugin probes.
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use std::{sync::Arc, time::Duration};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn safe(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).collect()
}
fn panel(title: &'static str, focus: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(title)
        .border_style(Style::new().fg(Color::DarkGray))
        .title_style(
            Style::new()
                .fg(if focus { Color::Cyan } else { Color::White })
                .add_modifier(Modifier::BOLD),
        )
}

struct Restore;
impl Drop for Restore {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

/// Attached to this server's observed state; never probes separate plugin processes.
pub fn server_dashboard(state: Arc<std::sync::Mutex<crate::server::Snapshot>>) -> Result<()> {
    let mut terminal = ratatui::try_init()?;
    let _restore = Restore;
    let mut selected = 0usize;
    let mut tools_view = false;
    let mut configuration = false;
    let mut discovery: Option<std::thread::JoinHandle<()>> = None;
    loop {
        let snapshot = state.lock().unwrap().clone();
        if snapshot.status != "listening" {
            break;
        }
        terminal
            .draw(|frame| draw_server(frame, &snapshot, selected, tools_view, configuration))?;
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                KeyCode::Down | KeyCode::Char('j') => {
                    selected = selected.saturating_add(1).min(
                        (if tools_view {
                            snapshot.tools.len()
                        } else {
                            snapshot.active.len() + snapshot.recent.len()
                        })
                        .saturating_sub(1),
                    )
                }
                KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
                KeyCode::Char('d')
                    if discovery.as_ref().is_none_or(|worker| worker.is_finished()) =>
                {
                    let state = state.clone();
                    discovery = Some(std::thread::spawn(move || discover(state)));
                }
                KeyCode::Char('i') => configuration = !configuration,
                KeyCode::Tab => {
                    configuration = false;
                    tools_view = !tools_view;
                    selected = 0;
                }
                KeyCode::Char('r') => { /* next frame reads fresh server state */ }
                _ => {}
            }
        }
    }
    Ok(())
}

const ACCENT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;

fn label(key: &str, value: impl Into<String>) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:<13}"), Style::new().fg(DIM)),
        Span::raw(safe(&value.into())),
    ])
}
fn outcome(status: &str) -> Color {
    if status.contains("fail") || status.contains("forbidden") || status.contains("error") {
        Color::Red
    } else if status == "running" || status == "queued" {
        ACCENT
    } else {
        Color::White
    }
}
fn draw_server(
    frame: &mut Frame,
    snapshot: &crate::server::Snapshot,
    selected: usize,
    tools_view: bool,
    configuration: bool,
) {
    let area = frame.area();
    if area.width < 48 || area.height < 16 {
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled("Rune Kit", Style::new().fg(ACCENT).bold()),
                Line::from("Enlarge terminal to at least 48 × 16"),
                Line::from("q stop server · Ctrl+C stop"),
            ]),
            area,
        );
        compatibility(frame);
        return;
    }
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);
    let seconds = snapshot.started.elapsed().as_secs();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ᚱ Rune Kit ", Style::new().fg(ACCENT).bold()),
            Span::styled(
                format!("v{}  ", env!("CARGO_PKG_VERSION")),
                Style::new().fg(DIM),
            ),
            Span::styled("● ", Style::new().fg(Color::White)),
            Span::raw(format!("{}  ", safe(&snapshot.endpoint))),
            Span::styled(
                format!(
                    "up {:02}:{:02}:{:02}",
                    seconds / 3600,
                    seconds / 60 % 60,
                    seconds % 60
                ),
                Style::new().fg(DIM),
            ),
        ]))
        .block(panel("", false)),
        header,
    );
    if configuration {
        let lines = vec![
            label("Status", &snapshot.status),
            label(
                "Access",
                if snapshot.read_only {
                    "Read-only tools"
                } else {
                    "Server-permitted tools"
                },
            ),
            label("Timeout", format!("{} seconds", snapshot.timeout)),
            label(
                "Allowlist",
                if snapshot.allowed_tools.is_empty() {
                    "All server-permitted tools".into()
                } else {
                    snapshot.allowed_tools.join(", ")
                },
            ),
            label(
                "Roots",
                if snapshot.roots.is_empty() {
                    "None configured".into()
                } else {
                    snapshot.roots.join(", ")
                },
            ),
            Line::default(),
            label("Executable:", snapshot.executable.display().to_string()),
            label("Data:", snapshot.data_root.display().to_string()),
            Line::default(),
            Line::styled(
                "Only this Kit server is observed. Arguments and secrets are hidden.",
                Style::new().fg(DIM),
            ),
        ];
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .block(panel(" Configuration ", true)),
            body,
        );
    } else {
        let [inventory, activity] = Layout::vertical([
            Constraint::Length(if area.height >= 26 { 9 } else { 6 }),
            Constraint::Min(3),
        ])
        .areas(body);
        if area.width >= 95 {
            let [plugins, server] =
                Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)])
                    .areas(inventory);
            draw_plugins(frame, snapshot, plugins);
            frame.render_widget(
                Paragraph::new(vec![
                    label("Status", &snapshot.status),
                    label(
                        "Access",
                        if snapshot.read_only {
                            "Read only"
                        } else {
                            "Configured tools"
                        },
                    ),
                    label("Active", snapshot.active.len().to_string()),
                    label("Timeout", format!("{}s", snapshot.timeout)),
                    label("Tools", snapshot.tools.len().to_string()),
                ])
                .block(panel(" Server ", false)),
                server,
            );
        } else {
            draw_plugins(frame, snapshot, inventory);
        }
        draw_activity(frame, snapshot, selected, tools_view, activity);
    }
    let hints = if area.width < 80 {
        [
            ("Tab", "view"),
            ("d", "discover"),
            ("i", "config"),
            ("q", "stop"),
        ]
        .as_slice()
    } else {
        [
            ("Tab", "calls / tools"),
            ("↑↓", "select"),
            ("d", "discover"),
            ("i", "config"),
            ("r", "refresh"),
            ("q", "stop"),
        ]
        .as_slice()
    };
    let spans: Vec<_> = hints
        .iter()
        .flat_map(|(key, meaning)| {
            [
                Span::styled(format!(" {key} "), Style::new().fg(Color::Black).bg(DIM)),
                Span::styled(format!(" {meaning}  "), Style::new().fg(DIM)),
            ]
        })
        .collect();
    frame.render_widget(Paragraph::new(Line::from(spans)), footer);
    compatibility(frame);
}
fn draw_plugins(frame: &mut Frame, snapshot: &crate::server::Snapshot, area: Rect) {
    let mut lines = Vec::new();
    if snapshot.plugins.is_empty() {
        lines.push(Line::from("No plugins installed"));
        lines.push(Line::styled(
            "Install a plugin, then restart serve.",
            Style::new().fg(DIM),
        ));
    } else {
        lines.extend(snapshot.plugins.iter().map(|plugin| {
            let (name, state) = plugin.split_once("; ").unwrap_or((plugin, ""));
            let name = name
                .replace(": loaded (", "  ")
                .trim_end_matches(')')
                .to_string();
            Line::from(vec![
                Span::styled(" ● ", Style::new().fg(outcome(state))),
                Span::raw(safe(&name)),
                Span::styled(format!("  {}", safe(state)), Style::new().fg(DIM)),
            ])
        }));
    }
    lines.push(Line::default());
    lines.push(Line::styled(
        safe(&snapshot.discovery_note),
        Style::new().fg(if snapshot.discovery_note.contains("failed") {
            Color::Red
        } else {
            DIM
        }),
    ));
    frame.render_widget(Paragraph::new(lines).block(panel(" Plugins ", false)), area);
}
fn draw_activity(
    frame: &mut Frame,
    snapshot: &crate::server::Snapshot,
    selected: usize,
    tools_view: bool,
    area: Rect,
) {
    let rows: Vec<ListItem> = if tools_view {
        snapshot
            .tools
            .iter()
            .map(|tool| ListItem::new(safe(tool)))
            .collect()
    } else {
        snapshot
            .active
            .iter()
            .chain(snapshot.recent.iter())
            .map(|call| {
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {:>4}  ", call.sequence), Style::new().fg(DIM)),
                    Span::raw(format!("{}  {}  ", safe(&call.method), safe(&call.name))),
                    Span::styled(safe(&call.status), Style::new().fg(outcome(&call.status))),
                    Span::styled(
                        if call.status == "running" || call.status == "queued" {
                            String::new()
                        } else {
                            format!("  {}ms", call.millis)
                        },
                        Style::new().fg(DIM),
                    ),
                ]))
            })
            .collect()
    };
    let empty = rows.is_empty();
    let rows = if empty {
        vec![ListItem::new(Line::styled(
            if tools_view {
                "  No tools discovered · press d to discover"
            } else {
                "  Waiting for requests · connect an HTTP client"
            },
            Style::new().fg(DIM),
        ))]
    } else {
        rows
    };
    frame.render_stateful_widget(
        List::new(rows)
            .block(panel(
                if tools_view {
                    " Tools · last discovery "
                } else {
                    " Requests · live / recent "
                },
                true,
            ))
            .highlight_symbol("› ")
            .highlight_style(Style::new().fg(Color::White).bg(Color::DarkGray)),
        area,
        &mut ListState::default().with_selected(if empty { None } else { Some(selected) }),
    );
}
fn compatibility(frame: &mut Frame) {
    let no_color = std::env::var_os("NO_COLOR").is_some();
    let ascii = std::env::var_os("RUNE_ASCII").is_some();
    if !no_color && !ascii {
        return;
    }
    for cell in &mut frame.buffer_mut().content {
        if no_color {
            cell.set_fg(Color::Reset);
            cell.set_bg(Color::Reset);
        }
        if ascii && !cell.symbol().is_ascii() {
            cell.set_symbol(match cell.symbol() {
                "│" => "|",
                "─" => "-",
                "╭" | "╮" | "╰" | "╯" => "+",
                "●" | "·" => ".",
                "›" => ">",
                _ => "*",
            });
        }
    }
}
// Discovery uses this server's HTTP dispatcher and permission ceilings. It never
// creates a second plugin runtime or changes server configuration.
fn discover(state: Arc<std::sync::Mutex<crate::server::Snapshot>>) {
    let endpoint = state.lock().unwrap().endpoint.clone();
    let result = (|| -> Result<()> {
        use serde_json::json;
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(35))
            .build()?;
        let mut request = client
            .post(endpoint)
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", "tools/list");
        if let Ok(token) = std::env::var("RUNE_KIT_HTTP_TOKEN") {
            request = request.bearer_auth(token);
        }
        let response = request
            .json(&json!({"jsonrpc":"2.0", "id":"kit-dashboard-discovery",
            "method":"tools/list", "params":{"_meta":{
                "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                "io.modelcontextprotocol/clientCapabilities":{}}}}))
            .send()?
            .error_for_status()?;
        // Consume the stream so the request is not cancelled by a disconnect.
        let body = response.text()?;
        if body
            .lines()
            .filter_map(|line| {
                serde_json::from_str::<serde_json::Value>(
                    line.strip_prefix("data: ").unwrap_or(line),
                )
                .ok()
            })
            .any(|value| value.get("error").is_some())
        {
            return Err("Discovery failed".into());
        }
        Ok(())
    })();
    if result.is_err() {
        state.lock().unwrap().discovery_note =
            "Discovery failed; inspect plugin installation, dependencies and server restrictions"
                .into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::{Call, Snapshot};
    use ratatui::{Terminal, backend::TestBackend};
    use std::{collections::VecDeque, path::PathBuf, time::Instant};

    fn snapshot() -> Snapshot {
        Snapshot {
            endpoint: "http://127.0.0.1:47601/mcp".into(),
            status: "listening".into(),
            started: Instant::now(),
            plugins: vec![
                "git 0.3.0: loaded (Native); discovered".into(),
                "time 0.3.0: loaded (Wasm); discovered".into(),
            ],
            read_only: true,
            allowed_tools: vec![],
            roots: vec!["filesystem: /workspace".into()],
            timeout: 30,
            executable: PathBuf::from("/home/user/.rune/rune-kit"),
            data_root: PathBuf::from("/home/user/.rune/tools"),
            active: vec![],
            recent: VecDeque::from([Call {
                sequence: 1,
                method: "tools/list".into(),
                name: String::new(),
                status: "complete".into(),
                millis: 12,
            }]),
            tools: vec!["git__status".into(), "time__now".into()],
            discovery_note: "2 tools returned by the last discovery".into(),
        }
    }
    fn render(snapshot: &Snapshot, width: u16, height: u16, tools: bool, config: bool) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| draw_server(frame, snapshot, 0, tools, config))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..height {
            for x in 0..width {
                text.push_str(buffer[(x, y)].symbol());
            }
            text.push('\n');
        }
        if std::env::var_os("NO_COLOR").is_some() {
            assert!(
                buffer
                    .content
                    .iter()
                    .all(|cell| cell.fg == Color::Reset && cell.bg == Color::Reset)
            );
        }
        if std::env::var_os("RUNE_ASCII").is_some() {
            assert!(text.is_ascii());
        }
        text
    }
    #[test]
    fn server_layout_keeps_inventory_activity_and_configuration_readable() {
        let snapshot = snapshot();
        let screen = render(&snapshot, 110, 28, false, false);
        assert!(screen.contains("Rune Kit"));
        assert!(screen.contains("Native"));
        assert!(screen.contains("Requests"));
        assert!(!screen.contains("Executable:"));
        assert!(render(&snapshot, 110, 28, true, false).contains("git__status"));
        let configuration = render(&snapshot, 80, 24, false, true);
        assert!(configuration.contains("Executable:"));
        assert!(configuration.contains("/home/user/.rune/tools"));
        assert!(configuration.contains("/workspace"));
        assert!(render(&snapshot, 60, 20, false, false).contains("Plugins"));
        assert!(render(&snapshot, 30, 8, false, false).contains("Enlarge terminal"));
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
        let suffix = if std::env::var_os("RUNE_ASCII").is_some() {
            "-plain"
        } else {
            ""
        };
        std::fs::write(
            root.join(format!("kit-dashboard{suffix}-preview.txt")),
            screen,
        )
        .unwrap();
        std::fs::write(
            root.join(format!("kit-configuration{suffix}-preview.txt")),
            configuration,
        )
        .unwrap();
    }
    #[test]
    fn empty_and_failed_discovery_states_have_actions() {
        let mut snapshot = snapshot();
        snapshot.plugins.clear();
        snapshot.tools.clear();
        snapshot.recent.clear();
        snapshot.discovery_note =
            "Discovery failed; inspect plugin installation and dependencies".into();
        let screen = render(&snapshot, 110, 28, true, false);
        assert!(screen.contains("No plugins installed"));
        assert!(screen.contains("restart serve"));
        assert!(screen.contains("Discovery failed"));
        assert!(screen.contains("press d"));
    }
}
