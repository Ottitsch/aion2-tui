//! DPS tab state and key handling. Rendering lives in `ui.rs`.

use crate::dps::feed::{Feed, Section, Status};
use crate::dps::overlay::Overlay;
use crate::dps::summary::chat_summary;
use meter_core::parser::EntityId;
use meter_core::snapshot::{ActorStats, EncounterSummary, Snapshot};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// How long a footer message stays.
const MESSAGE_MS: u64 = 4_000;
/// The Setup view polls the diagnostics this often.
const DIAGNOSTICS_MS: u64 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Live,
    History,
    Setup,
}

pub const TABS: [(Tab, &str); 3] = [(Tab::Live, "Live"), (Tab::History, "History"), (Tab::Setup, "Setup")];

impl Tab {
    pub fn index(self) -> usize {
        TABS.iter().position(|(t, _)| *t == self).unwrap_or(0)
    }

    fn from_index(i: usize) -> Tab {
        TABS[i % TABS.len()].0
    }
}

pub struct ViewOptions {
    /// Mask player names (streaming): rows show the class instead.
    pub hide_names: bool,
    /// Only you and your party.
    pub party_only: bool,
    /// Hide encounters whose main target is not a boss.
    pub boss_only: bool,
}

pub enum Prompt {
    Delete(u64),
}

pub struct App {
    pub feed: Box<dyn Feed>,
    pub overlay: Overlay,
    pub tab: Tab,
    pub options: ViewOptions,
    /// A past encounter opened from History, shown instead of the live one.
    pub opened: Option<Snapshot>,
    pub current: Option<Snapshot>,
    pub history: Vec<EncounterSummary>,
    pub self_name: Option<String>,
    pub status: Status,
    pub sections: Vec<Section>,
    sections_ms: u64,
    pub now_ms: u64,
    /// The player whose skills are shown, kept by id so it stays put while the ranking changes.
    pub selected: Option<EntityId>,
    pub skills_scroll: usize,
    pub history_selected: usize,
    pub setup_scroll: u16,
    pub prompt: Option<Prompt>,
    pub message: Option<(String, u64)>,
    pub quit: bool,
}

