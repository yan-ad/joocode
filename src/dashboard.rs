use std::{
    collections::{BTreeMap, BTreeSet},
    io::IsTerminal,
    net::SocketAddr,
    sync::mpsc::{Receiver, TryRecvError},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DashboardProviderRuntimeSnapshot {
    pub provider: String,
    pub state: String,
    pub active_requests: usize,
    pub concurrency_limit: usize,
    pub cooldown_ms: u64,
    pub latency_ms: Option<f64>,
    pub consecutive_failures: u32,
}

fn draw_success(frame: &mut Frame<'_>, message: &str) {
    let modal = draw_modal_shell(
        frame,
        "Operation complete",
        76,
        16,
        Line::from(Span::styled(
            "Enter return",
            Style::default().fg(MODAL_ACCENT),
        )),
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "Success",
                Style::default()
                    .fg(Color::LightGreen)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(message, Style::default().fg(Color::Gray))),
        ])
        .style(Style::default().bg(MODAL_BACKGROUND))
        .wrap(Wrap { trim: true }),
        modal.content,
    );
}

fn draw_codex_set_page(frame: &mut Frame<'_>, area: Rect, data: &DashboardData) {
    let advertised = data.subagent_catalog.resolve(&data.models).len();
    let enabled = data
        .proxy_targets
        .get(&ProxyTarget::Codex)
        .copied()
        .unwrap_or(false);
    let lines = vec![
        Line::from(Span::styled(
            "INTEGRATION",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(format!(
            "Reasoning effort cap     {}",
            data.subagent_catalog
                .reasoning_effort_cap
                .map(|cap| cap.label())
                .unwrap_or("no cap")
        )),
        Line::from(format!(
            "Codex target             {}",
            if enabled {
                "● Enabled"
            } else {
                "○ Disabled"
            }
        )),
        Line::from(format!("Gateway                  ● {}", data.listening)),
        Line::from(format!(
            "Keep running on exit     {}",
            if data.run_in_background { "On" } else { "Off" }
        )),
        Line::from(format!(
            "Auto-start after login   {}",
            data.autostart.label()
        )),
        Line::from(""),
        Line::from(Span::styled(
            "CATALOG",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(format!("Active Joocode models    {}", data.model_count)),
        Line::from(format!(
            "Subagent advertised      {advertised}/{}",
            data.subagent_catalog.max_entries
        )),
        Line::from(format!(
            "Featured / fallback      {} / {}",
            data.subagent_catalog.featured_models.len(),
            data.subagent_catalog.fallback_models.len()
        )),
        Line::from(""),
        Line::from("s / Enter   Synchronize Codex models"),
        Line::from("b           Toggle background handoff"),
        Line::from("a           Edit subagent policy"),
        Line::from("e           Cycle reasoning effort cap"),
        Line::from(""),
        Line::from(Span::styled(
            "Launcher: jcx codex -- [codex arguments]",
            Style::default().fg(Color::LightCyan),
        )),
    ];
    draw_read_only_page(frame, area, "Codex Set", lines);
}

fn draw_usage_page(frame: &mut Frame<'_>, area: Rect, runtime: &DashboardRuntimeSnapshot) {
    let mut lines = vec![
        Line::from(format!("Uptime: {}", format_duration(runtime.uptime_secs))),
        Line::from(format!(
            "Requests: {}   Active: {}",
            runtime.requests, runtime.active
        )),
        Line::from(format!(
            "Successes: {}   Failures: {}",
            runtime.successes, runtime.failures
        )),
        Line::from(format!(
            "Tool calls: {}   Browser/computer: {}",
            runtime.tool_calls, runtime.browser_tool_calls
        )),
        Line::from(format!(
            "Tokens: {} in / {} out   Retries: {}   Failovers: {}",
            runtime.input_tokens, runtime.output_tokens, runtime.retries, runtime.failovers
        )),
        Line::from(""),
        Line::from("Provider                  State          Latency   Cooldown"),
    ];
    if runtime.providers.is_empty() {
        lines.push(Line::from("No provider traffic has been observed yet."));
    } else {
        lines.extend(runtime.providers.iter().map(|provider| {
            Line::from(format!(
                "{:<25} {:<14} {:>7}   {:>8}",
                provider.provider,
                provider.state,
                provider
                    .latency_ms
                    .map(|value| format!("{value:.0}ms"))
                    .unwrap_or_else(|| "—".into()),
                if provider.cooldown_ms == 0 {
                    "—".into()
                } else {
                    format!("{}ms", provider.cooldown_ms)
                },
            ))
        }));
    }

    draw_read_only_page(frame, area, "Usage", lines);
}

fn draw_storage_page(frame: &mut Frame<'_>, area: Rect, storage: &DashboardStorageSnapshot) {
    let mut lines = vec![
        Line::from(format!(
            "Tracked files: {}   Total size: {}",
            storage.entries.len(),
            format_bytes(storage.total_bytes())
        )),
        Line::from(""),
    ];
    if storage.entries.is_empty() {
        lines.push(Line::from("Storage paths are unavailable."));
    } else {
        lines.extend(storage.entries.iter().map(|entry| {
            Line::from(format!(
                "{:<20} {:>10}  {}",
                entry.label,
                entry
                    .size_bytes
                    .map(format_bytes)
                    .unwrap_or_else(|| "missing".into()),
                entry.path,
            ))
        }));
    }
    draw_read_only_page(frame, area, "Storage", lines);
}

fn draw_system_page(frame: &mut Frame<'_>, area: Rect, data: &DashboardData) {
    let system = &data.system;
    let lines = vec![
        Line::from(Span::styled(
            "RUNTIME",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(format!("Mode                     {}", system.mode)),
        Line::from(format!("Listener                 {}", data.listening)),
        Line::from(format!(
            "Background handoff       {}",
            if data.run_in_background { "On" } else { "Off" }
        )),
        Line::from(format!(
            "Auto-start               {}",
            data.autostart.label()
        )),
        Line::from(""),
        Line::from(Span::styled(
            "SECURITY",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(format!("Data-plane auth          {}", system.data_auth)),
        Line::from(format!(
            "Management auth          {}",
            system.management_auth
        )),
        Line::from(format!(
            "Allowed CORS origins     {}",
            system.allowed_origins
        )),
        Line::from(format!("Remote rate limit        {}", system.rate_limit)),
        Line::from(""),
        Line::from(Span::styled(
            "BOUNDS",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(format!(
            "Request body             {}",
            format_bytes(system.max_request_bytes as u64)
        )),
        Line::from(format!(
            "SSE event                {}",
            format_bytes(system.max_sse_event_bytes as u64)
        )),
        Line::from(format!(
            "Tool arguments           {}",
            format_bytes(system.max_tool_argument_bytes as u64)
        )),
        Line::from(format!(
            "Stream idle timeout      {}s",
            system.stream_idle_timeout_secs
        )),
    ];
    draw_read_only_page(frame, area, "System", lines);
}

fn draw_logs_page(frame: &mut Frame<'_>, area: Rect, events: &[DashboardRequestEvent]) {
    let panel_area = area.inner(Margin::new(1, 1));
    let panel = dashboard_panel("Logs — newest first", Color::LightCyan);
    let inner = panel.inner(panel_area);
    frame.render_widget(panel, panel_area);

    if events.is_empty() {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    "No requests yet",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "Requests routed through Joocode will appear here.",
                    Style::default().fg(Color::DarkGray),
                )),
            ]),
            inner,
        );
        return;
    }

    let successes = events
        .iter()
        .filter(|event| (200..400).contains(&event.status))
        .count();
    let failures = events.len().saturating_sub(successes);
    let [summary, content] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).areas(inner);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("{} requests", events.len()),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("   ● ", Style::default().fg(Color::Green)),
            Span::styled(
                format!("{successes} successful"),
                Style::default().fg(Color::Gray),
            ),
            Span::styled("   ● ", Style::default().fg(Color::Red)),
            Span::styled(
                format!("{failures} failed"),
                Style::default().fg(Color::Gray),
            ),
        ])),
        summary,
    );

    if content.width < 100 {
        draw_compact_logs(frame, content, events);
    } else {
        draw_logs_table(frame, content, events);
    }
}

fn draw_logs_table(frame: &mut Frame<'_>, area: Rect, events: &[DashboardRequestEvent]) {
    let rows = events.iter().rev().map(|event| {
        let provider = event.provider.as_deref().unwrap_or("—");
        let model = event.model.as_deref().unwrap_or("—");
        Row::new(vec![
            Cell::from(event.status.to_string()).style(status_style(event.status)),
            Cell::from(event.method.clone()).style(Style::default().fg(Color::LightCyan)),
            Cell::from(event.path.clone()),
            Cell::from(provider.to_owned()).style(Style::default().fg(Color::LightMagenta)),
            Cell::from(model.to_owned()),
            Cell::from(format_token_pair(event.input_tokens, event.output_tokens)),
            Cell::from(format!("{}/{}", event.retries, event.failovers)),
            Cell::from(format_latency(event.duration_ms)),
        ])
    });
    let header = Row::new([
        "STATUS", "METHOD", "ENDPOINT", "PROVIDER", "MODEL", "TOKENS", "R/F", "LATENCY",
    ])
    .style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .bottom_margin(1);
    let widths = [
        Constraint::Length(7),
        Constraint::Length(7),
        Constraint::Min(22),
        Constraint::Length(18),
        Constraint::Length(24),
        Constraint::Length(13),
        Constraint::Length(7),
        Constraint::Length(10),
    ];
    frame.render_widget(
        Table::new(rows, widths)
            .header(header)
            .column_spacing(1)
            .row_highlight_style(Style::default()),
        area,
    );
}

fn draw_compact_logs(frame: &mut Frame<'_>, area: Rect, events: &[DashboardRequestEvent]) {
    let items = events.iter().rev().map(|event| {
        let route = match (&event.provider, &event.model) {
            (Some(provider), Some(model)) => format!("{provider}/{model}"),
            (Some(provider), None) => provider.clone(),
            _ => "unresolved route".into(),
        };
        ListItem::new(vec![
            Line::from(vec![
                Span::styled(
                    format!("{:>3}", event.status),
                    status_style(event.status).add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("{:<6}", event.method),
                    Style::default().fg(Color::LightCyan),
                ),
                Span::raw(format!("  {}", event.path)),
                Span::styled(
                    format!("  {}", format_latency(event.duration_ms)),
                    Style::default().fg(Color::Yellow),
                ),
            ]),
            Line::from(vec![
                Span::raw("     "),
                Span::styled(route, Style::default().fg(Color::LightMagenta)),
                Span::styled(
                    format!(
                        "   {} tokens   retry {}   failover {}",
                        format_token_pair(event.input_tokens, event.output_tokens),
                        event.retries,
                        event.failovers
                    ),
                    Style::default().fg(Color::DarkGray),
                ),
            ]),
            Line::from(""),
        ])
    });
    frame.render_widget(List::new(items), area);
}

fn status_style(status: u16) -> Style {
    let color = match status {
        200..=399 => Color::Green,
        400..=499 => Color::Yellow,
        _ => Color::Red,
    };
    Style::default().fg(color)
}

fn format_latency(duration_ms: u64) -> String {
    if duration_ms >= 1_000 {
        format!("{:.1}s", duration_ms as f64 / 1_000.0)
    } else {
        format!("{duration_ms}ms")
    }
}

fn format_token_pair(input: Option<u64>, output: Option<u64>) -> String {
    format!(
        "{}→{}",
        input
            .map(format_compact_number)
            .unwrap_or_else(|| "—".into()),
        output
            .map(format_compact_number)
            .unwrap_or_else(|| "—".into())
    )
}

