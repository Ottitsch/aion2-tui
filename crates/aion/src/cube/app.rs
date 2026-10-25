//! TUI state and key handling. Rendering lives in `ui.rs`, the 3D scene in `view3d.rs`.

use crate::cube::detector::{self, Tone, Tracked};
use crate::cube::feed::{Feed, Section, Status};
use cube_core::parser::EntityId;
use std::collections::HashSet;
use crate::cube::view3d::{metres, Camera3d};
use cube_core::geo::Vec3;
use cube_core::snapshot::{Alert, AlertKind, CubeState, CubeView, RadarSnapshot};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use serde::{Deserialize, Serialize};

/// How long a footer message stays.
const MESSAGE_MS: u64 = 4_000;
/// The Setup view polls the diagnostics this often.
const DIAGNOSTICS_MS: u64 = 1_000;
/// Zoom limits of the radar chart, as metres from its centre to the nearer edge.
const MAP_MIN_HALF_M: f64 = 20.0;
const MAP_MAX_HALF_M: f64 = 20_000.0;
/// 3D view: radians per key press and per dragged cell.
const TURN_KEY: f64 = 0.15;
const TILT_KEY: f64 = 0.1;
const TURN_DRAG: f64 = 0.05;
const TILT_DRAG: f64 = 0.08;
/// Your dot glides to a new fix in this long, or less when fixes come faster (seconds).
const GLIDE_MIN_S: f64 = 0.15;
const GLIDE_MAX_S: f64 = 0.6;
/// A move longer than this (a teleport, a loading screen) is shown at once.
const GLIDE_JUMP_M: f64 = 60.0;
/// A fix closer than this to where your dot is drawn is no move: even the closest zoom draws about 0.2 m per dot,
/// and a glide redraws the map 25 times a second.
const GLIDE_STILL_M: f64 = 0.1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Radar,
    View3d,
    Log,
    Setup,
}

pub const TABS: [(Tab, &str); 4] = [(Tab::Radar, "Radar"), (Tab::View3d, "3D"), (Tab::Log, "Log"), (Tab::Setup, "Setup")];

impl Tab {
    pub fn index(self) -> usize {
        TABS.iter().position(|(t, _)| *t == self).unwrap_or(0)
    }

    fn from_index(i: usize) -> Tab {
        TABS[i % TABS.len()].0
    }
}

/// How long an alert banner stays up.
pub fn banner_ms(kind: AlertKind) -> u64 {
    match kind {
        AlertKind::Appeared => 10_000,
        _ => 5_000,
    }
}

/// The banner text of an alert.
pub fn alert_title(kind: AlertKind) -> &'static str {
    match kind {
        AlertKind::Appeared => "HIDDEN CUBE NEARBY",
        AlertKind::StillThere => "HIDDEN CUBE STILL THERE",
        AlertKind::OpenedByYou => "CUBE OPENED",
        AlertKind::OpenedByOther => "CUBE TAKEN BY SOMEONE ELSE",
        AlertKind::Learned => "NEW CUBE TEMPLATE LEARNED",
    }
}

/// `A`, `B`, ... for the cubes in snapshot order, the same in every view.
pub fn cube_label(i: usize) -> String {
    char::from(b'A' + (i % 26) as u8).to_string()
}

pub struct ViewOptions {
    /// Mask the character name (streaming).
    pub hide_name: bool,
    pub sound: bool,
    /// Beeps for the nearest cube still there, faster as you get closer (`detector.rs`).
    pub detector: bool,
}

/// Where the sounds go: the sound thread, or nowhere in tests.
#[derive(Clone, Copy)]
pub struct Speaker {
    pub alert: fn(AlertKind),
    pub track: fn(Option<Tone>),
    pub preview: fn(),
}

impl Speaker {
    pub const LIVE: Speaker = Speaker { alert: crate::cube::sound::play, track: crate::cube::sound::track, preview: crate::cube::sound::preview };
    #[cfg(test)]
    pub const SILENT: Speaker = Speaker { alert: |_| {}, track: |_| {}, preview: || {} };
}

