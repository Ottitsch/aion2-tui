//! The options line under a module tab's header: what can be switched there, with its key and how it is set now.
//! The same settings shape that module's part of the overview, and they are kept for the next start.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

pub enum Opt<'a> {
    /// `p party only: on`
    Toggle { key: &'a str, label: &'a str, on: bool },
    /// `g goal: 5.00M`
    Value { key: &'a str, label: &'a str, value: String },
    /// `map: 1 radar  2 3D`, the picked one highlighted.
    Pick { label: &'a str, choices: Vec<(&'a str, &'a str, bool)> },
}

const NOTE: &str = "also on the overview (F1), kept for next time ";

fn key(k: &str) -> Span<'static> {
    Span::styled(format!("{k} "), Style::new().fg(Color::Gray).add_modifier(Modifier::BOLD))
}

fn muted(text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::new().fg(Color::DarkGray))
}

pub fn draw(f: &mut Frame, area: Rect, opts: &[Opt]) {
    let mut spans = vec![Span::raw(" ")];
    for (i, opt) in opts.iter().enumerate() {
        if i > 0 {
            spans.push(muted("    "));
        }
        match opt {
            Opt::Toggle { key: k, label, on } => {
                spans.extend([key(k), Span::raw(format!("{label}: "))]);
                spans.push(if *on { Span::styled("on", Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)) } else { muted("off") });
            }
            Opt::Value { key: k, label, value } => {
                spans.extend([key(k), Span::raw(format!("{label}: ")), Span::styled(value.clone(), Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD))]);
            }
            Opt::Pick { label, choices } => {
                spans.push(Span::raw(format!("{label}: ")));
                for (k, choice, picked) in choices {
                    let style = if *picked { Style::new().fg(Color::Black).bg(Color::Gray).add_modifier(Modifier::BOLD) } else { Style::new().fg(Color::Gray) };
                    spans.push(Span::styled(format!(" {k} {choice} "), style));
                }
            }
        }
    }
    let width: usize = spans.iter().map(Span::width).sum();
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    if width + NOTE.len() + 2 <= usize::from(area.width) {
        f.render_widget(Paragraph::new(Line::from(muted(NOTE)).alignment(Alignment::Right)), area);
    }
}
