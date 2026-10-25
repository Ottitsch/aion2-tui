//! Rendering of the DPS tab. Every view reads the `App` and draws; nothing here changes state except clamping
//! scroll offsets to what a list holds.
//!
//! The Live view's meter is the aion2dps overlay in its Campbell theme, drawn with terminal characters: the boss
//! line with its timer, the hp bar, the party line, then one row per player with a class coloured bar, DPS, share
//! and total. Beside it (or below, on a narrow terminal) the selected player's skills.

use crate::dps::app::{App, Prompt, Tab, TABS};
use crate::dps::classes::class_info;
use crate::dps::feed::{end_label, Health};
use crate::dps::summary::format_duration;
use crate::fmt::{clock, day, format_short, group};
use crate::options::Opt;
use meter_core::snapshot::{ActorStats, Snapshot};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::symbols::Marker;
use ratatui::widgets::{Axis, Block, Cell, Chart, Dataset, GraphType, Paragraph, Row, Sparkline, Table, TableState, Tabs, Wrap};
use ratatui::Frame;

// The Campbell palette of the overlay (`src/shared/themes.css`).
const TEXT: Color = Color::Rgb(0xcc, 0xcc, 0xcc);
const STRONG: Color = Color::Rgb(0xf2, 0xf2, 0xf2);
const DIM: Color = Color::Rgb(0x76, 0x76, 0x76);
const FAINT: Color = Color::Rgb(0x5a, 0x5a, 0x5a);
const TRACK: Color = Color::Rgb(0x3a, 0x3a, 0x3a);
const HP: Color = Color::Rgb(0xe7, 0x48, 0x56);
const OK: Color = Color::Rgb(0x16, 0xc6, 0x0c);
const WARN: Color = Color::Rgb(0xf9, 0xf1, 0xa5);
const DANGER: Color = Color::Rgb(0xe7, 0x48, 0x56);
const ACCENT: Color = Color::Rgb(0xd9, 0x77, 0x57);
const SELECTED: Color = Color::Rgb(0x26, 0x26, 0x26);
const MIN_WIDTH: u16 = 64;
const MIN_HEIGHT: u16 = 16;
/// The details panel goes beside the meter from this width, below it otherwise.
const SIDE_BY_SIDE: u16 = 118;

fn dim() -> Style {
    Style::new().fg(DIM)
}

fn strong() -> Style {
    Style::new().fg(STRONG).add_modifier(Modifier::BOLD)
}

fn block(title: impl Into<Line<'static>>) -> Block<'static> {
    Block::bordered().border_style(Style::new().fg(Color::DarkGray)).title(title)
}

fn title(text: &str) -> Line<'static> {
    Line::from(Span::styled(format!(" {text} "), Style::new().fg(Color::Gray)))
}

fn right(text: impl Into<String>) -> Cell<'static> {
    Cell::from(Line::from(text.into()).alignment(Alignment::Right))
}

/// The overlay's number format: `950`, `1.25K`, `23.4K`, `1.25M`, `2.10B`.
pub fn fmt_num(n: f64) -> String {
    if !n.is_finite() {
        return "0".into();
    }
    let a = n.abs();
    if a >= 1e9 {
        format!("{:.2}B", n / 1e9)
    } else if a >= 1e6 {
        format!("{:.2}M", n / 1e6)
    } else if a >= 1e4 {
        format!("{:.1}K", n / 1e3)
    } else if a >= 1e3 {
        format!("{:.2}K", n / 1e3)
    } else {
        format!("{}", n.round() as i64)
    }
}

fn fmt_pct(share: f64) -> String {
    if share.is_finite() {
        format!("{:.1}%", share * 100.0)
    } else {
        "0.0%".into()
    }
}

