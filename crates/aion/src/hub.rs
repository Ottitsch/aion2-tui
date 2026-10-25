//! The one window: a top bar with the tabs and what matters from each module, and below it the active tab: the
//! overview of all four, or one module.
//!
//! Keys go to the active tab, except the tab switches (F1 to F5, or Alt+1 to 5). Every module is stepped on every
//! tick whichever tab is open, so a cube alert still sounds (and shows as a strip on the module tabs), kina
//! sessions still save and the meter still files fights.

use crate::settings::{Settings, Store};
use crate::source::{Health, Shared};
use crate::{cube, dps, keys, kina, overview};
use cube_core::geo::{arrow, compass};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Module {
    Overview,
    Dps,
    Cube,
    Kina,
    Keys,
}

pub const MODULES: [(Module, &str); 5] =
    [(Module::Overview, "Overview"), (Module::Dps, "DPS"), (Module::Cube, "Cube"), (Module::Kina, "Kina"), (Module::Keys, "Keys")];

impl Module {
    pub fn from_name(name: &str) -> Option<Module> {
        if name.eq_ignore_ascii_case("all") {
            return Some(Module::Overview);
        }
        MODULES.iter().find(|(_, label)| label.eq_ignore_ascii_case(name)).map(|(m, _)| *m)
    }

    /// The function key that opens it: `F1` for the overview, `F3` for the cube radar.
    pub fn key(self) -> String {
        let at = MODULES.iter().position(|(m, _)| *m == self).unwrap_or(0);
        format!("F{}", at + 1)
    }
}

const BAR_BG: Color = Color::Rgb(0x20, 0x20, 0x20);

pub struct Hub {
    pub module: Module,
    pub dps: dps::app::App,
    pub cube: cube::app::App,
    pub kina: kina::app::App,
    pub keys: keys::App,
    /// The shared capture in live mode; `None` for demo and replay.
    pub live: Option<Arc<Shared>>,
    /// What the tabs are set to, saved for the next start whenever it changes.
    pub settings: Option<Store>,
    /// The game's sound in the Windows mixer (ctrl+m); `None` in tests, which never touch it.
    pub game_audio: Option<crate::game_audio::GameAudio>,
    /// Ctrl+R and Ctrl+M from anywhere, also the game; `None` in tests, which never register them.
    pub hotkeys: Option<crate::hotkeys::Hotkeys>,
    pub quit: bool,
}

impl Hub {
    pub fn step(&mut self, now_ms: u64) {
        if let Some(shared) = &self.live {
            shared.step(now_ms);
        }
        for key in self.hotkeys.as_ref().map(crate::hotkeys::Hotkeys::pressed).unwrap_or_default() {
            self.on_hotkey(key);
        }
        self.dps.step(now_ms);
        self.cube.step(now_ms);
        self.kina.step(now_ms);
        self.keys.step(now_ms);
        if self.settings.is_some() {
            let now = Settings::of(self);
            if let Some(store) = &mut self.settings {
                store.update(now);
            }
        }
    }

    /// Whether to redraw faster than the data changes: only the cube radar and 3D view animate, on the cube tab
    /// and on the overview.
    pub fn animating(&self) -> bool {
        match self.module {
            Module::Cube => self.cube.animating(),
            Module::Overview => self.cube.overview_animating(),
            _ => false,
        }
    }

    pub fn animate(&mut self, dt: f64) {
        self.cube.animate(dt);
    }