impl App {
    pub fn new(feed: Box<dyn Feed>, overlay: Overlay, options: ViewOptions, now_ms: u64) -> App {
        let status = feed.status();
        let mut app = App {
            feed,
            overlay,
            tab: Tab::Live,
            options,
            opened: None,
            current: None,
            history: Vec::new(),
            self_name: None,
            status,
            sections: Vec::new(),
            sections_ms: 0,
            now_ms,
            selected: None,
            skills_scroll: 0,
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
        self.overlay.poll(now_ms);
        self.refresh(now_ms);
    }

    fn refresh(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
        self.current = self.feed.current();
        self.history = self.feed.history();
        if self.options.boss_only {
            self.history.retain(|s| s.is_boss);
        }
        self.self_name = self.feed.self_name();
        self.status = self.feed.status();
        if self.tab == Tab::Setup && now_ms.saturating_sub(self.sections_ms) >= DIAGNOSTICS_MS {
            self.sections = self.diagnostics();
            self.sections_ms = now_ms;
        }
        self.history_selected = self.history_selected.min(self.history.len().saturating_sub(1));
        if matches!(&self.message, Some((_, until)) if *until <= now_ms) {
            self.message = None;
        }
    }

    fn diagnostics(&self) -> Vec<Section> {
        let mut sections = self.feed.diagnostics();
        let exe = self.overlay.exe().map(|p| p.display().to_string()).unwrap_or_else(|| "not found (pass --dps-exe)".into());
        sections.push(Section { title: "Overlay app", rows: vec![("aion2dps".into(), exe), ("state".into(), self.overlay.label())] });
        sections
    }

    /// The encounter the Live view shows: an opened past one, else the live (or last) one, with the display
    /// filters applied. `None` when there is none or the boss filter hides it.
    pub fn shown(&self) -> Option<Snapshot> {
        self.filtered(self.opened.clone().or_else(|| self.current.clone()))
    }

    /// The live (or last) encounter with the display filters applied, whatever the Live view has open.
    pub fn live(&self) -> Option<Snapshot> {
        self.filtered(self.current.clone())
    }

    fn filtered(&self, snap: Option<Snapshot>) -> Option<Snapshot> {
        let mut snap = snap?;
        if self.options.boss_only && !snap.boss.as_ref().is_some_and(|b| b.is_boss) {
            return None;
        }
        if self.options.party_only {
            snap.actors.retain(|a| a.is_party || a.is_self);
        }
        Some(snap)
    }

    /// The meter rows: players, most damage first, as the overlay ranks them.
    pub fn rows(snap: &Snapshot) -> Vec<&ActorStats> {
        let mut players: Vec<&ActorStats> = snap.actors.iter().filter(|a| a.is_player).collect();
        players.sort_by_key(|a| std::cmp::Reverse(a.damage));
        players
    }

    /// The selected player, or you, or the top of the meter.
    pub fn selected_actor<'a>(&self, rows: &[&'a ActorStats]) -> Option<&'a ActorStats> {
        self.selected
            .and_then(|id| rows.iter().find(|a| a.id == id))
            .or_else(|| rows.iter().find(|a| a.is_self))
            .or_else(|| rows.first())
            .copied()
    }

    /// A name for display: the class while names are hidden, `*` for you.
    pub fn display_name(&self, a: &ActorStats) -> String {
        let info = crate::dps::classes::class_info(a.class_id, a.class_name.as_deref());
        let name = if self.options.hide_names { info.name.to_string() } else { a.name.clone() };
        if a.is_self {
            format!("{name}*")
        } else {
            name
        }
    }

    pub fn you_label(&self) -> String {
        match &self.self_name {
            Some(_) if self.options.hide_names => "You".into(),
            Some(n) => n.clone(),
            None => "not identified".into(),
        }
    }

    pub fn say(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), self.now_ms + MESSAGE_MS));
    }

    fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
        if tab == Tab::Setup {
            self.sections = self.diagnostics();
            self.sections_ms = self.now_ms;
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if let Some(prompt) = self.prompt.take() {
            match (prompt, key.code) {
                (Prompt::Delete(id), KeyCode::Char('y') | KeyCode::Char('Y')) => match self.feed.delete(id) {
                    Ok(()) => {
                        if self.opened.as_ref().map(|s| s.id) == Some(id) {
                            self.opened = None;
                        }
                        self.say(format!("Encounter #{id} deleted"));
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
                    self.selected = None;
                    self.say("Back to the live encounter");
                } else if self.tab != Tab::Live {
                    self.set_tab(Tab::Live);
                }
            }
            KeyCode::Tab => self.set_tab(Tab::from_index(self.tab.index() + 1)),
            KeyCode::BackTab => self.set_tab(Tab::from_index(self.tab.index() + TABS.len() - 1)),
            KeyCode::Char(c @ '1'..='3') => self.set_tab(Tab::from_index(c as usize - '1' as usize)),
            KeyCode::Char('r') if self.tab == Tab::Setup => {
                self.feed.retry();
                self.say("Capture restarted");
            }
            KeyCode::Char('r') => {
                if self.opened.is_some() {
                    self.say("Close the opened encounter first (Esc)");
                } else {
                    self.new_fight();
                }
            }
            KeyCode::Char('c') => self.copy_summary(),
            KeyCode::Char('h') => {
                self.options.hide_names = !self.options.hide_names;
                self.say(if self.options.hide_names { "Names hidden" } else { "Names shown" });
            }
            KeyCode::Char('p') => {
                self.options.party_only = !self.options.party_only;
                self.say(if self.options.party_only { "Showing only you and your party" } else { "Showing everyone" });
            }
            KeyCode::Char('b') => {
                self.options.boss_only = !self.options.boss_only;
                self.refresh(self.now_ms);
                self.say(if self.options.boss_only { "Showing only boss fights" } else { "Showing every fight" });
            }
            KeyCode::Char('o') => self.toggle_overlay(),
            KeyCode::Char('w') => match self.overlay.toggle_main_window() {
                Ok(done) => self.say(done),
                Err(e) => self.say(e),
            },
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
            KeyCode::Char('[') => self.skills_scroll = self.skills_scroll.saturating_sub(5),
            KeyCode::Char(']') => self.skills_scroll += 5,
            _ => {}
        }
    }

    /// Ends the live encounter, so the next hit starts a new one.
    pub fn new_fight(&mut self) {
        self.feed.reset(self.now_ms);
        self.refresh(self.now_ms);
        self.say("Encounter ended, the next hit starts a new one");
    }

    fn toggle_overlay(&mut self) {
        if matches!(self.overlay.state(), crate::dps::overlay::State::Running { .. }) {
            self.overlay.stop();
            self.say("Overlay app closed, o starts it again");
        } else {
            match self.overlay.start(self.now_ms) {
                Ok(()) if crate::dps::overlay::app_overlay_visible() => self.say("Overlay app starting"),
                Ok(()) => self.say("Overlay app starting; its overlay is off in its settings, its Ctrl+Shift+O shows it"),
                Err(e) => self.say(e),
            }
        }
    }

    /// Moves the selection of the active view; the views clamp what they show.
    fn scroll(&mut self, by: i64) {
        match self.tab {
            Tab::Live => self.move_selection(by),
            Tab::History => {
                let v = (self.history_selected as i64 + by).max(0) as usize;
                self.history_selected = v.min(self.history.len().saturating_sub(1));
            }
            Tab::Setup => self.setup_scroll = (i64::from(self.setup_scroll) + by).clamp(0, i64::from(u16::MAX)) as u16,
        }
    }

    fn move_selection(&mut self, by: i64) {
        let Some(snap) = self.shown() else { return };
        let rows = App::rows(&snap);
        if rows.is_empty() {
            return;
        }
        let at = self.selected_actor(&rows).and_then(|a| rows.iter().position(|r| r.id == a.id)).unwrap_or(0);
        let next = (at as i64 + by).clamp(0, rows.len() as i64 - 1) as usize;
        self.selected = Some(rows[next].id);
        self.skills_scroll = 0;
    }

    fn open_selected(&mut self) {
        let Some(summary) = self.history.get(self.history_selected) else { return };
        let id = summary.id;
        match self.feed.encounter(id) {
            Some(snap) => {
                self.opened = Some(snap);
                self.selected = None;
                self.skills_scroll = 0;
                self.set_tab(Tab::Live);
                self.say(format!("Viewing encounter #{id}. Esc returns to the live one"));
            }
            None => self.say(format!("Encounter #{id} could not be loaded")),
        }
    }

    fn copy_summary(&mut self) {
        let Some(snap) = self.shown() else {
            self.say("No encounter to summarise yet");
            return;
        };
        let line = chat_summary(&snap);
        let sent = crate::clipboard::copy(&line);
        self.say(if sent { format!("Copied: {line}") } else { line });
    }

    pub fn shutdown(&mut self) {
        self.overlay.shutdown();
    }
}