/// What the radar chart centres on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapMode {
    /// You, at the chosen zoom.
    Follow,
    /// Everything on the map: your path, the cubes and the spots.
    Fit,
    /// Where you panned to.
    Free,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapView {
    pub mode: MapMode,
    /// Metres from the centre to the nearer edge.
    pub half_m: f64,
    /// Centre in metres east and north, as last drawn.
    pub center: (f64, f64),
}

/// Your dot on the radar gliding from where it was drawn to the newest fix, so it moves instead of jumping.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Glide {
    from: (f64, f64),
    to: (f64, f64),
    /// Seconds gone and seconds it takes.
    t: f64,
    dur: f64,
    fix_ms: u64,
}

impl Glide {
    fn at(&self) -> (f64, f64) {
        let x = (self.t / self.dur).clamp(0.0, 1.0);
        let f = 1.0 - (1.0 - x) * (1.0 - x);
        (self.from.0 + (self.to.0 - self.from.0) * f, self.from.1 + (self.to.1 - self.from.1) * f)
    }
}

/// A mouse drag in the 3D view: the last cell and whether it pans (right button) or turns.
#[derive(Debug, Clone, Copy)]
struct Drag {
    column: u16,
    row: u16,
    pan: bool,
}

/// The map the cube tab opens on and the overview shows: the radar or the 3D view, whichever was opened last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MapKind {
    #[default]
    #[serde(rename = "radar")]
    Radar,
    #[serde(rename = "3d")]
    View3d,
}

pub struct App {
    pub feed: Box<dyn Feed>,
    pub tab: Tab,
    pub map_kind: MapKind,
    pub options: ViewOptions,
    pub snap: RadarSnapshot,
    pub status: Status,
    pub sections: Vec<Section>,
    sections_ms: u64,
    pub now_ms: u64,
    /// The alert on the banner and when it goes away.
    pub banner: Option<(Alert, u64)>,
    pub map: MapView,
    pub cam: Camera3d,
    drag: Option<Drag>,
    pub log_scroll: usize,
    pub setup_scroll: u16,
    pub message: Option<(String, u64)>,
    pub quit: bool,
    glide: Option<Glide>,
    speaker: Speaker,
    /// Cubes `esc` silenced the detector for.
    quiet: HashSet<EntityId>,
    /// The cube the detector beeps for now.
    pub tracking: Option<Tracked>,
}

impl App {
    pub fn new(feed: Box<dyn Feed>, options: ViewOptions, now_ms: u64) -> App {
        Self::with_speaker(feed, options, now_ms, Speaker::LIVE)
    }

    pub fn with_speaker(feed: Box<dyn Feed>, options: ViewOptions, now_ms: u64, speaker: Speaker) -> App {
        let status = feed.status();
        let mut app = App {
            feed,
            tab: Tab::Radar,
            map_kind: MapKind::Radar,
            options,
            snap: RadarSnapshot::default(),
            status,
            sections: Vec::new(),
            sections_ms: 0,
            now_ms,
            banner: None,
            map: MapView { mode: MapMode::Follow, half_m: 150.0, center: (0.0, 0.0) },
            cam: Camera3d::default(),
            drag: None,
            log_scroll: 0,
            setup_scroll: 0,
            message: None,
            quit: false,
            glide: None,
            speaker,
            quiet: HashSet::new(),
            tracking: None,
        };
        app.refresh(now_ms);
        app
    }

    /// Advances the feed, raises its alerts and copies what the views need.
    pub fn step(&mut self, now_ms: u64) {
        self.feed.step(now_ms);
        for alert in self.feed.drain_alerts() {
            self.raise(alert, now_ms);
        }
        self.refresh(now_ms);
    }