    /// The mouse comes to the app while hold buttons run, and in the 3D view, which turns with it. Hold buttons
    /// press both buttons wherever the pointer is: over this window the terminal would start a text selection and
    /// paste the clipboard on the right button, typing it into the app as keys (a space toggled the hold again, a
    /// `q` would quit). Shift+drag still selects text then. Otherwise the terminal keeps the mouse, so plain
    /// dragging selects.
    pub fn wants_mouse(&self) -> bool {
        self.keys.engine.running() || (self.module == Module::Cube && self.cube.tab == cube::app::Tab::View3d)
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::F(n @ 1..=5) => {
                self.module = MODULES[usize::from(n) - 1].0;
                return;
            }
            KeyCode::Char(c @ '1'..='5') if alt => {
                self.module = MODULES[c as usize - '1' as usize].0;
                return;
            }
            _ => {}
        }
        match self.module {
            Module::Overview => overview::on_key(self, key),
            Module::Dps => self.dps.on_key(key),
            Module::Cube => self.cube.on_key(key),
            Module::Kina => self.kina.on_key(key),
            Module::Keys => self.keys.on_key(key),
        }
        if self.dps.quit || self.cube.quit || self.kina.quit || self.keys.quit {
            self.quit = true;
        }
    }

    /// A global hotkey, pressed in the game or anywhere else.
    pub fn on_hotkey(&mut self, key: crate::hotkeys::Hotkey) {
        match key {
            crate::hotkeys::Hotkey::NewFight => self.dps.new_fight(),
            crate::hotkeys::Hotkey::GameSound => {
                if let Some(audio) = &self.game_audio {
                    audio.toggle();
                }
            }
        }
    }

    pub fn on_mouse(&mut self, m: MouseEvent) {
        if self.module == Module::Cube {
            self.cube.on_mouse(m);
        }
    }

    /// Lets go of the keys first (a held mouse button must never outlive the app), then ends the kina session,
    /// closes the overlay app if it was started here, and stops capture. Returns the kina session it ended.
    pub fn shutdown(&mut self) -> Option<kina_core::snapshot::SessionSnapshot> {
        self.keys.shutdown();
        let last = self.kina.feed.shutdown();
        self.cube.feed.shutdown();
        self.dps.shutdown();
        if let Some(shared) = &self.live {
            shared.capture.stop();
            if let Some(recorder) = shared.recorder.get() {
                recorder.flush();
            }
        }
        last
    }
}

fn muted() -> Style {
    Style::new().fg(Color::DarkGray).bg(BAR_BG)
}

pub fn draw(f: &mut Frame, hub: &mut Hub) {
    let area = f.area();
    // The cube tab and the overview show the alert themselves.
    let alert = hub.cube.banner.is_some() && !matches!(hub.module, Module::Cube | Module::Overview);
    let [top, strip, body] = Layout::vertical([Constraint::Length(1), Constraint::Length(alert as u16), Constraint::Min(0)]).areas(area);
    draw_top(f, hub, top);
    if alert {
        draw_cube_strip(f, hub, strip);
    }
    match hub.module {
        Module::Overview => overview::draw(f, body, hub),
        Module::Dps => dps::ui::draw(f, body, &mut hub.dps),
        Module::Cube => cube::ui::draw(f, body, &mut hub.cube),
        Module::Kina => kina::ui::draw(f, body, &mut hub.kina),
        Module::Keys => keys::draw(f, body, &mut hub.keys),
    }
}

