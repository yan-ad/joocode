use std::{
    collections::BTreeMap,
    io::IsTerminal,
    net::SocketAddr,
    sync::mpsc::{Receiver, TryRecvError},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Wrap,
    },
};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    autostart::{self, Status as AutoStartStatus},
    desktop::DesktopTargets,
    local_config::{self, ProviderSummary},
    provider::Registry,
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
    pub port_warning: Option<String>,
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

const MODAL_BACKGROUND: Color = Color::Rgb(24, 42, 59);
const MODAL_OVERLAY: Color = Color::Rgb(13, 17, 19);
const MODAL_ACCENT: Color = Color::Rgb(62, 139, 255);
const PANEL_BACKGROUND: Color = Color::Rgb(19, 25, 28);
const PANEL_BORDER: Color = Color::Rgb(48, 61, 66);
const MUTED_TEXT: Color = Color::Rgb(120, 132, 136);

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
            .style(Style::default().fg(Color::Rgb(39, 71, 96)))
            .thumb_style(Style::default().fg(Color::Gray)),
        area,
        &mut state,
    );
}

fn header_animation_tick() -> usize {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    (elapsed.as_millis() / 450) as usize
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, tick: usize) {
    let icons = ["🦀", "🌴", "🦀", "🌴"];
    let colors = [
        Color::LightRed,
        Color::LightGreen,
        Color::LightMagenta,
        Color::LightCyan,
    ];
    let frame_index = tick % icons.len();
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    format!("{} ", icons[frame_index]),
                    Style::default()
                        .fg(colors[frame_index])
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "Joocode",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::raw("   "),
                Span::styled(
                    format!("v{}", env!("CARGO_PKG_VERSION")),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" · running", Style::default().fg(Color::DarkGray)),
            ]),
        ])
        .block(Block::default().borders(Borders::BOTTOM)),
        area,
    );
}

fn draw_providers(frame: &mut Frame<'_>, providers: &[ProviderSummary], selected: usize) {
    let default_model = providers
        .get(selected)
        .and_then(|provider| provider.default_model.as_deref());
    let modal = draw_modal_shell(
        frame,
        "Providers",
        96,
        30,
        Line::from(vec![
            Span::styled("Enter", Style::default().fg(MODAL_ACCENT)),
            Span::raw(" new provider    "),
            Span::styled("Del", Style::default().fg(Color::LightRed)),
            Span::raw(" remove    "),
            Span::styled("\\", Style::default().fg(MODAL_ACCENT)),
            Span::raw(match default_model {
                Some(model) => format!(" {model}"),
                None => " Set default model".to_owned(),
            }),
        ]),
    );
    let items = if providers.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            "No providers configured. Press Enter to add one.",
            Style::default().fg(Color::DarkGray),
        )))]
    } else {
        providers
            .iter()
            .enumerate()
            .map(|(index, provider)| {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        &provider.label,
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  {} models", provider.model_count),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(
                        provider
                            .default_model
                            .as_ref()
                            .map(|model| format!("  \\ {model}"))
                            .unwrap_or_default(),
                        Style::default().fg(Color::LightCyan),
                    ),
                ]))
                .style(selected_style(index == selected))
            })
            .collect()
    };
    let mut state = ListState::default().with_selected((!providers.is_empty()).then_some(selected));
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
    draw_modal_scrollbar(frame, modal.content, providers.len(), selected);
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
        Color::Rgb(255, 75, 118),
        Color::Rgb(255, 154, 72),
        Color::Rgb(255, 226, 89),
        Color::Rgb(75, 216, 146),
        Color::Rgb(72, 177, 255),
        Color::Rgb(154, 112, 255),
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
                .style(Style::default().bg(Color::Rgb(15, 18, 28)))
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
const FIRST_SOURCE_ITEM: usize = 2;
const FIRST_PROXY_ITEM: usize = FIRST_SOURCE_ITEM + SourceKind::DETECTED.len();

fn config_items() -> Vec<usize> {
    (AUTO_START_ITEM..FIRST_PROXY_ITEM + ProxyTarget::ALL.len()).collect()
}