    /// Shows the alert, unless a cube to go for is on the banner and this one is less urgent, and plays its sound.
    fn raise(&mut self, alert: Alert, now_ms: u64) {
        if self.options.sound {
            (self.speaker.alert)(alert.kind);
        }
        let urgent = |k: AlertKind| matches!(k, AlertKind::Appeared | AlertKind::StillThere);
        let keep = matches!(&self.banner, Some((shown, until)) if *until > now_ms && urgent(shown.kind) && !urgent(alert.kind));
        if !keep {
            let until = now_ms + banner_ms(alert.kind);
            self.banner = Some((alert, until));
        }
    }

    fn refresh(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
        self.snap = self.feed.snapshot();
        self.status = self.feed.status();
        self.follow_fix();
        if self.tab == Tab::Setup && now_ms.saturating_sub(self.sections_ms) >= DIAGNOSTICS_MS {
            self.sections = self.feed.diagnostics();
            self.sections_ms = now_ms;
        }
        if matches!(&self.banner, Some((_, until)) if *until <= now_ms) {
            self.banner = None;
        }
        if matches!(&self.message, Some((_, until)) if *until <= now_ms) {
            self.message = None;
        }
        self.update_detector();
    }

    /// Tells the sound thread what to beep for the nearest cube still there, when that changed.
    fn update_detector(&mut self) {
        self.quiet.retain(|id| self.snap.cubes.iter().any(|c| c.id == *id));
        let tracking = (self.options.sound && self.options.detector).then(|| detector::nearest(&self.snap, &self.quiet)).flatten();
        let tone = tracking.map(|t| t.tone);
        if tone != self.tracking.map(|t| t.tone) {
            (self.speaker.track)(tone);
        }
        self.tracking = tracking;
    }

    /// Takes the alert off the banner and silences the detector for the cubes around now; a new cube beeps again.
    /// Returns whether there was anything to dismiss.
    pub fn dismiss(&mut self) -> bool {
        let had_banner = self.banner.take().is_some();
        let beeping = self.tracking.is_some();
        if beeping {
            self.quiet.extend(self.snap.cubes.iter().map(|c| c.id));
            self.update_detector();
            self.say("Detector quiet until another cube comes");
        }
        had_banner || beeping
    }

    pub fn toggle_detector(&mut self) {
        self.options.detector = !self.options.detector;
        self.update_detector();
        self.say(if self.options.detector { "Detector on: beeps faster as you get closer, higher above you, lower below" } else { "Detector off: one alert per cube" });
    }

    /// Starts your dot gliding toward a new fix: from where it is drawn now, over about the time since the last
    /// fix, so it keeps moving until the next one is due.
    fn follow_fix(&mut self) {
        let Some(fix) = self.snap.me else {
            self.glide = None;
            return;
        };
        let m = metres(&fix.pos);
        let to = (m[0], m[1]);
        if self.glide.is_some_and(|g| g.fix_ms == fix.t_ms && g.to == to) {
            return;
        }
        self.glide = Some(match self.glide {
            Some(g) if (g.at().0 - to.0).hypot(g.at().1 - to.1) < GLIDE_STILL_M => Glide { from: to, to, t: 1.0, dur: 1.0, fix_ms: fix.t_ms },
            Some(g) if (g.at().0 - to.0).hypot(g.at().1 - to.1) <= GLIDE_JUMP_M => {
                let gap = fix.t_ms.saturating_sub(g.fix_ms) as f64 / 1_000.0;
                Glide { from: g.at(), to, t: 0.0, dur: (gap * 0.8).clamp(GLIDE_MIN_S, GLIDE_MAX_S), fix_ms: fix.t_ms }
            }
            _ => Glide { from: to, to, t: 1.0, dur: 1.0, fix_ms: fix.t_ms },
        });
    }

    /// Where your dot is drawn, in metres east and north: on its way to the newest fix.
    pub fn you(&self) -> Option<(f32, f32)> {
        self.glide.map(|g| {
            let (x, y) = g.at();
            (x as f32, y as f32)
        })
    }

