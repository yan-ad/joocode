use std::{io::IsTerminal, net::SocketAddr, time::Duration};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::{desktop::DesktopTargets, provider::Registry};

#[derive(Clone, Debug)]
pub struct DashboardData {
    pub config_sources: Vec<String>,
    pub ide_targets: Vec<String>,
    pub listening: String,
}

impl DashboardData {
    pub fn new(registry: &Registry, targets: &DesktopTargets, address: SocketAddr) -> Self {
        let config_sources = registry
            .source_reports()
            .iter()
            .filter(|report| report.status == "loaded")
            .map(|report| display_source(&report.source))
            .collect();
        Self {
            config_sources,
            ide_targets: targets.names().into_iter().map(str::to_owned).collect(),
            listening: format!("http://{address}"),
        }
    }
}

pub fn is_interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

pub fn run(data: DashboardData) -> anyhow::Result<()> {
    ratatui::run(|terminal| {
        loop {
            terminal.draw(|frame| draw(frame, &data))?;
            if event::poll(Duration::from_millis(250))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
                && (key.code == KeyCode::Esc
                    || (key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)))
            {
                return Ok::<(), std::io::Error>(());
            }
        }
    })?;
    Ok(())
}

fn draw(frame: &mut Frame<'_>, data: &DashboardData) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(7),
        Constraint::Length(3),
    ])
    .areas(frame.area());

    let title = Paragraph::new(Line::from(vec![Span::styled(
        "Joocode",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )]))
    .block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(title, header);

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
        Line::from(vec![
            Span::styled("Config: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(sources),
        ]),
        Line::from(vec![
            Span::styled(
                "IDE Target: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(targets),
        ]),
        Line::from(vec![
            Span::styled("Listening: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&data.listening),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), body);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Esc",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" to exit"),
        ]))
        .block(Block::default().borders(Borders::TOP)),
        footer,
    );
}

fn display_source(source: &str) -> String {
    match source {
        "opencode" => "OpenCode".into(),
        "ocx" => "OpenCodex".into(),
        "hermes" => "Hermes".into(),
        "copilot" => "GitHub Copilot".into(),
        other => other.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_labels_are_human_readable() {
        assert_eq!(display_source("opencode"), "OpenCode");
        assert_eq!(display_source("ocx"), "OpenCodex");
        assert_eq!(display_source("hermes"), "Hermes");
        assert_eq!(display_source("copilot"), "GitHub Copilot");
    }
}
