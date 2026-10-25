//! TUI state and key handling. Rendering lives in `ui.rs`.

use crate::fmt::{format_short, parse_amount};
use crate::kina::feed::{Feed, Limits, Section, Status};
use crate::kina::summary::chat_summary;
use kina_core::snapshot::{Character, SessionSnapshot, SessionSummary};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// How long a footer message stays.
const MESSAGE_MS: u64 = 4_000;
/// The Setup view polls the diagnostics this often.
const DIAGNOSTICS_MS: u64 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Live,
    Runs,
    History,
    Setup,
}

pub const TABS: [(Tab, &str); 4] = [(Tab::Live, "Live"), (Tab::Runs, "Runs"), (Tab::History, "History"), (Tab::Setup, "Setup")];

impl Tab {
    pub fn index(self) -> usize {
        TABS.iter().position(|(t, _)| *t == self).unwrap_or(0)
    }

    fn from_index(i: usize) -> Tab {
        TABS[i % TABS.len()].0
    }
}

pub struct ViewOptions {
    /// Mask the character name (streaming).
    pub hide_name: bool,
    /// Net kina the session aims for; 0 is off.
    pub goal: u64,
}

pub enum Prompt {
    Delete(u64),
    /// The net kina goal being typed in.
    Goal(String),
}

pub struct App {
    pub feed: Box<dyn Feed>,
    pub tab: Tab,
    pub options: ViewOptions,
    /// A past session opened from History, shown instead of the live one in Live and Runs.
    pub opened: Option<SessionSnapshot>,
    pub current: Option<SessionSnapshot>,
    pub history: Vec<SessionSummary>,
    pub character: Option<Character>,
    pub balance: Option<u64>,
    pub limits: Limits,
    pub status: Status,
    pub sections: Vec<Section>,
    sections_ms: u64,
    pub now_ms: u64,
    /// Rows scrolled past at the top of the kina log (newest first).
    pub log_scroll: usize,
    pub runs_scroll: usize,
    /// Runs view: every map visit, or only the ones where kina moved.
    pub runs_all: bool,
    pub history_selected: usize,
    pub setup_scroll: u16,
    pub prompt: Option<Prompt>,
    pub message: Option<(String, u64)>,
    pub quit: bool,
}

impl App {
    pub fn new(feed: Box<dyn Feed>, options: ViewOptions, now_ms: u64) -> App {
        let status = feed.status();
        let mut app = App {
            feed,
            tab: Tab::Live,
            options,
            opened: None,
            current: None,
            history: Vec::new(),
            character: None,
            balance: None,
            limits: Limits::default(),
            status,
            sections: Vec::new(),
            sections_ms: 0,
            now_ms,
            log_scroll: 0,
            runs_scroll: 0,
            runs_all: false,
            history_selected: 0,
            setup_scroll: 0,
            prompt: None,
            message: None,
            quit: false,
        };
        app.refresh(now_ms);
        app
    }

    /// Advances the feed and copies what the views need.
    pub fn step(&mut self, now_ms: u64) {
        self.feed.step(now_ms);
        self.refresh(now_ms);
    }

    fn refresh(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
        self.current = self.feed.current();
        self.history = self.feed.history();
        self.character = self.feed.character();
        self.balance = self.feed.balance();
        self.limits = self.feed.limits();
        self.status = self.feed.status();
        if self.tab == Tab::Setup && now_ms.saturating_sub(self.sections_ms) >= DIAGNOSTICS_MS {
            self.sections = self.feed.diagnostics();
            self.sections_ms = now_ms;
        }
        self.history_selected = self.history_selected.min(self.history.len().saturating_sub(1));
        if matches!(&self.message, Some((_, until)) if *until <= now_ms) {
            self.message = None;
        }
    }

    /// The session the Live and Runs views show: an opened past one, else the live (or last) one.
    pub fn shown(&self) -> Option<&SessionSnapshot> {
        self.opened.as_ref().or(self.current.as_ref())
    }

    pub fn character_label(&self, c: Option<&Character>) -> String {
        match c {
            Some(_) if self.options.hide_name => "Character".into(),
            Some(c) => c.name.clone(),
            None => "unidentified".into(),
        }
    }