    /// Whether the screen should be redrawn faster than the data changes (your dot glides or the 3D view turns).
    pub fn animating(&self) -> bool {
        (self.tab == Tab::Radar && self.gliding()) || (self.tab == Tab::View3d && (self.cam.spin || self.drag.is_some()))
    }

    /// Whether your dot on the radar is still on its way to the newest fix.
    pub fn gliding(&self) -> bool {
        self.glide.is_some_and(|g| g.t < g.dur)
    }

    /// Whether the overview's map moves: your dot gliding on the radar, or the 3D view orbiting.
    pub fn overview_animating(&self) -> bool {
        match self.map_kind {
            MapKind::Radar => self.gliding(),
            MapKind::View3d => self.cam.spin,
        }
    }

    /// Moves your dot and the 3D camera's auto orbit on by `dt` seconds.
    pub fn animate(&mut self, dt: f64) {
        if let Some(g) = &mut self.glide {
            g.t = (g.t + dt).min(g.dur);
        }
        if self.map_kind == MapKind::View3d {
            self.cam.animate(dt);
        }
    }

    /// The banner's cube as the radar sees it now (it may have moved on since the alert), else as alerted.
    pub fn banner_cube(&self) -> Option<(AlertKind, CubeView)> {
        let (alert, _) = self.banner.as_ref()?;
        let current = self.snap.cubes.iter().find(|c| c.id == alert.cube.id).cloned();
        Some((alert.kind, current.unwrap_or_else(|| alert.cube.clone())))
    }

    pub fn character_label(&self) -> String {
        match &self.snap.character {
            Some(_) if self.options.hide_name => "Character".into(),
            Some(c) => c.name.clone(),
            None => "unidentified".into(),
        }
    }

