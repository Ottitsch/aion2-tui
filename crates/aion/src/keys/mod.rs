//! The Keys tab: key-mouse's hold buttons, started with the TUI. `^` works everywhere (it is a global hotkey);
//! here `space` does the same, `s` stops or starts the virtual devices.

pub mod hold;
pub mod viiper;

use crate::fmt::{clock_secs, format_duration};
use hold::{Engine, Phase, State, BREAK_EVERY, BREAK_FOR, HOTKEY, TAP_EVERY, TAP_KEYS};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;
use std::time::Instant;

const MESSAGE_MS: u64 = 4_000;

pub struct App {
    pub engine: Engine,
    pub log_scroll: usize,
    pub message: Option<(String, u64)>,
    pub now_ms: u64,
    pub quit: bool,
}

impl App {
    pub fn new(engine: Engine, start: bool, now_ms: u64) -> App {
        let mut app = App { engine, log_scroll: 0, message: None, now_ms, quit: false };
        if start {
            app.engine.start();
        }
        app
    }

    pub fn step(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
        if matches!(&self.message, Some((_, until)) if *until <= now_ms) {
            self.message = None;
        }
    }

    fn say(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), self.now_ms + MESSAGE_MS));
    }

    pub fn phase(&self) -> Phase {
        self.engine.state.lock().phase.clone()
    }

    /// A short label and colour for the hub's top bar.
    pub fn badge(&self) -> (String, Color) {
        match self.phase() {
            Phase::Holding => ("HOLDING".into(), Color::Green),
            Phase::Break { until } => (format!("break {:.0}s", until.saturating_duration_since(Instant::now()).as_secs_f64().ceil()), Color::Yellow),
            Phase::Ready => (format!("ready, {HOTKEY} holds"), Color::Gray),
            Phase::Starting(_) => ("starting".into(), Color::DarkGray),
            Phase::Off => ("off".into(), Color::DarkGray),
            Phase::Failed(_) => ("failed".into(), Color::Red),
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('c') if ctrl => self.quit = true,
            KeyCode::Char(' ') | KeyCode::Enter => match self.phase() {
                Phase::Ready | Phase::Holding | Phase::Break { .. } => {
                    self.engine.toggle();
                }
                _ => self.say("The devices are not ready; s starts them"),
            },
            KeyCode::Char('s') => {
                if self.engine.running() {
                    self.engine.stop();
                    self.say("Stopped: buttons let go, devices removed");
                } else {
                    self.engine.start();
                    self.say("Starting");
                }
            }
            // The log is newest first: down goes to older lines.
            KeyCode::Down | KeyCode::Char('j') => self.log_scroll += 1,
            KeyCode::Up | KeyCode::Char('k') => self.log_scroll = self.log_scroll.saturating_sub(1),
            KeyCode::PageDown => self.log_scroll += 10,
            KeyCode::PageUp => self.log_scroll = self.log_scroll.saturating_sub(10),
            KeyCode::Home => self.log_scroll = 0,
            _ => {}
        }
    }

    pub fn shutdown(&mut self) {
        self.engine.stop();
    }
}

fn muted() -> Style {
    Style::new().fg(Color::DarkGray)
}

fn block(title: &str) -> Block<'static> {
    Block::bordered().border_style(muted()).title(Span::styled(format!(" {title} "), Style::new().fg(Color::Gray)))
}

/// `6.2 s`, or `1:05` past a minute.
fn secs(d: std::time::Duration) -> String {
    if d.as_secs() >= 60 {
        format_duration(d.as_millis() as u64)
    } else {
        format!("{:.1} s", d.as_secs_f64())
    }
}

fn range(r: (f64, f64)) -> String {
    if r.1 < 1.0 {
        format!("{:.0} to {:.0} ms", r.0 * 1000.0, r.1 * 1000.0)
    } else {
        format!("{} to {} s", r.0, r.1)
    }
}

fn big(text: &str, bg: Color) -> Span<'static> {
    Span::styled(format!("  {text}  "), Style::new().fg(Color::Black).bg(bg).add_modifier(Modifier::BOLD))
}

/// The state as a big label, and what it means right now.
fn phase_line(state: &State, now: Instant) -> Line<'static> {
    let (label, bg, what) = match &state.phase {
        Phase::Holding => {
            let held = state.holding_since.map(|t| secs(now.saturating_duration_since(t))).unwrap_or_default();
            ("HOLDING", Color::Green, Span::raw(format!("  left + right mouse button for {held}")))
        }
        Phase::Break { until } => ("BREAK", Color::Yellow, Span::raw(format!("  mouse buttons up, back in {}", secs(until.saturating_duration_since(now))))),
        Phase::Ready => ("READY", Color::Gray, Span::raw(format!("  press {HOTKEY} (or space here) to hold left + right"))),
        Phase::Starting(what) => ("STARTING", Color::DarkGray, Span::styled(format!("  {what}..."), muted())),
        Phase::Off => ("OFF", Color::DarkGray, Span::raw("  s starts the virtual keyboard and mouse")),
        Phase::Failed(e) => ("FAILED", Color::Red, Span::raw(format!("  {e}"))),
    };
    Line::from(vec![Span::raw("  "), big(label, bg), what])
}