fn source_for_config_item(item: usize) -> Option<SourceKind> {
    item.checked_sub(FIRST_SOURCE_ITEM)
        .and_then(|index| SourceKind::DETECTED.get(index))
        .copied()
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
    if let Some(index) = item.checked_sub(FIRST_SOURCE_ITEM)
        && index < SourceKind::DETECTED.len()
    {
        return 5 + index;
    }
    item.checked_sub(FIRST_PROXY_ITEM)
        .map(|index| 7 + SourceKind::DETECTED.len() + index)
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
        ListItem::new(Line::from("")),
        ListItem::new(Line::from(Span::styled(
            "Detected Providers",
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        ))),
    ];
    for (index, source) in SourceKind::DETECTED.into_iter().enumerate() {
        let enabled = data.detected_sources.get(&source).copied().unwrap_or(false);
        let marker = if enabled { "●" } else { "○" };
        items.push(
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{marker} "),
                    Style::default().fg(if enabled {
                        Color::Green
                    } else {
                        Color::DarkGray
                    }),
                ),
                Span::raw(source.label()),
                Span::raw(format!(" ({})", if enabled { "On" } else { "Off" })),
            ]))
            .style(selected_style(selected == FIRST_SOURCE_ITEM + index)),
        );
    }
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
        9 + SourceKind::DETECTED.len() + ProxyTarget::ALL.len(),
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
        Color::Rgb(255, 72, 120),
        Color::Rgb(255, 145, 64),
        Color::Rgb(255, 221, 74),
        Color::Rgb(73, 211, 137),
        Color::Rgb(73, 174, 255),
        Color::Rgb(145, 105, 255),
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
                .style(Style::default().bg(Color::Rgb(15, 18, 28)))
                .borders(Borders::ALL),
        )
        .wrap(Wrap { trim: true }),
        message_area,
    );
}

fn detect_easter_egg(screen: &mut Screen, input: &mut String, key: KeyCode) -> bool {
    if !matches!(screen, Screen::Dashboard) {
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
        Self {
            config_sources: config_sources(registry),
            ide_targets: targets.names().into_iter().map(str::to_owned).collect(),
            listening: format!("http://{address}"),
            model_count: registry.models().len(),
            provider_count: registry.provider_count(),
            autostart: autostart::status(),
            run_in_background: crate::target_config::TargetPreferences::load()
                .unwrap_or_default()
                .run_in_background,
            proxy_targets: ProxyTarget::ALL
                .into_iter()
                .map(|target| (target, targets.enabled(target)))
                .collect(),
            detected_sources: SourceKind::DETECTED
                .into_iter()
                .map(|source| (source, selection.enabled(source)))
                .collect(),
            providers: local_config::summaries().unwrap_or_default(),
            port_warning,
        }
    }
}

#[derive(Debug)]
pub enum DashboardCommand {
    AddProvider { base_url: String, api_key: String },
    RemoveProvider { name: String },
    SetDefaultModel { provider: String, model: String },
    ToggleAutoStart,
    ToggleRunInBackground,
    ToggleSource { source: SourceKind },
    ToggleProxyTarget { target: ProxyTarget },
    InstallUpdate { tag: String },
}

#[derive(Debug)]
pub enum DashboardEvent {
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
    SourceUpdated {
        source: SourceKind,
        enabled: bool,
        config_sources: Vec<String>,
        model_count: usize,
        provider_count: usize,
    },
    ProxyTargetUpdated {
        target: ProxyTarget,
        enabled: bool,
    },
    UpdateAvailable(String),
    UpdateInstalled,
    ShutdownRequested,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DashboardExit {
    Quit,
    Restart,
}

#[derive(Default)]
enum Screen {
    #[default]
    Dashboard,
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
    ProviderLoading {
        selected: usize,
    },
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
    let mut screen = Screen::Dashboard;
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
                if matches!(screen, Screen::Dashboard) {
                    return Ok::<DashboardExit, std::io::Error>(DashboardExit::Quit);
                }
                screen = match screen {
                    Screen::ProviderBaseUrl { selected, .. }
                    | Screen::ProviderApiKey { selected, .. } => Screen::Providers { selected },
                    Screen::ProviderModels {
                        provider_selected, ..
                    } => Screen::Providers {
                        selected: provider_selected,
                    },
                    _ => Screen::Dashboard,
                };
                continue;
            }
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                return Ok(DashboardExit::Quit);
            }
            if detect_easter_egg(&mut screen, &mut secret_input, key.code) {
                continue;
            }
            handle_key_with_providers(&mut screen, key.code, &command_tx, &data.providers);
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
                let selected = data
                    .providers
                    .iter()
                    .position(|entry| entry.name == provider)
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
                    | Screen::ProviderApiKey { selected, .. } => *selected,
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
                    .unwrap_or_default();
                *screen = Screen::Providers { selected };
            }
            Ok(DashboardEvent::ProviderError(error)) => *screen = Screen::Error(error),
            Ok(DashboardEvent::AutoStartUpdated(status)) => data.autostart = status,
            Ok(DashboardEvent::RunInBackgroundUpdated(enabled)) => {
                data.run_in_background = enabled;
            }
            Ok(DashboardEvent::SourceUpdated {
                source,
                enabled,
                config_sources,
                model_count,
                provider_count,
            }) => {
                data.detected_sources.insert(source, enabled);
                data.config_sources = config_sources;
                data.model_count = model_count;
                data.provider_count = provider_count;
            }
            Ok(DashboardEvent::ProxyTargetUpdated { target, enabled }) => {
                data.proxy_targets.insert(target, enabled);
                data.ide_targets = ProxyTarget::ALL
                    .into_iter()
                    .filter(|target| data.proxy_targets.get(target).copied().unwrap_or(false))
                    .map(|target| target.label().to_owned())
                    .collect();
            }
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