fn format_compact_number(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}m", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn format_duration(seconds: u64) -> String {
    format!(
        "{}d {:02}:{:02}:{:02}",
        seconds / 86_400,
        (seconds / 3_600) % 24,
        (seconds / 60) % 60,
        seconds % 60
    )
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DashboardRuntimeSnapshot {
    pub uptime_secs: u64,
    pub requests: u64,
    pub active: u64,
    pub successes: u64,
    pub failures: u64,
    pub tool_calls: u64,
    pub browser_tool_calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub retries: u64,
    pub failovers: u64,
    pub providers: Vec<DashboardProviderRuntimeSnapshot>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DashboardStorageEntry {
    pub label: String,
    pub path: String,
    pub size_bytes: Option<u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DashboardStorageSnapshot {
    pub entries: Vec<DashboardStorageEntry>,
}

impl DashboardStorageSnapshot {
    fn total_bytes(&self) -> u64 {
        self.entries
            .iter()
            .filter_map(|entry| entry.size_bytes)
            .sum()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DashboardSystemSnapshot {
    pub mode: String,
    pub data_auth: String,
    pub management_auth: String,
    pub allowed_origins: usize,
    pub max_request_bytes: usize,
    pub max_sse_event_bytes: usize,
    pub max_tool_argument_bytes: usize,
    pub stream_idle_timeout_secs: u64,
    pub rate_limit: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DashboardRequestEvent {
    pub method: String,
    pub path: String,
    pub status: u16,
    pub duration_ms: u64,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub retries: u64,
    pub failovers: u64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row,
        Scrollbar, ScrollbarOrientation, ScrollbarState, Table, Wrap,
    },
};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    autostart::{self, Status as AutoStartStatus},
    combo::{Combo, ComboModel, Strategy as ComboStrategy},
    desktop::DesktopTargets,
    local_config::{self, ProviderSummary},
    provider::{ModelInfo, Registry},
    sources::{SourceKind, SourceSelection},
    target_config::ProxyTarget,
};

#[derive(Clone, Debug)]
pub struct DashboardData {
    pub config_sources: Vec<String>,
    pub ide_targets: Vec<String>,
    pub listening: String,
    pub model_count: usize,
    pub provider_count: usize,
    pub autostart: AutoStartStatus,
    pub run_in_background: bool,
    pub proxy_targets: BTreeMap<ProxyTarget, bool>,
    pub detected_sources: BTreeMap<SourceKind, bool>,
    pub providers: Vec<ProviderSummary>,
    pub disabled_local_providers: BTreeSet<String>,
    pub combos: Vec<Combo>,
    pub models: Vec<ModelInfo>,
    pub disabled_models: BTreeSet<String>,
    pub subagent_catalog: crate::target_config::SubagentCatalogPolicy,
    pub port_warning: Option<String>,
    pub runtime: DashboardRuntimeSnapshot,
    pub storage: DashboardStorageSnapshot,
    pub system: DashboardSystemSnapshot,
    pub request_events: Vec<DashboardRequestEvent>,
}

fn page_screen(page: Page) -> Screen {
    match page {
        Page::Providers => Screen::Providers { selected: 0 },
        Page::Models => Screen::Models { selected: 0 },
        Page::Subagents => Screen::Subagents { selected: 0 },
        page => Screen::Base { page },
    }
}

impl Default for Screen {
    fn default() -> Self {
        Self::Base {
            page: Page::Overview,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Page {
    #[default]
    Overview,
    Providers,
    Models,
    Subagents,
    CodexSet,
    Logs,
    Usage,
    Storage,
    Integrations,
    System,
}

impl Page {
    const ALL: [Self; 10] = [
        Self::Overview,
        Self::Providers,
        Self::Models,
        Self::Subagents,
        Self::CodexSet,
        Self::Logs,
        Self::Usage,
        Self::Storage,
        Self::Integrations,
        Self::System,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Providers => "Providers",
            Self::Models => "Models",
            Self::Subagents => "Subagents",
            Self::CodexSet => "Codex Set",
            Self::Logs => "Logs",
            Self::Usage => "Usage",
            Self::Storage => "Storage",
            Self::Integrations => "Integrations",
            Self::System => "System",
        }
    }

    fn adjacent(self, forward: bool) -> Self {
        let index = Self::ALL
            .iter()
            .position(|page| *page == self)
            .unwrap_or_default();
        let next = if forward {
            (index + 1) % Self::ALL.len()
        } else {
            (index + Self::ALL.len() - 1) % Self::ALL.len()
        };
        Self::ALL[next]
    }

    fn from_number(character: char) -> Option<Self> {
        if character == '0' {
            return Some(Self::System);
        }
        character
            .to_digit(10)
            .and_then(|number| number.checked_sub(1))
            .and_then(|index| Self::ALL.get(index as usize))
            .copied()
    }
}

fn draw_provider_models(frame: &mut Frame<'_>, provider: &ProviderSummary, selected: usize) {
    let modal = draw_modal_shell(
        frame,
        "Default model",
        88,
        28,
        Line::from(vec![
            Span::styled("Enter", Style::default().fg(MODAL_ACCENT)),
            Span::raw(" set default    "),
            Span::styled("esc", Style::default().fg(Color::DarkGray)),
            Span::raw(" back"),
        ]),
    );
    let items = provider
        .models
        .iter()
        .enumerate()
        .map(|(index, model)| {
            let suffix = if provider.default_model.as_deref() == Some(model.as_str()) {
                "  default"
            } else {
                ""
            };
            ListItem::new(Line::from(vec![
                Span::styled(model, Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(suffix, Style::default().fg(Color::LightCyan)),
            ]))
            .style(selected_style(index == selected))
        })
        .collect::<Vec<_>>();
    let mut state = ListState::default().with_selected(Some(selected));
    frame.render_stateful_widget(
        List::new(items)
            .style(Style::default().fg(Color::Gray).bg(MODAL_BACKGROUND))
            .highlight_style(
                Style::default()
                    .fg(Color::White)
                    .bg(MODAL_ACCENT)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("● "),
        modal.content,
        &mut state,
    );
    draw_modal_scrollbar(frame, modal.content, provider.models.len(), selected);
}

// Named ANSI colours deliberately avoid true-colour escape sequences. Some
// terminals advertise colour support inconsistently and render RGB values as
// solid magenta/green surfaces. The ANSI palette remains readable across
// Terminal.app, iTerm2, Windows Terminal, tmux, SSH, and 16-colour terminals.
const MODAL_BACKGROUND: Color = Color::Reset;
const MODAL_OVERLAY: Color = Color::Reset;
const MODAL_ACCENT: Color = Color::LightBlue;
const PANEL_BACKGROUND: Color = Color::Reset;
const PANEL_BORDER: Color = Color::DarkGray;
const MUTED_TEXT: Color = Color::Gray;

#[derive(Clone, Copy)]
struct ModalAreas {
    content: Rect,
}

fn centered_rect(area: Rect, max_width: u16, max_height: u16) -> Rect {
    let width = area.width.saturating_sub(2).min(max_width).max(1);
    let height = area.height.saturating_sub(2).min(max_height).max(1);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn draw_modal_shell(
    frame: &mut Frame<'_>,
    title: &str,
    max_width: u16,
    max_height: u16,
    footer: Line<'static>,
) -> ModalAreas {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(MODAL_OVERLAY)),
        area,
    );

    let popup = centered_rect(area, max_width, max_height);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Block::default().style(Style::default().bg(MODAL_BACKGROUND)),
        popup,
    );

    let inner = popup.inner(Margin {
        horizontal: 3.min(popup.width.saturating_sub(1) / 2),
        vertical: 1.min(popup.height.saturating_sub(1) / 2),
    });
    let [header, content, footer_area] = Layout::vertical([
        Constraint::Length(2.min(inner.height)),
        Constraint::Min(1),
        Constraint::Length(2.min(inner.height)),
    ])
    .areas(inner);
    let [title_area, escape_area] =
        Layout::horizontal([Constraint::Min(1), Constraint::Length(3)]).areas(header);

    frame.render_widget(
        Paragraph::new(Span::styled(
            title,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        title_area,
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            "esc",
            Style::default()
                .fg(MODAL_ACCENT)
                .add_modifier(Modifier::BOLD),
        ))
        .alignment(Alignment::Right),
        escape_area,
    );
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().bg(MODAL_BACKGROUND)),
        footer_area,
    );

    ModalAreas { content }
}

fn draw_modal_scrollbar(frame: &mut Frame<'_>, area: Rect, content_length: usize, position: usize) {
    if content_length <= usize::from(area.height) {
        return;
    }

    let mut state = ScrollbarState::new(content_length).position(position);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_symbol("┃")
            .track_symbol(Some("│"))
            .begin_symbol(None)
            .end_symbol(None)
            .style(Style::default().fg(Color::DarkGray))
            .thumb_style(Style::default().fg(Color::Gray)),
        area,
        &mut state,
    );
}

fn header_animation_tick() -> usize {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    (elapsed.as_millis() / 100) as usize
}

const JOOCODE_LOGO: [&str; 5] = [
    r"       __                           __   ",
    r"      / /___  ____  _________  ____/ /__ ",
    r" __  / / __ \/ __ \/ ___/ __ \/ __  / _ \",
    r"/ /_/ / /_/ / /_/ / /__/ /_/ / /_/ /  __/",
    r"\____/\____/\____/\___/\____/\__,_/\___/",
];

const HEADER_RAINBOW: [Color; 7] = [
    Color::LightRed,
    Color::LightYellow,
    Color::Yellow,
    Color::LightGreen,
    Color::LightCyan,
    Color::LightBlue,
    Color::LightMagenta,
];

fn rainbow_logo_line(line: &str, row: usize, tick: usize) -> Line<'static> {
    Line::from(
        line.chars()
            .enumerate()
            .map(|(column, character)| {
                let phase = (column / 3 + row + HEADER_RAINBOW.len() - tick % HEADER_RAINBOW.len())
                    % HEADER_RAINBOW.len();
                Span::styled(
                    character.to_string(),
                    Style::default()
                        .fg(HEADER_RAINBOW[phase])
                        .add_modifier(Modifier::BOLD),
                )
            })
            .collect::<Vec<_>>(),
    )
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, tick: usize) {
    if area.width < 52 || area.height < 7 {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled(
                        "Joocode",
                        Style::default()
                            .fg(HEADER_RAINBOW[tick % HEADER_RAINBOW.len()])
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  LOCAL AI ROUTER", Style::default().fg(Color::LightCyan)),
                ]),
                Line::from(vec![
                    Span::styled(
                        format!("v{}", env!("CARGO_PKG_VERSION")),
                        Style::default().fg(Color::LightYellow),
                    ),
                    Span::styled("  •  ", Style::default().fg(Color::DarkGray)),
                    Span::styled("ONLINE", Style::default().fg(Color::LightGreen)),
                ]),
            ])
            .block(
                Block::default()
                    .borders(Borders::BOTTOM)
                    .border_style(Style::default().fg(Color::DarkGray)),
            ),
            area,
        );
        return;
    }

    let logo_width = JOOCODE_LOGO
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or_default() as u16;
    let [logo_area, status_area] = Layout::horizontal([
        Constraint::Length(logo_width.min(area.width)),
        Constraint::Min(0),
    ])
    .areas(area);

    frame.render_widget(
        Paragraph::new(
            JOOCODE_LOGO
                .iter()
                .enumerate()
                .map(|(row, line)| rainbow_logo_line(line, row, tick))
                .collect::<Vec<_>>(),
        )
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(Color::DarkGray)),
        ),
        logo_area,
    );

    if status_area.width >= 18 {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(""),
                Line::from(Span::styled(
                    " LOCAL AI ROUTER ",
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::LightCyan)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(vec![
                    Span::styled(
                        format!("v{}", env!("CARGO_PKG_VERSION")),
                        Style::default()
                            .fg(Color::LightYellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  •  ", Style::default().fg(Color::DarkGray)),
                    Span::styled("ONLINE", Style::default().fg(Color::LightGreen)),
                ]),
            ])
            .block(
                Block::default()
                    .borders(Borders::BOTTOM)
                    .border_style(Style::default().fg(Color::DarkGray)),
            ),
            status_area,
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProviderRow {
    Source(SourceKind),
    Local(usize),
    Combo(usize),
}

const COMBO_STRATEGIES: [ComboStrategy; 3] = [
    ComboStrategy::Failover,
    ComboStrategy::WeightedRoundRobin,
    ComboStrategy::LowestLatency,
];

fn combo_strategy_label(strategy: ComboStrategy) -> &'static str {
    match strategy {
        ComboStrategy::Failover => "Failover",
        ComboStrategy::WeightedRoundRobin => "Weighted round-robin",
        ComboStrategy::LowestLatency => "Lowest latency",
    }
}

fn provider_rows(data: &DashboardData) -> Vec<ProviderRow> {
    SourceKind::DETECTED
        .into_iter()
        .map(ProviderRow::Source)
        .chain((0..data.providers.len()).map(ProviderRow::Local))
        .chain((0..data.combos.len()).map(ProviderRow::Combo))
        .collect()
}

fn status_marker(enabled: bool) -> Span<'static> {
    Span::styled(
        if enabled { "● " } else { "○ " },
        Style::default().fg(if enabled {
            Color::Green
        } else {
            Color::DarkGray
        }),
    )
}

fn provider_section(title: &'static str) -> ListItem<'static> {
    ListItem::new(Span::styled(
        title,
        Style::default()
            .fg(Color::LightCyan)
            .add_modifier(Modifier::BOLD),
    ))
}

fn draw_providers_page(frame: &mut Frame<'_>, area: Rect, data: &DashboardData, selected: usize) {
    let panel_area = area.inner(Margin::new(1, 1));
    let panel = dashboard_panel(
        "Providers — Space toggle · Enter manage · n new · c combo · Del remove",
        Color::LightCyan,
    );
    let inner = panel.inner(panel_area);
    frame.render_widget(panel, panel_area);
    let rows = provider_rows(data);
    let selected = selected.min(rows.len().saturating_sub(1));
    let mut items = vec![provider_section("DETECTED PROVIDERS")];
    let mut selected_row = 1;
    let mut logical = 0;

    for source in SourceKind::DETECTED {
        let enabled = data.detected_sources.get(&source).copied().unwrap_or(false);
        if logical == selected {
            selected_row = items.len();
        }
        items.push(
            ListItem::new(Line::from(vec![
                status_marker(enabled),
                Span::styled(
                    source.label(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled("  auto-discovered source", Style::default().fg(MUTED_TEXT)),
            ]))
            .style(selected_style(logical == selected)),
        );
        logical += 1;
    }

    items.extend([ListItem::new(""), provider_section("CUSTOM PROVIDERS")]);
    if data.providers.is_empty() {
        items.push(ListItem::new(Span::styled(
            "  No custom providers. Press n to connect one.",
            Style::default().fg(MUTED_TEXT),
        )));
    }
    for provider in &data.providers {
        let enabled = !data.disabled_local_providers.contains(&provider.name);
        if logical == selected {
            selected_row = items.len();
        }
        items.push(
            ListItem::new(Line::from(vec![
                status_marker(enabled),
                Span::styled(
                    provider.label.clone(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(
                        "  {} models · {} key(s)",
                        provider.model_count, provider.key_count
                    ),
                    Style::default().fg(MUTED_TEXT),
                ),
            ]))
            .style(selected_style(logical == selected)),
        );
        logical += 1;
    }

    items.extend([ListItem::new(""), provider_section("COMBOS")]);
    if data.combos.is_empty() {
        items.push(ListItem::new(Span::styled(
            "  No combos. Press c to build one.",
            Style::default().fg(MUTED_TEXT),
        )));
    }
    for combo in &data.combos {
        let enabled = !data
            .disabled_models
            .contains(&format!("combo/{}", combo.name));
        if logical == selected {
            selected_row = items.len();
        }
        items.push(
            ListItem::new(Line::from(vec![
                status_marker(enabled),
                Span::styled(
                    format!("combo/{}", combo.name),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(
                        "  {} · {} model(s)",
                        combo_strategy_label(combo.strategy),
                        combo.models.len()
                    ),
                    Style::default().fg(MUTED_TEXT),
                ),
            ]))
            .style(selected_style(logical == selected)),
        );
        logical += 1;
    }

    let mut state = ListState::default().with_selected((!rows.is_empty()).then_some(selected_row));
    frame.render_stateful_widget(List::new(items).highlight_symbol("› "), inner, &mut state);
    draw_modal_scrollbar(frame, inner, logical + 5, selected_row);
}

fn draw_combo_name(frame: &mut Frame<'_>, value: &str) {
    draw_provider_input(frame, "Step 1/4 — Combo name", value, false);
}

fn draw_combo_strategy(frame: &mut Frame<'_>, selected: usize) {
    let modal = draw_modal_shell(
        frame,
        "Combo · Step 2/4 — Strategy",
        78,
        16,
        Line::from("Enter continue   ↑/↓ select"),
    );
    let items = COMBO_STRATEGIES
        .iter()
        .enumerate()
        .map(|(index, strategy)| {
            ListItem::new(combo_strategy_label(*strategy)).style(selected_style(index == selected))
        })
        .collect::<Vec<_>>();
    let mut state = ListState::default().with_selected(Some(selected));
    frame.render_stateful_widget(
        List::new(items).highlight_symbol("● "),
        modal.content,
        &mut state,
    );
}

fn draw_combo_models(
    frame: &mut Frame<'_>,
    data: &DashboardData,
    strategy: ComboStrategy,
    selected: usize,
    chosen: &[(String, u32)],
) {
    let modal = draw_modal_shell(
        frame,
        "Combo · Step 3/4 — Models",
        104,
        32,
        Line::from("Space toggle   ←/→ weight/order   Enter review"),
    );
    let models = data
        .models
        .iter()
        .filter(|model| !model.id.starts_with("combo/"))
        .collect::<Vec<_>>();
    let items = models
        .iter()
        .enumerate()
        .map(|(index, model)| {
            let chosen_index = chosen.iter().position(|(id, _)| id == &model.id);
            let detail = chosen_index.map_or_else(String::new, |position| {
                if strategy == ComboStrategy::WeightedRoundRobin {
                    format!("  weight {}", chosen[position].1)
                } else {
                    format!("  order {}", position + 1)
                }
            });
            ListItem::new(Line::from(vec![
                status_marker(chosen_index.is_some()),
                Span::raw(model.id.clone()),
                Span::styled(detail, Style::default().fg(Color::LightCyan)),
            ]))
            .style(selected_style(index == selected))
        })
        .collect::<Vec<_>>();
    let mut state = ListState::default().with_selected(Some(selected));
    frame.render_stateful_widget(
        List::new(items).highlight_symbol("› "),
        modal.content,
        &mut state,
    );
    draw_modal_scrollbar(frame, modal.content, models.len(), selected);
}

fn draw_combo_review(
    frame: &mut Frame<'_>,
    name: &str,
    strategy: ComboStrategy,
    models: &[(String, u32)],
) {
    let modal = draw_modal_shell(
        frame,
        "Combo · Step 4/4 — Review",
        92,
        28,
        Line::from("Enter save   esc back"),
    );
    let mut lines = vec![
        Line::from(format!("Model ID   combo/{name}")),
        Line::from(format!("Strategy   {}", combo_strategy_label(strategy))),
        Line::from(""),
    ];
    lines.extend(models.iter().enumerate().map(|(index, (model, weight))| {
        Line::from(format!(
            "{:>2}. {}{}",
            index + 1,
            model,
            if strategy == ComboStrategy::WeightedRoundRobin {
                format!("  ×{weight}")
            } else {
                String::new()
            }
        ))
    }));
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }),
        modal.content,
    );
}

fn draw_provider_input(frame: &mut Frame<'_>, title: &str, value: &str, secret: bool) {
    let modal = draw_modal_shell(
        frame,
        "New provider",
        76,
        13,
        Line::from(vec![
            Span::styled("Enter", Style::default().fg(MODAL_ACCENT)),
            Span::raw(" continue    "),
            Span::styled("esc", Style::default().fg(Color::DarkGray)),
            Span::raw(" cancel"),
        ]),
    );
    let shown = if secret {
        "•".repeat(value.chars().count())
    } else {
        value.to_owned()
    };
    let [label, input] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).areas(modal.content);
    frame.render_widget(
        Paragraph::new(Span::styled(
            title,
            Style::default()
                .fg(MODAL_ACCENT)
                .add_modifier(Modifier::BOLD),
        )),
        label,
    );
    frame.render_widget(
        Paragraph::new(format!("▌{shown}"))
            .style(Style::default().fg(Color::White).bg(MODAL_BACKGROUND))
            .wrap(Wrap { trim: false }),
        input,
    );
}

fn draw_provider_loading(frame: &mut Frame<'_>) {
    let modal = draw_modal_shell(
        frame,
        "New provider",
        68,
        12,
        Line::from(Span::styled(
            "Fetching /models…",
            Style::default().fg(Color::DarkGray),
        )),
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "◐  Connecting provider",
                Style::default()
                    .fg(MODAL_ACCENT)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Fetching /models and reloading Joocode…",
                Style::default().fg(Color::Gray),
            )),
        ])
        .alignment(Alignment::Center)
        .style(Style::default().bg(MODAL_BACKGROUND))
        .wrap(Wrap { trim: true }),
        modal.content,
    );
}

fn selected_style(selected: bool) -> Style {
    if selected {
        Style::default()
            .fg(Color::White)
            .bg(MODAL_ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray).bg(MODAL_BACKGROUND)
    }
}

fn draw_update_animation(frame: &mut Frame<'_>, tag: &str, tick: usize) {
    let area = frame.area();
    frame.render_widget(Clear, area);

    let palette = [
        Color::LightRed,
        Color::LightYellow,
        Color::Yellow,
        Color::LightGreen,
        Color::LightCyan,
        Color::LightMagenta,
    ];
    let sparkles = [' ', ' ', ' ', '·', '✦', '⋆'];
    let background = (0..area.height)
        .map(|row| {
            let spans = (0..area.width)
                .map(|column| {
                    let wave = usize::from(column) / 6 + usize::from(row) / 2 + tick / 2;
                    let color = palette[wave % palette.len()];
                    let seed =
                        usize::from(column) * 19 + usize::from(row) * 29 + tick.saturating_mul(11);
                    Span::styled(
                        sparkles[seed % sparkles.len()].to_string(),
                        Style::default().fg(Color::White).bg(color),
                    )
                })
                .collect::<Vec<_>>();
            Line::from(spans)
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(background), area);

    let width = area.width.saturating_sub(4).min(74);
    let height = 11_u16.min(area.height.saturating_sub(2));
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let spinner = ["◐", "◓", "◑", "◒"][tick % 4];
    let bar_width = usize::from(width.saturating_sub(12)).max(8);
    let shimmer = tick % bar_width;
    let progress = (0..bar_width)
        .map(|index| {
            if index.abs_diff(shimmer) <= 2 {
                '◆'
            } else {
                '─'
            }
        })
        .collect::<String>();

    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("{spinner}  Upgrading Joocode to {tag}"),
                Style::default()
                    .fg(palette[tick % palette.len()])
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                progress,
                Style::default()
                    .fg(Color::LightCyan)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Downloading · verifying · installing",
                Style::default().fg(Color::White),
            )),
            Line::from(Span::styled(
                "Joocode will restart automatically",
                Style::default().fg(Color::Gray),
            )),
        ])
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .title(" ✦ RAINBOW UPGRADE ✦ ")
                .title_alignment(Alignment::Center)
                .title_style(
                    Style::default()
                        .fg(Color::LightMagenta)
                        .add_modifier(Modifier::BOLD),
                )
                .style(Style::default().bg(Color::Black))
                .borders(Borders::ALL),
        ),
        popup,
    );
}