/// `AION │ F1 Overview  F2 DPS  F3 Cube  F4 Kina  F5 Keys` on the left; capture, kina and keys on the right.
fn draw_top(f: &mut Frame, hub: &Hub, area: Rect) {
    f.render_widget(Paragraph::new("").style(Style::new().bg(BAR_BG)), area);
    let mut left = vec![Span::styled(" AION ", Style::new().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)), Span::styled(" ", muted())];
    for (i, (module, label)) in MODULES.iter().enumerate() {
        let selected = *module == hub.module;
        // A cube waiting to be opened marks its tab, so it is seen from any other tab.
        let flag = match module {
            Module::Cube if hub.cube.banner.is_some() => " ◆",
            Module::Dps if hub.dps.current.as_ref().is_some_and(|s| s.active) => " ●",
            Module::Keys if matches!(hub.keys.phase(), keys::hold::Phase::Holding | keys::hold::Phase::Break { .. }) => " ●",
            _ => "",
        };
        let text = format!(" F{} {label}{flag} ", i + 1);
        let style = if selected {
            Style::new().fg(Color::Black).bg(Color::Gray).add_modifier(Modifier::BOLD)
        } else if *module == Module::Cube && hub.cube.banner.is_some() {
            Style::new().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(Color::Gray).bg(BAR_BG)
        };
        left.push(Span::styled(text, style));
        left.push(Span::styled(" ", muted()));
    }

    let status = &hub.kina.status;
    let (dot, short) = match status.health {
        Health::Good if status.text.starts_with("capturing") => (Color::Green, "capturing".to_string()),
        Health::Good => (Color::Green, status.text.split(':').next().unwrap_or("").to_string()),
        Health::Waiting => (Color::Yellow, if status.text.starts_with("waiting") { "waiting for the game".into() } else { status.text.clone() }),
        Health::Bad => (Color::Red, "capture error, see Setup".into()),
    };
    let kina = hub.kina.current.as_ref().filter(|s| s.active && s.changes > 0).map(|s| {
        let color = if s.net >= 0 { Color::Green } else { Color::Red };
        [Span::styled("kina ", muted()), Span::styled(crate::fmt::format_signed(s.net as f64), Style::new().fg(color).bg(BAR_BG))]
    });
    let (badge, color) = hub.keys.badge();
    let keys = [Span::styled("keys ", muted()), Span::styled(badge, Style::new().fg(color).bg(BAR_BG).add_modifier(Modifier::BOLD))];
    // The game's sound in the mixer: muted always shows (like a recording), on goes with the kina figure.
    let game = hub.game_audio.as_ref().and_then(|a| match a.state() {
        crate::game_audio::GameSound::On => Some((false, Span::styled("on", Style::new().fg(Color::Gray).bg(BAR_BG)))),
        crate::game_audio::GameSound::Muted => Some((true, Span::styled(" MUTED ", Style::new().fg(Color::White).bg(Color::Red).add_modifier(Modifier::BOLD)))),
        crate::game_audio::GameSound::NotPlaying => None,
    });
    // `--record` always shows, so a recording is never forgotten.
    let rec = hub.live.as_ref().and_then(|s| s.recorder.get()).map(|r| {
        let text = if r.is_recording() { format!(" REC {:.1} MB ", r.bytes() as f64 / 1e6) } else { " REC FAILED, see the log ".into() };
        Span::styled(text, Style::new().fg(Color::White).bg(Color::Red).add_modifier(Modifier::BOLD))
    });
    // On a narrow terminal the kina figure goes first, then the keys badge (the overview shows both), then the
    // capture text, so the tabs stay readable.
    let left_width: usize = left.iter().map(Span::width).sum();
    let room = usize::from(area.width).saturating_sub(left_width + 1);
    let build = |show_kina: bool, show_keys: bool, show_text: bool| {
        let sep = || Span::styled("  │  ", muted());
        let mut right = Vec::new();
        if let Some(rec) = &rec {
            right.extend([rec.clone(), Span::styled(" ", muted())]);
        }
        right.push(Span::styled("● ", Style::new().fg(dot).bg(BAR_BG)));
        if show_text {
            right.push(Span::styled(short.clone(), Style::new().fg(Color::Gray).bg(BAR_BG)));
        }
        if let Some((_, state)) = game.as_ref().filter(|(muted, _)| *muted || show_kina) {
            let key = crate::hotkeys::label(crate::hotkeys::Hotkey::GameSound);
            right.extend([sep(), Span::styled(format!("{key} game sound "), muted()), state.clone()]);
        }
        if let Some(k) = kina.as_ref().filter(|_| show_kina) {
            right.push(sep());
            right.extend(k.iter().cloned());
        }
        if show_keys {
            right.push(sep());
            right.extend(keys.iter().cloned());
        }
        right.push(Span::styled(" ", muted()));
        right
    };
    let right = [(true, true, true), (false, true, true), (false, false, true), (false, false, false)]
        .into_iter()
        .map(|(k, h, t)| build(k, h, t))
        .find(|r| r.iter().map(Span::width).sum::<usize>() <= room)
        .unwrap_or_else(|| build(false, false, false));

    let right_width: u16 = right.iter().map(|s| s.width() as u16).sum();
    let [l, r] = Layout::horizontal([Constraint::Min(0), Constraint::Length(right_width)]).areas(area);
    f.render_widget(Paragraph::new(Line::from(left)).style(Style::new().bg(BAR_BG)), l);
    f.render_widget(Paragraph::new(Line::from(right)).style(Style::new().bg(BAR_BG)), r);
}