fn handle_key_with_providers(
    screen: &mut Screen,
    key: KeyCode,
    command_tx: &UnboundedSender<DashboardCommand>,
    providers: &[ProviderSummary],
) {
    match screen {
        Screen::Dashboard if key == KeyCode::Tab => *screen = Screen::Providers { selected: 0 },
        Screen::Dashboard if key == KeyCode::Char('/') => *screen = Screen::Config { selected: 0 },
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
                if let Some(source) = source_for_config_item(*selected) {
                    let _ = command_tx.send(DashboardCommand::ToggleSource { source });
                } else if let Some(target) = target_for_config_item(*selected) {
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
        Screen::Providers { selected } => match key {
            KeyCode::Up => *selected = selected.saturating_sub(1),
            KeyCode::Down => {
                *selected = selected
                    .saturating_add(1)
                    .min(providers.len().saturating_sub(1));
            }
            KeyCode::Enter => {
                *screen = Screen::ProviderBaseUrl {
                    selected: *selected,
                    value: String::new(),
                };
            }
            KeyCode::Delete | KeyCode::Backspace => {
                if let Some(provider) = providers.get(*selected) {
                    let _ = command_tx.send(DashboardCommand::RemoveProvider {
                        name: provider.name.clone(),
                    });
                }
            }
            KeyCode::Char('\\') => {
                if let Some(provider) = providers.get(*selected)
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
                        provider_selected: *selected,
                        model_selected,
                    };
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
        Screen::Error(_) | Screen::EasterEgg { .. } if key == KeyCode::Enter => {
            *screen = Screen::Dashboard;
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

    let [header, body, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(7),
        Constraint::Length(3),
    ])
    .areas(frame.area());

    draw_header(frame, header, header_animation_tick());

    match screen {
        Screen::Dashboard => draw_dashboard(frame, body, data),
        Screen::Config { selected } => {
            draw_dashboard(frame, body, data);
            draw_config(frame, data, *selected);
        }
        Screen::Providers { selected } => draw_providers(frame, &data.providers, *selected),
        Screen::ProviderModels {
            provider_selected,
            model_selected,
        } => {
            draw_providers(frame, &data.providers, *provider_selected);
            if let Some(provider) = data.providers.get(*provider_selected) {
                draw_provider_models(frame, provider, *model_selected);
            }
        }
        Screen::ProviderBaseUrl { selected, value } => {
            draw_providers(frame, &data.providers, *selected);
            draw_provider_input(frame, "Step 1/3 — Base URL", value, false);
        }
        Screen::ProviderApiKey {
            selected, api_key, ..
        } => {
            draw_providers(frame, &data.providers, *selected);
            draw_provider_input(frame, "Step 2/3 — API key", api_key, true);
        }
        Screen::ProviderLoading { selected } => {
            draw_providers(frame, &data.providers, *selected);
            draw_provider_loading(frame);
        }
        Screen::UpdateAvailable(tag) => {
            draw_dashboard(frame, body, data);
            draw_update_prompt(frame, tag);
        }
        Screen::Updating { .. } => {
            unreachable!("updating is rendered as a full-screen animated scene")
        }
        Screen::EasterEgg { .. } => unreachable!("easter egg is rendered as a full-screen scene"),
        Screen::Error(error) => draw_error(frame, error),
    }

    if !matches!(screen, Screen::Dashboard) {
        return;
    }

    let help = match screen {
        Screen::Dashboard => vec![
            Span::styled(
                " esc ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  Exit    ", Style::default().fg(MUTED_TEXT)),
            Span::styled(
                " tab ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  Providers    ", Style::default().fg(MUTED_TEXT)),
            Span::styled(
                " / ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  Config", Style::default().fg(MUTED_TEXT)),
        ],
        Screen::Config { .. }
        | Screen::Providers { .. }
        | Screen::ProviderModels { .. }
        | Screen::ProviderBaseUrl { .. }
        | Screen::ProviderApiKey { .. }
        | Screen::ProviderLoading { .. }
        | Screen::UpdateAvailable(_)
        | Screen::Error(_) => unreachable!("modal screens return before global footer rendering"),
        Screen::Updating { .. } => {
            unreachable!("updating has its own full-screen progress scene")
        }
        Screen::EasterEgg { .. } => unreachable!("easter egg has its own full-screen controls"),
    };
    frame.render_widget(
        Paragraph::new(Line::from(help)),
        footer.inner(Margin::new(1, 1)),
    );
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
    let canvas = area.inner(Margin::new(1, 1));
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Rgb(15, 19, 21))),
        area,
    );

    let warning_height = u16::from(data.port_warning.is_some()) * 3;
    let [warning_area, content_area, stats_area] = Layout::vertical([
        Constraint::Length(warning_height),
        Constraint::Length(if canvas.width >= 76 { 8 } else { 14 }),
        Constraint::Length(5),
    ])
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
            Layout::vertical([Constraint::Length(6), Constraint::Length(8)]).areas(content_area);
        draw_routing_panel(frame, routing_area, &sources, &targets);
        draw_gateway_panel(frame, gateway_area, &data.listening, &openai_compatible);
    }

    draw_stats(frame, stats_area, data.model_count, data.provider_count);
}