fn draw_update_prompt(frame: &mut Frame<'_>, tag: &str) {
    let modal = draw_modal_shell(
        frame,
        "Joocode update",
        76,
        14,
        Line::from(vec![
            Span::styled("Enter", Style::default().fg(MODAL_ACCENT)),
            Span::raw(" update & restart"),
        ]),
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "New version available",
                Style::default()
                    .fg(MODAL_ACCENT)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled(tag, Style::default().fg(Color::White)),
                Span::styled(" is ready to install.", Style::default().fg(Color::Gray)),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "Joocode will restart immediately after the update.",
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .style(Style::default().bg(MODAL_BACKGROUND))
        .wrap(Wrap { trim: true }),
        modal.content,
    );
}

const AUTO_START_ITEM: usize = 0;
const RUN_IN_BACKGROUND_ITEM: usize = 1;
const FIRST_PROXY_ITEM: usize = 2;

fn config_items() -> Vec<usize> {
    (AUTO_START_ITEM..FIRST_PROXY_ITEM + ProxyTarget::ALL.len()).collect()
}

fn target_for_config_item(item: usize) -> Option<ProxyTarget> {
    item.checked_sub(FIRST_PROXY_ITEM)
        .and_then(|index| ProxyTarget::ALL.get(index))
        .copied()
}

fn adjacent_config_item(selected: usize, forward: bool) -> usize {
    let items = config_items();
    let Some(index) = items.iter().position(|item| *item == selected) else {
        return items.first().copied().unwrap_or_default();
    };
    let adjacent = if forward {
        index.checked_add(1)
    } else {
        index.checked_sub(1)
    };
    adjacent
        .and_then(|index| items.get(index))
        .copied()
        .unwrap_or(selected)
}

fn config_row_for_item(item: usize) -> usize {
    if item == AUTO_START_ITEM {
        return 1;
    }
    if item == RUN_IN_BACKGROUND_ITEM {
        return 2;
    }
    item.checked_sub(FIRST_PROXY_ITEM)
        .map(|index| 5 + index)
        .unwrap_or(1)
}

fn draw_config(frame: &mut Frame<'_>, data: &DashboardData, selected: usize) {
    let modal = draw_modal_shell(
        frame,
        "Configuration",
        86,
        30,
        Line::from(vec![
            Span::styled("Space", Style::default().fg(MODAL_ACCENT)),
            Span::raw(" toggle    "),
            Span::styled("↑/↓", Style::default().fg(MODAL_ACCENT)),
            Span::raw(" navigate"),
        ]),
    );

    let marker = if data.autostart.enabled() {
        "●"
    } else {
        "○"
    };
    let auto_start = Line::from(vec![
        Span::styled(
            format!("{marker} "),
            Style::default().fg(if data.autostart.enabled() {
                Color::Green
            } else {
                Color::DarkGray
            }),
        ),
        Span::raw("Auto-start after login/restart"),
        Span::raw(format!(" ({})", data.autostart.label())),
    ]);
    let background_marker = if data.run_in_background { "●" } else { "○" };
    let run_in_background = Line::from(vec![
        Span::styled(
            format!("{background_marker} "),
            Style::default().fg(if data.run_in_background {
                Color::Green
            } else {
                Color::DarkGray
            }),
        ),
        Span::raw("Run in background"),
        Span::raw(format!(
            " ({})",
            if data.run_in_background { "On" } else { "Off" }
        )),
    ]);
    let mut items = vec![
        ListItem::new(Line::from(Span::styled(
            "Setting",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))),
        ListItem::new(auto_start).style(selected_style(selected == AUTO_START_ITEM)),
        ListItem::new(run_in_background).style(selected_style(selected == RUN_IN_BACKGROUND_ITEM)),
    ];
    items.extend([
        ListItem::new(Line::from("")),
        ListItem::new(Line::from(Span::styled(
            "Proxy to",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ))),
    ]);
    for (index, target) in ProxyTarget::ALL.into_iter().enumerate() {
        let enabled = data.proxy_targets.get(&target).copied().unwrap_or(false);
        let marker = if enabled { "●" } else { "○" };
        let mut spans = vec![
            Span::styled(
                format!("{marker} "),
                Style::default().fg(if enabled {
                    Color::Green
                } else {
                    Color::DarkGray
                }),
            ),
            Span::raw(target.label()),
            Span::raw(format!(" ({})", if enabled { "On" } else { "Off" })),
        ];
        if let Some(note) = target.support_note() {
            spans.push(Span::styled(
                format!(" · {note}"),
                Style::default().fg(Color::Yellow),
            ));
        }
        items.push(
            ListItem::new(Line::from(spans))
                .style(selected_style(selected == FIRST_PROXY_ITEM + index)),
        );
    }

    let mut state = ListState::default().with_selected(Some(config_row_for_item(selected)));
    frame.render_stateful_widget(
        List::new(items)
            .style(Style::default().fg(Color::Gray).bg(MODAL_BACKGROUND))
            .highlight_style(
                Style::default()
                    .fg(Color::White)
                    .bg(MODAL_ACCENT)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("● "),
        modal.content,
        &mut state,
    );
    draw_modal_scrollbar(
        frame,
        modal.content,
        7 + ProxyTarget::ALL.len(),
        config_row_for_item(selected),
    );
}

fn draw_error(frame: &mut Frame<'_>, error: &str) {
    let modal = draw_modal_shell(
        frame,
        "Something went wrong",
        76,
        16,
        Line::from(Span::styled(
            "Enter return",
            Style::default().fg(MODAL_ACCENT),
        )),
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "Error",
                Style::default()
                    .fg(Color::LightRed)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(error, Style::default().fg(Color::Gray))),
        ])
        .style(Style::default().bg(MODAL_BACKGROUND))
        .wrap(Wrap { trim: true }),
        modal.content,
    );
}

fn draw_easter_egg(frame: &mut Frame<'_>, tick: usize) {
    let area = frame.area();
    frame.render_widget(Clear, area);

    let palette = [
        Color::LightRed,
        Color::LightYellow,
        Color::Yellow,
        Color::LightGreen,
        Color::LightCyan,
        Color::LightMagenta,
    ];
    let rain = ['│', '╎', '✦', '·'];
    let background = (0..area.height)
        .map(|row| {
            let spans = (0..area.width)
                .map(|column| {
                    let color_index =
                        (usize::from(column) / 5 + usize::from(row) / 2 + tick / 2) % palette.len();
                    let seed =
                        usize::from(column) * 17 + usize::from(row) * 31 + tick.saturating_mul(7);
                    let character = if seed.is_multiple_of(47) {
                        rain[(seed / 47) % rain.len()]
                    } else {
                        ' '
                    };
                    Span::styled(
                        character.to_string(),
                        Style::default().fg(Color::White).bg(palette[color_index]),
                    )
                })
                .collect::<Vec<_>>();
            Line::from(spans)
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(background), area);

    let sprite_width = 27_u16.min(area.width);
    let travel = usize::from(area.width.saturating_add(sprite_width));
    let sprite_offset = if travel == 0 { 0 } else { tick % travel };
    let sprite_x = i32::try_from(sprite_offset).unwrap_or_default() - i32::from(sprite_width);
    if sprite_x < i32::from(area.width) {
        let clipped_x = sprite_x.max(0) as u16;
        let clipped_width = sprite_width
            .saturating_sub(sprite_x.unsigned_abs() as u16 * u16::from(sprite_x < 0))
            .min(area.width.saturating_sub(clipped_x));
        if clipped_width > 0 && area.height >= 8 {
            let sprite = Rect::new(
                area.x + clipped_x,
                area.y + area.height.saturating_sub(8) / 2,
                clipped_width,
                7,
            );
            let art = vec![
                Line::from("          \\ | /"),
                Line::from("       ---\\|/---"),
                Line::from("  ≋≋≋≋  .-^^^^^-.   "),
                Line::from(" ≋≋≋≋  /  o   o  \\  "),
                Line::from("≋≋≋≋  (     ^     )  "),
                Line::from(" ≋≋≋≋  \\  '---'  /  "),
                Line::from("  ≋≋≋≋  '-.___.-'   "),
            ];
            frame.render_widget(
                Paragraph::new(art)
                    .style(
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    )
                    .alignment(Alignment::Left),
                sprite,
            );
        }
    }

    let message_width = area.width.saturating_sub(4).min(72);
    let message_height = 9_u16.min(area.height.saturating_sub(2));
    let message_area = Rect::new(
        area.x + area.width.saturating_sub(message_width) / 2,
        area.y + area.height.saturating_sub(message_height) / 2,
        message_width,
        message_height,
    );
    frame.render_widget(Clear, message_area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                EASTER_EGG_MESSAGE,
                Style::default()
                    .fg(palette[tick % palette.len()])
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "🌴  rain, rainbow, repeat  🦀",
                Style::default().fg(Color::LightCyan),
            )),
        ])
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .title(" ✦ JOOCODE SECRET MODE ✦ ")
                .title_alignment(Alignment::Center)
                .title_style(
                    Style::default()
                        .fg(Color::LightMagenta)
                        .add_modifier(Modifier::BOLD),
                )
                .style(Style::default().bg(Color::Black))
                .borders(Borders::ALL),
        )
        .wrap(Wrap { trim: true }),
        message_area,
    );
}

fn detect_easter_egg(screen: &mut Screen, input: &mut String, key: KeyCode) -> bool {
    if !matches!(
        screen,
        Screen::Base { .. } | Screen::Models { .. } | Screen::Subagents { .. }
    ) {
        input.clear();
        return false;
    }

    let KeyCode::Char(character) = key else {
        if key != KeyCode::Backspace {
            input.clear();
        }
        return false;
    };
    if !character.is_ascii_alphabetic() {
        input.clear();
        return false;
    }

    input.push(character.to_ascii_lowercase());
    const MAX_TRIGGER_LENGTH: usize = 7;
    if input.len() > MAX_TRIGGER_LENGTH {
        input.drain(..input.len() - MAX_TRIGGER_LENGTH);
    }

    if EASTER_EGG_TRIGGERS
        .iter()
        .any(|trigger| input.ends_with(trigger))
    {
        input.clear();
        *screen = Screen::EasterEgg { tick: 0 };
        return true;
    }

    false
}

fn handle_paste(screen: &mut Screen, value: &str) {
    let value = value.replace(['\r', '\n'], "");
    match screen {
        Screen::ProviderBaseUrl {
            value: base_url, ..
        } => base_url.push_str(&value),
        Screen::ProviderApiKey { api_key, .. } => api_key.push_str(&value),
        Screen::ProviderPoolKey { api_key, .. } => api_key.push_str(&value),
        Screen::ComboName { value: name, .. } => name.push_str(&value),
        _ => {}
    }
}

