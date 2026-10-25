//! The Overview tab: what matters from every module on one screen. The live meter on the left, with the party
//! chart below it when there is room; on the right the cube alert with the radar, kina, and hold buttons. On a
//! narrow terminal the meter goes on top and the other three sit side by side below it.
//!
//! Its own key is esc, which dismisses the cube alert and quiets the detector. The footer also lists the global
//! hotkeys (`hotkeys.rs`), which work from the game too: ctrl+r for a new fight, ctrl+m for the game's sound.

use crate::hub::{Hub, MODULES};
use crate::hotkeys::{self, Hotkey};
use crate::{cube, dps, keys, kina};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// From this width the meter goes on the left and the rest beside it.
const SIDE_BY_SIDE: u16 = 120;
/// Rows the side by side panels keep below the meter on a narrow terminal.
const PANELS_MIN: u16 = 7;

pub fn draw(f: &mut Frame, area: Rect, hub: &mut Hub) {
    let [body, footer] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);
    if body.width >= SIDE_BY_SIDE {
        let meter_width = (body.width * 11 / 20).clamp(64, 100);
        let [left, right] = Layout::horizontal([Constraint::Length(meter_width), Constraint::Min(0)]).areas(body);
        dps::ui::draw_overview(f, left, &hub.dps);
        let kina_height = if right.height >= 40 { 11 } else { 8 };
        let [cube_area, kina_area, keys_area] =
            Layout::vertical([Constraint::Min(4), Constraint::Length(kina_height), Constraint::Length(keys::OVERVIEW_HEIGHT)]).areas(right);
        cube::ui::draw_overview(f, cube_area, &mut hub.cube);
        kina::ui::draw_overview(f, kina_area, &hub.kina);
        keys::draw_overview(f, keys_area, &hub.keys);
    } else {
        let meter_height = dps::ui::live_meter_height(&hub.dps).min(body.height.saturating_sub(PANELS_MIN));
        let [meter, panels] = Layout::vertical([Constraint::Length(meter_height), Constraint::Min(0)]).areas(body);
        dps::ui::draw_overview(f, meter, &hub.dps);
        let [cube_area, kina_area, keys_area] = Layout::horizontal([Constraint::Ratio(1, 3); 3]).areas(panels);
        cube::ui::draw_overview(f, cube_area, &mut hub.cube);
        kina::ui::draw_overview(f, kina_area, &hub.kina);
        keys::draw_overview(f, keys_area, &hub.keys);
    }
    draw_footer(f, hub, footer);
}

/// The newest message any module has to say, else the keys this tab takes.
fn draw_footer(f: &mut Frame, hub: &Hub, area: Rect) {
    let message = [&hub.dps.message, &hub.cube.message, &hub.kina.message, &hub.keys.message].into_iter().flatten().max_by_key(|(_, until)| *until);
    if let Some((text, _)) = message {
        f.render_widget(Paragraph::new(Line::from(vec![Span::raw(" "), Span::styled(text.clone(), Style::new().fg(Color::Yellow))])), area);
        return;
    }
    let tabs = format!("F2-F{}", MODULES.len());
    let mut keys: Vec<(&str, &str)> = vec![("q", "quit"), (&tabs, "details")];
    if hub.cube.banner.is_some() || hub.cube.tracking.is_some() {
        keys.push(("esc", "dismiss"));
    }
    if let Some(hotkeys) = &hub.hotkeys {
        for (key, label) in hotkeys::HOTKEYS {
            let what = match (key, hotkeys.registered(key)) {
                (_, Some(false)) => "taken by another program",
                (Hotkey::NewFight, _) => "new fight",
                (Hotkey::GameSound, _) => "game sound",
            };
            keys.push((label, what));
        }
    }
    let mut spans = vec![Span::raw(" ")];
    for (k, label) in keys {
        spans.push(Span::styled(k.to_string(), Style::new().fg(Color::Gray).bold()));
        spans.push(Span::styled(format!(" {label}   "), Style::new().fg(Color::DarkGray)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub fn on_key(hub: &mut Hub, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('q') => hub.quit = true,
        KeyCode::Char('c') if ctrl => hub.quit = true,
        KeyCode::Esc => {
            hub.cube.dismiss();
        }
        _ => {}
    }
}