/// A thin bar like the overlay's: heavy rules in `color` over a dark track, with half cell steps.
pub fn bar(width: u16, fraction: f64, color: Color) -> Vec<Span<'static>> {
    let width = width as usize;
    if width == 0 {
        return Vec::new();
    }
    let halves = (fraction.clamp(0.0, 1.0) * width as f64 * 2.0).round() as usize;
    let (full, half) = (halves / 2, halves % 2);
    let mut spans = vec![Span::styled("━".repeat(full), Style::new().fg(color))];
    if half == 1 {
        spans.push(Span::styled("╸", Style::new().fg(color)));
    }
    let rest = width.saturating_sub(full + half);
    if rest > 0 {
        spans.push(Span::styled("━".repeat(rest), Style::new().fg(TRACK)));
    }
    spans
}

/// Pads or cuts `text` to exactly `width` columns, on the left or the right.
fn fit(text: &str, width: usize, align_right: bool) -> String {
    let count = text.chars().count();
    if count > width {
        let mut cut: String = text.chars().take(width.saturating_sub(1)).collect();
        cut.push('…');
        cut
    } else if align_right {
        format!("{text:>width$}")
    } else {
        format!("{text:<width$}")
    }
}

/// Draws the tab into `area`, which the hub leaves below its own top bar.
pub fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let text = format!("The terminal is {}x{}; the meter needs at least {MIN_WIDTH}x{MIN_HEIGHT}.", area.width, area.height);
        f.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), area);
        return;
    }
    let banner = app.opened.is_some() as u16;
    let [header, options, banner_area, body, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Length(banner), Constraint::Min(0), Constraint::Length(1)]).areas(area);
    draw_header(f, app, header);
    let o = &app.options;
    crate::options::draw(
        f,
        options,
        &[
            Opt::Toggle { key: "p", label: "party only", on: o.party_only },
            Opt::Toggle { key: "b", label: "bosses only", on: o.boss_only },
            Opt::Toggle { key: "h", label: "hide names", on: o.hide_names },
        ],
    );
    if let Some(s) = &app.opened {
        let text = format!(" Viewing encounter #{} from {} {}. Esc returns to the live one. ", s.id, day(s.started_ms, app.now_ms), clock(s.started_ms));
        f.render_widget(Paragraph::new(text).style(Style::new().fg(Color::Black).bg(WARN)), banner_area);
    }
    match app.tab {
        Tab::Live => draw_live(f, app, body),
        Tab::History => draw_history(f, app, body),
        Tab::Setup => draw_setup(f, app, body),
    }
    draw_footer(f, app, footer);
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let tabs_width = TABS.iter().map(|(_, l)| l.len() as u16 + 5).sum::<u16>();
    let [left, tabs_area] = Layout::horizontal([Constraint::Min(0), Constraint::Length(tabs_width)]).areas(area);
    let dot = match app.status.health {
        Health::Good => OK,
        Health::Waiting => Color::Yellow,
        Health::Bad => DANGER,
    };
    let sep = || Span::styled("  │  ", Style::new().fg(Color::DarkGray));
    let mut spans = vec![Span::raw(" "), Span::styled(app.you_label(), Style::new().bold()), sep()];
    spans.push(Span::styled("● ", Style::new().fg(dot)));
    spans.push(Span::raw(app.status.text.clone()));
    spans.push(sep());
    let overlay_on = matches!(app.overlay.state(), crate::dps::overlay::State::Running { .. });
    spans.push(Span::styled(app.overlay.label(), if overlay_on { Style::new().fg(TEXT) } else { dim() }));
    f.render_widget(Paragraph::new(Line::from(spans)), left);
    let titles: Vec<Line> = TABS.iter().enumerate().map(|(i, (_, l))| Line::from(format!("{} {l}", i + 1))).collect();
    let tabs = Tabs::new(titles)
        .select(app.tab.index())
        .style(Style::new().fg(Color::DarkGray))
        .highlight_style(Style::new().fg(Color::Black).bg(Color::Gray).bold())
        .divider(" ");
    f.render_widget(tabs, tabs_area);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    if let Some(Prompt::Delete(id)) = &app.prompt {
        let line = Line::from(vec![Span::styled(format!(" Delete encounter #{id}? "), Style::new().fg(DANGER).bold()), Span::raw("y deletes, any other key keeps it")]);
        f.render_widget(Paragraph::new(line), area);
        return;
    }
    if let Some((text, _)) = &app.message {
        f.render_widget(Paragraph::new(Line::from(vec![Span::raw(" "), Span::styled(text.clone(), Style::new().fg(WARN))])), area);
        return;
    }
    let mut keys: Vec<(&str, &str)> = vec![("q", "quit"), ("1-3", "views")];
    match app.tab {
        Tab::Live => keys.extend([("↑↓", "player"), ("[ ]", "skills"), ("r", "new fight"), ("c", "copy")]),
        Tab::History => keys.extend([("↑↓", "select"), ("enter", "open"), ("d", "delete")]),
        Tab::Setup => keys.extend([("↑↓", "scroll"), ("r", "retry capture")]),
    }
    keys.extend([("o", "overlay"), ("w", "app window")]);
    if app.opened.is_some() {
        keys.push(("esc", "back to live"));
    }
    let mut spans = vec![Span::raw(" ")];
    for (k, label) in keys {
        spans.push(Span::styled(k, Style::new().fg(Color::Gray).bold()));
        spans.push(Span::styled(format!(" {label}  "), Style::new().fg(Color::DarkGray)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The meter is as tall as the overlay would be: three header lines, a gap, the rows, a gap and the status.
fn meter_height(snap: Option<&Snapshot>) -> u16 {
    let players = snap.map(|s| App::rows(s).len()).unwrap_or(0) as u16;
    players.max(5) + 8
}

/// How tall the overview's meter wants to be for the live encounter.
pub fn live_meter_height(app: &App) -> u16 {
    meter_height(app.live().as_ref())
}

/// The overview's part: the live meter (whatever the Live view has open), and the party chart below it when
/// there is room.
pub fn draw_overview(f: &mut Frame, area: Rect, app: &App) {
    let snap = app.live();
    let meter_height = meter_height(snap.as_ref()).min(area.height);
    if area.height >= meter_height + 8 {
        let [meter, chart] = Layout::vertical([Constraint::Length(meter_height), Constraint::Min(0)]).areas(area);
        draw_meter(f, app, snap.as_ref(), meter);
        draw_party_chart(f, app, snap.as_ref(), chart);
    } else {
        draw_meter(f, app, snap.as_ref(), area);
    }
}

fn draw_live(f: &mut Frame, app: &mut App, area: Rect) {
    let snap = app.shown();
    let meter_height = meter_height(snap.as_ref()).min(area.height);
    if area.width >= SIDE_BY_SIDE {
        let meter_width = (area.width * 11 / 20).clamp(64, 96);
        let [left, right] = Layout::horizontal([Constraint::Length(meter_width), Constraint::Min(0)]).areas(area);
        if left.height >= meter_height + 8 {
            let [meter, chart] = Layout::vertical([Constraint::Length(meter_height), Constraint::Min(0)]).areas(left);
            draw_meter(f, app, snap.as_ref(), meter);
            draw_party_chart(f, app, snap.as_ref(), chart);
        } else {
            draw_meter(f, app, snap.as_ref(), left);
        }
        draw_details(f, app, snap.as_ref(), right);
    } else if area.height.saturating_sub(meter_height) >= 10 {
        let [meter, details] = Layout::vertical([Constraint::Length(meter_height), Constraint::Min(0)]).areas(area);
        draw_meter(f, app, snap.as_ref(), meter);
        draw_details(f, app, snap.as_ref(), details);
    } else {
        draw_meter(f, app, snap.as_ref(), area);
    }
}

/// How the meter moved over the fight: each player's DPS so far, second by second, in their class colour.
fn draw_party_chart(f: &mut Frame, app: &App, snap: Option<&Snapshot>, area: Rect) {
    let outer = block(title("DPS over the fight"));
    let rows = snap.map(App::rows).unwrap_or_default();
    let selected = app.selected_actor(&rows).map(|a| a.id);
    // The selected player last, so their line is drawn on top.
    let mut order: Vec<&ActorStats> = rows.iter().copied().filter(|a| a.damage > 0).take(10).collect();
    order.sort_by_key(|a| Some(a.id) == selected);
    let series: Vec<(Color, Vec<(f64, f64)>)> = order
        .iter()
        .map(|a| {
            let mut total = 0u64;
            let mut points = Vec::with_capacity(a.timeline.len());
            for p in &a.timeline {
                total += p.damage;
                // The first seconds swing too wildly to read.
                if p.t_s >= 3 {
                    points.push((f64::from(p.t_s), total as f64 / f64::from(p.t_s + 1)));
                }
            }
            (class_info(a.class_id, a.class_name.as_deref()).color, points)
        })
        .collect();
    let end = series.iter().flat_map(|(_, p)| p.last().map(|q| q.0)).fold(0.0f64, f64::max);
    let top = series.iter().flat_map(|(_, p)| p.iter().map(|q| q.1)).fold(0.0f64, f64::max);
    if end < 4.0 || top <= 0.0 {
        let text = vec![Line::from(""), Line::from(Span::styled("Each player's DPS over the fight shows here a few seconds in.", dim()))];
        f.render_widget(Paragraph::new(text).alignment(Alignment::Center).block(outer), area);
        return;
    }
    let datasets: Vec<Dataset> = series
        .iter()
        .map(|(color, points)| Dataset::default().marker(Marker::Braille).graph_type(GraphType::Line).style(Style::new().fg(*color)).data(points))
        .collect();
    let axis_style = Style::new().fg(Color::DarkGray);
    let chart = Chart::new(datasets)
        .block(outer)
        .x_axis(Axis::default().style(axis_style).bounds([0.0, end]).labels([format_duration(0), format_duration((end * 500.0) as u64), format_duration((end * 1000.0) as u64)]))
        .y_axis(Axis::default().style(axis_style).bounds([0.0, top * 1.08]).labels(["0".to_string(), fmt_num(top / 2.0), fmt_num(top)]))
        .legend_position(None);
    f.render_widget(chart, area);
}

/// The overlay: title and timer, hp, party, one row per player, and the capture status.
fn draw_meter(f: &mut Frame, app: &App, snap: Option<&Snapshot>, area: Rect) {
    let mut heading = vec!["Meter".to_string()];
    heading.extend(snap.and_then(|s| s.map_name.clone()));
    // The filters show on the overview too, where the options line is not.
    if app.options.party_only {
        heading.push("party only".into());
    }
    if app.options.boss_only {
        heading.push("bosses only".into());
    }
    let outer = block(title(&heading.join(" · ")));
    let inner = outer.inner(area);
    f.render_widget(outer, area);
    let inner = Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner };
    let w = inner.width as usize;

    let active = snap.is_some_and(|s| s.active);
    let boss = snap.and_then(|s| s.boss.as_ref());
    let name = match snap {
        Some(_) => boss.map(|b| b.name.trim()).filter(|n| !n.is_empty()).unwrap_or("Combat").to_string(),
        None => "Idle".into(),
    };
    let mut head = vec![Span::styled(name, strong()), Span::raw("  "), Span::styled(format_duration(snap.map(|s| s.duration_ms).unwrap_or(0)), Style::new().fg(WARN).bold())];
    if let Some(s) = snap.filter(|s| !s.active && s.end_reason.is_some()) {
        head.push(Span::styled(format!("  [{}]", end_label(s.end_reason)), dim()));
    }

    let (cur, max) = (boss.and_then(|b| b.cur_hp), boss.and_then(|b| b.max_hp));
    let hp_pct = boss.and_then(|b| b.hp_pct).or_else(|| match (cur, max) {
        (Some(c), Some(m)) if m > 0 => Some(c as f64 / m as f64),
        _ => None,
    });
    let dead = boss.is_some_and(|b| b.dead);
    let mut hp = vec![Span::styled("hp    ", dim())];
    let hp_bar = (w / 3).clamp(10, 40) as u16;
    hp.extend(bar(hp_bar, hp_pct.unwrap_or(0.0), if dead { FAINT } else { HP }));
    hp.push(Span::styled(format!(" {:>6}", hp_pct.map(fmt_pct).unwrap_or_else(|| "--".into())), strong()));
    if let (Some(c), Some(m)) = (cur, max) {
        hp.push(Span::styled(format!("  {}/{}", fmt_num(c as f64), fmt_num(m as f64)), dim()));
    }

    let party_dps = snap.map(|s| s.party_dps).unwrap_or(0.0);
    let mut party = vec![
        Span::styled("party ", dim()),
        Span::styled(fmt_num(party_dps), strong()),
        Span::styled("/s · ", dim()),
        Span::styled(fmt_num(snap.map(|s| s.total_damage).unwrap_or(0) as f64), Style::new().fg(TEXT)),
        Span::styled(" total", dim()),
    ];
    if let Some(c) = cur.filter(|c| active && *c > 0 && party_dps > 0.0) {
        party.push(Span::styled(" · kill in ", dim()));
        party.push(Span::styled(format_duration((c as f64 / party_dps * 1000.0) as u64), Style::new().fg(WARN)));
    }
    let mut lines = vec![Line::from(head), Line::from(hp), Line::from(party), Line::from("")];

    // Player rows, keeping the selected one in view.
    let rows = snap.map(App::rows).unwrap_or_default();
    let room = (inner.height as usize).saturating_sub(lines.len() + 2);
    if rows.is_empty() {
        lines.push(Line::from(Span::styled(if snap.is_some() { "no damage yet" } else { "waiting for a fight" }, dim())));
    } else {
        let selected = app.selected_actor(&rows).map(|a| a.id);
        let at = rows.iter().position(|a| Some(a.id) == selected).unwrap_or(0);
        let skip = (at + 1).saturating_sub(room);
        let top = rows.first().map(|a| a.damage).unwrap_or(1).max(1) as f64;
        let name_w = rows.iter().map(|a| app.display_name(a).chars().count()).max().unwrap_or(6).clamp(6, 14);
        // rank 3, name, bar, dps 8, share 8, total 8
        let bar_w = w.saturating_sub(3 + name_w + 1 + 8 + 8 + 8 + 1).max(4) as u16;
        for (i, a) in rows.iter().enumerate().skip(skip).take(room) {
            lines.push(meter_row(app, a, i + 1, name_w, bar_w, a.damage as f64 / top, Some(a.id) == selected));
        }
        if rows.len() > room {
            lines.push(Line::from(Span::styled(format!("   {} of {} players, ↑↓ moves", room.min(rows.len()), rows.len()), dim())));
        }
    }
    f.render_widget(Paragraph::new(lines), inner);

    // The overlay's footer: link state on the left, who you are on the right.
    let foot = Rect { y: inner.y + inner.height.saturating_sub(1), height: 1, ..inner };
    let text = &app.status.text;
    let (link, color) = match app.status.health {
        Health::Good if text.starts_with("capturing") => ("✓ capturing", OK),
        Health::Good if text.starts_with("replay") => ("✓ replay", OK),
        Health::Good => ("✓ demo", OK),
        Health::Waiting if text.starts_with("waiting") => ("waiting", DIM),
        Health::Waiting => ("off", DIM),
        Health::Bad if text.contains("Npcap is not installed") => ("no npcap", DANGER),
        Health::Bad => ("error", DANGER),
    };
    f.render_widget(Paragraph::new(Span::styled(link, Style::new().fg(color))), foot);
    f.render_widget(Paragraph::new(Line::from(Span::styled(app.you_label(), dim())).alignment(Alignment::Right)), foot);
}

fn meter_row(app: &App, a: &ActorStats, rank: usize, name_w: usize, bar_w: u16, fraction: f64, selected: bool) -> Line<'static> {
    let color = class_info(a.class_id, a.class_name.as_deref()).color;
    let mut name = app.display_name(a);
    match a.deaths {
        0 => {}
        1 => name.push('†'),
        n => name.push_str(&format!("†{n}")),
    }
    let name_style = if a.deaths > 0 { Style::new().fg(DANGER).bold() } else { strong() };
    let mut spans = vec![
        Span::styled(format!("{rank:>2} "), dim()),
        Span::styled(fit(&name, name_w, false), name_style),
        Span::raw(" "),
    ];
    spans.extend(bar(bar_w, fraction, color));
    spans.push(Span::styled(fit(&fmt_num(a.dps), 8, true), strong()));
    spans.push(Span::styled(fit(&fmt_pct(a.share), 8, true), Style::new().fg(TEXT)));
    spans.push(Span::styled(fit(&fmt_num(a.damage as f64), 8, true), dim()));
    let line = Line::from(spans);
    if selected {
        line.style(Style::new().bg(SELECTED))
    } else {
        line
    }
}

/// The selected player: their numbers, skills and damage over the fight.
fn draw_details(f: &mut Frame, app: &mut App, snap: Option<&Snapshot>, area: Rect) {
    let rows = snap.map(App::rows).unwrap_or_default();
    let Some(a) = app.selected_actor(&rows) else {
        let text = vec![Line::from(""), Line::from(Span::styled("The selected player's skills show here once a fight starts.", dim()))];
        f.render_widget(Paragraph::new(text).alignment(Alignment::Center).block(block(title("Player"))), area);
        return;
    };
    let info = class_info(a.class_id, a.class_name.as_deref());
    let mut heading = vec![Span::raw(" "), Span::styled(app.display_name(a), Style::new().fg(info.color).bold()), Span::styled(format!(" · {}", info.name), dim())];
    if let Some(level) = a.level {
        heading.push(Span::styled(format!(" · Lv {level}"), dim()));
    }
    if let Some(cp) = a.combat_power {
        heading.push(Span::styled(format!(" · CP {}", group(cp)), dim()));
    }
    heading.push(Span::raw(" "));
    let outer = block(Line::from(heading));
    let inner = outer.inner(area);
    f.render_widget(outer, area);
    let inner = Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner };

    let sep = || Span::styled(" · ", dim());
    let mut stats = vec![
        Span::styled(fmt_num(a.dps), strong()),
        Span::styled("/s", dim()),
        sep(),
        Span::styled("active ", dim()),
        Span::raw(fmt_num(a.active_dps)),
        Span::styled("/s", dim()),
        sep(),
        Span::raw(fmt_num(a.damage as f64)),
        Span::styled(" on target", dim()),
    ];
    if a.damage_all_targets > a.damage {
        stats.extend([sep(), Span::raw(fmt_num(a.damage_all_targets as f64)), Span::styled(" all", dim())]);
    }
    let mut rates = vec![
        Span::styled("crit ", dim()),
        Span::raw(fmt_pct(a.crit_rate)),
        sep(),
        Span::styled("back ", dim()),
        Span::raw(fmt_pct(a.back_rate)),
        sep(),
        Span::styled("hits ", dim()),
        Span::raw(group(a.hits)),
        sep(),
        Span::styled("max ", dim()),
        Span::raw(fmt_num(a.max_hit as f64)),
    ];
    if a.healing > 0 {
        rates.extend([sep(), Span::styled("heal ", dim()), Span::raw(fmt_num(a.healing as f64))]);
    }
    if a.deaths > 0 {
        rates.extend([sep(), Span::styled(format!("died ×{}", a.deaths), Style::new().fg(DANGER))]);
    }
    // The skills take the rows they need; the damage per second chart gets the rest, when that is enough.
    let room = inner.height.saturating_sub(3);
    let wanted = a.skills.len().max(1) as u16 + 1;
    let (table_height, spark_height) = if room >= wanted + 5 {
        (wanted, room - wanted)
    } else if room >= 12 {
        (room - 5, 5)
    } else {
        (room, 0)
    };
    let [stats_area, table_area, spark_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Length(table_height), Constraint::Length(spark_height)]).areas(inner);
    f.render_widget(Paragraph::new(vec![Line::from(stats), Line::from(rates)]), stats_area);

    // Skills, most damage first.
    let visible = table_area.height.saturating_sub(1) as usize;
    let wide = table_area.width >= 76;
    app.skills_scroll = app.skills_scroll.min(a.skills.len().saturating_sub(visible));
    let skill_rows: Vec<Row> = a
        .skills
        .iter()
        .skip(app.skills_scroll)
        .map(|s| {
            let name = if s.name.trim().is_empty() { format!("skill {}", s.skill_id) } else { s.name.clone() };
            let kind = if s.dot { Span::styled(" dot", dim()) } else if s.healing > 0 && s.damage == 0 { Span::styled(" heal", dim()) } else { Span::raw("") };
            let mut cells = vec![
                Cell::from(Line::from(vec![Span::raw(name), kind])),
                right(fmt_num(s.damage as f64)),
                right(fmt_pct(s.share)),
                right(group(s.hits)),
                right(fmt_pct(s.crit_rate)),
                right(fmt_num(s.max_hit as f64)),
            ];
            if wide {
                cells.push(right(fmt_num(s.avg_hit)));
            }
            Row::new(cells)
        })
        .collect();
    let mut header = vec![Cell::from("Skill"), right("Damage"), right("Share"), right("Hits"), right("Crit"), right("Max")];
    let mut widths = vec![Constraint::Min(14), Constraint::Length(8), Constraint::Length(7), Constraint::Length(6), Constraint::Length(7), Constraint::Length(8)];
    if wide {
        header.push(right("Avg"));
        widths.push(Constraint::Length(8));
    }
    let header = Row::new(header).style(dim());
    if skill_rows.is_empty() {
        f.render_widget(Paragraph::new(Span::styled("no skills recorded yet", dim())), table_area);
    } else {
        f.render_widget(Table::new(skill_rows, widths).header(header).column_spacing(1), table_area);
    }

    if spark_height > 0 {
        let [_, label, chart] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)]).areas(spark_area);
        f.render_widget(Paragraph::new(Span::styled("damage on target per second, over the fight", dim())), label);
        let data = resample(&a.timeline.iter().map(|p| p.damage).collect::<Vec<_>>(), chart.width as usize);
        f.render_widget(Sparkline::default().data(&data).style(Style::new().fg(info.color)), chart);
    }
}