impl DashboardData {
    pub fn new(
        registry: &Registry,
        targets: &DesktopTargets,
        selection: &SourceSelection,
        address: SocketAddr,
        port_warning: Option<String>,
    ) -> Self {
        let preferences = crate::target_config::TargetPreferences::load().unwrap_or_default();
        let mut models = registry.models().to_vec();
        for id in &preferences.disabled_models {
            if !models.iter().any(|model| &model.id == id) {
                let (provider, upstream_id) = id.split_once('/').unwrap_or(("disabled", id));
                models.push(ModelInfo {
                    id: id.clone(),
                    provider: provider.to_owned(),
                    upstream_id: upstream_id.to_owned(),
                    name: format!("{upstream_id} (disabled)"),
                    reasoning: false,
                    context_window: None,
                    max_output_tokens: None,
                });
            }
        }
        models.sort_by(|left, right| left.id.cmp(&right.id));
        Self {
            config_sources: config_sources(registry),
            ide_targets: targets.names().into_iter().map(str::to_owned).collect(),
            listening: format!("http://{address}"),
            model_count: registry.models().len(),
            provider_count: registry.provider_count(),
            autostart: autostart::status(),
            run_in_background: preferences.run_in_background,
            proxy_targets: ProxyTarget::ALL
                .into_iter()
                .map(|target| (target, targets.enabled(target)))
                .collect(),
            detected_sources: SourceKind::DETECTED
                .into_iter()
                .map(|source| (source, selection.enabled(source)))
                .collect(),
            providers: local_config::summaries().unwrap_or_default(),
            disabled_local_providers: preferences.disabled_local_providers,
            combos: crate::combo::load().unwrap_or_default(),
            models,
            disabled_models: preferences.disabled_models,
            subagent_catalog: preferences.subagent_catalog,
            port_warning,
            runtime: DashboardRuntimeSnapshot::default(),
            storage: DashboardStorageSnapshot::default(),
            system: DashboardSystemSnapshot::default(),
            request_events: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub enum DashboardCommand {
    AddProvider {
        base_url: String,
        api_key: String,
    },
    AddProviderKey {
        provider: String,
        api_key: String,
    },
    RemoveProviderKey {
        provider: String,
    },
    RemoveProvider {
        name: String,
    },
    SetDefaultModel {
        provider: String,
        model: String,
    },
    ToggleAutoStart,
    ToggleRunInBackground,
    ToggleSource {
        source: SourceKind,
    },
    ToggleLocalProvider {
        provider: String,
    },
    SaveCombo {
        original_name: Option<String>,
        combo: Combo,
    },
    RemoveCombo {
        name: String,
    },
    ToggleProxyTarget {
        target: ProxyTarget,
    },
    ToggleModel {
        model: String,
    },
    ToggleSubagentFeatured {
        model: String,
    },
    ToggleSubagentFallback {
        model: String,
    },
    AdjustSubagentMaxEntries {
        delta: i8,
    },
    CycleReasoningEffortCap,
    TestProvider {
        name: String,
    },
    SyncCodex,
    InstallUpdate {
        tag: String,
    },
}

#[derive(Debug)]
pub enum DashboardEvent {
    ObservabilityUpdated {
        runtime: DashboardRuntimeSnapshot,
        storage: DashboardStorageSnapshot,
        request_events: Vec<DashboardRequestEvent>,
    },
    ProviderAdded {
        provider: String,
        config_sources: Vec<String>,
        model_count: usize,
        provider_count: usize,
        providers: Vec<ProviderSummary>,
    },
    ProviderDefaultUpdated {
        provider: String,
        providers: Vec<ProviderSummary>,
    },
    ProviderRemoved {
        config_sources: Vec<String>,
        model_count: usize,
        provider_count: usize,
        providers: Vec<ProviderSummary>,
    },
    ProviderError(String),
    AutoStartUpdated(AutoStartStatus),
    RunInBackgroundUpdated(bool),
    ProviderControlsUpdated {
        config_sources: Vec<String>,
        model_count: usize,
        provider_count: usize,
        models: Vec<ModelInfo>,
        disabled_local_providers: BTreeSet<String>,
        combos: Vec<Combo>,
        detected_sources: BTreeMap<SourceKind, bool>,
    },
    ProxyTargetUpdated {
        target: ProxyTarget,
        enabled: bool,
    },
    CatalogUpdated {
        models: Vec<ModelInfo>,
        model_count: usize,
        provider_count: usize,
        config_sources: Vec<String>,
        disabled_models: BTreeSet<String>,
        subagent_catalog: crate::target_config::SubagentCatalogPolicy,
    },
    ProviderTested(String),
    UpdateAvailable(String),
    UpdateInstalled,
    ShutdownRequested,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DashboardExit {
    Quit,
    Restart,
}

enum Screen {
    Base {
        page: Page,
    },
    Models {
        selected: usize,
    },
    Subagents {
        selected: usize,
    },
    Config {
        selected: usize,
    },
    Providers {
        selected: usize,
    },
    ProviderModels {
        provider_selected: usize,
        model_selected: usize,
    },
    ProviderBaseUrl {
        selected: usize,
        value: String,
    },
    ProviderApiKey {
        selected: usize,
        base_url: String,
        api_key: String,
    },
    ProviderPoolKey {
        selected: usize,
        api_key: String,
    },
    ProviderLoading {
        selected: usize,
    },
    ComboName {
        original_name: Option<String>,
        value: String,
    },
    ComboStrategy {
        original_name: Option<String>,
        name: String,
        selected: usize,
        models: Vec<(String, u32)>,
    },
    ComboModels {
        original_name: Option<String>,
        name: String,
        strategy: ComboStrategy,
        selected: usize,
        models: Vec<(String, u32)>,
    },
    ComboReview {
        original_name: Option<String>,
        name: String,
        strategy: ComboStrategy,
        models: Vec<(String, u32)>,
    },
    Success(String),
    Error(String),
    UpdateAvailable(String),
    Updating {
        tag: String,
        tick: usize,
    },
    EasterEgg {
        tick: usize,
    },
}

const EASTER_EGG_TRIGGERS: [&str; 3] = ["jokowi", "sawit", "prabowo"];
const EASTER_EGG_MESSAGE: &str = "You really love the regime, huh? Hahaha. Keep planting palm oil!";

pub fn config_sources(registry: &Registry) -> Vec<String> {
    registry
        .source_reports()
        .iter()
        .filter(|report| report.status == "loaded")
        .map(|report| display_source(&report.source))
        .collect()
}

pub fn is_interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

pub fn run(
    mut data: DashboardData,
    command_tx: UnboundedSender<DashboardCommand>,
    event_rx: Receiver<DashboardEvent>,
) -> anyhow::Result<DashboardExit> {
    let mut screen = Screen::default();
    let mut secret_input = String::new();
    let exit = ratatui::run(|terminal| {
        loop {
            if let Some(exit) = receive_events(&mut data, &mut screen, &event_rx) {
                return Ok::<DashboardExit, std::io::Error>(exit);
            }
            if let Screen::EasterEgg { tick } | Screen::Updating { tick, .. } = &mut screen {
                *tick = tick.wrapping_add(1);
            }
            terminal.draw(|frame| draw(frame, &data, &screen))?;
            let poll_interval =
                if matches!(screen, Screen::EasterEgg { .. } | Screen::Updating { .. }) {
                    Duration::from_millis(80)
                } else {
                    Duration::from_millis(100)
                };
            if !event::poll(poll_interval)? {
                continue;
            }
            let input = event::read()?;
            if let Event::Paste(value) = input {
                handle_paste(&mut screen, &value);
                continue;
            }
            let Event::Key(key) = input else { continue };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if key.code == KeyCode::Esc {
                if matches!(screen, Screen::Updating { .. }) {
                    continue;
                }
                if matches!(screen, Screen::Base { .. }) {
                    return Ok::<DashboardExit, std::io::Error>(DashboardExit::Quit);
                }
                screen = match screen {
                    Screen::ProviderBaseUrl { selected, .. }
                    | Screen::ProviderApiKey { selected, .. } => Screen::Providers { selected },
                    Screen::ProviderPoolKey { selected, .. } => Screen::Providers {
                        selected: SourceKind::DETECTED.len() + selected,
                    },
                    Screen::ProviderModels {
                        provider_selected, ..
                    } => Screen::Providers {
                        selected: SourceKind::DETECTED.len() + provider_selected,
                    },
                    Screen::Providers { .. } => return Ok(DashboardExit::Quit),
                    Screen::ComboName { .. }
                    | Screen::ComboStrategy { .. }
                    | Screen::ComboModels { .. }
                    | Screen::ComboReview { .. } => Screen::Providers { selected: 0 },
                    Screen::Models { .. } => Screen::Base { page: Page::Models },
                    Screen::Subagents { .. } => Screen::Base {
                        page: Page::Subagents,
                    },
                    Screen::Config { .. } => Screen::Base {
                        page: Page::Integrations,
                    },
                    _ => Screen::default(),
                };
                continue;
            }
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                return Ok(DashboardExit::Quit);
            }
            if detect_easter_egg(&mut screen, &mut secret_input, key.code) {
                continue;
            }
            handle_key_with_data(&mut screen, key.code, &command_tx, &data);
        }
    })?;
    Ok(exit)
}

fn receive_events(
    data: &mut DashboardData,
    screen: &mut Screen,
    event_rx: &Receiver<DashboardEvent>,
) -> Option<DashboardExit> {
    loop {
        match event_rx.try_recv() {
            Ok(DashboardEvent::ObservabilityUpdated {
                runtime,
                storage,
                request_events,
            }) => {
                data.runtime = runtime;
                data.storage = storage;
                data.request_events = request_events;
            }
            Ok(DashboardEvent::ProviderAdded {
                provider,
                config_sources,
                model_count,
                provider_count,
                providers,
            }) => {
                data.config_sources = config_sources;
                data.model_count = model_count;
                data.provider_count = provider_count;
                data.providers = providers;
                data.combos = crate::combo::load().unwrap_or_default();
                data.disabled_local_providers = crate::target_config::TargetPreferences::load()
                    .unwrap_or_default()
                    .disabled_local_providers;
                let selected = data
                    .providers
                    .iter()
                    .position(|entry| entry.name == provider)
                    .map(|index| SourceKind::DETECTED.len() + index)
                    .unwrap_or_default();
                *screen = Screen::Providers { selected };
            }
            Ok(DashboardEvent::ProviderRemoved {
                config_sources,
                model_count,
                provider_count,
                providers,
            }) => {
                data.config_sources = config_sources;
                data.model_count = model_count;
                data.provider_count = provider_count;
                data.providers = providers;
                let selected = match screen {
                    Screen::Providers { selected }
                    | Screen::ProviderBaseUrl { selected, .. }
                    | Screen::ProviderApiKey { selected, .. }
                    | Screen::ProviderPoolKey { selected, .. } => *selected,
                    _ => 0,
                };
                *screen = Screen::Providers {
                    selected: selected.min(data.providers.len().saturating_sub(1)),
                };
            }
            Ok(DashboardEvent::ProviderDefaultUpdated {
                provider,
                providers,
            }) => {
                data.providers = providers;
                let selected = data
                    .providers
                    .iter()
                    .position(|entry| entry.name == provider)
                    .map(|index| SourceKind::DETECTED.len() + index)
                    .unwrap_or_default();
                *screen = Screen::Providers { selected };
            }
            Ok(DashboardEvent::ProviderError(error)) => *screen = Screen::Error(error),
            Ok(DashboardEvent::AutoStartUpdated(status)) => data.autostart = status,
            Ok(DashboardEvent::RunInBackgroundUpdated(enabled)) => {
                data.run_in_background = enabled;
            }
            Ok(DashboardEvent::ProviderControlsUpdated {
                config_sources,
                model_count,
                provider_count,
                models,
                disabled_local_providers,
                combos,
                detected_sources,
            }) => {
                data.config_sources = config_sources;
                data.model_count = model_count;
                data.provider_count = provider_count;
                data.models = models;
                data.disabled_local_providers = disabled_local_providers;
                data.combos = combos;
                data.detected_sources = detected_sources;
                let selected = match screen {
                    Screen::Providers { selected } => *selected,
                    _ => 0,
                };
                *screen = Screen::Providers {
                    selected: selected.min(provider_rows(data).len().saturating_sub(1)),
                };
            }
            Ok(DashboardEvent::ProxyTargetUpdated { target, enabled }) => {
                data.proxy_targets.insert(target, enabled);
                data.ide_targets = ProxyTarget::ALL
                    .into_iter()
                    .filter(|target| data.proxy_targets.get(target).copied().unwrap_or(false))
                    .map(|target| target.label().to_owned())
                    .collect();
            }
            Ok(DashboardEvent::CatalogUpdated {
                models,
                model_count,
                provider_count,
                config_sources,
                disabled_models,
                subagent_catalog,
            }) => {
                let old_models = std::mem::take(&mut data.models);
                data.models = models;
                for model in old_models {
                    if disabled_models.contains(&model.id)
                        && !data.models.iter().any(|candidate| candidate.id == model.id)
                    {
                        data.models.push(model);
                    }
                }
                data.models.sort_by(|left, right| left.id.cmp(&right.id));
                data.model_count = model_count;
                data.provider_count = provider_count;
                data.config_sources = config_sources;
                data.disabled_models = disabled_models;
                data.subagent_catalog = subagent_catalog;
            }
            Ok(DashboardEvent::ProviderTested(message)) => *screen = Screen::Success(message),
            Ok(DashboardEvent::UpdateAvailable(tag)) => *screen = Screen::UpdateAvailable(tag),
            Ok(DashboardEvent::UpdateInstalled) => return Some(DashboardExit::Restart),
            Ok(DashboardEvent::ShutdownRequested) => return Some(DashboardExit::Quit),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
        }
    }
    None
}

#[cfg(test)]
fn handle_key(screen: &mut Screen, key: KeyCode, command_tx: &UnboundedSender<DashboardCommand>) {
    handle_key_with_providers(screen, key, command_tx, &[]);
}

fn handle_key_with_data(
    screen: &mut Screen,
    key: KeyCode,
    command_tx: &UnboundedSender<DashboardCommand>,
    data: &DashboardData,
) {
    match screen {
        Screen::Providers { selected } => {
            let rows = provider_rows(data);
            *selected = (*selected).min(rows.len().saturating_sub(1));
            match key {
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Down => {
                    *selected = selected.saturating_add(1).min(rows.len().saturating_sub(1));
                }
                KeyCode::Char(' ') | KeyCode::Enter => match rows.get(*selected).copied() {
                    Some(ProviderRow::Source(source)) => {
                        let _ = command_tx.send(DashboardCommand::ToggleSource { source });
                    }
                    Some(ProviderRow::Local(index)) if key == KeyCode::Char(' ') => {
                        if let Some(provider) = data.providers.get(index) {
                            let _ = command_tx.send(DashboardCommand::ToggleLocalProvider {
                                provider: provider.name.clone(),
                            });
                        }
                    }
                    Some(ProviderRow::Local(index)) => {
                        if let Some(provider) = data.providers.get(index)
                            && !provider.models.is_empty()
                        {
                            let model_selected = provider
                                .default_model
                                .as_ref()
                                .and_then(|default| {
                                    provider.models.iter().position(|model| model == default)
                                })
                                .unwrap_or_default();
                            *screen = Screen::ProviderModels {
                                provider_selected: index,
                                model_selected,
                            };
                        }
                    }
                    Some(ProviderRow::Combo(index)) if key == KeyCode::Char(' ') => {
                        if let Some(combo) = data.combos.get(index) {
                            let _ = command_tx.send(DashboardCommand::ToggleModel {
                                model: format!("combo/{}", combo.name),
                            });
                        }
                    }
                    Some(ProviderRow::Combo(index)) => {
                        if let Some(combo) = data.combos.get(index) {
                            *screen = Screen::ComboStrategy {
                                original_name: Some(combo.name.clone()),
                                name: combo.name.clone(),
                                selected: COMBO_STRATEGIES
                                    .iter()
                                    .position(|strategy| strategy == &combo.strategy)
                                    .unwrap_or_default(),
                                models: combo
                                    .models
                                    .iter()
                                    .map(|model| (model.model().to_owned(), model.weight()))
                                    .collect(),
                            };
                        }
                    }
                    None => {}
                },
                KeyCode::Char('n') => {
                    *screen = Screen::ProviderBaseUrl {
                        selected: *selected,
                        value: String::new(),
                    };
                }
                KeyCode::Char('c') => {
                    *screen = Screen::ComboName {
                        original_name: None,
                        value: String::new(),
                    };
                }
                KeyCode::Delete | KeyCode::Backspace => match rows.get(*selected).copied() {
                    Some(ProviderRow::Local(index)) => {
                        if let Some(provider) = data.providers.get(index) {
                            let _ = command_tx.send(DashboardCommand::RemoveProvider {
                                name: provider.name.clone(),
                            });
                        }
                    }
                    Some(ProviderRow::Combo(index)) => {
                        if let Some(combo) = data.combos.get(index) {
                            let _ = command_tx.send(DashboardCommand::RemoveCombo {
                                name: combo.name.clone(),
                            });
                        }
                    }
                    _ => {}
                },
                KeyCode::Char('k')
                | KeyCode::Char('x')
                | KeyCode::Char('t')
                | KeyCode::Char('\\') => {
                    if let Some(ProviderRow::Local(index)) = rows.get(*selected).copied()
                        && let Some(provider) = data.providers.get(index)
                    {
                        match key {
                            KeyCode::Char('k') => {
                                *screen = Screen::ProviderPoolKey {
                                    selected: index,
                                    api_key: String::new(),
                                };
                            }
                            KeyCode::Char('x') => {
                                let _ = command_tx.send(DashboardCommand::RemoveProviderKey {
                                    provider: provider.name.clone(),
                                });
                            }
                            KeyCode::Char('t') => {
                                let _ = command_tx.send(DashboardCommand::TestProvider {
                                    name: provider.name.clone(),
                                });
                            }
                            KeyCode::Char('\\') if !provider.models.is_empty() => {
                                *screen = Screen::ProviderModels {
                                    provider_selected: index,
                                    model_selected: provider
                                        .default_model
                                        .as_ref()
                                        .and_then(|default| {
                                            provider
                                                .models
                                                .iter()
                                                .position(|model| model == default)
                                        })
                                        .unwrap_or_default(),
                                };
                            }
                            _ => {}
                        }
                    }
                }
                KeyCode::Tab => *screen = page_screen(Page::Models),
                KeyCode::BackTab => *screen = page_screen(Page::Overview),
                KeyCode::Char(character) if Page::from_number(character).is_some() => {
                    *screen = page_screen(Page::from_number(character).unwrap_or(Page::Providers));
                }
                KeyCode::Char('/') => *screen = Screen::Config { selected: 0 },
                _ => {}
            }
        }
        Screen::ComboModels {
            original_name,
            name,
            strategy,
            selected,
            models,
        } => {
            let available = data
                .models
                .iter()
                .filter(|model| !model.id.starts_with("combo/"))
                .collect::<Vec<_>>();
            *selected = (*selected).min(available.len().saturating_sub(1));
            match key {
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Down => {
                    *selected = selected
                        .saturating_add(1)
                        .min(available.len().saturating_sub(1));
                }
                KeyCode::Char(' ') => {
                    if let Some(model) = available.get(*selected) {
                        if let Some(index) = models.iter().position(|(id, _)| id == &model.id) {
                            models.remove(index);
                        } else {
                            models.push((model.id.clone(), 1));
                        }
                    }
                }
                KeyCode::Left | KeyCode::Right => {
                    if let Some(model) = available.get(*selected)
                        && let Some(index) = models.iter().position(|(id, _)| id == &model.id)
                    {
                        if *strategy == ComboStrategy::WeightedRoundRobin {
                            let weight = &mut models[index].1;
                            *weight = if key == KeyCode::Right {
                                weight.saturating_add(1).min(100)
                            } else {
                                weight.saturating_sub(1).max(1)
                            };
                        } else if key == KeyCode::Left && index > 0 {
                            models.swap(index, index - 1);
                        } else if key == KeyCode::Right && index + 1 < models.len() {
                            models.swap(index, index + 1);
                        }
                    }
                }
                KeyCode::Enter if models.len() >= 2 => {
                    *screen = Screen::ComboReview {
                        original_name: original_name.clone(),
                        name: name.clone(),
                        strategy: *strategy,
                        models: models.clone(),
                    };
                }
                _ => {}
            }
        }
        Screen::Models { selected } => {
            *selected = (*selected).min(data.models.len().saturating_sub(1));
            match key {
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Down => {
                    *selected = selected
                        .saturating_add(1)
                        .min(data.models.len().saturating_sub(1));
                }
                KeyCode::Char(' ') => {
                    if let Some(model) = data.models.get(*selected) {
                        let _ = command_tx.send(DashboardCommand::ToggleModel {
                            model: model.id.clone(),
                        });
                    }
                }
                _ => handle_key_with_providers(screen, key, command_tx, &data.providers),
            }
        }
        Screen::Subagents { selected } => {
            *selected = (*selected).min(data.models.len().saturating_sub(1));
            let command = match key {
                KeyCode::Char(' ') => data.models.get(*selected).map(|model| {
                    DashboardCommand::ToggleSubagentFeatured {
                        model: model.id.clone(),
                    }
                }),
                KeyCode::Char('f') => data.models.get(*selected).map(|model| {
                    DashboardCommand::ToggleSubagentFallback {
                        model: model.id.clone(),
                    }
                }),
                KeyCode::Char('+') | KeyCode::Char('=') => {
                    Some(DashboardCommand::AdjustSubagentMaxEntries { delta: 1 })
                }
                KeyCode::Char('-') => {
                    Some(DashboardCommand::AdjustSubagentMaxEntries { delta: -1 })
                }
                _ => None,
            };
            if let Some(command) = command {
                let _ = command_tx.send(command);
            } else {
                handle_key_with_providers(screen, key, command_tx, &data.providers);
            }
        }
        _ => handle_key_with_providers(screen, key, command_tx, &data.providers),
    }
}

fn handle_key_with_providers(
    screen: &mut Screen,
    key: KeyCode,
    command_tx: &UnboundedSender<DashboardCommand>,
    providers: &[ProviderSummary],
) {
    match screen {
        Screen::Base { page } => match key {
            KeyCode::Tab => *screen = page_screen(page.adjacent(true)),
            KeyCode::BackTab => *screen = page_screen(page.adjacent(false)),
            KeyCode::Char(character) if Page::from_number(character).is_some() => {
                *screen = page_screen(Page::from_number(character).unwrap_or(*page));
            }
            KeyCode::Enter if *page == Page::Providers => {
                *screen = Screen::Providers { selected: 0 };
            }
            KeyCode::Char('t') if *page == Page::Providers => {
                if let Some(provider) = providers.first() {
                    let _ = command_tx.send(DashboardCommand::TestProvider {
                        name: provider.name.clone(),
                    });
                }
            }
            KeyCode::Enter if *page == Page::Integrations => {
                *screen = Screen::Config { selected: 0 };
            }
            KeyCode::Enter | KeyCode::Char('s') if *page == Page::CodexSet => {
                let _ = command_tx.send(DashboardCommand::SyncCodex);
            }
            KeyCode::Char('b') if *page == Page::CodexSet => {
                let _ = command_tx.send(DashboardCommand::ToggleRunInBackground);
            }
            KeyCode::Char('a') if *page == Page::CodexSet => {
                *screen = Screen::Subagents { selected: 0 };
            }
            KeyCode::Char('e') if *page == Page::CodexSet => {
                let _ = command_tx.send(DashboardCommand::CycleReasoningEffortCap);
            }
            KeyCode::Char('/') => *screen = Screen::Config { selected: 0 },
            _ => {}
        },
        Screen::ComboName {
            original_name,
            value,
        } => match key {
            KeyCode::Enter if !value.trim().is_empty() => {
                *screen = Screen::ComboStrategy {
                    original_name: original_name.clone(),
                    name: value.trim().to_owned(),
                    selected: 0,
                    models: Vec::new(),
                };
            }
            KeyCode::Backspace => {
                value.pop();
            }
            KeyCode::Char(character) => value.push(character),
            _ => {}
        },
        Screen::ComboStrategy {
            original_name,
            name,
            selected,
            models,
        } => match key {
            KeyCode::Up => *selected = selected.saturating_sub(1),
            KeyCode::Down => {
                *selected = selected
                    .saturating_add(1)
                    .min(COMBO_STRATEGIES.len().saturating_sub(1));
            }
            KeyCode::Enter => {
                *screen = Screen::ComboModels {
                    original_name: original_name.clone(),
                    name: name.clone(),
                    strategy: COMBO_STRATEGIES[*selected],
                    selected: 0,
                    models: models.clone(),
                };
            }
            _ => {}
        },
        Screen::ComboReview {
            original_name,
            name,
            strategy,
            models,
        } if key == KeyCode::Enter => {
            let combo = Combo {
                name: name.clone(),
                strategy: *strategy,
                models: models
                    .iter()
                    .map(|(model, weight)| {
                        if *strategy == ComboStrategy::WeightedRoundRobin {
                            ComboModel::Weighted {
                                model: model.clone(),
                                weight: *weight,
                            }
                        } else {
                            ComboModel::Model(model.clone())
                        }
                    })
                    .collect(),
            };
            let _ = command_tx.send(DashboardCommand::SaveCombo {
                original_name: original_name.clone(),
                combo,
            });
        }
        Screen::ProviderPoolKey { selected, api_key } => match key {
            KeyCode::Enter if !api_key.trim().is_empty() => {
                if let Some(provider) = providers.get(*selected) {
                    let command = DashboardCommand::AddProviderKey {
                        provider: provider.name.clone(),
                        api_key: api_key.clone(),
                    };
                    if command_tx.send(command).is_ok() {
                        *screen = Screen::ProviderLoading {
                            selected: *selected,
                        };
                    }
                }
            }
            KeyCode::Backspace => {
                api_key.pop();
            }
            KeyCode::Char(character) => api_key.push(character),
            _ => {}
        },
        Screen::Models { selected } => match key {
            KeyCode::Up => *selected = selected.saturating_sub(1),
            KeyCode::Down => {
                // The worker validates the ID; selection is clamped while drawing/event updates.
                *selected = selected.saturating_add(1);
            }
            KeyCode::Char(' ') => {
                // Models are supplied separately because provider-manager models use local IDs.
                // The caller passes an empty slice only in narrow unit tests.
            }
            KeyCode::Tab => *screen = page_screen(Page::Subagents),
            KeyCode::BackTab => *screen = page_screen(Page::Providers),
            KeyCode::Char(character) if Page::from_number(character).is_some() => {
                *screen = page_screen(Page::from_number(character).unwrap_or(Page::Models));
            }
            _ => {}
        },
        Screen::Subagents { selected } => match key {
            KeyCode::Up => *selected = selected.saturating_sub(1),
            KeyCode::Down => *selected = selected.saturating_add(1),
            KeyCode::Tab => *screen = page_screen(Page::CodexSet),
            KeyCode::BackTab => *screen = page_screen(Page::Models),
            KeyCode::Char(character) if Page::from_number(character).is_some() => {
                *screen = page_screen(Page::from_number(character).unwrap_or(Page::Subagents));
            }
            _ => {}
        },
        Screen::Config { selected } => match key {
            KeyCode::Up => *selected = adjacent_config_item(*selected, false),
            KeyCode::Down => *selected = adjacent_config_item(*selected, true),
            KeyCode::Char(' ') if *selected == AUTO_START_ITEM => {
                let _ = command_tx.send(DashboardCommand::ToggleAutoStart);
            }
            KeyCode::Char(' ') if *selected == RUN_IN_BACKGROUND_ITEM => {
                let _ = command_tx.send(DashboardCommand::ToggleRunInBackground);
            }
            KeyCode::Char(' ') => {
                if let Some(target) = target_for_config_item(*selected) {
                    let _ = command_tx.send(DashboardCommand::ToggleProxyTarget { target });
                }
            }
            _ => {}
        },
        Screen::ProviderModels {
            provider_selected,
            model_selected,
        } => match key {
            KeyCode::Up => *model_selected = model_selected.saturating_sub(1),
            KeyCode::Down => {
                if let Some(provider) = providers.get(*provider_selected) {
                    *model_selected = model_selected
                        .saturating_add(1)
                        .min(provider.models.len().saturating_sub(1));
                }
            }
            KeyCode::Enter => {
                if let Some(provider) = providers.get(*provider_selected)
                    && let Some(model) = provider.models.get(*model_selected)
                {
                    let _ = command_tx.send(DashboardCommand::SetDefaultModel {
                        provider: provider.name.clone(),
                        model: model.clone(),
                    });
                }
            }
            _ => {}
        },
        Screen::ProviderBaseUrl { selected, value } => match key {
            KeyCode::Enter if !value.trim().is_empty() => {
                *screen = Screen::ProviderApiKey {
                    selected: *selected,
                    base_url: value.trim().to_owned(),
                    api_key: String::new(),
                };
            }
            KeyCode::Backspace => {
                value.pop();
            }
            KeyCode::Char(character) => value.push(character),
            _ => {}
        },
        Screen::ProviderApiKey {
            selected,
            base_url,
            api_key,
        } => match key {
            KeyCode::Enter => {
                let command = DashboardCommand::AddProvider {
                    base_url: base_url.clone(),
                    api_key: api_key.clone(),
                };
                if command_tx.send(command).is_ok() {
                    *screen = Screen::ProviderLoading {
                        selected: *selected,
                    };
                } else {
                    *screen = Screen::Error("provider reload channel is unavailable".into());
                }
            }
            KeyCode::Backspace => {
                api_key.pop();
            }
            KeyCode::Char(character) => api_key.push(character),
            _ => {}
        },
        Screen::UpdateAvailable(tag) if key == KeyCode::Enter => {
            let tag = tag.clone();
            if command_tx
                .send(DashboardCommand::InstallUpdate { tag: tag.clone() })
                .is_ok()
            {
                *screen = Screen::Updating { tag, tick: 0 };
            } else {
                *screen = Screen::Error("update channel is unavailable".into());
            }
        }
        Screen::Success(_) | Screen::Error(_) | Screen::EasterEgg { .. }
            if key == KeyCode::Enter =>
        {
            *screen = Screen::Base {
                page: Page::Overview,
            };
        }
        _ => {}
    }
}

fn draw(frame: &mut Frame<'_>, data: &DashboardData, screen: &Screen) {
    if let Screen::EasterEgg { tick } = screen {
        draw_easter_egg(frame, *tick);
        return;
    }
    if let Screen::Updating { tag, tick } = screen {
        draw_update_animation(frame, tag, *tick);
        return;
    }

    let header_height = if frame.area().width >= 82 && frame.area().height >= 22 {
        7
    } else {
        3
    };
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(header_height),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .areas(frame.area());

    draw_header(frame, header, header_animation_tick());

    match screen {
        Screen::Base { page } => draw_shell(frame, body, data, *page),
        Screen::Models { selected } => {
            draw_shell_selected(frame, body, data, Page::Models, *selected);
        }
        Screen::Subagents { selected } => {
            draw_shell_selected(frame, body, data, Page::Subagents, *selected);
        }
        Screen::Config { selected } => {
            draw_shell(frame, body, data, Page::Integrations);
            draw_config(frame, data, *selected);
        }
        Screen::Providers { selected } => {
            draw_shell_selected(frame, body, data, Page::Providers, *selected);
        }
        Screen::ProviderModels {
            provider_selected,
            model_selected,
        } => {
            draw_shell_selected(
                frame,
                body,
                data,
                Page::Providers,
                SourceKind::DETECTED.len() + *provider_selected,
            );
            if let Some(provider) = data.providers.get(*provider_selected) {
                draw_provider_models(frame, provider, *model_selected);
            }
        }
        Screen::ProviderBaseUrl { selected, value } => {
            draw_shell_selected(frame, body, data, Page::Providers, *selected);
            draw_provider_input(frame, "Step 1/3 — Base URL", value, false);
        }
        Screen::ProviderApiKey {
            selected, api_key, ..
        } => {
            draw_shell_selected(frame, body, data, Page::Providers, *selected);
            draw_provider_input(frame, "Step 2/3 — API key", api_key, true);
        }
        Screen::ProviderPoolKey { selected, api_key } => {
            draw_shell_selected(
                frame,
                body,
                data,
                Page::Providers,
                SourceKind::DETECTED.len() + *selected,
            );
            draw_provider_input(frame, "Add API key to pool", api_key, true);
        }
        Screen::ProviderLoading { selected } => {
            draw_shell_selected(frame, body, data, Page::Providers, *selected);
            draw_provider_loading(frame);
        }
        Screen::ComboName { value, .. } => {
            draw_shell(frame, body, data, Page::Providers);
            draw_combo_name(frame, value);
        }
        Screen::ComboStrategy { selected, .. } => {
            draw_shell(frame, body, data, Page::Providers);
            draw_combo_strategy(frame, *selected);
        }
        Screen::ComboModels {
            strategy,
            selected,
            models,
            ..
        } => {
            draw_shell(frame, body, data, Page::Providers);
            draw_combo_models(frame, data, *strategy, *selected, models);
        }
        Screen::ComboReview {
            name,
            strategy,
            models,
            ..
        } => {
            draw_shell(frame, body, data, Page::Providers);
            draw_combo_review(frame, name, *strategy, models);
        }
        Screen::UpdateAvailable(tag) => {
            draw_shell(frame, body, data, Page::Overview);
            draw_update_prompt(frame, tag);
        }
        Screen::Updating { .. } => {
            unreachable!("updating is rendered as a full-screen animated scene")
        }
        Screen::EasterEgg { .. } => unreachable!("easter egg is rendered as a full-screen scene"),
        Screen::Success(message) => draw_success(frame, message),
        Screen::Error(error) => draw_error(frame, error),
    }

    if !matches!(screen, Screen::Base { .. }) {
        return;
    }

    let help = match screen {
        Screen::Base { .. } | Screen::Models { .. } | Screen::Subagents { .. } => vec![
            Span::styled(
                " esc ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  Exit    ", Style::default().fg(MUTED_TEXT)),
            Span::styled(
                " tab/shift+tab ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  Navigate    ", Style::default().fg(MUTED_TEXT)),
            Span::styled(
                " 1-9 ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  Jump    Enter open", Style::default().fg(MUTED_TEXT)),
        ],
        Screen::Config { .. }
        | Screen::Providers { .. }
        | Screen::ProviderModels { .. }
        | Screen::ProviderBaseUrl { .. }
        | Screen::ProviderApiKey { .. }
        | Screen::ProviderPoolKey { .. }
        | Screen::ProviderLoading { .. }
        | Screen::ComboName { .. }
        | Screen::ComboStrategy { .. }
        | Screen::ComboModels { .. }
        | Screen::ComboReview { .. }
        | Screen::UpdateAvailable(_)
        | Screen::Success(_)
        | Screen::Error(_) => unreachable!("modal screens return before global footer rendering"),
        Screen::Updating { .. } => {
            unreachable!("updating has its own full-screen progress scene")
        }
        Screen::EasterEgg { .. } => unreachable!("easter egg has its own full-screen controls"),
    };
    frame.render_widget(
        Paragraph::new(Line::from(help)),
        footer.inner(Margin::new(1, 0)),
    );
}

fn draw_shell(frame: &mut Frame<'_>, area: Rect, data: &DashboardData, page: Page) {
    draw_shell_selected(frame, area, data, page, 0);
}

fn draw_shell_selected(
    frame: &mut Frame<'_>,
    area: Rect,
    data: &DashboardData,
    page: Page,
    selected: usize,
) {
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Reset)),
        area,
    );
    if area.width >= 86 {
        let [navigation, content] = Layout::horizontal([
            Constraint::Length(20.min(area.width / 3)),
            Constraint::Min(1),
        ])
        .spacing(1)
        .areas(area);
        draw_sidebar(frame, navigation, page);
        draw_page(frame, content, data, page, selected);
    } else {
        let [navigation, content] =
            Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).areas(area);
        draw_tabs(frame, navigation, page);
        draw_page(frame, content, data, page, selected);
    }
}