/// When the next break comes, or how breaks go while nothing is held.
fn break_line(state: &State, now: Instant) -> Line<'static> {
    let holding = matches!(state.phase, Phase::Holding | Phase::Break { .. });
    match state.next_break {
        Some(at) => Line::from(vec![
            Span::styled("  break in   ", muted()),
            Span::raw(secs(at.saturating_duration_since(now))),
            Span::styled(format!("   (every {} for {})", range(BREAK_EVERY), range(BREAK_FOR)), muted()),
        ]),
        None if !holding => Line::from(Span::styled(format!("  breaks     every {} for {}", range(BREAK_EVERY), range(BREAK_FOR)), muted())),
        None => Line::from(""),
    }
}

/// One tap key: down, its next press or how often it goes, and how often it was pressed.
fn key_line(state: &State, key: char, hold: (f64, f64), now: Instant) -> Line<'static> {
    let down = state.down_until.iter().find(|(k, _)| *k == key).map(|(_, t)| *t);
    let next = state.next_press.iter().find(|(k, _)| *k == key).map(|(_, t)| *t);
    let count = state.presses.iter().find(|(k, _)| *k == key).map(|(_, n)| *n).unwrap_or(0);
    let what = match (down, next) {
        (Some(t), _) => Span::styled(format!("DOWN for {}", secs(t.saturating_duration_since(now))), Style::new().fg(Color::Green).bold()),
        (None, Some(t)) => Span::raw(format!("next in {}", secs(t.saturating_duration_since(now)))),
        _ => Span::styled(format!("every {}", range(TAP_EVERY)), muted()),
    };
    Line::from(vec![Span::styled(format!("  key {key}      "), muted()), what, Span::styled(format!("   (held {}, pressed {count}×)", range(hold)), muted())])
}

/// Lines the overview's box holds: the state, the break, and one per tap key.
pub const OVERVIEW_HEIGHT: u16 = 2 + 2 + TAP_KEYS.len() as u16;

/// The overview's part: the same state, break and tap key lines as the tab, without the log.
pub fn draw_overview(f: &mut Frame, area: Rect, app: &App) {
    let state = app.engine.state.lock();
    let now = Instant::now();
    let mut lines = vec![phase_line(&state, now), break_line(&state, now)];
    lines.extend(TAP_KEYS.iter().map(|&(key, hold)| key_line(&state, key, hold, now)));
    let holding = matches!(state.phase, Phase::Holding | Phase::Break { .. });
    let border = if holding { Color::Green } else { Color::DarkGray };
    let block = Block::bordered().border_style(Style::new().fg(border)).title(Span::styled(" Hold buttons ", Style::new().fg(Color::Gray)));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

pub fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    let [header, body, footer] = Layout::vertical([Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)]).areas(area);
    // Before the lock below: `badge` takes it too, and it is not reentrant.
    let (badge, color) = app.badge();
    let state = app.engine.state.lock();
    let now = Instant::now();

    // Header: state, what the hotkey does, where VIIPER came from.
    let mut spans = vec![Span::raw(" "), Span::styled("● ", Style::new().fg(color)), Span::raw(badge), Span::styled("  │  ", muted())];
    spans.push(Span::raw(format!("{HOTKEY} toggles from anywhere, also in game")));
    if !matches!(state.phase, Phase::Off | Phase::Failed(_)) {
        spans.push(Span::styled("  │  ", muted()));
        spans.push(Span::styled(if state.own_server { "VIIPER started here" } else { "VIIPER was already running" }, muted()));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), header);

    let [status_area, log_area] = Layout::vertical([Constraint::Length(9), Constraint::Min(3)]).areas(body);
    let mut lines = vec![Line::from(""), phase_line(&state, now)];
    if matches!(state.phase, Phase::Off | Phase::Failed(_)) {
        lines.push(Line::from(Span::styled(format!("  VIIPER     {} (--viiper changes it)", app.engine.viiper_exe().display()), muted())));
    }
    lines.push(Line::from(""));
    lines.push(break_line(&state, now));
    lines.extend(TAP_KEYS.iter().map(|&(key, hold)| key_line(&state, key, hold, now)));
    lines.push(Line::from(Span::styled(format!("  breaks taken {}", state.breaks), muted())));
    f.render_widget(Paragraph::new(lines).block(block("Hold buttons")), status_area);

    let visible = log_area.height.saturating_sub(2) as usize;
    app.log_scroll = app.log_scroll.min(state.log.len().saturating_sub(visible));
    let end = state.log.len().saturating_sub(app.log_scroll);
    let start = end.saturating_sub(visible);
    let log: Vec<Line> = state
        .log
        .range(start..end)
        .rev()
        .map(|(t, text)| Line::from(vec![Span::styled(format!(" {} ", clock_secs(*t)), muted()), Span::raw(text.clone())]))
        .collect();
    let title = if app.log_scroll > 0 { format!("Log, newest first ({} newer hidden, Home jumps back)", app.log_scroll) } else { "Log, newest first".into() };
    f.render_widget(Paragraph::new(log).wrap(Wrap { trim: false }).block(block(&title)), log_area);
    drop(state);

    if let Some((text, _)) = &app.message {
        f.render_widget(Paragraph::new(Line::from(vec![Span::raw(" "), Span::styled(text.clone(), Style::new().fg(Color::Yellow))])), footer);
        return;
    }
    let running = app.engine.running();
    let keys: Vec<(&str, &str)> = vec![("q", "quit"), ("space", "hold / let go"), ("s", if running { "stop devices" } else { "start devices" }), ("↑↓", "scroll log")];
    let mut spans = vec![Span::raw(" ")];
    for (k, label) in keys {
        spans.push(Span::styled(k, Style::new().fg(Color::Gray).bold()));
        spans.push(Span::styled(format!(" {label}   "), muted()));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), footer);
}
