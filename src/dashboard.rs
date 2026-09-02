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
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    autostart::{self, Status as AutoStartStatus},
    desktop::DesktopTargets,
    local_config::{self, ProviderSummary},
    provider::Registry,
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
    pub proxy_targets: BTreeMap<ProxyTarget, bool>,
    pub providers: Vec<ProviderSummary>,
    pub port_warning: Option<String>,
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

fn draw_providers(
    frame: &mut Frame<'_>,
    area: Rect,
    providers: &[ProviderSummary],
    selected: usize,
) {
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
                        if index == selected { "◆ " } else { "◇ " },
                        Style::default().fg(if index == selected {
                            Color::Green
                        } else {
                            Color::DarkGray
                        }),
                    ),
                    Span::styled(
                        &provider.label,
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  {} models", provider.model_count),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]))
                .style(selected_style(index == selected))
            })
            .collect()
    };
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .title(" Providers ")
                .title_style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn draw_provider_input(frame: &mut Frame<'_>, title: &str, value: &str, secret: bool) {
    let area = frame.area();
    let width = area.width.saturating_sub(4).min(72);
    let height = 7_u16.min(area.height.saturating_sub(2));
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let shown = if secret {
        "•".repeat(value.chars().count())
    } else {
        value.to_owned()
    };
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(vec![Line::from(""), Line::from(shown)])
            .block(
                Block::default()
                    .title(format!(" New provider · {title} "))
                    .title_style(Style::default().fg(Color::Green))
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false }),
        popup,
    );
}

fn draw_provider_loading(frame: &mut Frame<'_>) {
    let area = frame.area();
    let width = area.width.saturating_sub(4).min(60);
    let height = 7_u16.min(area.height.saturating_sub(2));
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new("Fetching /models and reloading Joocode…")
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .title(" New provider · Step 3/3 ")
                    .title_style(Style::default().fg(Color::Yellow))
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: true }),
        popup,
    );
}

fn selected_style(selected: bool) -> Style {
    if selected {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
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
    let area = frame.area();
    let width = area.width.saturating_sub(4).min(72);
    let height = 9_u16.min(area.height.saturating_sub(2));
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("There's a new version available: {tag}"),
                Style::default()
                    .fg(Color::LightGreen)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("Press Enter to update and run immediately after updating."),
        ])
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .title(" ↑ Joocode update ")
                .title_style(Style::default().fg(Color::Cyan))
                .borders(Borders::ALL),
        ),
        popup,
    );
}

const AUTO_START_ITEM: usize = 0;
const FIRST_PROXY_ITEM: usize = 1;