fn draw_sidebar(frame: &mut Frame<'_>, area: Rect, selected: Page) {
    let items = Page::ALL
        .iter()
        .enumerate()
        .map(|(index, page)| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{} ", index + 1),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(page.label()),
            ]))
            .style(selected_style(*page == selected))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .title(" JOOCODE ")
                .borders(Borders::RIGHT)
                .border_style(Style::default().fg(PANEL_BORDER)),
        ),
        area,
    );
}

fn draw_tabs(frame: &mut Frame<'_>, area: Rect, selected: Page) {
    let mut lines = Vec::<Line<'static>>::new();
    let mut spans = Vec::<Span<'static>>::new();
    let mut line_width = 0usize;
    let mut selected_line = 0usize;
    for (index, page) in Page::ALL.iter().enumerate() {
        let style = if *page == selected {
            Style::default()
                .fg(Color::Black)
                .bg(MODAL_ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(MUTED_TEXT)
        };
        let label = format!(" {} {} ", index + 1, page.label());
        let tab_width = label.len() + usize::from(!spans.is_empty());
        if !spans.is_empty() && line_width + tab_width > area.width as usize {
            lines.push(Line::from(std::mem::take(&mut spans)));
            line_width = 0;
        }
        if *page == selected {
            selected_line = lines.len();
        }
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
            line_width += 1;
        }
        spans.push(Span::styled(label, style));
        line_width += tab_width - usize::from(line_width > 0);
    }
    if !spans.is_empty() {
        lines.push(Line::from(spans));
    }

    let visible_height = area.height.max(1) as usize;
    let start = selected_line
        .saturating_add(1)
        .saturating_sub(visible_height)
        .min(lines.len().saturating_sub(visible_height));
    frame.render_widget(
        Paragraph::new(
            lines
                .into_iter()
                .skip(start)
                .take(visible_height)
                .collect::<Vec<_>>(),
        ),
        area,
    );
}

fn draw_page(frame: &mut Frame<'_>, area: Rect, data: &DashboardData, page: Page, selected: usize) {
    match page {
        Page::Overview => draw_dashboard(frame, area, data),
        Page::Providers => draw_providers_page(frame, area, data, selected),
        Page::Models => draw_models_page(frame, area, data, selected),
        Page::Subagents => draw_subagents_page(frame, area, data, selected),
        Page::CodexSet => draw_codex_set_page(frame, area, data),
        Page::Logs => draw_logs_page(frame, area, &data.request_events),
        Page::Usage => draw_usage_page(frame, area, &data.runtime),
        Page::Storage => draw_storage_page(frame, area, &data.storage),
        Page::Integrations => draw_read_only_page(
            frame,
            area,
            "Integrations",
            vec![
                Line::from(format!(
                    "{} desktop targets enabled",
                    data.ide_targets.len()
                )),
                Line::from(""),
                Line::from("Press Enter to open configuration."),
            ],
        ),
        Page::System => draw_system_page(frame, area, data),
    }
}

fn draw_read_only_page(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &'static str,
    lines: Vec<Line<'static>>,
) {
    let panel = dashboard_panel(title, Color::LightCyan);
    let inner = panel.inner(area.inner(Margin::new(1, 1)));
    let panel_area = area.inner(Margin::new(1, 1));
    frame.render_widget(panel, panel_area);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn draw_models_page(frame: &mut Frame<'_>, area: Rect, data: &DashboardData, selected: usize) {
    let panel_area = area.inner(Margin::new(1, 1));
    let panel = dashboard_panel(
        "Models — ↑/↓ select, Space enable/disable",
        Color::LightCyan,
    );
    let inner = panel.inner(panel_area);
    frame.render_widget(panel, panel_area);
    let resolved = data.subagent_catalog.resolve(&data.models);
    let [summary, inner] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);
    frame.render_widget(
        Paragraph::new(format!(
            "Advertised: {}/{}   Featured: {}   Fallback: {}",
            resolved.len(),
            data.subagent_catalog.max_entries,
            data.subagent_catalog.featured_models.len(),
            data.subagent_catalog.fallback_models.len()
        )),
        summary,
    );
    let [status, legend, inner] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(format!(
            "Maximum catalog entries: {}",
            data.subagent_catalog.max_entries
        )),
        status,
    );
    frame.render_widget(
        Paragraph::new("* reasoning is declared by provider metadata; not actively verified")
            .style(Style::default().fg(Color::DarkGray)),
        legend,
    );
    if data.models.is_empty() {
        frame.render_widget(Paragraph::new("No models are currently loaded."), inner);
        return;
    }
    let items = data
        .models
        .iter()
        .enumerate()
        .map(|(index, model)| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    if data.disabled_models.contains(&model.id) {
                        "○ "
                    } else {
                        "● "
                    },
                    Style::default().fg(if data.disabled_models.contains(&model.id) {
                        Color::DarkGray
                    } else {
                        Color::Green
                    }),
                ),
                Span::styled(
                    model.id.clone(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  {}", model.name), Style::default().fg(MUTED_TEXT)),
                Span::styled(
                    if model.reasoning { "  reasoning*" } else { "" },
                    Style::default().fg(Color::LightCyan),
                ),
            ]))
            .style(selected_style(index == selected))
        })
        .collect::<Vec<_>>();
    let selected = selected.min(data.models.len().saturating_sub(1));
    let mut state = ListState::default().with_selected(Some(selected));
    frame.render_stateful_widget(List::new(items).highlight_symbol("› "), inner, &mut state);
    draw_modal_scrollbar(frame, inner, data.models.len(), selected);
}