/// Fits a per second series into `width` columns: seconds that share a column are averaged, and a short fight is
/// stretched so it spans the whole width.
fn resample(series: &[u64], width: usize) -> Vec<u64> {
    if width == 0 || series.is_empty() {
        return series.to_vec();
    }
    if series.len() < width {
        return (0..width).map(|i| series[i * series.len() / width]).collect();
    }
    let per = series.len().div_ceil(width);
    series.chunks(per).map(|c| c.iter().sum::<u64>() / c.len() as u64).collect()
}

fn draw_history(f: &mut Frame, app: &mut App, area: Rect) {
    let rows: Vec<Row> = app
        .history
        .iter()
        .map(|h| {
            let what = h.boss_name.clone().filter(|n| !n.trim().is_empty()).or_else(|| h.map_name.clone()).unwrap_or_else(|| "Encounter".into());
            let top = h.actors.iter().max_by_key(|a| a.damage).map(|a| {
                let name = if app.options.hide_names { class_info(None, a.class_name.as_deref()).name.to_string() } else { a.name.clone() };
                format!("{name} {}", fmt_num(a.dps))
            });
            let result_style = match h.end_reason {
                Some(meter_core::snapshot::EndReason::BossDied) => Style::new().fg(OK),
                Some(meter_core::snapshot::EndReason::Wipe) => Style::new().fg(DANGER),
                _ => dim(),
            };
            Row::new(vec![
                Cell::from(day(h.started_ms, app.now_ms)),
                Cell::from(clock(h.started_ms)),
                Cell::from(Line::from(vec![Span::raw(what), Span::styled(if h.is_boss { "" } else { "  (no boss)" }, dim())])),
                Cell::from(Span::styled(h.map_name.clone().unwrap_or_default(), dim())),
                right(format_duration(h.duration_ms)),
                Cell::from(Span::styled(end_label(h.end_reason), result_style)),
                right(fmt_num(h.party_dps)),
                right(format_short(h.total_damage as f64)),
                Cell::from(top.unwrap_or_default()),
            ])
        })
        .collect();
    let empty = rows.is_empty();
    let header = Row::new(vec![
        Cell::from("Day"),
        Cell::from("Start"),
        Cell::from("Encounter"),
        Cell::from("Map"),
        right("Time"),
        Cell::from("Result"),
        right("Party/s"),
        right("Total"),
        Cell::from("Top"),
    ])
    .style(dim());
    let widths = [
        Constraint::Length(11),
        Constraint::Length(5),
        Constraint::Min(18),
        Constraint::Length(20),
        Constraint::Length(7),
        Constraint::Length(7),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(20),
    ];
    let saved = if app.feed.persistent() { "" } else { ", not saved: demo or replay" };
    let table = Table::new(rows, widths)
        .header(header)
        .block(block(title(&format!("Encounters ({}{saved})", app.history.len()))))
        .row_highlight_style(Style::new().bg(Color::Indexed(236)).bold())
        .highlight_symbol("› ");
    let mut state = TableState::default().with_selected((!empty).then_some(app.history_selected));
    f.render_stateful_widget(table, area, &mut state);
    if empty {
        let inner = Rect { x: area.x + 2, y: area.y + 2, width: area.width.saturating_sub(4), height: 1 };
        let text = "No finished fights yet. A fight is saved when it ends: a kill, a wipe, a zone change, idle, or r.";
        f.render_widget(Paragraph::new(Span::styled(text, dim())), inner);
    }
}