    pub fn summary_name<'a>(&self, name: Option<&'a str>) -> &'a str {
        match name {
            Some(_) if self.options.hide_name => "Character",
            Some(n) => n,
            None => "unidentified",
        }
    }

    fn say(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), self.now_ms + MESSAGE_MS));
    }

    fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
        if tab == Tab::Setup {
            self.sections = self.feed.diagnostics();
            self.sections_ms = self.now_ms;
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if let Some(Prompt::Goal(text)) = &mut self.prompt {
            match key.code {
                KeyCode::Enter => {
                    let text = std::mem::take(text);
                    self.prompt = None;
                    let goal = if text.trim().is_empty() { Some(0) } else { parse_amount(&text) };
                    match goal {
                        Some(0) => {
                            self.options.goal = 0;
                            self.say("Goal cleared");
                        }
                        Some(goal) => {
                            self.options.goal = goal;
                            self.say(format!("Goal set: {} net kina this session", format_short(goal as f64)));
                        }
                        None => self.say(format!("{text:?} is not an amount like 5m or 750k; the goal is unchanged")),
                    }
                }
                KeyCode::Esc => {
                    self.prompt = None;
                    self.say("Goal unchanged");
                }
                KeyCode::Backspace => {
                    text.pop();
                }
                KeyCode::Char(c) if (c.is_ascii_alphanumeric() || c == '.' || c == ',') && text.len() < 16 => text.push(c),
                _ => {}
            }
            return;
        }
        if let Some(prompt) = self.prompt.take() {
            match (prompt, key.code) {
                (Prompt::Delete(id), KeyCode::Char('y') | KeyCode::Char('Y')) => match self.feed.delete(id) {
                    Ok(()) => {
                        if self.opened.as_ref().map(|s| s.id) == Some(id) {
                            self.opened = None;
                        }
                        self.say(format!("Session #{id} deleted"));
                        self.refresh(self.now_ms);
                    }
                    Err(e) => self.say(format!("Delete failed: {e}")),
                },
                _ => self.say("Kept"),
            }
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('c') if ctrl => self.quit = true,
            KeyCode::Esc => {
                if self.opened.take().is_some() {
                    self.say("Back to the live session");
                } else if self.tab != Tab::Live {
                    self.set_tab(Tab::Live);
                }
            }
            KeyCode::Tab => self.set_tab(Tab::from_index(self.tab.index() + 1)),
            KeyCode::BackTab => self.set_tab(Tab::from_index(self.tab.index() + TABS.len() - 1)),
            KeyCode::Char(c @ '1'..='4') => self.set_tab(Tab::from_index(c as usize - '1' as usize)),
            KeyCode::Char('n') => {
                if self.opened.is_some() {
                    self.say("Close the opened session first (Esc)");
                } else {
                    let had = self.current.as_ref().map(|s| s.active && s.changes > 0).unwrap_or(false);
                    self.feed.new_session(self.now_ms);
                    self.refresh(self.now_ms);
                    self.log_scroll = 0;
                    self.runs_scroll = 0;
                    let saved = if had && self.feed.persistent() { "saved, " } else { "" };
                    self.say(match self.current.as_ref().filter(|s| s.active) {
                        Some(_) => format!("Session {saved}a new one starts at the current balance"),
                        None => "A new session starts at the next balance the game sends".into(),
                    });
                }
            }
            KeyCode::Char('c') => self.copy_summary(),
            KeyCode::Char('g') => self.prompt = Some(Prompt::Goal(String::new())),
            KeyCode::Char('h') => {
                self.options.hide_name = !self.options.hide_name;
                self.say(if self.options.hide_name { "Character name hidden" } else { "Character name shown" });
            }
            KeyCode::Char('a') if self.tab == Tab::Runs => {
                self.runs_all = !self.runs_all;
                self.runs_scroll = 0;
            }
            KeyCode::Char('r') if self.tab == Tab::Setup => {
                self.feed.retry();
                self.say("Capture restarted");
            }
            KeyCode::Enter if self.tab == Tab::History => self.open_selected(),
            KeyCode::Char('d') | KeyCode::Delete if self.tab == Tab::History => {
                if let Some(s) = self.history.get(self.history_selected) {
                    self.prompt = Some(Prompt::Delete(s.id));
                }
            }
            KeyCode::Up | KeyCode::Char('k') => self.scroll(-1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll(1),
            KeyCode::PageUp => self.scroll(-10),
            KeyCode::PageDown => self.scroll(10),
            KeyCode::Home => self.scroll(-1_000_000),
            KeyCode::End => self.scroll(1_000_000),
            _ => {}
        }
    }

    /// Moves the list of the active view; the views clamp the offsets to what they hold.
    fn scroll(&mut self, by: i64) {
        let apply = |v: usize| (v as i64 + by).max(0) as usize;
        match self.tab {
            Tab::Live => self.log_scroll = apply(self.log_scroll),
            Tab::Runs => self.runs_scroll = apply(self.runs_scroll),
            Tab::History => self.history_selected = apply(self.history_selected).min(self.history.len().saturating_sub(1)),
            Tab::Setup => self.setup_scroll = apply(self.setup_scroll as usize).min(u16::MAX as usize) as u16,
        }
    }

    fn open_selected(&mut self) {
        let Some(summary) = self.history.get(self.history_selected) else { return };
        let id = summary.id;
        match self.feed.session(id) {
            Some(snap) => {
                self.opened = Some(snap);
                self.log_scroll = 0;
                self.runs_scroll = 0;
                self.set_tab(Tab::Live);
                self.say(format!("Viewing session #{id}. Esc returns to the live session"));
            }
            None => self.say(format!("Session #{id} could not be loaded")),
        }
    }

    /// Puts the one line summary on the clipboard through OSC 52 (Windows Terminal, most modern terminals)
    /// and shows it in the footer for terminals that ignore it.
    fn copy_summary(&mut self) {
        let Some(snap) = self.shown() else {
            self.say("No session to summarise yet");
            return;
        };
        let line = chat_summary(snap);
        let sent = crate::clipboard::copy(&line);
        self.say(if sent { format!("Copied: {line}") } else { line });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kina::demo::Demo;
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent { code, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE }
    }

    fn demo_app() -> App {
        let now = 1_791_722_000_000;
        App::new(Box::new(Demo::new(now)), ViewOptions { hide_name: false, goal: 0 }, now)
    }

    #[test]
    fn tabs_cycle_and_jump() {
        let mut app = demo_app();
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.tab, Tab::Runs);
        app.on_key(key(KeyCode::BackTab));
        app.on_key(key(KeyCode::BackTab));
        assert_eq!(app.tab, Tab::Setup);
        assert!(!app.sections.is_empty(), "the Setup view loads its diagnostics");
        app.on_key(key(KeyCode::Char('3')));
        assert_eq!(app.tab, Tab::History);
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.tab, Tab::Live);
        app.on_key(key(KeyCode::Char('q')));
        assert!(app.quit);
    }

    #[test]
    fn opens_deletes_and_starts_sessions() {
        let mut app = demo_app();
        assert_eq!(app.history.len(), 2);
        app.on_key(key(KeyCode::Char('3')));
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.history_selected, 1);
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.history_selected, 1, "the selection stops at the last row");
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.tab, Tab::Live);
        let opened = app.opened.as_ref().map(|s| s.id).unwrap();
        assert_eq!(app.shown().map(|s| s.id), Some(opened));
        app.on_key(key(KeyCode::Char('n')));
        assert_eq!(app.history.len(), 2, "no new session while a past one is open");
        app.on_key(key(KeyCode::Esc));
        assert!(app.opened.is_none());
        app.on_key(key(KeyCode::Char('n')));
        assert_eq!(app.history.len(), 3);
        assert_eq!(app.current.as_ref().map(|s| s.net), Some(0));
        app.on_key(key(KeyCode::Char('3')));
        app.on_key(key(KeyCode::Char('d')));
        app.on_key(key(KeyCode::Char('x')));
        assert_eq!(app.history.len(), 3, "any key but y keeps it");
        app.on_key(key(KeyCode::Char('d')));
        app.on_key(key(KeyCode::Char('y')));
        assert_eq!(app.history.len(), 2);
    }

    #[test]
    fn hiding_the_name() {
        let mut app = demo_app();
        let c = app.character.clone();
        assert_eq!(app.character_label(c.as_ref()), "Seraphel");
        app.on_key(key(KeyCode::Char('h')));
        assert_eq!(app.character_label(c.as_ref()), "Character");
        assert_eq!(app.summary_name(Some("Nyxara")), "Character");
    }
}