fn draw_subagents_page(frame: &mut Frame<'_>, area: Rect, data: &DashboardData, selected: usize) {
    let panel_area = area.inner(Margin::new(1, 1));
    let panel = dashboard_panel(
        "Subagents — Space featured, f fallback, +/- adjusts max",
        Color::LightCyan,
    );
    let inner = panel.inner(panel_area);
    frame.render_widget(panel, panel_area);
    if data.models.is_empty() {
        frame.render_widget(Paragraph::new("No models are currently loaded."), inner);
        return;
    }
    let items = data
        .models
        .iter()
        .enumerate()
        .map(|(index, model)| {
            let featured = data.subagent_catalog.featured_models.contains(&model.id);
            let fallback = data.subagent_catalog.fallback_models.contains(&model.id);
            ListItem::new(Line::from(vec![
                Span::styled(
                    if featured { "★ " } else { "  " },
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled(
                    if fallback { "F " } else { "  " },
                    Style::default().fg(Color::LightCyan),
                ),
                Span::raw(model.id.clone()),
                Span::styled(format!("  {}", model.name), Style::default().fg(MUTED_TEXT)),
            ]))
            .style(selected_style(index == selected))
        })
        .collect::<Vec<_>>();
    let selected = selected.min(data.models.len().saturating_sub(1));
    let mut state = ListState::default().with_selected(Some(selected));
    frame.render_stateful_widget(List::new(items).highlight_symbol("› "), inner, &mut state);
    draw_modal_scrollbar(frame, inner, data.models.len(), selected);
}

fn draw_dashboard(frame: &mut Frame<'_>, area: ratatui::layout::Rect, data: &DashboardData) {
    let sources = if data.config_sources.is_empty() {
        "None".to_owned()
    } else {
        data.config_sources.join(", ")
    };
    let targets = if data.ide_targets.is_empty() {
        "None detected".to_owned()
    } else {
        data.ide_targets.join(", ")
    };
    let openai_compatible = format!("{}/v1", data.listening.trim_end_matches('/'));
    let canvas = area.inner(Margin::new(1, 0));
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Reset)),
        area,
    );

    let warning_height = u16::from(data.port_warning.is_some()) * 3;
    let [warning_area, content_area, stats_area] = Layout::vertical([
        Constraint::Length(warning_height),
        Constraint::Length(if canvas.width >= 76 { 7 } else { 14 }),
        Constraint::Length(3),
    ])
    .spacing(1)
    .areas(canvas);

    if let Some(warning) = &data.port_warning {
        let panel = dashboard_panel(" PORT CONFLICT ", Color::Yellow);
        let inner = panel.inner(warning_area);
        frame.render_widget(panel, warning_area);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("⚠  ", Style::default().fg(Color::Yellow)),
                Span::styled(warning, Style::default().fg(Color::LightYellow)),
            ]))
            .wrap(Wrap { trim: true }),
            inner,
        );
    }

    if canvas.width >= 76 {
        let [routing_area, gateway_area] =
            Layout::horizontal([Constraint::Percentage(42), Constraint::Percentage(58)])
                .spacing(1)
                .areas(content_area);
        draw_routing_panel(frame, routing_area, &sources, &targets);
        draw_gateway_panel(frame, gateway_area, &data.listening, &openai_compatible);
    } else {
        let [routing_area, gateway_area] =
            Layout::vertical([Constraint::Length(7), Constraint::Length(7)]).areas(content_area);
        draw_routing_panel(frame, routing_area, &sources, &targets);
        draw_gateway_panel(frame, gateway_area, &data.listening, &openai_compatible);
    }

    draw_stats(frame, stats_area, data.model_count, data.provider_count);
}

fn dashboard_panel(title: &'static str, accent: Color) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(PANEL_BORDER))
        .title(Span::styled(
            title,
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(PANEL_BACKGROUND))
}

fn draw_routing_panel(frame: &mut Frame<'_>, area: Rect, sources: &str, targets: &str) {
    let panel = dashboard_panel(" ROUTING ", Color::Cyan);
    let inner = panel.inner(area);
    frame.render_widget(panel, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "SOURCES",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![
                Span::styled("◆  ", Style::default().fg(Color::Cyan)),
                Span::styled(sources, Style::default().fg(Color::White)),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "DESKTOP TARGETS",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![
                Span::styled("⌘  ", Style::default().fg(Color::LightMagenta)),
                Span::styled(targets, Style::default().fg(Color::White)),
            ]),
        ])
        .wrap(Wrap { trim: true }),
        inner,
    );
}

fn draw_gateway_panel(frame: &mut Frame<'_>, area: Rect, listening: &str, openai_compatible: &str) {
    let panel = dashboard_panel(" LOCAL GATEWAY ", Color::Green);
    let inner = panel.inner(area);
    frame.render_widget(panel, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    "● ONLINE",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
                Span::styled(listening, Style::default().fg(Color::Gray)),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "OpenAI-compatible endpoint",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![
                Span::styled("↗  ", Style::default().fg(Color::LightCyan)),
                Span::styled(openai_compatible, Style::default().fg(Color::LightCyan)),
            ]),
            Line::from(vec![
                Span::styled("KEY  ", Style::default().fg(Color::DarkGray)),
                Span::styled("any non-empty value", Style::default().fg(Color::Gray)),
                Span::styled("  (e.g. joocode)", Style::default().fg(Color::DarkGray)),
            ]),
        ])
        .wrap(Wrap { trim: true }),
        inner,
    );
}

fn draw_stats(frame: &mut Frame<'_>, area: Rect, models: usize, providers: usize) {
    let [models_area, providers_area, status_area] = if area.width >= 64 {
        Layout::horizontal([
            Constraint::Percentage(32),
            Constraint::Percentage(32),
            Constraint::Percentage(36),
        ])
        .spacing(1)
        .areas(area)
    } else {
        Layout::horizontal([
            Constraint::Percentage(50),
            Constraint::Percentage(50),
            Constraint::Length(0),
        ])
        .spacing(1)
        .areas(area)
    };

    draw_stat_card(frame, models_area, "MODELS", models, Color::Yellow, "◉");
    draw_stat_card(
        frame,
        providers_area,
        "PROVIDERS",
        providers,
        Color::Blue,
        "◇",
    );

    if status_area.width > 0 {
        let panel = dashboard_panel(" HEALTH ", Color::Green);
        let inner = panel.inner(status_area);
        frame.render_widget(panel, status_area);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("● ", Style::default().fg(Color::Green)),
                Span::styled("Ready", Style::default().fg(Color::LightGreen)),
            ]))
            .alignment(Alignment::Center),
            inner,
        );
    }
}

fn draw_stat_card(
    frame: &mut Frame<'_>,
    area: Rect,
    label: &'static str,
    value: usize,
    accent: Color,
    icon: &'static str,
) {
    let panel = dashboard_panel("", accent);
    let inner = panel.inner(area);
    frame.render_widget(panel, area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{label}  "), Style::default().fg(MUTED_TEXT)),
            Span::styled(format!("{icon} "), Style::default().fg(accent)),
            Span::styled(
                value.to_string(),
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
        ]))
        .alignment(Alignment::Center),
        inner,
    );
}