    pub fn say(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), self.now_ms + MESSAGE_MS));
    }

    /// Opens a view by its name (`radar`, `3d`, `log`, `setup`).
    #[cfg(test)]
    pub fn show_view(&mut self, name: &str) -> bool {
        match TABS.iter().find(|(_, label)| label.eq_ignore_ascii_case(name)) {
            Some(&(tab, _)) => {
                self.set_tab(tab);
                true
            }
            None => false,
        }
    }

    /// Opens the radar or the 3D view, which the overview then shows too.
    pub fn open_map(&mut self, kind: MapKind) {
        self.set_tab(match kind {
            MapKind::Radar => Tab::Radar,
            MapKind::View3d => Tab::View3d,
        });
    }

    fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.drag = None;
        match tab {
            Tab::Radar => self.map_kind = MapKind::Radar,
            Tab::View3d => self.map_kind = MapKind::View3d,
            _ => {}
        }
        if tab == Tab::Setup {
            self.sections = self.feed.diagnostics();
            self.sections_ms = self.now_ms;
        }
    }

    pub fn toggle_sound(&mut self) {
        self.options.sound = !self.options.sound;
        self.update_detector();
        self.say(if self.options.sound { "Sound on" } else { "Sound muted" });
    }

    /// A made up cube 60 m north east, to check the banner and the volume.
    fn test_alert(&mut self) {
        let cube = CubeView {
            id: 0,
            template: 1_200_039,
            zone: Some("test".into()),
            evidence: None,
            pos: Vec3::default(),
            state: CubeState::Visible,
            opener: None,
            opened_by_you: false,
            first_seen_ms: self.now_ms,
            state_ms: self.now_ms,
            age_ms: Some(42_000),
            distance_m: Some(60.0),
            height_m: Some(2.0),
            bearing: Some(45.0),
            estimated: false,
        };
        self.raise(Alert { t_ms: self.now_ms, kind: AlertKind::Appeared, cube }, self.now_ms);
        if !self.options.sound {
            self.say("Test alert (sound is muted, m turns it on)");
        } else if self.options.detector {
            (self.speaker.preview)();
            self.say("The alert, then the detector from 80 m to 3 m, then a cube above you (high) and below (low)");
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if !ctrl && self.tab == Tab::Radar && self.on_map_key(key.code) {
            return;
        }
        if !ctrl && self.tab == Tab::View3d && self.on_3d_key(key) {
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('c') if ctrl => self.quit = true,
            KeyCode::Esc => {
                if !self.dismiss() && self.tab != Tab::Radar {
                    self.set_tab(Tab::Radar);
                }
            }
            KeyCode::Char('d') => self.toggle_detector(),
            KeyCode::Tab => self.set_tab(Tab::from_index(self.tab.index() + 1)),
            KeyCode::BackTab => self.set_tab(Tab::from_index(self.tab.index() + TABS.len() - 1)),
            KeyCode::Char(c @ '1'..='4') => self.set_tab(Tab::from_index(c as usize - '1' as usize)),
            KeyCode::Char('m') => self.toggle_sound(),
            KeyCode::Char('t') => self.test_alert(),
            KeyCode::Char('h') => {
                self.options.hide_name = !self.options.hide_name;
                self.say(if self.options.hide_name { "Character name hidden" } else { "Character name shown" });
            }
            KeyCode::Char('r') if self.tab == Tab::Setup => {
                self.feed.retry();
                self.say("Capture restarted");
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

    /// Zoom and pan keys of the radar chart. Returns whether the key was one of them.
    fn on_map_key(&mut self, code: KeyCode) -> bool {
        let m = &mut self.map;
        let pan = m.half_m * 0.25;
        match code {
            KeyCode::Char('+') | KeyCode::Char('=') => m.half_m = (m.half_m / 1.5).max(MAP_MIN_HALF_M),
            KeyCode::Char('-') => m.half_m = (m.half_m * 1.5).min(MAP_MAX_HALF_M),
            KeyCode::Char('c') => m.mode = MapMode::Follow,
            KeyCode::Char('a') => m.mode = MapMode::Fit,
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => {
                // Panning starts from what is shown; in Fit mode that zoom becomes the free zoom.
                m.mode = MapMode::Free;
                let (east, north) = match code {
                    KeyCode::Left => (-pan, 0.0),
                    KeyCode::Right => (pan, 0.0),
                    KeyCode::Up => (0.0, pan),
                    _ => (0.0, -pan),
                };
                m.center = (m.center.0 + east, m.center.1 + north);
            }
            _ => return false,
        }
        if matches!(code, KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Char('-')) && m.mode == MapMode::Fit {
            m.mode = MapMode::Free;
        }
        true
    }

    /// Where the 3D camera looks now: its own target, else you.
    fn cam_target(&self) -> [f64; 3] {
        self.cam.target.or_else(|| self.snap.me.map(|m| metres(&m.pos))).unwrap_or_default()
    }

    /// Camera keys of the 3D view. Returns whether the key was one of them.
    fn on_3d_key(&mut self, key: KeyEvent) -> bool {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let pan = self.cam.dist * 0.15;
        let from = self.cam_target();
        let cam = &mut self.cam;
        match key.code {
            KeyCode::Left if shift => cam.pan(from, -pan, 0.0),
            KeyCode::Right if shift => cam.pan(from, pan, 0.0),
            KeyCode::Up if shift => cam.pan(from, 0.0, pan),
            KeyCode::Down if shift => cam.pan(from, 0.0, -pan),
            KeyCode::Left => cam.rotate(TURN_KEY, 0.0),
            KeyCode::Right => cam.rotate(-TURN_KEY, 0.0),
            KeyCode::Up => cam.rotate(0.0, TILT_KEY),
            KeyCode::Down => cam.rotate(0.0, -TILT_KEY),
            KeyCode::Char('+') | KeyCode::Char('=') => cam.zoom(1.0 / 1.3),
            KeyCode::Char('-') => cam.zoom(1.3),
            KeyCode::Char('v') => cam.toggle_top(),
            KeyCode::Char('o') => cam.spin = !cam.spin,
            KeyCode::Char('c') => cam.target = None,
            KeyCode::Char('p') => cam.show_path = !cam.show_path,
            KeyCode::Char('s') => cam.show_spots = !cam.show_spots,
            KeyCode::Char('0') => cam.reset(),
            _ => return false,
        }
        true
    }

    /// Mouse in the 3D view: drag to turn, right drag to pan, wheel to zoom.
    pub fn on_mouse(&mut self, m: MouseEvent) {
        if self.tab != Tab::View3d {
            self.drag = None;
            return;
        }
        match m.kind {
            MouseEventKind::Down(button) => {
                let pan = button == MouseButton::Right || m.modifiers.contains(KeyModifiers::SHIFT);
                self.drag = Some(Drag { column: m.column, row: m.row, pan });
            }
            MouseEventKind::Drag(_) => {
                let Some(d) = self.drag else { return };
                let (dx, dy) = (f64::from(m.column) - f64::from(d.column), f64::from(m.row) - f64::from(d.row));
                if d.pan {
                    // About as far as the ground moves under the pointer at the target.
                    let per_cell = self.cam.dist * 0.012;
                    let from = self.cam_target();
                    self.cam.pan(from, -dx * per_cell, dy * per_cell * 2.0);
                } else {
                    self.cam.rotate(-dx * TURN_DRAG, dy * TILT_DRAG);
                }
                self.drag = Some(Drag { column: m.column, row: m.row, ..d });
            }
            MouseEventKind::Up(_) => self.drag = None,
            MouseEventKind::ScrollUp => self.cam.zoom(1.0 / 1.15),
            MouseEventKind::ScrollDown => self.cam.zoom(1.15),
            _ => {}
        }
    }

    /// Moves the list of the active view; the views clamp the offsets to what they hold.
    fn scroll(&mut self, by: i64) {
        let apply = |v: usize| (v as i64 + by).max(0) as usize;
        match self.tab {
            Tab::Radar | Tab::View3d => {}
            Tab::Log => self.log_scroll = apply(self.log_scroll),
            Tab::Setup => self.setup_scroll = apply(self.setup_scroll as usize).min(u16::MAX as usize) as u16,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::cube::demo::Demo;
    use cube_core::cubes::CubeBook;
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};

    pub(crate) const START: u64 = 1_791_722_000_000;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent { code, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE }
    }


    /// A demo app advanced `secs` seconds into its first round.
    pub(crate) fn demo_app(secs: u64) -> App {
        let options = ViewOptions { hide_name: false, sound: true, detector: true };
        let mut app = App::with_speaker(Box::new(Demo::new(START, CubeBook::default())), options, START, Speaker::SILENT);
        for i in 0..=secs * 4 {
            app.step(START + i * 250);
        }
        app
    }

    std::thread_local! {
        static TOLD: std::cell::RefCell<Vec<Option<Tone>>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    fn told() -> Vec<Option<Tone>> {
        TOLD.with(|t| t.borrow().clone())
    }

    /// A speaker that keeps what the detector was told, per test thread.
    const RECORDING: Speaker = Speaker { alert: |_| {}, track: |t| TOLD.with(|told| told.borrow_mut().push(t)), preview: || {} };

    #[test]
    fn the_detector_beeps_for_the_nearest_cube_until_dismissed() {
        let options = ViewOptions { hide_name: false, sound: true, detector: true };
        let mut app = App::with_speaker(Box::new(Demo::new(START, CubeBook::default())), options, START, RECORDING);
        let mut t = START;
        while app.tracking.is_none() && t < START + 120_000 {
            t += 250;
            app.step(t);
        }
        let tracked = app.tracking.expect("the demo brings a cube within range");
        let cube = app.snap.cubes.iter().find(|c| c.id == tracked.id).unwrap();
        assert!(matches!(cube.state, CubeState::Visible | CubeState::OutOfView));
        assert_eq!(told().last(), Some(&Some(tracked.tone)), "the sound thread was told");
        assert!(tracked.straight_m >= cube.distance_m.unwrap(), "height only adds to the distance");

        app.on_key(key(KeyCode::Char('d')));
        assert!(app.tracking.is_none() && !app.options.detector);
        assert_eq!(told().last(), Some(&None), "off stops the beeping");
        app.on_key(key(KeyCode::Char('d')));
        assert!(app.tracking.is_some());

        app.on_key(key(KeyCode::Char('m')));
        assert!(app.tracking.is_none(), "muted is quiet too");
        app.on_key(key(KeyCode::Char('m')));
        assert!(app.tracking.is_some());

        app.banner = None;
        app.on_key(key(KeyCode::Esc));
        assert!(app.tracking.is_none() && app.tab == Tab::Radar, "esc silences the cubes around, and stays on the radar");
        assert_eq!(told().last(), Some(&None));
        app.step(t + 250);
        assert!(app.tracking.is_none(), "and they stay quiet");
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE }
    }

    #[test]
    fn alerts_put_the_cube_on_the_banner() {
        let app = demo_app(5);
        let (kind, cube) = app.banner_cube().unwrap();
        assert_eq!(kind, AlertKind::Appeared);
        assert_eq!(cube.template, 1_200_039, "cube A, the latest");
        assert!(cube.distance_m.is_some());
    }

    #[test]
    fn a_less_urgent_alert_does_not_hide_a_cube_to_go_for() {
        let mut app = demo_app(0);
        app.on_key(key(KeyCode::Char('t')));
        let cube = app.banner.as_ref().unwrap().0.cube.clone();
        let now = app.now_ms;
        app.raise(Alert { t_ms: now + 1_000, kind: AlertKind::OpenedByOther, cube: cube.clone() }, now + 1_000);
        assert_eq!(app.banner.as_ref().unwrap().0.kind, AlertKind::Appeared);
        let later = now + banner_ms(AlertKind::Appeared);
        app.raise(Alert { t_ms: later, kind: AlertKind::OpenedByOther, cube }, later);
        assert_eq!(app.banner.as_ref().unwrap().0.kind, AlertKind::OpenedByOther, "once the cube's banner is over");
    }

    #[test]
    fn banners_expire() {
        // Cube B's banner from 14 s and the news that someone opened it at 24.25 s are both over at 36 s.
        assert!(demo_app(20).banner.is_some());
        assert!(demo_app(36).banner.is_none());
    }

    #[test]
    fn keys_switch_views_mute_and_test() {
        let mut app = demo_app(1);
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.tab, Tab::View3d);
        app.on_key(key(KeyCode::BackTab));
        app.on_key(key(KeyCode::BackTab));
        assert_eq!(app.tab, Tab::Setup);
        assert!(!app.sections.is_empty(), "the Setup view loads its diagnostics");
        app.on_key(key(KeyCode::Char('m')));
        assert!(!app.options.sound);
        app.on_key(key(KeyCode::Char('t')));
        assert!(app.banner.is_some());
        app.on_key(key(KeyCode::Esc));
        assert!(app.banner.is_none(), "Esc dismisses the banner first");
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.tab, Tab::Radar);
        app.on_key(key(KeyCode::Char('h')));
        assert_eq!(app.character_label(), "Character");
        app.on_key(key(KeyCode::Char('q')));
        assert!(app.quit);
    }

    #[test]
    fn radar_keys_zoom_pan_and_follow() {
        let mut app = demo_app(1);
        assert_eq!(app.tab, Tab::Radar);
        let half = app.map.half_m;
        app.on_key(key(KeyCode::Char('+')));
        assert!(app.map.half_m < half);
        app.on_key(key(KeyCode::Char('-')));
        app.on_key(key(KeyCode::Char('-')));
        assert!(app.map.half_m > half);
        app.map.center = (100.0, 100.0);
        app.on_key(key(KeyCode::Right));
        assert_eq!(app.map.mode, MapMode::Free);
        assert!(app.map.center.0 > 100.0);
        app.on_key(key(KeyCode::Char('a')));
        assert_eq!(app.map.mode, MapMode::Fit);
        app.on_key(key(KeyCode::Char('c')));
        assert_eq!(app.map.mode, MapMode::Follow);
        app.on_key(key(KeyCode::Char('3')));
        assert_eq!(app.tab, Tab::Log, "number keys still switch views");
    }

    #[test]
    fn keys_and_the_mouse_move_the_3d_camera() {
        let mut app = demo_app(1);
        app.on_key(key(KeyCode::Char('2')));
        assert_eq!(app.tab, Tab::View3d);
        let start = app.cam;
        app.on_key(key(KeyCode::Left));
        app.on_key(key(KeyCode::Up));
        app.on_key(key(KeyCode::Char('+')));
        assert!(app.cam.yaw > start.yaw && app.cam.pitch > start.pitch && app.cam.dist < start.dist);
        app.on_key(KeyEvent { code: KeyCode::Right, modifiers: KeyModifiers::SHIFT, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        assert!(app.cam.target.is_some(), "shift pans away from you");
        app.on_key(key(KeyCode::Char('c')));
        assert!(app.cam.target.is_none(), "c follows you again");
        app.on_key(key(KeyCode::Char('o')));
        assert!(app.animating());
        let yaw = app.cam.yaw;
        app.animate(1.0);
        assert!(app.cam.yaw > yaw);
        app.on_key(key(KeyCode::Char('o')));
        // Drag left to turn, wheel to zoom.
        let yaw = app.cam.yaw;
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 50, 10));
        assert!(app.animating(), "redraw fast while dragging");
        app.on_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 40, 10));
        assert!(app.cam.yaw > yaw);
        app.on_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 40, 10));
        assert!(!app.animating());
        let dist = app.cam.dist;
        app.on_mouse(mouse(MouseEventKind::ScrollDown, 40, 10));
        assert!(app.cam.dist > dist);
        app.on_key(key(KeyCode::Char('v')));
        assert!(app.cam.top);
        app.on_key(key(KeyCode::Char('0')));
        assert_eq!(app.cam, Camera3d::default());
        app.on_key(key(KeyCode::Char('4')));
        assert_eq!(app.tab, Tab::Setup);
    }

    #[test]
    fn views_open_by_name() {
        let mut app = demo_app(0);
        assert!(app.show_view("3d"));
        assert_eq!(app.tab, Tab::View3d);
        assert!(app.show_view("RADAR"));
        assert_eq!(app.tab, Tab::Radar);
        assert!(!app.show_view("map"), "the map is the radar now");
    }

    #[test]
    fn your_dot_glides_to_each_new_fix() {
        // The demo sends a fix every second.
        let mut app = demo_app(6);
        app.animate(5.0);
        let rest = app.you().unwrap();
        assert!(!app.animating(), "settled on the fix");
        app.step(START + 7_000);
        let fix = app.snap.me.map(|m| metres(&m.pos)).unwrap();
        assert_eq!(app.you().unwrap(), rest, "the new fix starts from where the dot was");
        assert!(app.animating());
        app.animate(0.2);
        let mid = app.you().unwrap();
        assert!(mid != rest && (f64::from(mid.0) - fix[0]).abs() > 0.01, "on its way: {mid:?} toward {fix:?}");
        app.animate(1.0);
        let end = app.you().unwrap();
        assert!((f64::from(end.0) - fix[0]).abs() < 1e-3 && (f64::from(end.1) - fix[1]).abs() < 1e-3);
        assert!(!app.animating());
    }

    #[test]
    fn a_fix_on_the_same_spot_does_not_animate() {
        let mut app = demo_app(6);
        app.animate(5.0);
        let mut me = app.snap.me.unwrap();
        me.t_ms += 600;
        app.snap.me = Some(me);
        app.follow_fix();
        assert!(!app.animating(), "standing still in a fight sends a fix every 0.6 s, nothing to redraw");
        me.t_ms += 600;
        me.pos.x += 300.0;
        app.snap.me = Some(me);
        app.follow_fix();
        assert!(app.animating(), "a 3 m move glides as before");
    }
}