fn config_items() -> Vec<usize> {
    (AUTO_START_ITEM..=ProxyTarget::ALL.len()).collect()
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

fn draw_config(frame: &mut Frame<'_>, data: &DashboardData, selected: usize) {
    let [_, vertical, _] = Layout::vertical([
        Constraint::Percentage(30),
        Constraint::Length(15),
        Constraint::Min(0),
    ])
    .areas(frame.area());
    let [_, popup, _] = Layout::horizontal([
        Constraint::Percentage(20),
        Constraint::Percentage(60),
        Constraint::Percentage(20),
    ])
    .areas(vertical);

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
    let mut items = vec![
        ListItem::new(Line::from(Span::styled(
            "Setting",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))),
        ListItem::new(auto_start).style(selected_style(selected == AUTO_START_ITEM)),
        ListItem::new(Line::from("")),
        ListItem::new(Line::from(Span::styled(
            "Proxy to",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ))),
    ];
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

    frame.render_widget(Clear, popup);
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .title(" ⚙ Configuration ")
                .title_style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
                .borders(Borders::ALL),
        ),
        popup,
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
            proxy_targets: ProxyTarget::ALL
                .into_iter()
                .map(|target| (target, targets.enabled(target)))
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
    ToggleAutoStart,
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
    ProviderRemoved {
        config_sources: Vec<String>,
        model_count: usize,
        provider_count: usize,
        providers: Vec<ProviderSummary>,
    },
    ProviderError(String),
    AutoStartUpdated(AutoStartStatus),
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
            Ok(DashboardEvent::ProviderError(error)) => *screen = Screen::Error(error),
            Ok(DashboardEvent::AutoStartUpdated(status)) => data.autostart = status,
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
            KeyCode::Char(' ') => {
                if let Some(target) = target_for_config_item(*selected) {
                    let _ = command_tx.send(DashboardCommand::ToggleProxyTarget { target });
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
        Screen::Providers { selected } => draw_providers(frame, body, &data.providers, *selected),
        Screen::ProviderBaseUrl { selected, value } => {
            draw_providers(frame, body, &data.providers, *selected);
            draw_provider_input(frame, "Step 1/3 — Base URL", value, false);
        }
        Screen::ProviderApiKey {
            selected, api_key, ..
        } => {
            draw_providers(frame, body, &data.providers, *selected);
            draw_provider_input(frame, "Step 2/3 — API key", api_key, true);
        }
        Screen::ProviderLoading { selected } => {
            draw_providers(frame, body, &data.providers, *selected);
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
        Screen::Error(error) => frame.render_widget(
            Paragraph::new(error.as_str())
                .style(Style::default().fg(Color::Red))
                .wrap(Wrap { trim: true }),
            body,
        ),
    }

    let help = match screen {
        Screen::Dashboard => vec![
            Span::styled(
                "Esc",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" to exit  ·  "),
            Span::styled(
                "Tab",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" Providers  ·  "),
            Span::styled(
                "/",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" Config"),
        ],
        Screen::Config { .. } => vec![
            Span::styled("↑/↓", Style::default().fg(Color::Cyan)),
            Span::raw(" navigate  ·  "),
            Span::styled("Space", Style::default().fg(Color::Green)),
            Span::raw(" toggle  ·  Esc to close"),
        ],
        Screen::Providers { .. } => vec![
            Span::styled("Enter", Style::default().fg(Color::Green)),
            Span::raw(" new provider  ·  "),
            Span::styled("Del", Style::default().fg(Color::Red)),
            Span::raw(" remove selected provider  ·  Esc close"),
        ],
        Screen::ProviderBaseUrl { .. } | Screen::ProviderApiKey { .. } => vec![
            Span::styled("Enter", Style::default().fg(Color::Green)),
            Span::raw(" to continue  ·  Esc to cancel"),
        ],
        Screen::ProviderLoading { .. } => vec![Span::raw("Fetching /models…")],
        Screen::UpdateAvailable(_) => vec![
            Span::styled("Enter", Style::default().fg(Color::Green)),
            Span::raw(" update & restart  ·  Esc dismiss"),
        ],
        Screen::Updating { .. } => {
            unreachable!("updating has its own full-screen progress scene")
        }
        Screen::Error(_) => vec![
            Span::styled("Enter", Style::default().fg(Color::Green)),
            Span::raw(" to return  ·  Esc to cancel"),
        ],
        Screen::EasterEgg { .. } => unreachable!("easter egg has its own full-screen controls"),
    };
    frame.render_widget(
        Paragraph::new(Line::from(help)).block(Block::default().borders(Borders::TOP)),
        footer,
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
    let lines = vec![
        data.port_warning
            .as_ref()
            .map(|warning| {
                Line::from(vec![
                    Span::styled("⚠ ", Style::default().fg(Color::Yellow)),
                    Span::styled(
                        warning,
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                ])
            })
            .unwrap_or_else(|| Line::from("")),
        Line::from(vec![
            Span::styled("◆ ", Style::default().fg(Color::Cyan)),
            Span::styled("Config: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(sources),
        ]),
        Line::from(vec![
            Span::styled("⌘ ", Style::default().fg(Color::Magenta)),
            Span::styled(
                "IDE Target: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(targets),
        ]),
        Line::from(vec![
            Span::styled("● ", Style::default().fg(Color::Green)),
            Span::styled("Listening: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(&data.listening, Style::default().fg(Color::Green)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("◉ ", Style::default().fg(Color::Yellow)),
            Span::styled("Models: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                data.model_count.to_string(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("    "),
            Span::styled("◇ ", Style::default().fg(Color::Blue)),
            Span::styled("Providers: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                data.provider_count.to_string(),
                Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), area);
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
    fn delete_removes_selected_provider() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let providers = vec![
            ProviderSummary {
                name: "gunamaya".into(),
                label: "gunamaya.id".into(),
                model_count: 10,
            },
            ProviderSummary {
                name: "openai".into(),
                label: "openai.com".into(),
                model_count: 4,
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
            proxy_targets: BTreeMap::new(),
            providers: vec![
                ProviderSummary {
                    name: "gunamaya".into(),
                    label: "gunamaya.id".into(),
                    model_count: 10,
                },
                ProviderSummary {
                    name: "openai".into(),
                    label: "openai.com".into(),
                    model_count: 4,
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
            proxy_targets: BTreeMap::new(),
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
            proxy_targets: ProxyTarget::ALL
                .into_iter()
                .map(|target| (target, target == ProxyTarget::Codex))
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
    fn autostart_event_refreshes_dashboard_status() {
        let mut data = DashboardData {
            config_sources: vec![],
            ide_targets: vec![],
            listening: "http://127.0.0.1:10100".into(),
            model_count: 0,
            provider_count: 0,
            autostart: AutoStartStatus::Off,
            proxy_targets: BTreeMap::new(),
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
            proxy_targets: BTreeMap::new(),
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
            proxy_targets: BTreeMap::new(),
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
            proxy_targets: BTreeMap::new(),
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
            proxy_targets: BTreeMap::new(),
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
            proxy_targets: BTreeMap::new(),
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