fn display_source(source: &str) -> String {
    match source {
        "opencode" => "OpenCode".into(),
        "crabcode" => "CrabCode".into(),
        "ocx" => "OpenCodex".into(),
        "hermes" => "Hermes".into(),
        "copilot" => "GitHub Copilot".into(),
        "antigravity" => "Antigravity".into(),
        "joocode" => "Joocode".into(),
        other => other.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;

    fn empty_dashboard_data() -> DashboardData {
        DashboardData {
            config_sources: vec![],
            ide_targets: vec![],
            listening: "http://127.0.0.1:10100".into(),
            model_count: 0,
            provider_count: 0,
            autostart: AutoStartStatus::Off,
            run_in_background: false,
            proxy_targets: BTreeMap::new(),
            detected_sources: BTreeMap::new(),
            providers: vec![],
            disabled_local_providers: BTreeSet::new(),
            combos: vec![],
            models: vec![],
            disabled_models: BTreeSet::new(),
            subagent_catalog: crate::target_config::SubagentCatalogPolicy::default(),
            port_warning: None,
            runtime: DashboardRuntimeSnapshot::default(),
            system: DashboardSystemSnapshot::default(),
            storage: DashboardStorageSnapshot::default(),
            request_events: vec![],
        }
    }

    #[test]
    fn observability_event_refreshes_typed_snapshots() {
        let mut data = DashboardData {
            config_sources: vec![],
            ide_targets: vec![],
            listening: String::new(),
            model_count: 0,
            provider_count: 0,
            autostart: AutoStartStatus::Off,
            run_in_background: false,
            proxy_targets: BTreeMap::new(),
            detected_sources: BTreeMap::new(),
            providers: vec![],
            disabled_local_providers: BTreeSet::new(),
            combos: vec![],
            models: vec![],
            disabled_models: BTreeSet::new(),
            subagent_catalog: crate::target_config::SubagentCatalogPolicy::default(),
            port_warning: None,
            runtime: DashboardRuntimeSnapshot::default(),
            system: DashboardSystemSnapshot::default(),
            storage: DashboardStorageSnapshot::default(),
            request_events: vec![],
        };
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(DashboardEvent::ObservabilityUpdated {
            runtime: DashboardRuntimeSnapshot {
                requests: 7,
                ..Default::default()
            },
            storage: DashboardStorageSnapshot {
                entries: vec![DashboardStorageEntry {
                    label: "settings.json".into(),
                    path: "/tmp/settings.json".into(),
                    size_bytes: Some(42),
                }],
            },
            request_events: vec![DashboardRequestEvent {
                method: "POST".into(),
                path: "/v1/responses".into(),
                status: 200,
                duration_ms: 12,
                provider: None,
                model: None,
                retries: 0,
                failovers: 0,
                input_tokens: None,
                output_tokens: None,
            }],
        })
        .unwrap();
        receive_events(&mut data, &mut Screen::default(), &rx);
        assert_eq!(data.runtime.requests, 7);
        assert_eq!(data.storage.entries[0].size_bytes, Some(42));
        assert_eq!(data.request_events[0].path, "/v1/responses");
    }

    #[test]
    fn observability_formatters_are_compact() {
        assert_eq!(format_duration(90_061), "1d 01:01:01");
        assert_eq!(format_bytes(1536), "1.5 KiB");
    }

    #[test]
    fn backslash_opens_default_model_picker_and_enter_selects_model() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let providers = vec![ProviderSummary {
            name: "gunamaya".into(),
            label: "gunamaya.id".into(),
            model_count: 2,
            models: vec!["gpt-5.4".into(), "gpt-5.5".into()],
            default_model: None,
            key_count: 1,
        }];
        let mut data = empty_dashboard_data();
        data.providers = providers;
        let mut screen = Screen::Providers {
            selected: SourceKind::DETECTED.len(),
        };
        handle_key_with_data(&mut screen, KeyCode::Char('\\'), &tx, &data);
        assert!(matches!(
            screen,
            Screen::ProviderModels {
                provider_selected: 0,
                model_selected: 0
            }
        ));
        handle_key_with_data(&mut screen, KeyCode::Down, &tx, &data);
        handle_key_with_data(&mut screen, KeyCode::Enter, &tx, &data);
        assert!(matches!(
            rx.try_recv(),
            Ok(DashboardCommand::SetDefaultModel { provider, model })
                if provider == "gunamaya" && model == "gpt-5.5"
        ));
    }

    #[test]
    fn space_toggles_selected_local_provider() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut data = empty_dashboard_data();
        data.providers = vec![ProviderSummary {
            name: "openai".into(),
            label: "openai.com".into(),
            model_count: 1,
            models: vec!["gpt-5.5".into()],
            default_model: None,
            key_count: 1,
        }];
        let mut screen = Screen::Providers {
            selected: SourceKind::DETECTED.len(),
        };
        handle_key_with_data(&mut screen, KeyCode::Char(' '), &tx, &data);
        assert!(matches!(
            rx.try_recv(),
            Ok(DashboardCommand::ToggleLocalProvider { provider }) if provider == "openai"
        ));
    }

    #[test]
    fn header_shows_running_version_and_animated_rainbow_logo() {
        let backend = TestBackend::new(110, 7);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_header(frame, frame.area(), 0))
            .unwrap();
        let first = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        let first_colors = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .take(45)
            .map(|cell| cell.fg)
            .collect::<Vec<_>>();
        assert!(first.contains("/ /___"));
        assert!(first.contains("/ /_/ /"));
        assert!(first.contains(&format!("v{}", env!("CARGO_PKG_VERSION"))));
        assert!(first.contains("ONLINE"));

        terminal
            .draw(|frame| draw_header(frame, frame.area(), 2))
            .unwrap();
        let second_colors = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .take(45)
            .map(|cell| cell.fg)
            .collect::<Vec<_>>();
        assert_ne!(first_colors, second_colors);
    }

    #[test]
    fn source_labels_are_human_readable() {
        assert_eq!(display_source("opencode"), "OpenCode");
        assert_eq!(display_source("crabcode"), "CrabCode");
        assert_eq!(display_source("ocx"), "OpenCodex");
        assert_eq!(display_source("hermes"), "Hermes");
        assert_eq!(display_source("copilot"), "GitHub Copilot");
        assert_eq!(display_source("antigravity"), "Antigravity");
        assert_eq!(display_source("joocode"), "Joocode");
    }

    #[test]
    fn tab_navigates_to_providers_page() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut screen = Screen::Base {
            page: Page::Overview,
        };
        handle_key(&mut screen, KeyCode::Tab, &tx);
        assert!(matches!(screen, Screen::Providers { selected: 0 }));
    }

    #[test]
    fn shell_navigation_wraps_and_number_keys_select_pages() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut screen = Screen::Base {
            page: Page::Overview,
        };
        handle_key(&mut screen, KeyCode::BackTab, &tx);
        assert!(matches!(screen, Screen::Base { page: Page::System }));
        handle_key(&mut screen, KeyCode::Tab, &tx);
        assert!(matches!(
            screen,
            Screen::Base {
                page: Page::Overview
            }
        ));
        handle_key(&mut screen, KeyCode::Char('4'), &tx);
        assert!(matches!(screen, Screen::Subagents { selected: 0 }));
    }

    #[test]
    fn enter_opens_page_specific_existing_modals() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut providers = Screen::Base {
            page: Page::Providers,
        };
        handle_key(&mut providers, KeyCode::Enter, &tx);
        assert!(matches!(providers, Screen::Providers { selected: 0 }));

        let mut integrations = Screen::Base {
            page: Page::Integrations,
        };
        handle_key(&mut integrations, KeyCode::Enter, &tx);
        assert!(matches!(integrations, Screen::Config { selected: 0 }));
    }

    #[test]
    fn wide_shell_renders_sidebar_and_models_from_dashboard_data() {
        let backend = TestBackend::new(110, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let data = DashboardData {
            config_sources: vec![],
            ide_targets: vec![],
            listening: "http://127.0.0.1:10100".into(),
            model_count: 1,
            provider_count: 1,
            autostart: AutoStartStatus::Off,
            run_in_background: true,
            proxy_targets: BTreeMap::new(),
            detected_sources: BTreeMap::new(),
            providers: vec![],
            disabled_local_providers: BTreeSet::new(),
            combos: vec![],
            models: vec![ModelInfo {
                id: "demo/reasoner".into(),
                provider: "demo".into(),
                upstream_id: "reasoner".into(),
                name: "Demo Reasoner".into(),
                reasoning: true,
                context_window: Some(128_000),
                max_output_tokens: Some(8_192),
            }],
            disabled_models: BTreeSet::new(),
            subagent_catalog: crate::target_config::SubagentCatalogPolicy::default(),
            runtime: DashboardRuntimeSnapshot::default(),
            system: DashboardSystemSnapshot::default(),
            storage: DashboardStorageSnapshot::default(),
            request_events: vec![],
            port_warning: None,
        };
        terminal
            .draw(|frame| draw(frame, &data, &Screen::Base { page: Page::Models }))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Overview"));
        assert!(rendered.contains("Integrations"));
        assert!(rendered.contains("demo/reasoner"));
        assert!(rendered.contains("Demo Reasoner"));
        assert!(rendered.contains("reasoning*"));
        assert!(rendered.contains("declared by provider metadata"));
    }

    #[test]
    fn system_and_storage_pages_show_runtime_policy_and_totals() {
        let backend = TestBackend::new(110, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut data = DashboardData {
            config_sources: vec![],
            ide_targets: vec![],
            listening: "http://0.0.0.0:10100".into(),
            model_count: 1,
            provider_count: 1,
            autostart: AutoStartStatus::On,
            run_in_background: true,
            proxy_targets: BTreeMap::new(),
            detected_sources: BTreeMap::new(),
            providers: vec![],
            disabled_local_providers: BTreeSet::new(),
            combos: vec![],
            models: vec![],
            disabled_models: BTreeSet::new(),
            subagent_catalog: crate::target_config::SubagentCatalogPolicy::default(),
            runtime: DashboardRuntimeSnapshot::default(),
            system: DashboardSystemSnapshot {
                mode: "Remote hub".into(),
                data_auth: "Required".into(),
                management_auth: "Separate token".into(),
                allowed_origins: 2,
                max_request_bytes: 16 * 1024 * 1024,
                max_sse_event_bytes: 1024 * 1024,
                max_tool_argument_bytes: 1024 * 1024,
                stream_idle_timeout_secs: 90,
                rate_limit: "20/s burst 40".into(),
            },
            storage: DashboardStorageSnapshot {
                entries: vec![DashboardStorageEntry {
                    label: "settings.json".into(),
                    path: "/tmp/settings.json".into(),
                    size_bytes: Some(2048),
                }],
            },
            request_events: vec![],
            port_warning: None,
        };
        terminal
            .draw(|frame| draw(frame, &data, &Screen::Base { page: Page::System }))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Remote hub"));
        assert!(rendered.contains("Separate token"));
        assert!(rendered.contains("16.0 MiB"));

        data.listening = "http://127.0.0.1:10100".into();
        terminal
            .draw(|frame| {
                draw(
                    frame,
                    &data,
                    &Screen::Base {
                        page: Page::Storage,
                    },
                )
            })
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Tracked files: 1"));
        assert!(rendered.contains("2.0 KiB"));
    }

    #[test]
    fn narrow_shell_uses_top_tabs() {
        let backend = TestBackend::new(72, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let data = DashboardData {
            config_sources: vec![],
            ide_targets: vec![],
            listening: "http://127.0.0.1:10100".into(),
            model_count: 0,
            provider_count: 0,
            autostart: AutoStartStatus::Off,
            run_in_background: true,
            proxy_targets: BTreeMap::new(),
            detected_sources: BTreeMap::new(),
            providers: vec![],
            disabled_local_providers: BTreeSet::new(),
            combos: vec![],
            models: vec![],
            disabled_models: BTreeSet::new(),
            subagent_catalog: crate::target_config::SubagentCatalogPolicy::default(),
            runtime: DashboardRuntimeSnapshot::default(),
            system: DashboardSystemSnapshot::default(),
            storage: DashboardStorageSnapshot::default(),
            request_events: vec![],
            port_warning: None,
        };
        terminal
            .draw(|frame| draw(frame, &data, &Screen::Base { page: Page::Logs }))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        let logs_index = Page::ALL
            .iter()
            .position(|page| *page == Page::Logs)
            .expect("Logs page should be present")
            + 1;
        assert!(rendered.contains("1 Overview"));
        assert!(rendered.contains(&format!("{logs_index} Logs")));
        assert!(rendered.contains("No requests yet"));
        assert!(rendered.contains("Requests routed through Joocode will appear here"));
    }

    #[test]
    fn logs_page_renders_scannable_request_table() {
        let backend = TestBackend::new(150, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut data = empty_dashboard_data();
        data.request_events = vec![DashboardRequestEvent {
            method: "POST".into(),
            path: "/v1/chat/completions".into(),
            status: 200,
            duration_ms: 8_112,
            provider: Some("joocode:ai".into()),
            model: Some("gpt-5.6-sol".into()),
            retries: 1,
            failovers: 0,
            input_tokens: Some(12_400),
            output_tokens: Some(842),
        }];
        terminal
            .draw(|frame| draw(frame, &data, &Screen::Base { page: Page::Logs }))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("STATUS"));
        assert!(rendered.contains("ENDPOINT"));
        assert!(rendered.contains("PROVIDER"));
        assert!(rendered.contains("LATENCY"));
        assert!(rendered.contains("/v1/chat/completions"));
        assert!(rendered.contains("gpt-5.6-sol"));
        assert!(rendered.contains("12.4k"));
        assert!(rendered.contains("8.1s"));
    }

    #[test]
    fn combo_builder_walks_name_strategy_models_and_review() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut data = empty_dashboard_data();
        data.models = vec![
            ModelInfo {
                id: "a/model".into(),
                provider: "a".into(),
                upstream_id: "model".into(),
                name: "A".into(),
                reasoning: false,
                context_window: None,
                max_output_tokens: None,
            },
            ModelInfo {
                id: "b/model".into(),
                provider: "b".into(),
                upstream_id: "model".into(),
                name: "B".into(),
                reasoning: false,
                context_window: None,
                max_output_tokens: None,
            },
        ];
        let mut screen = Screen::Providers { selected: 0 };
        handle_key_with_data(&mut screen, KeyCode::Char('c'), &tx, &data);
        assert!(matches!(screen, Screen::ComboName { .. }));
        for character in "coding".chars() {
            handle_key_with_data(&mut screen, KeyCode::Char(character), &tx, &data);
        }
        handle_key_with_data(&mut screen, KeyCode::Enter, &tx, &data);
        handle_key_with_data(&mut screen, KeyCode::Enter, &tx, &data);
        handle_key_with_data(&mut screen, KeyCode::Char(' '), &tx, &data);
        handle_key_with_data(&mut screen, KeyCode::Down, &tx, &data);
        handle_key_with_data(&mut screen, KeyCode::Char(' '), &tx, &data);
        handle_key_with_data(&mut screen, KeyCode::Enter, &tx, &data);
        assert!(matches!(screen, Screen::ComboReview { .. }));
        handle_key_with_data(&mut screen, KeyCode::Enter, &tx, &data);
        assert!(matches!(
            rx.try_recv(),
            Ok(DashboardCommand::SaveCombo { combo, .. })
                if combo.name == "coding" && combo.models.len() == 2
        ));
    }

    #[test]
    fn enter_opens_new_provider_modal() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut screen = Screen::Providers { selected: 1 };
        handle_key_with_data(
            &mut screen,
            KeyCode::Char('n'),
            &tx,
            &empty_dashboard_data(),
        );
        assert!(matches!(
            screen,
            Screen::ProviderBaseUrl {
                selected: 1,
                value
            } if value.is_empty()
        ));
    }

    #[test]
    fn space_toggles_run_in_background() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut screen = Screen::Config {
            selected: RUN_IN_BACKGROUND_ITEM,
        };
        handle_key(&mut screen, KeyCode::Char(' '), &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(DashboardCommand::ToggleRunInBackground)
        ));
    }

    #[test]
    fn space_toggles_selected_detected_provider() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut data = empty_dashboard_data();
        data.detected_sources.insert(SourceKind::OpenCode, true);
        let mut screen = Screen::Providers { selected: 0 };
        handle_key_with_data(&mut screen, KeyCode::Char(' '), &tx, &data);
        assert!(matches!(
            rx.try_recv(),
            Ok(DashboardCommand::ToggleSource {
                source: SourceKind::OpenCode
            })
        ));
    }

    #[test]
    fn source_event_refreshes_dashboard_catalog() {
        let mut data = DashboardData {
            config_sources: vec!["OpenCode".into(), "CrabCode".into()],
            ide_targets: vec![],
            listening: "http://127.0.0.1:10100".into(),
            model_count: 60,
            provider_count: 10,
            autostart: AutoStartStatus::Off,
            run_in_background: true,
            proxy_targets: BTreeMap::new(),
            detected_sources: SourceKind::DETECTED
                .into_iter()
                .map(|source| (source, true))
                .collect(),
            providers: vec![],
            disabled_local_providers: BTreeSet::new(),
            combos: vec![],
            models: vec![],
            disabled_models: BTreeSet::new(),
            subagent_catalog: crate::target_config::SubagentCatalogPolicy::default(),
            runtime: DashboardRuntimeSnapshot::default(),
            system: DashboardSystemSnapshot::default(),
            storage: DashboardStorageSnapshot::default(),
            request_events: vec![],
            port_warning: None,
        };
        let mut screen = Screen::Config {
            selected: FIRST_PROXY_ITEM,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let mut detected_sources = data.detected_sources.clone();
        detected_sources.insert(SourceKind::OpenCode, false);
        tx.send(DashboardEvent::ProviderControlsUpdated {
            config_sources: vec!["CrabCode".into()],
            model_count: 30,
            provider_count: 5,
            models: vec![],
            disabled_local_providers: BTreeSet::new(),
            combos: vec![],
            detected_sources,
        })
        .unwrap();

        receive_events(&mut data, &mut screen, &rx);

        assert_eq!(
            data.detected_sources.get(&SourceKind::OpenCode),
            Some(&false)
        );
        assert_eq!(data.config_sources, vec!["CrabCode"]);
        assert_eq!(data.model_count, 30);
        assert_eq!(data.provider_count, 5);
    }

    #[test]
    fn dashboard_shows_openai_compatible_endpoint() {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let data = DashboardData {
            config_sources: vec!["OpenCode".into()],
            ide_targets: vec!["Codex".into()],
            listening: "http://127.0.0.1:10123".into(),
            model_count: 30,
            provider_count: 5,
            autostart: AutoStartStatus::Off,
            run_in_background: true,
            proxy_targets: BTreeMap::new(),
            detected_sources: BTreeMap::new(),
            providers: vec![],
            disabled_local_providers: BTreeSet::new(),
            combos: vec![],
            models: vec![],
            disabled_models: BTreeSet::new(),
            subagent_catalog: crate::target_config::SubagentCatalogPolicy::default(),
            runtime: DashboardRuntimeSnapshot::default(),
            system: DashboardSystemSnapshot::default(),
            storage: DashboardStorageSnapshot::default(),
            request_events: vec![],
            port_warning: None,
        };

        terminal
            .draw(|frame| {
                draw(
                    frame,
                    &data,
                    &Screen::Base {
                        page: Page::Overview,
                    },
                )
            })
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("LOCAL GATEWAY"));
        assert!(rendered.contains("ONLINE"));
        assert!(rendered.contains("OpenAI"));
        assert!(rendered.contains("http://127.0.0.1:10123/v1"));
        assert!(rendered.contains("KEY"));
        assert!(rendered.contains("any non-empty value"));
    }

    #[test]
    fn wide_dashboard_keeps_ascii_logo_on_shorter_terminals() {
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let data = DashboardData {
            config_sources: vec!["OpenCode".into()],
            ide_targets: vec!["Codex".into()],
            listening: "http://127.0.0.1:10100".into(),
            model_count: 30,
            provider_count: 5,
            autostart: AutoStartStatus::Off,
            run_in_background: true,
            proxy_targets: BTreeMap::new(),
            detected_sources: BTreeMap::new(),
            providers: vec![],
            disabled_local_providers: BTreeSet::new(),
            combos: vec![],
            models: vec![],
            disabled_models: BTreeSet::new(),
            subagent_catalog: crate::target_config::SubagentCatalogPolicy::default(),
            runtime: DashboardRuntimeSnapshot::default(),
            system: DashboardSystemSnapshot::default(),
            storage: DashboardStorageSnapshot::default(),
            request_events: vec![],
            port_warning: None,
        };

        terminal
            .draw(|frame| {
                draw(
                    frame,
                    &data,
                    &Screen::Base {
                        page: Page::Overview,
                    },
                )
            })
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("/ /___"));
        assert!(rendered.contains("\\____/\\____/"));
    }

    #[test]
    fn dashboard_stacks_panels_on_narrow_terminals() {
        let backend = TestBackend::new(58, 28);
        let mut terminal = Terminal::new(backend).unwrap();
        let data = DashboardData {
            config_sources: vec!["OpenCode".into(), "Joocode".into()],
            ide_targets: vec!["Codex".into(), "Zed".into()],
            listening: "http://127.0.0.1:10100".into(),
            model_count: 30,
            provider_count: 5,
            autostart: AutoStartStatus::Off,
            run_in_background: true,
            proxy_targets: BTreeMap::new(),
            detected_sources: BTreeMap::new(),
            providers: vec![],
            disabled_local_providers: BTreeSet::new(),
            combos: vec![],
            models: vec![],
            disabled_models: BTreeSet::new(),
            subagent_catalog: crate::target_config::SubagentCatalogPolicy::default(),
            runtime: DashboardRuntimeSnapshot::default(),
            system: DashboardSystemSnapshot::default(),
            storage: DashboardStorageSnapshot::default(),
            request_events: vec![],
            port_warning: None,
        };

        terminal
            .draw(|frame| {
                draw(
                    frame,
                    &data,
                    &Screen::Base {
                        page: Page::Overview,
                    },
                )
            })
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("ROUTING"));
        assert!(rendered.contains("LOCAL GATEWAY"));
        assert!(rendered.contains("http://127.0.0.1:10100/v1"));
        assert!(rendered.contains("MODELS"));
        assert!(rendered.contains("PROVIDERS"));
    }

    #[test]
    fn delete_removes_selected_provider() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let providers = vec![
            ProviderSummary {
                name: "gunamaya".into(),
                label: "gunamaya.id".into(),
                model_count: 10,
                models: vec!["gpt-5.5".into()],
                default_model: None,
                key_count: 1,
            },
            ProviderSummary {
                name: "openai".into(),
                label: "openai.com".into(),
                model_count: 4,
                models: vec!["gpt-5.4".into()],
                default_model: None,
                key_count: 1,
            },
        ];
        let mut data = empty_dashboard_data();
        data.providers = providers;
        let mut screen = Screen::Providers {
            selected: SourceKind::DETECTED.len() + 1,
        };
        handle_key_with_data(&mut screen, KeyCode::Delete, &tx, &data);
        assert!(matches!(
            rx.try_recv(),
            Ok(DashboardCommand::RemoveProvider { name }) if name == "openai"
        ));
    }

    #[test]
    fn provider_manager_renders_domains() {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let data = DashboardData {
            config_sources: vec!["Joocode".into()],
            ide_targets: vec![],
            listening: "http://127.0.0.1:10100".into(),
            model_count: 14,
            provider_count: 2,
            autostart: AutoStartStatus::Off,
            run_in_background: true,
            proxy_targets: BTreeMap::new(),
            detected_sources: BTreeMap::new(),
            providers: vec![
                ProviderSummary {
                    name: "gunamaya".into(),
                    label: "gunamaya.id".into(),
                    model_count: 10,
                    models: vec!["gpt-5.5".into()],
                    default_model: Some("gpt-5.5".into()),
                    key_count: 1,
                },
                ProviderSummary {
                    name: "openai".into(),
                    label: "openai.com".into(),
                    model_count: 4,
                    models: vec!["gpt-5.4".into()],
                    default_model: None,
                    key_count: 1,
                },
            ],
            disabled_local_providers: BTreeSet::new(),
            combos: vec![],
            models: vec![],
            disabled_models: BTreeSet::new(),
            subagent_catalog: crate::target_config::SubagentCatalogPolicy::default(),
            runtime: DashboardRuntimeSnapshot::default(),
            system: DashboardSystemSnapshot::default(),
            storage: DashboardStorageSnapshot::default(),
            request_events: vec![],
            port_warning: None,
        };
        terminal
            .draw(|frame| draw(frame, &data, &Screen::Providers { selected: 0 }))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Providers"));
        assert!(rendered.contains("gunamaya.id"));
        assert!(rendered.contains("openai.com"));
        assert!(rendered.contains("DETECTED PROVIDERS"));
        assert!(rendered.contains("CUSTOM PROVIDERS"));
        assert!(rendered.contains("COMBOS"));
        assert!(!rendered.contains("secret"));
    }

    #[test]
    fn standard_modal_uses_opencode_inspired_visual_shell() {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let data = DashboardData {
            config_sources: vec![],
            ide_targets: vec![],
            listening: "http://127.0.0.1:10100".into(),
            model_count: 14,
            provider_count: 2,
            autostart: AutoStartStatus::Off,
            run_in_background: true,
            proxy_targets: BTreeMap::new(),
            detected_sources: BTreeMap::new(),
            providers: vec![ProviderSummary {
                name: "gunamaya".into(),
                label: "gunamaya.id".into(),
                model_count: 10,
                models: vec!["gpt-5.5".into()],
                default_model: None,
                key_count: 1,
            }],
            disabled_local_providers: BTreeSet::new(),
            combos: vec![],
            models: vec![],
            disabled_models: BTreeSet::new(),
            subagent_catalog: crate::target_config::SubagentCatalogPolicy::default(),
            runtime: DashboardRuntimeSnapshot::default(),
            system: DashboardSystemSnapshot::default(),
            storage: DashboardStorageSnapshot::default(),
            request_events: vec![],
            port_warning: None,
        };

        terminal
            .draw(|frame| draw(frame, &data, &Screen::Providers { selected: 0 }))
            .unwrap();
        let cells = terminal.backend().buffer().content();
        let rendered = cells.iter().map(|cell| cell.symbol()).collect::<String>();

        assert!(rendered.contains("Providers"));
        assert!(rendered.contains("DETECTED PROVIDERS"));
        assert!(rendered.contains("CUSTOM PROVIDERS"));
        assert!(rendered.contains("COMBOS"));
        assert!(cells.iter().any(|cell| cell.bg == Color::Reset));
        assert!(cells.iter().any(|cell| cell.bg == MODAL_ACCENT));
        assert!(!cells.iter().any(|cell| cell.bg == Color::Black));
    }

    #[test]
    fn successful_operation_uses_success_modal_instead_of_error_modal() {
        let mut data = DashboardData {
            config_sources: vec![],
            ide_targets: vec![],
            listening: "http://127.0.0.1:10100".into(),
            model_count: 0,
            provider_count: 0,
            autostart: AutoStartStatus::Off,
            run_in_background: true,
            proxy_targets: BTreeMap::new(),
            detected_sources: BTreeMap::new(),
            providers: vec![],
            disabled_local_providers: BTreeSet::new(),
            combos: vec![],
            models: vec![],
            disabled_models: BTreeSet::new(),
            subagent_catalog: crate::target_config::SubagentCatalogPolicy::default(),
            runtime: DashboardRuntimeSnapshot::default(),
            system: DashboardSystemSnapshot::default(),
            storage: DashboardStorageSnapshot::default(),
            request_events: vec![],
            port_warning: None,
        };
        let mut screen = Screen::default();
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(DashboardEvent::ProviderTested(
            "Codex synchronized: 52 Joocode models.".into(),
        ))
        .unwrap();

        receive_events(&mut data, &mut screen, &rx);
        assert!(matches!(screen, Screen::Success(_)));

        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &data, &screen)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Operation complete"));
        assert!(rendered.contains("Success"));
        assert!(!rendered.contains("Something went wrong"));
    }

    #[test]
    fn slash_opens_configuration_modal() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut screen = Screen::Base {
            page: Page::Overview,
        };
        handle_key(&mut screen, KeyCode::Char('/'), &tx);
        assert!(matches!(screen, Screen::Config { selected: 0 }));
    }

    #[test]
    fn space_toggles_selected_configuration_item() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut screen = Screen::Config { selected: 0 };
        handle_key(&mut screen, KeyCode::Char(' '), &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(DashboardCommand::ToggleAutoStart)
        ));
    }

    #[test]
    fn space_toggles_selected_proxy_target() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut screen = Screen::Config {
            selected: FIRST_PROXY_ITEM,
        };
        handle_key(&mut screen, KeyCode::Char(' '), &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(DashboardCommand::ToggleProxyTarget {
                target: ProxyTarget::Codex
            })
        ));
    }

    #[test]
    fn proxy_target_event_refreshes_dashboard_targets() {
        let mut data = DashboardData {
            config_sources: vec![],
            ide_targets: vec![],
            listening: "http://127.0.0.1:10100".into(),
            model_count: 0,
            provider_count: 0,
            autostart: AutoStartStatus::Off,
            run_in_background: true,
            proxy_targets: BTreeMap::new(),
            detected_sources: BTreeMap::new(),
            providers: vec![],
            disabled_local_providers: BTreeSet::new(),
            combos: vec![],
            models: vec![],
            disabled_models: BTreeSet::new(),
            subagent_catalog: crate::target_config::SubagentCatalogPolicy::default(),
            runtime: DashboardRuntimeSnapshot::default(),
            system: DashboardSystemSnapshot::default(),
            storage: DashboardStorageSnapshot::default(),
            request_events: vec![],
            port_warning: None,
        };
        let mut screen = Screen::Base {
            page: Page::Overview,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(DashboardEvent::ProxyTargetUpdated {
            target: ProxyTarget::GrokBuild,
            enabled: true,
        })
        .unwrap();

        receive_events(&mut data, &mut screen, &rx);

        assert_eq!(data.ide_targets, vec!["Grok Build"]);
        assert_eq!(data.proxy_targets.get(&ProxyTarget::GrokBuild), Some(&true));
    }

    #[test]
    fn configuration_modal_renders_grouped_targets() {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let data = DashboardData {
            config_sources: vec!["OpenCode".into()],
            ide_targets: vec!["Codex".into()],
            listening: "http://127.0.0.1:10100".into(),
            model_count: 30,
            provider_count: 5,
            autostart: AutoStartStatus::On,
            run_in_background: true,
            proxy_targets: ProxyTarget::ALL
                .into_iter()
                .map(|target| (target, target == ProxyTarget::Codex))
                .collect(),
            detected_sources: SourceKind::DETECTED
                .into_iter()
                .map(|source| (source, source == SourceKind::OpenCode))
                .collect(),
            providers: vec![],
            disabled_local_providers: BTreeSet::new(),
            combos: vec![],
            models: vec![],
            disabled_models: BTreeSet::new(),
            subagent_catalog: crate::target_config::SubagentCatalogPolicy::default(),
            runtime: DashboardRuntimeSnapshot::default(),
            system: DashboardSystemSnapshot::default(),
            storage: DashboardStorageSnapshot::default(),
            request_events: vec![],
            port_warning: None,
        };
        terminal
            .draw(|frame| draw(frame, &data, &Screen::Config { selected: 1 }))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        for label in [
            "Setting",
            "Run in background",
            "Proxy to",
            "Codex",
            "GitHub Copilot App",
            "JetBrains",
            "Antigravity",
            "Zed",
            "Claude Code",
            "Grok Build",
        ] {
            assert!(rendered.contains(label), "missing {label}");
        }
    }

    #[test]
    fn configuration_modal_scrolls_to_selected_item_in_small_terminal() {
        let backend = TestBackend::new(52, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        let data = DashboardData {
            config_sources: vec!["OpenCode".into()],
            ide_targets: vec!["Codex".into()],
            listening: "http://127.0.0.1:10100".into(),
            model_count: 30,
            provider_count: 5,
            autostart: AutoStartStatus::On,
            run_in_background: true,
            proxy_targets: ProxyTarget::ALL
                .into_iter()
                .map(|target| (target, true))
                .collect(),
            detected_sources: SourceKind::DETECTED
                .into_iter()
                .map(|source| (source, true))
                .collect(),
            providers: vec![],
            disabled_local_providers: BTreeSet::new(),
            combos: vec![],
            models: vec![],
            disabled_models: BTreeSet::new(),
            subagent_catalog: crate::target_config::SubagentCatalogPolicy::default(),
            runtime: DashboardRuntimeSnapshot::default(),
            system: DashboardSystemSnapshot::default(),
            storage: DashboardStorageSnapshot::default(),
            request_events: vec![],
            port_warning: None,
        };
        let selected = FIRST_PROXY_ITEM + ProxyTarget::ALL.len() - 1;
        terminal
            .draw(|frame| draw(frame, &data, &Screen::Config { selected }))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("Grok Build"));
        assert!(!rendered.contains("Setting"));
    }

    #[test]
    fn autostart_event_refreshes_dashboard_status() {
        let mut data = DashboardData {
            config_sources: vec![],
            ide_targets: vec![],
            listening: "http://127.0.0.1:10100".into(),
            model_count: 0,
            provider_count: 0,
            autostart: AutoStartStatus::Off,
            run_in_background: true,
            proxy_targets: BTreeMap::new(),
            detected_sources: BTreeMap::new(),
            providers: vec![],
            disabled_local_providers: BTreeSet::new(),
            combos: vec![],
            models: vec![],
            disabled_models: BTreeSet::new(),
            subagent_catalog: crate::target_config::SubagentCatalogPolicy::default(),
            runtime: DashboardRuntimeSnapshot::default(),
            system: DashboardSystemSnapshot::default(),
            storage: DashboardStorageSnapshot::default(),
            request_events: vec![],
            port_warning: None,
        };
        let mut screen = Screen::Base {
            page: Page::Overview,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(DashboardEvent::AutoStartUpdated(AutoStartStatus::On))
            .unwrap();

        receive_events(&mut data, &mut screen, &rx);

        assert_eq!(data.autostart, AutoStartStatus::On);
    }

    #[test]
    fn background_event_refreshes_dashboard_status() {
        let mut data = DashboardData {
            config_sources: vec![],
            ide_targets: vec![],
            listening: "http://127.0.0.1:10100".into(),
            model_count: 0,
            provider_count: 0,
            autostart: AutoStartStatus::Off,
            run_in_background: true,
            proxy_targets: BTreeMap::new(),
            detected_sources: BTreeMap::new(),
            providers: vec![],
            disabled_local_providers: BTreeSet::new(),
            combos: vec![],
            models: vec![],
            disabled_models: BTreeSet::new(),
            subagent_catalog: crate::target_config::SubagentCatalogPolicy::default(),
            runtime: DashboardRuntimeSnapshot::default(),
            system: DashboardSystemSnapshot::default(),
            storage: DashboardStorageSnapshot::default(),
            request_events: vec![],
            port_warning: None,
        };
        let mut screen = Screen::Base {
            page: Page::Overview,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(DashboardEvent::RunInBackgroundUpdated(false))
            .unwrap();

        receive_events(&mut data, &mut screen, &rx);

        assert!(!data.run_in_background);
    }

    #[test]
    fn toggle_command_is_available_to_the_dashboard_worker() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tx.send(DashboardCommand::ToggleAutoStart).unwrap();
        assert!(matches!(
            rx.try_recv(),
            Ok(DashboardCommand::ToggleAutoStart)
        ));
    }

    #[test]
    fn paste_populates_wizard_fields_without_newlines() {
        let mut screen = Screen::ProviderBaseUrl {
            selected: 0,
            value: String::new(),
        };
        handle_paste(&mut screen, "https://example.test/v1\n");
        assert!(matches!(
            screen,
            Screen::ProviderBaseUrl { value, .. } if value == "https://example.test/v1"
        ));
    }

    #[test]
    fn provider_event_refreshes_dashboard_totals() {
        let mut data = DashboardData {
            config_sources: vec!["OpenCode".into()],
            ide_targets: vec!["Zed".into()],
            listening: "http://127.0.0.1:10100".into(),
            model_count: 30,
            provider_count: 5,
            autostart: AutoStartStatus::Off,
            run_in_background: true,
            proxy_targets: BTreeMap::new(),
            detected_sources: BTreeMap::new(),
            providers: vec![],
            disabled_local_providers: BTreeSet::new(),
            combos: vec![],
            models: vec![],
            disabled_models: BTreeSet::new(),
            subagent_catalog: crate::target_config::SubagentCatalogPolicy::default(),
            runtime: DashboardRuntimeSnapshot::default(),
            system: DashboardSystemSnapshot::default(),
            storage: DashboardStorageSnapshot::default(),
            request_events: vec![],
            port_warning: None,
        };
        let mut screen = Screen::Base {
            page: Page::Overview,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(DashboardEvent::ProviderAdded {
            provider: "local".into(),
            config_sources: vec!["OpenCode".into(), "Joocode".into()],
            model_count: 31,
            provider_count: 6,
            providers: vec![ProviderSummary {
                name: "local".into(),
                label: "example.test".into(),
                model_count: 1,
                models: vec!["model-a".into()],
                default_model: None,
                key_count: 1,
            }],
        })
        .unwrap();

        receive_events(&mut data, &mut screen, &rx);

        assert_eq!(data.model_count, 31);
        assert_eq!(data.provider_count, 6);
        assert_eq!(data.config_sources, vec!["OpenCode", "Joocode"]);
        assert_eq!(data.providers[0].label, "example.test");
    }

    #[test]
    fn hidden_words_open_easter_egg_modal() {
        for trigger in EASTER_EGG_TRIGGERS {
            let mut screen = Screen::Base {
                page: Page::Overview,
            };
            let mut input = String::new();
            for character in trigger.chars() {
                detect_easter_egg(&mut screen, &mut input, KeyCode::Char(character));
            }
            assert!(matches!(screen, Screen::EasterEgg { tick: 0 }));
        }
    }

    #[test]
    fn unrelated_input_does_not_open_easter_egg_modal() {
        let mut screen = Screen::Base {
            page: Page::Overview,
        };
        let mut input = String::new();
        for character in "joocode".chars() {
            detect_easter_egg(&mut screen, &mut input, KeyCode::Char(character));
        }
        assert!(matches!(
            screen,
            Screen::Base {
                page: Page::Overview
            }
        ));
    }

    #[test]
    fn update_event_opens_the_update_prompt() {
        let mut data = DashboardData {
            config_sources: vec![],
            ide_targets: vec![],
            listening: "http://127.0.0.1:10100".into(),
            model_count: 0,
            provider_count: 0,
            autostart: AutoStartStatus::Off,
            run_in_background: true,
            proxy_targets: BTreeMap::new(),
            detected_sources: BTreeMap::new(),
            providers: vec![],
            disabled_local_providers: BTreeSet::new(),
            combos: vec![],
            models: vec![],
            disabled_models: BTreeSet::new(),
            subagent_catalog: crate::target_config::SubagentCatalogPolicy::default(),
            runtime: DashboardRuntimeSnapshot::default(),
            system: DashboardSystemSnapshot::default(),
            storage: DashboardStorageSnapshot::default(),
            request_events: vec![],
            port_warning: None,
        };
        let mut screen = Screen::Base {
            page: Page::Overview,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(DashboardEvent::UpdateAvailable("v0.2.0".into()))
            .unwrap();

        assert_eq!(receive_events(&mut data, &mut screen, &rx), None);
        assert!(matches!(screen, Screen::UpdateAvailable(tag) if tag == "v0.2.0"));
    }

    #[test]
    fn enter_accepts_an_available_update() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut screen = Screen::UpdateAvailable("v0.2.0".into());

        handle_key(&mut screen, KeyCode::Enter, &tx);

        assert!(matches!(
            screen,
            Screen::Updating { tag, tick: 0 } if tag == "v0.2.0"
        ));
        assert!(matches!(
            rx.try_recv(),
            Ok(DashboardCommand::InstallUpdate { tag }) if tag == "v0.2.0"
        ));
    }

    #[test]
    fn installed_update_requests_process_restart() {
        let mut data = DashboardData {
            config_sources: vec![],
            ide_targets: vec![],
            listening: "http://127.0.0.1:10100".into(),
            model_count: 0,
            provider_count: 0,
            autostart: AutoStartStatus::Off,
            run_in_background: true,
            proxy_targets: BTreeMap::new(),
            detected_sources: BTreeMap::new(),
            providers: vec![],
            disabled_local_providers: BTreeSet::new(),
            combos: vec![],
            models: vec![],
            disabled_models: BTreeSet::new(),
            subagent_catalog: crate::target_config::SubagentCatalogPolicy::default(),
            runtime: DashboardRuntimeSnapshot::default(),
            system: DashboardSystemSnapshot::default(),
            storage: DashboardStorageSnapshot::default(),
            request_events: vec![],
            port_warning: None,
        };
        let mut screen = Screen::Updating {
            tag: "v0.2.0".into(),
            tick: 0,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(DashboardEvent::UpdateInstalled).unwrap();

        assert_eq!(
            receive_events(&mut data, &mut screen, &rx),
            Some(DashboardExit::Restart)
        );
    }

    #[test]
    fn easter_egg_replaces_the_entire_dashboard() {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let data = DashboardData {
            config_sources: vec!["OpenCode".into(), "Joocode".into()],
            ide_targets: vec!["Codex".into(), "Zed".into()],
            listening: "http://127.0.0.1:10100".into(),
            model_count: 30,
            provider_count: 5,
            autostart: AutoStartStatus::Off,
            run_in_background: true,
            proxy_targets: BTreeMap::new(),
            detected_sources: BTreeMap::new(),
            providers: vec![],
            disabled_local_providers: BTreeSet::new(),
            combos: vec![],
            models: vec![],
            disabled_models: BTreeSet::new(),
            subagent_catalog: crate::target_config::SubagentCatalogPolicy::default(),
            runtime: DashboardRuntimeSnapshot::default(),
            system: DashboardSystemSnapshot::default(),
            storage: DashboardStorageSnapshot::default(),
            request_events: vec![],
            port_warning: None,
        };

        terminal
            .draw(|frame| draw(frame, &data, &Screen::EasterEgg { tick: 4 }))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("JOOCODE SECRET MODE"));
        assert!(!rendered.contains("Listening:"));
        assert!(!rendered.contains("Config:"));
    }

    #[test]
    fn updating_replaces_the_dashboard_with_rainbow_progress() {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let data = DashboardData {
            config_sources: vec!["OpenCode".into()],
            ide_targets: vec!["Codex".into()],
            listening: "http://127.0.0.1:10100".into(),
            model_count: 30,
            provider_count: 5,
            autostart: AutoStartStatus::Off,
            run_in_background: true,
            proxy_targets: BTreeMap::new(),
            detected_sources: BTreeMap::new(),
            providers: vec![],
            disabled_local_providers: BTreeSet::new(),
            combos: vec![],
            models: vec![],
            disabled_models: BTreeSet::new(),
            subagent_catalog: crate::target_config::SubagentCatalogPolicy::default(),
            runtime: DashboardRuntimeSnapshot::default(),
            system: DashboardSystemSnapshot::default(),
            storage: DashboardStorageSnapshot::default(),
            request_events: vec![],
            port_warning: None,
        };

        terminal
            .draw(|frame| {
                draw(
                    frame,
                    &data,
                    &Screen::Updating {
                        tag: "v0.2.0".into(),
                        tick: 5,
                    },
                )
            })
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("RAINBOW UPGRADE"));
        assert!(rendered.contains("Upgrading Joocode to v0.2.0"));
        assert!(!rendered.contains("Listening:"));
        assert!(!rendered.contains("Config:"));
    }
}