fn draw_setup(f: &mut Frame, app: &mut App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    let heading = |t: &str| Line::from(Span::styled(t.to_string(), Style::new().fg(ACCENT).bold()));
    lines.push(heading("Getting started"));
    for step in [
        "1. Install Npcap from https://npcap.com. The meter only reads traffic: it never sends or changes packets and",
        "   never touches the game's memory.",
        "2. Start before you pull. Names and classes come from the party and identity packets, so a meter started mid",
        "   fight still counts damage but may show entity numbers until the next loading screen.",
        "3. The in game overlay is the aion2dps app. It starts with this terminal (o stops or starts it); its main",
        "   window is hidden because this tab replaces it (w shows it). Its hotkeys still work: Ctrl+Shift+O, L and R.",
        "4. Global client (NA/EU) uses port 13328. Other regions: --port <n>.",
    ] {
        lines.push(Line::from(step.to_string()));
    }
    for section in &app.sections {
        lines.push(Line::from(""));
        lines.push(heading(section.title));
        let width = section.rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0) + 2;
        for (k, v) in &section.rows {
            lines.push(Line::from(vec![Span::styled(format!("  {k:<width$}"), dim()), Span::raw(v.clone())]));
        }
    }
    let text = Text::from(lines);
    let max_scroll = (text.height() as u16).saturating_sub(area.height.saturating_sub(2));
    app.setup_scroll = app.setup_scroll.min(max_scroll);
    f.render_widget(Paragraph::new(text).block(block(title("Setup and diagnostics"))).scroll((app.setup_scroll, 0)), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_read_like_the_overlay() {
        assert_eq!(fmt_num(0.0), "0");
        assert_eq!(fmt_num(950.4), "950");
        assert_eq!(fmt_num(1_234.0), "1.23K");
        assert_eq!(fmt_num(13_500.0), "13.5K");
        assert_eq!(fmt_num(302_900.0), "302.9K");
        assert_eq!(fmt_num(1_250_000.0), "1.25M");
        assert_eq!(fmt_pct(1.0), "100.0%");
    }

    #[test]
    fn bars_fill_in_half_cells() {
        let text = |spans: Vec<Span>| spans.iter().map(|s| s.content.to_string()).collect::<String>();
        assert_eq!(text(bar(4, 1.0, Color::Red)), "━━━━");
        assert_eq!(text(bar(4, 0.0, Color::Red)), "━━━━");
        assert_eq!(text(bar(4, 0.5, Color::Red)).chars().count(), 4);
        assert_eq!(text(bar(4, 0.375, Color::Red)), "━╸━━");
    }

    #[test]
    fn series_fill_the_width() {
        assert_eq!(resample(&[1, 2, 3], 6), vec![1, 1, 2, 2, 3, 3], "a short fight is stretched");
        assert_eq!(resample(&[2, 4, 6, 8], 2), vec![3, 7], "a long one is averaged");
        assert!(resample(&[], 5).is_empty());
    }
}