/// The cube alert, one line, on the tabs that are not the radar.
fn draw_cube_strip(f: &mut Frame, hub: &Hub, area: Rect) {
    let Some((kind, c)) = hub.cube.banner_cube() else { return };
    let bg = match kind {
        cube_core::snapshot::AlertKind::Appeared | cube_core::snapshot::AlertKind::StillThere => Color::Yellow,
        cube_core::snapshot::AlertKind::OpenedByOther => Color::Red,
        _ => Color::Green,
    };
    let mut facts = Vec::new();
    if let Some(d) = c.distance_m {
        facts.push(format!("{}{d:.0} m", if c.estimated { "~" } else { "" }));
    }
    if let Some(b) = c.bearing {
        facts.push(format!("{} {}", arrow(b), compass(b)));
    }
    match c.height_m {
        Some(h) if h > cube::chart::LEVEL_M => facts.push(format!("▲ {h:.0} m")),
        Some(h) if h < -cube::chart::LEVEL_M => facts.push(format!("▼ {:.0} m", -h)),
        Some(_) => facts.push("level".into()),
        None => {}
    }
    let style = Style::new().fg(Color::Black).bg(bg);
    let line = Line::from(vec![
        Span::styled(format!(" ◆ {}  ", cube::app::alert_title(kind)), style.add_modifier(Modifier::BOLD)),
        Span::styled(facts.join("  ·  "), style),
        Span::styled(format!("   {} shows the radar", Module::Cube.key()), style),
    ]);
    f.render_widget(Paragraph::new(line).style(style), area);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::dps::app::App;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::Terminal;

    pub(crate) const START: u64 = 1_791_722_000_000;

    /// Every module on its demo data, `secs` seconds in. Nothing is captured, no keys, no overlay app.
    pub(crate) fn demo_hub(secs: u64) -> Hub {
        let overlay = crate::dps::overlay::Overlay::unmanaged(None);
        let dps_options = crate::dps::app::ViewOptions { hide_names: false, party_only: false, boss_only: false };
        let cube_options = crate::cube::app::ViewOptions { hide_name: false, sound: false, detector: true };
        let kina_options = crate::kina::app::ViewOptions { hide_name: false, goal: 5_000_000 };
        let mut hub = Hub {
            module: Module::Dps,
            dps: crate::dps::app::App::new(Box::new(crate::dps::demo::Demo::new(START)), overlay, dps_options, START),
            cube: crate::cube::app::App::with_speaker(
                Box::new(crate::cube::demo::Demo::new(START, Default::default())),
                cube_options,
                START,
                crate::cube::app::Speaker::SILENT,
            ),
            kina: crate::kina::app::App::new(Box::new(crate::kina::demo::Demo::new(START)), kina_options, START),
            keys: crate::keys::App::new(crate::keys::hold::Engine::new("viiper.exe".into()), false, START),
            live: None,
            settings: None,
            game_audio: None,
            hotkeys: None,
            quit: false,
        };
        for i in 0..=secs * 10 {
            hub.step(START + i * 100);
        }
        hub
    }

    pub(crate) fn render_buffer(hub: &mut Hub, w: u16, h: u16) -> Buffer {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, hub)).unwrap();
        term.backend().buffer().clone()
    }

    pub(crate) fn render(hub: &mut Hub, w: u16, h: u16) -> String {
        let buf = render_buffer(hub, w, h);
        let mut out = String::new();
        for y in 0..h {
            for x in 0..w {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn every_module_renders_at_common_sizes() {
        let mut hub = demo_hub(30);
        let tabs = [(1, Module::Overview, "Hidden Cube"), (2, Module::Dps, "party"), (3, Module::Cube, "1 Radar"), (4, Module::Kina, "1 Live"), (5, Module::Keys, "Hold buttons")];
        for (key, module, marker) in tabs {
            hub.on_key(KeyEvent::from(KeyCode::F(key)));
            assert_eq!(hub.module, module);
            for (w, h) in [(80, 24), (120, 36), (170, 48), (180, 50)] {
                let screen = render(&mut hub, w, h);
                assert!(screen.contains("F1 Overview") && screen.contains(marker), "{module:?} at {w}x{h}:\n{screen}");
            }
        }
    }

    #[test]
    fn the_overview_shows_every_module_at_once() {
        let mut hub = demo_hub(30);
        hub.on_key(KeyEvent::from(KeyCode::F(1)));
        let snap = hub.dps.current.clone().expect("the demo fight runs");
        let boss = snap.boss.as_ref().unwrap().name.clone();
        let top = App::rows(&snap)[0].name.clone();
        for (w, h) in [(80, 24), (120, 36), (170, 48)] {
            let screen = render(&mut hub, w, h);
            for part in [boss.as_str(), top.as_str(), "party", "Hidden Cube", "Kina", "net", "Hold buttons", "F2-F5"] {
                assert!(screen.contains(part), "{part:?} missing at {w}x{h}:\n{screen}");
            }
        }
        let wide = render(&mut hub, 170, 48);
        assert!(wide.contains("DPS over the fight") && wide.contains("100N"), "the party chart and the radar fit at 170x48:\n{wide}");
    }

    #[test]
    fn the_overview_keeps_esc_and_the_hotkeys_work_from_anywhere() {
        let mut hub = demo_hub(30);
        hub.on_key(KeyEvent::from(KeyCode::F(1)));
        let before = (hub.cube.options.sound, hub.dps.options.hide_names, hub.dps.current.as_ref().map(|s| s.id));
        for c in ['h', 'm', 'r', ' ', 's'] {
            hub.on_key(KeyEvent::from(KeyCode::Char(c)));
        }
        let after = (hub.cube.options.sound, hub.dps.options.hide_names, hub.dps.current.as_ref().map(|s| s.id));
        assert_eq!(before, after, "those keys belong to their own tabs now");
        let screen = render(&mut hub, 170, 48);
        assert!(!screen.contains("hold / let go") && !screen.contains("hide names"), "{screen}");
        // Ctrl+R, from the game or anywhere: a tick later, so the meter's message is the newest.
        hub.step(START + 30_100);
        hub.on_hotkey(crate::hotkeys::Hotkey::NewFight);
        assert!(render(&mut hub, 170, 48).contains("next hit starts a new one"), "a module's message shows in the overview's footer");
        hub.on_key(KeyEvent::from(KeyCode::Esc));
        assert!(hub.cube.banner.is_none());
        hub.on_key(KeyEvent::from(KeyCode::Char('q')));
        assert!(hub.quit);
    }

    #[test]
    fn what_the_tabs_are_set_to_shapes_the_overview() {
        let mut hub = demo_hub(30);
        let key = |hub: &mut Hub, code: KeyCode| hub.on_key(KeyEvent::from(code));
        let overview = |hub: &mut Hub| {
            key(hub, KeyCode::F(1));
            render(hub, 170, 48)
        };
        assert!(!overview(&mut hub).contains("party only"));

        // The meter's party filter, picked on the DPS tab.
        key(&mut hub, KeyCode::F(2));
        assert!(render(&mut hub, 170, 48).contains("party only: off"), "the DPS tab lists its options");
        key(&mut hub, KeyCode::Char('p'));
        assert!(render(&mut hub, 170, 48).contains("party only: on"));
        assert!(overview(&mut hub).contains("Meter · Kromede's Trial · party only"));

        // The cube map: 3D on the cube tab is 3D on the overview, and stays so on the log.
        key(&mut hub, KeyCode::F(3));
        key(&mut hub, KeyCode::Char('2'));
        key(&mut hub, KeyCode::Char('3'));
        let screen = overview(&mut hub);
        assert!(screen.contains("3D, looking") && !screen.contains("100N"), "{screen}");
        key(&mut hub, KeyCode::F(3));
        key(&mut hub, KeyCode::Char('1'));
        assert!(overview(&mut hub).contains("100N"), "the radar, by its axis labels");

        // The kina goal, typed in on the kina tab.
        key(&mut hub, KeyCode::F(4));
        key(&mut hub, KeyCode::Char('g'));
        assert!(render(&mut hub, 170, 48).contains("Net kina goal:"));
        for c in ['7', 'm'] {
            key(&mut hub, KeyCode::Char(c));
        }
        key(&mut hub, KeyCode::Enter);
        assert_eq!(hub.kina.options.goal, 7_000_000);
        assert!(render(&mut hub, 170, 48).contains("goal: 7.00M"));
        assert!(overview(&mut hub).contains("of 7.00M"));
        key(&mut hub, KeyCode::F(4));
        key(&mut hub, KeyCode::Char('g'));
        key(&mut hub, KeyCode::Char('x'));
        key(&mut hub, KeyCode::Enter);
        assert_eq!(hub.kina.options.goal, 7_000_000, "a goal that is not an amount changes nothing");
    }

    #[test]
    fn changed_settings_are_saved_for_the_next_start() {
        let dir = std::env::temp_dir().join(format!("aion-hub-settings-{}", std::process::id()));
        let path = dir.join("settings.json");
        let mut hub = demo_hub(1);
        hub.settings = Some(Store::new(Some(path.clone()), Settings::default(), Settings::of(&hub)));
        hub.on_key(KeyEvent::from(KeyCode::F(3)));
        hub.on_key(KeyEvent::from(KeyCode::Char('2')));
        hub.on_key(KeyEvent::from(KeyCode::F(2)));
        hub.on_key(KeyEvent::from(KeyCode::Char('b')));
        hub.step(START + 1_100);
        let saved = Settings::load(&path);
        assert_eq!(saved.cube_map, crate::cube::app::MapKind::View3d);
        assert!(saved.boss_only && !saved.party_only);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_cube_strip_names_the_radar_key() {
        assert_eq!(Module::Cube.key(), "F3");
        assert_eq!(Module::from_name("all"), Some(Module::Overview));
    }

    #[test]
    fn module_keys_switch_and_quit_reaches_the_hub() {
        let mut hub = demo_hub(1);
        hub.on_key(KeyEvent::new(KeyCode::Char('4'), KeyModifiers::ALT));
        assert_eq!(hub.module, Module::Kina);
        hub.on_key(KeyEvent::from(KeyCode::Char('2')));
        assert_eq!(hub.module, Module::Kina, "a plain digit is the module's own view key");
        assert_eq!(hub.kina.tab, crate::kina::app::Tab::Runs);
        hub.on_key(KeyEvent::from(KeyCode::Char('q')));
        assert!(hub.quit);
    }

    #[test]
    fn the_meter_shows_the_fight_like_the_overlay() {
        let mut hub = demo_hub(30);
        hub.on_key(KeyEvent::from(KeyCode::F(2)));
        let screen = render(&mut hub, 140, 40);
        let snap = hub.dps.current.clone().expect("the demo fight runs");
        let boss = snap.boss.as_ref().unwrap().name.clone();
        assert!(screen.contains(&boss), "{screen}");
        assert!(screen.contains("hp") && screen.contains("party") && screen.contains("total"), "{screen}");
        for actor in snap.actors.iter().filter(|a| a.is_player) {
            assert!(screen.contains(&actor.name), "{} missing:\n{screen}", actor.name);
        }
        assert!(screen.contains("Skill") && screen.contains("Damage"), "the selected player's skills:\n{screen}");
    }

    /// Prints every module as text: `cargo test -p aion print_hub -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn print_hub() {
        let mut hub = demo_hub(45);
        let (w, h) = (150, 42);
        for key in 1..=MODULES.len() as u8 {
            hub.on_key(KeyEvent::from(KeyCode::F(key)));
            println!("{}", render(&mut hub, w, h));
        }
    }
}