fn dashboard_panel(title: &'static str, accent: Color) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
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
            Line::from(vec![
                Span::styled("◆  Sources   ", Style::default().fg(MUTED_TEXT)),
                Span::styled(sources, Style::default().fg(Color::White)),
            ]),
            Line::from(vec![
                Span::styled("⌘  Targets   ", Style::default().fg(MUTED_TEXT)),
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
                    "●  ONLINE    ",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(listening, Style::default().fg(Color::LightGreen)),
            ]),
            Line::from(vec![
                Span::styled("↗  OpenAI    ", Style::default().fg(MUTED_TEXT)),
                Span::styled(openai_compatible, Style::default().fg(Color::LightCyan)),
            ]),
            Line::from(vec![
                Span::styled("◇  API key   ", Style::default().fg(MUTED_TEXT)),
                Span::styled("any non-empty value", Style::default().fg(Color::Gray)),
                Span::styled("  ·  e.g. joocode", Style::default().fg(Color::DarkGray)),
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
        let panel = dashboard_panel(" STATUS ", Color::Green);
        let inner = panel.inner(status_area);
        frame.render_widget(panel, status_area);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("● ", Style::default().fg(Color::Green)),
                Span::styled("Ready for connections", Style::default().fg(Color::Gray)),
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
        Paragraph::new(vec![
            Line::from(Span::styled(label, Style::default().fg(MUTED_TEXT))),
            Line::from(vec![
                Span::styled(format!("{icon} "), Style::default().fg(accent)),
                Span::styled(
                    value.to_string(),
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                ),
            ]),
        ])
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

    #[test]
    fn backslash_opens_default_model_picker_and_enter_selects_model() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let providers = vec![ProviderSummary {
            name: "gunamaya".into(),
            label: "gunamaya.id".into(),
            model_count: 2,
            models: vec!["gpt-5.4".into(), "gpt-5.5".into()],
            default_model: None,
        }];
        let mut screen = Screen::Providers { selected: 0 };
        handle_key_with_providers(&mut screen, KeyCode::Char('\\'), &tx, &providers);
        assert!(matches!(
            screen,
            Screen::ProviderModels {
                provider_selected: 0,
                model_selected: 0
            }
        ));
        handle_key_with_providers(&mut screen, KeyCode::Down, &tx, &providers);
        handle_key_with_providers(&mut screen, KeyCode::Enter, &tx, &providers);
        assert!(matches!(
            rx.try_recv(),
            Ok(DashboardCommand::SetDefaultModel { provider, model })
                if provider == "gunamaya" && model == "gpt-5.5"
        ));
    }

    #[test]
    fn header_shows_running_version_and_animated_icons() {
        let backend = TestBackend::new(80, 5);
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
        assert!(first.contains("🦀"));
        assert!(first.contains("Joocode"));
        assert!(first.contains(&format!("v{}", env!("CARGO_PKG_VERSION"))));
        assert!(first.contains("running"));

        terminal
            .draw(|frame| draw_header(frame, frame.area(), 1))
            .unwrap();
        let second = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(second.contains("🌴"));
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
    fn tab_opens_provider_manager() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut screen = Screen::Dashboard;
        handle_key(&mut screen, KeyCode::Tab, &tx);
        assert!(matches!(screen, Screen::Providers { selected: 0 }));
    }

    #[test]
    fn enter_opens_new_provider_modal() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut screen = Screen::Providers { selected: 1 };
        handle_key_with_providers(&mut screen, KeyCode::Enter, &tx, &[]);
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
        let mut screen = Screen::Config {
            selected: FIRST_SOURCE_ITEM,
        };
        handle_key(&mut screen, KeyCode::Char(' '), &tx);
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
            port_warning: None,
        };
        let mut screen = Screen::Config {
            selected: FIRST_SOURCE_ITEM,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(DashboardEvent::SourceUpdated {
            source: SourceKind::OpenCode,
            enabled: false,
            config_sources: vec!["CrabCode".into()],
            model_count: 30,
            provider_count: 5,
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
            port_warning: None,
        };

        terminal
            .draw(|frame| draw(frame, &data, &Screen::Dashboard))
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
        assert!(rendered.contains("API key"));
        assert!(rendered.contains("any non-empty value"));
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
            port_warning: None,
        };

        terminal
            .draw(|frame| draw(frame, &data, &Screen::Dashboard))
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
            },
            ProviderSummary {
                name: "openai".into(),
                label: "openai.com".into(),
                model_count: 4,
                models: vec!["gpt-5.4".into()],
                default_model: None,
            },
        ];
        let mut screen = Screen::Providers { selected: 1 };
        handle_key_with_providers(&mut screen, KeyCode::Delete, &tx, &providers);
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
                },
                ProviderSummary {
                    name: "openai".into(),
                    label: "openai.com".into(),
                    model_count: 4,
                    models: vec!["gpt-5.4".into()],
                    default_model: None,
                },
            ],
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
            }],
            port_warning: None,
        };

        terminal
            .draw(|frame| draw(frame, &data, &Screen::Providers { selected: 0 }))
            .unwrap();
        let cells = terminal.backend().buffer().content();
        let rendered = cells.iter().map(|cell| cell.symbol()).collect::<String>();

        assert!(rendered.contains("Providers"));
        assert!(rendered.contains("esc"));
        assert!(!rendered.contains('┌'));
        assert!(cells.iter().any(|cell| cell.bg == MODAL_BACKGROUND));
        assert!(cells.iter().any(|cell| cell.bg == MODAL_ACCENT));
    }

    #[test]
    fn slash_opens_configuration_modal() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut screen = Screen::Dashboard;
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
            port_warning: None,
        };
        let mut screen = Screen::Dashboard;
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
            "Detected Providers",
            "OpenCode",
            "CrabCode",
            "Proxy to",
            "Codex",
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
            port_warning: None,
        };
        let mut screen = Screen::Dashboard;
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
            port_warning: None,
        };
        let mut screen = Screen::Dashboard;
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
            port_warning: None,
        };
        let mut screen = Screen::Dashboard;
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
            let mut screen = Screen::Dashboard;
            let mut input = String::new();
            for character in trigger.chars() {
                detect_easter_egg(&mut screen, &mut input, KeyCode::Char(character));
            }
            assert!(matches!(screen, Screen::EasterEgg { tick: 0 }));
        }
    }

    #[test]
    fn unrelated_input_does_not_open_easter_egg_modal() {
        let mut screen = Screen::Dashboard;
        let mut input = String::new();
        for character in "joocode".chars() {
            detect_easter_egg(&mut screen, &mut input, KeyCode::Char(character));
        }
        assert!(matches!(screen, Screen::Dashboard));
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
            port_warning: None,
        };
        let mut screen = Screen::Dashboard;
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
