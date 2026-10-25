//! Rendering. Every view reads the `App` and draws; nothing here changes state except clamping scroll
//! offsets to what a list holds.
//!
//! Colours: yellow is a cube still there to be opened (and the alert banner), green one you opened, red one
//! someone else took, grey everything that is only context. The radar chart and its side panel use the chart's
//! own palette (`chart::cube_color`). State is always written out too, so nothing depends on colour alone.

use crate::cube::app::{alert_title, cube_label, App, MapKind, MapMode, Tab, TABS};
use crate::options::Opt;
use crate::cube::feed::Health;
use crate::fmt::{clock_secs, format_duration};
use crate::cube::{chart, view3d};
use cube_core::geo::{arrow, compass};
use cube_core::snapshot::{AlertKind, CubeState, CubeView, LogEntry, LogKind};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::canvas::{Canvas, Points};
use ratatui::widgets::{Block, Cell, Padding, Paragraph, Row, Table, Tabs, Wrap};
use ratatui::Frame;

const MUTED: Color = Color::DarkGray;
const CUBE: Color = Color::Yellow;
const MINE: Color = Color::Green;
const TAKEN: Color = Color::Red;
const MIN_WIDTH: u16 = 64;
const MIN_HEIGHT: u16 = 18;

fn muted() -> Style {
    Style::new().fg(MUTED)
}

fn block(title: &str) -> Block<'static> {
    Block::bordered().border_style(muted()).title(Span::styled(format!(" {title} "), Style::new().fg(Color::Gray)))
}

fn right(text: impl Into<String>) -> Cell<'static> {
    Cell::from(Line::from(text.into()).alignment(Alignment::Right))
}

fn state_label(c: &CubeView) -> (&'static str, Color) {
    match c.state {
        CubeState::Visible => ("in view", CUBE),
        CubeState::Opening if c.opened_by_you => ("you open it", MINE),
        CubeState::Opening => ("being opened", TAKEN),
        CubeState::OutOfView => ("out of view", Color::Gray),
        CubeState::Opened if c.opened_by_you => ("opened by you", MINE),
        CubeState::Opened => ("taken", TAKEN),
        CubeState::Gone => ("gone", TAKEN),
    }
}

/// `86 m`, `~86 m` when the fix is old, `?` without one.
fn distance(c: &CubeView) -> String {
    match c.distance_m {
        Some(d) if c.estimated => format!("~{d:.0} m"),
        Some(d) => format!("{d:.0} m"),
        None => "?".into(),
    }
}

/// ♪ beside the cube the detector beeps for.
fn beep_mark(app: &App, c: &CubeView) -> Span<'static> {
    if app.tracking.is_some_and(|t| t.id == c.id) {
        Span::styled(" ♪", Style::new().fg(CUBE).bold())
    } else {
        Span::raw("")
    }
}

/// `↗ NE`.
fn direction(c: &CubeView) -> String {
    c.bearing.map(|b| format!("{} {}", arrow(b), compass(b))).unwrap_or_default()
}

/// `▲ 4 m` above, `▼ 3 m` below, `level` within 2 m.
fn height(c: &CubeView) -> String {
    match c.height_m {
        Some(h) if h > chart::LEVEL_M => format!("▲ {h:.0} m"),
        Some(h) if h < -chart::LEVEL_M => format!("▼ {:.0} m", -h),
        Some(_) => "level".into(),
        None => String::new(),
    }
}

/// Draws the module into `area`, which the hub leaves below its own top bar.
pub fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let text = format!("The terminal is {}x{}; the cube radar needs at least {MIN_WIDTH}x{MIN_HEIGHT}.", area.width, area.height);
        f.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), area);
        return;
    }
    let banner_height = if app.banner.is_some() { 3 } else { 0 };
    let [header, options, banner, body, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Length(banner_height), Constraint::Min(0), Constraint::Length(1)]).areas(area);
    draw_header(f, app, header);
    crate::options::draw(
        f,
        options,
        &[
            Opt::Pick { label: "map", choices: vec![("1", "radar", app.map_kind == MapKind::Radar), ("2", "3D", app.map_kind == MapKind::View3d)] },
            Opt::Toggle { key: "m", label: "sound", on: app.options.sound },
            Opt::Toggle { key: "d", label: "detector", on: app.options.detector },
            Opt::Toggle { key: "h", label: "hide name", on: app.options.hide_name },
        ],
    );
    if banner_height > 0 {
        draw_banner(f, app, banner);
    }
    match app.tab {
        Tab::Radar => draw_radar(f, app, body),
        Tab::View3d => draw_3d(f, app, body),
        Tab::Log => draw_log(f, app, body),
        Tab::Setup => draw_setup(f, app, body),
    }
    draw_footer(f, app, footer);
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let tabs_width = TABS.iter().map(|(_, l)| l.len() as u16 + 5).sum::<u16>();
    let [left, tabs_area] = Layout::horizontal([Constraint::Min(0), Constraint::Length(tabs_width)]).areas(area);
    let dot_color = match app.status.health {
        Health::Good => MINE,
        Health::Waiting => Color::Yellow,
        Health::Bad => TAKEN,
    };
    let mut spans = vec![Span::raw(" ")];
    if let Some(c) = &app.snap.character {
        spans.push(Span::styled(app.character_label(), Style::new().bold()));
        spans.push(Span::styled(format!(" · server {}", c.server), muted()));
        spans.push(Span::styled("  │  ", muted()));
    }
    if let Some(m) = &app.snap.map {
        spans.push(Span::raw(m.name.clone()));
        spans.push(Span::styled("  │  ", muted()));
    }
    spans.push(Span::styled("● ", Style::new().fg(dot_color)));
    spans.push(Span::raw(app.status.text.clone()));
    f.render_widget(Paragraph::new(Line::from(spans)), left);
    let titles: Vec<Line> = TABS.iter().enumerate().map(|(i, (_, l))| Line::from(format!("{} {l}", i + 1))).collect();
    let tabs = Tabs::new(titles).select(app.tab.index()).style(muted()).highlight_style(Style::new().fg(Color::Black).bg(Color::Gray).bold()).divider(" ");
    f.render_widget(tabs, tabs_area);
}

fn alert_color(kind: AlertKind) -> Color {
    match kind {
        AlertKind::Appeared | AlertKind::StillThere => CUBE,
        AlertKind::OpenedByYou | AlertKind::Learned => MINE,
        AlertKind::OpenedByOther => TAKEN,
    }
}

fn draw_banner(f: &mut Frame, app: &App, area: Rect) {
    let Some((kind, c)) = app.banner_cube() else { return };
    let title = alert_title(kind);
    let bg = alert_color(kind);
    let (state, _) = state_label(&c);
    let mut facts = vec![distance(&c), direction(&c), height(&c)];
    if let Some(a) = c.age_ms {
        facts.push(format!("spawned {} ago", format_duration(a)));
    }
    if kind != AlertKind::Appeared && kind != AlertKind::StillThere {
        facts.push(state.to_string());
    }
    facts.push(format!("template {}", c.template));
    let facts: Vec<String> = facts.into_iter().filter(|s| !s.is_empty() && s != "?").collect();
    let style = Style::new().fg(Color::Black).bg(bg);
    let text = vec![
        Line::from(""),
        Line::from(vec![Span::styled(format!(" ◆ {title}  "), style.add_modifier(Modifier::BOLD)), Span::styled(facts.join("  ·  "), style)]),
        Line::from(""),
    ];
    f.render_widget(Paragraph::new(text).style(style), area);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    if let Some((text, _)) = &app.message {
        f.render_widget(Paragraph::new(Line::from(vec![Span::raw(" "), Span::styled(text.clone(), Style::new().fg(CUBE))])), area);
        return;
    }
    let mut keys: Vec<(&str, &str)> = vec![("q", "quit"), ("1-4", "views"), ("t", "test sounds")];
    match app.tab {
        Tab::Radar => keys.extend([("+/-", "zoom"), ("←↑↓→", "pan"), ("c", "follow"), ("a", "all")]),
        Tab::View3d => keys.extend([("←↑↓→", "turn"), ("+/-", "zoom"), ("v", "top"), ("o", "orbit"), ("c", "follow")]),
        Tab::Log => keys.push(("↑↓", "scroll")),
        Tab::Setup => keys.extend([("↑↓", "scroll"), ("r", "retry capture")]),
    }
    if app.banner.is_some() || app.tracking.is_some() {
        keys.push(("esc", "dismiss"));
    }
    let mut spans = vec![Span::raw(" ")];
    for (k, label) in keys {
        spans.push(Span::styled(k, Style::new().fg(Color::Gray).bold()));
        spans.push(Span::styled(format!(" {label}   "), muted()));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The overview's part: the alert, the cubes around and this run's counts in a box, and the radar below it when
/// there is room.
pub fn draw_overview(f: &mut Frame, area: Rect, app: &mut App) {
    let s = &app.snap;
    let mut lines = Vec::new();
    if let Some((kind, c)) = app.banner_cube() {
        let style = Style::new().fg(Color::Black).bg(alert_color(kind));
        let facts: Vec<String> = [distance(&c), direction(&c), height(&c)].into_iter().filter(|t| !t.is_empty() && t != "?").collect();
        lines.push(Line::from(vec![Span::styled(format!(" ◆ {} ", alert_title(kind)), style.add_modifier(Modifier::BOLD)), Span::styled(format!(" {}", facts.join("  ·  ")), style)]).style(style));
    }
    if s.cubes.is_empty() && lines.is_empty() {
        lines.push(Line::from(Span::styled("No cube in range. The alert sounds once the game sends one.", muted())));
    }
    // Narrow, where to go comes before what state the cube is in.
    let compact = area.width < 50;
    for (i, c) in s.cubes.iter().enumerate() {
        let color = chart::cube_color(c);
        let (state, _) = state_label(c);
        let label = Span::styled(format!(" {} ", cube_label(i)), Style::new().fg(Color::Black).bg(color).bold());
        lines.push(Line::from(if compact {
            vec![label, Span::raw(format!(" {} {}", distance(c), direction(c))), Span::styled(format!(" {state}"), Style::new().fg(color)), beep_mark(app, c)]
        } else {
            vec![
                label,
                Span::styled(format!(" {state:<13}"), Style::new().fg(color)),
                Span::raw(format!("{:>7} ", distance(c))),
                Span::styled(format!("{:<5} {}", direction(c), height(c)), muted()),
                beep_mark(app, c),
            ]
        }));
    }
    let n = &s.counts;
    let counts = if compact {
        format!("{} seen · {} yours · {} others", n.cubes_seen, n.opened_by_you, n.opened_by_others)
    } else {
        format!("this run {} seen · {} opened by you · {} by others", n.cubes_seen, n.opened_by_you, n.opened_by_others)
    };
    let mut counts = vec![Span::styled(counts, muted())];
    if !app.options.sound {
        counts.push(Span::styled(" · muted", Style::new().fg(CUBE)));
    } else if !app.options.detector {
        counts.push(Span::styled(" · detector off", muted()));
    }
    lines.push(Line::from(counts));
    let open = s.open_cubes().count();
    let title = match open {
        0 => "Hidden Cube".to_string(),
        1 => "Hidden Cube: 1 still there".to_string(),
        n => format!("Hidden Cube: {n} still there"),
    };
    let border = if app.banner.is_some() || open > 0 { CUBE } else { MUTED };
    let block = Block::bordered().border_style(Style::new().fg(border)).title(Span::styled(format!(" {title} "), Style::new().fg(Color::Gray))).padding(Padding::horizontal(1));

    let text_height = lines.len() as u16 + 2;
    if area.height >= text_height + 8 && area.width >= 30 {
        let [text_area, chart_area] = Layout::vertical([Constraint::Length(text_height), Constraint::Min(0)]).areas(area);
        f.render_widget(Paragraph::new(lines).block(block), text_area);
        match app.map_kind {
            MapKind::Radar => draw_chart(f, app, chart_area),
            MapKind::View3d => draw_scene(f, app, chart_area),
        }
    } else {
        f.render_widget(Paragraph::new(lines).block(block), area);
    }
}

/// The Radar view: the chart (`chart.rs`) and beside it the cubes and what the chart's marks mean.
fn draw_radar(f: &mut Frame, app: &mut App, area: Rect) {
    let [main, summary] = Layout::vertical([Constraint::Min(8), Constraint::Length(1)]).areas(area);
    let side = if main.width >= 100 { 46 } else { 0 };
    let [chart_area, side_area] = Layout::horizontal([Constraint::Min(30), Constraint::Length(side)]).areas(main);
    draw_chart(f, app, chart_area);
    if side > 0 {
        draw_side(f, app, side_area);
    }
    draw_summary(f, app, summary);
}

/// The chart, centred on your dot (or where the zoom and pan keys put it), in a box without a title.
fn draw_chart(f: &mut Frame, app: &mut App, area: Rect) {
    let s = &app.snap;
    let at = |p: &cube_core::geo::Vec3| {
        let m = view3d::metres(p);
        (m[0] as f32, m[1] as f32)
    };
    let you = app.you();
    let points: Vec<(f32, f32)> =
        s.trail.iter().map(|t| at(&t.pos)).chain(you).chain(s.cubes.iter().map(|c| at(&c.pos))).chain(s.spots.iter().map(|p| at(&p.pos))).collect();
    if points.is_empty() {
        let text = vec![
            Line::from(""),
            Line::from(Span::styled("No position yet.", Style::new().bold())),
            Line::from(""),
            Line::from("Cubes still alert without one. The chart fills with your path, the cubes the game sends and the"),
            Line::from("spots seen before once the game sends a position (a buff, a teleport or a loading screen)."),
        ];
        f.render_widget(Paragraph::new(text).alignment(Alignment::Center).wrap(Wrap { trim: true }).block(Block::bordered().border_style(muted())), area);
        return;
    }
    let (center, radius) = match app.map.mode {
        MapMode::Follow => (you.or_else(|| s.trail.last().map(|t| at(&t.pos))).unwrap_or(points[0]), app.map.half_m as f32),
        MapMode::Free => ((app.map.center.0 as f32, app.map.center.1 as f32), app.map.half_m as f32),
        MapMode::Fit => {
            let (x0, x1) = points.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.0), hi.max(p.0)));
            let (y0, y1) = points.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.1), hi.max(p.1)));
            let mid = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
            let far = points.iter().map(|p| (p.0 - mid.0).hypot(p.1 - mid.1)).fold(0.0f32, f32::max);
            (mid, (far * 1.15).max(60.0))
        }
    };
    // Zooming or panning starts from what is shown.
    app.map.center = (f64::from(center.0), f64::from(center.1));
    app.map.half_m = f64::from(radius);
    let outer = Block::bordered().border_style(muted());
    let inner = outer.inner(area);
    f.render_widget(outer, area);
    let blink = (app.now_ms / 400).is_multiple_of(2);
    chart::draw(&app.snap, chart::MapFrame { center, radius_m: radius }, you, blink, inner, f.buffer_mut());
}

/// Beside the chart: which way you head, the cubes, what the chart's marks mean and its keys.
fn draw_side(f: &mut Frame, app: &App, area: Rect) {
    let s = &app.snap;
    let swatch = |c: Color| Style::new().fg(c);
    let title = match s.open_cubes().count() {
        0 => "Hidden Cubes: none around".to_string(),
        1 => "Hidden Cubes: 1 still there".to_string(),
        n => format!("Hidden Cubes: {n} still there"),
    };
    let heading = match chart::heading(s) {
        Some(h) => format!("moving {} {}", arrow(h), compass(h)),
        None => "not moving anywhere yet".into(),
    };
    let mut lines = vec![Line::from(vec![Span::styled("you  ", muted()), Span::raw(heading)]), Line::from("")];
    if s.cubes.is_empty() {
        for text in ["No cube in range. The game sends a cube", "once you are within about 100 m of it,", "and the radar lights up with a sound."] {
            lines.push(Line::from(Span::styled(text, muted())));
        }
    }
    for (i, c) in s.cubes.iter().enumerate() {
        let color = chart::cube_color(c);
        let (state, _) = state_label(c);
        lines.push(Line::from(vec![
            Span::styled(format!(" {} ", cube_label(i)), Style::new().fg(Color::Black).bg(color).bold()),
            Span::styled(format!(" {state:<13}"), Style::new().fg(color)),
            Span::raw(format!("{:>7} ", distance(c))),
            Span::styled(format!("{:<5} {}", direction(c), height(c)), muted()),
            beep_mark(app, c),
        ]));
    }
    let level = chart::LEVEL_M;
    lines.extend([
        Line::from(""),
        Line::from(vec![Span::styled("▲  ", Style::new().bold()), Span::raw(format!("more than {level:.0} m above you"))]),
        Line::from(vec![Span::styled("▼  ", Style::new().bold()), Span::raw(format!("more than {level:.0} m below you"))]),
        Line::from(vec![Span::styled("■  ", Style::new().bold()), Span::raw(format!("within {level:.0} m of your height"))]),
        Line::from(vec![Span::styled("■  ", swatch(chart::GREEN)), Span::raw("in view   "), Span::styled("■  ", swatch(chart::RED)), Span::raw("someone opens it")]),
        Line::from(vec![Span::styled("■  ", swatch(chart::YELLOW)), Span::raw("out of view, still there")]),
        Line::from(vec![Span::styled("■  ", swatch(chart::GREY)), Span::raw("opened or gone")]),
        Line::from(vec![Span::styled("▪  ", swatch(chart::GREY)), Span::raw(format!("spot where cubes were seen ({} here)", s.spots.len()))]),
        Line::from(vec![Span::styled("●→ ", swatch(chart::AMBER)), Span::raw("you, the way you travel (after 15 m)")]),
        Line::from(vec![Span::styled("╌  ", swatch(chart::AMBER)), Span::raw("the 100 m send range")]),
        Line::from(vec![Span::styled("─  ", swatch(chart::PATH)), Span::raw("your path, fading with age")]),
        Line::from(vec![Span::styled("CLOSER", swatch(chart::GREEN)), Span::raw(" / "), Span::styled("AWAY", swatch(chart::RED)), Span::raw(": metres in the last 30 s")]),
        Line::from(""),
        Line::from(Span::styled("+/- zoom  ←↑↓→ pan  c follow  a all", muted())),
    ]);
    f.render_widget(Paragraph::new(lines).block(block(&title)), area);
}

/// The 3D view: the scene from `view3d`, a short cube list beside it when there is room, and the camera help.
fn draw_3d(f: &mut Frame, app: &mut App, area: Rect) {
    let [main, help] = Layout::vertical([Constraint::Min(6), Constraint::Length(1)]).areas(area);
    let side = if main.width >= 110 { 34 } else { 0 };
    let [view_area, list_area] = Layout::horizontal([Constraint::Min(30), Constraint::Length(side)]).areas(main);
    draw_scene(f, app, view_area);
    if side > 0 {
        let lines: Vec<Line> = if app.snap.cubes.is_empty() {
            vec![Line::from(Span::styled("None around.", muted()))]
        } else {
            app.snap
                .cubes
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let (state, color) = state_label(c);
                    Line::from(vec![
                        Span::styled(format!("{} ", cube_label(i)), Style::new().fg(color).bold()),
                        Span::styled(format!("{state:<13}"), Style::new().fg(color)),
                        Span::raw(format!("{:>7} ", distance(c))),
                        Span::styled(height(c), muted()),
                        beep_mark(app, c),
                    ])
                })
                .collect()
        };
        f.render_widget(Paragraph::new(lines).block(block("Hidden Cubes")), list_area);
    }
    let text = " mouse: drag turns, right drag or shift drag pans, wheel zooms   shift+arrows pan   p path   s spots   0 reset";
    f.render_widget(Paragraph::new(Span::styled(text, muted())), help);
}

/// The 3D scene from `view3d` in a box that says where the camera looks, as the 3D view's keys left it.
fn draw_scene(f: &mut Frame, app: &App, view_area: Rect) {
    let cam = &app.cam;
    let follow = if cam.target.is_none() { "follows you" } else { "free, c follows you" };
    let look = if cam.top { "top view".to_string() } else { format!("looking {}, tilt {:.0}°", compass(cam.heading() as f32), cam.tilt()) };
    let title = format!("3D, {look}, {:.0} m out, {follow}{}", cam.dist, if cam.spin { ", orbiting" } else { "" });
    let outer = block(&title);
    let inner = outer.inner(view_area);
    let scene = view3d::render(cam, &app.snap, inner.width, inner.height);
    if scene.empty {
        let text = vec![
            Line::from(""),
            Line::from(Span::styled("Nothing to show yet.", Style::new().bold())),
            Line::from(""),
            Line::from("The 3D view fills with your path, the cubes the game sends and the spots seen before,"),
            Line::from("once the game sends a position (a buff, a teleport or a loading screen)."),
        ];
        f.render_widget(Paragraph::new(text).alignment(Alignment::Center).wrap(Wrap { trim: true }).block(outer), view_area);
    } else {
        let (w, h) = (scene.w, scene.h);
        let canvas = Canvas::default().block(outer).marker(Marker::Braille).x_bounds([0.0, w - 1.0]).y_bounds([0.0, h - 1.0]).paint(move |ctx| {
            for (color, points) in &scene.batches {
                ctx.draw(&Points { coords: points, color: *color });
            }
            ctx.layer();
            for l in &scene.labels {
                // A character is two dots wide: start half the text to the left so it centres on the point.
                let x = (l.x - l.text.chars().count() as f64).max(0.0);
                ctx.print(x, l.y, Span::styled(l.text.clone(), Style::new().fg(l.color).bold()));
            }
        });
        f.render_widget(canvas, view_area);
    }
}

fn draw_summary(f: &mut Frame, app: &App, area: Rect) {
    let s = &app.snap;
    let fix = match s.me {
        Some(m) => format!("position {} s old", s.now_ms.saturating_sub(m.t_ms) / 1000),
        None => "no position yet".into(),
    };
    let who = if s.self_guessed { "  ·  you: guessed from buffs" } else { "" };
    let text = format!(
        " cubes this run: {} seen, {} opened by you, {} by others  ·  {} objects in view  ·  {fix}{who}",
        s.counts.cubes_seen, s.counts.opened_by_you, s.counts.opened_by_others, s.objects_in_view
    );
    f.render_widget(Paragraph::new(Span::styled(text, muted())), area);
}

fn log_row(app: &App, l: &LogEntry) -> Row<'static> {
    let (what, color) = match l.kind {
        LogKind::Appeared => ("appeared", CUBE),
        LogKind::BackInView => ("back in view", CUBE),
        LogKind::LeftView => ("left view", Color::Gray),
        LogKind::OpeningStarted if l.by_you => ("you open it", MINE),
        LogKind::OpeningStarted => ("being opened", TAKEN),
        LogKind::Opened if l.by_you => ("opened by you", MINE),
        LogKind::Opened => ("taken", TAKEN),
        LogKind::Gone => ("gone", TAKEN),
        LogKind::Learned => ("template learned", MINE),
    };
    let who = match (l.by_you, l.actor) {
        (true, _) => app.character_label(),
        (false, Some(a)) => format!("entity {a}"),
        (false, None) => String::new(),
    };
    let map = l.map_id.map(|m| cube_core::gamedata::GameData::get().map(m).name).unwrap_or_default();
    Row::new(vec![
        Cell::from(Span::styled(clock_secs(l.t_ms), muted())),
        Cell::from(Span::styled(what, Style::new().fg(color))),
        right(l.cube.to_string()),
        right(l.template.to_string()),
        right(l.distance_m.map(|d| format!("{d:.0} m")).unwrap_or_default()),
        Cell::from(who),
        Cell::from(Span::styled(map, muted())),
    ])
}

fn draw_log(f: &mut Frame, app: &mut App, area: Rect) {
    let visible = area.height.saturating_sub(3) as usize;
    let total = app.snap.log.len();
    app.log_scroll = app.log_scroll.min(total.saturating_sub(visible.max(1)));
    let rows: Vec<Row> = app.snap.log.iter().rev().skip(app.log_scroll).take(visible).map(|l| log_row(app, l)).collect();
    let title = if total > visible { format!("Cube log, {} of {total}, newest first", app.log_scroll + 1) } else { "Cube log, newest first".into() };
    let header = Row::new(vec![Cell::from("Time"), Cell::from("Event"), right("Cube"), right("Template"), right("Dist"), Cell::from("Who"), Cell::from("Map")]).style(muted());
    let widths = [Constraint::Length(8), Constraint::Length(16), Constraint::Length(8), Constraint::Length(9), Constraint::Length(7), Constraint::Length(16), Constraint::Min(8)];
    f.render_widget(Table::new(rows, widths).header(header).block(block(&title)), area);
    if total == 0 {
        let inner = Rect { x: area.x + 2, y: area.y + 2, width: area.width.saturating_sub(4), height: 1 };
        f.render_widget(Paragraph::new(Span::styled("Nothing yet. Every cube that comes into view, and what happens to it, shows up here.", muted())), inner);
    }
}

fn draw_setup(f: &mut Frame, app: &mut App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    let heading = |t: &str| Line::from(Span::styled(t.to_string(), Style::new().fg(CUBE).bold()));
    lines.push(heading("Getting started"));
    for step in [
        "1. Install Npcap from https://npcap.com (default options are fine). aion2cube only reads traffic: it never sends",
        "   or changes packets and never touches the game's memory.",
        "2. Start it before you log in, or take any loading screen after starting: cubes already in view when it starts",
        "   are only seen once the game sends them again (teleport, loading screen, walking out of range and back).",
        "3. The game sends a cube once you are within about 100 m (planar). The alert needs no position; distance and",
        "   direction need one, which comes with your buffs, teleports and loading screens (so it can be seconds old).",
        "4. North is world -Y as the community maps read it; check it against the in game map once.",
        "5. Global client (NA/EU) uses port 13328. Other regions: --port <n>.",
        "6. Third party tools that read game traffic may be against the game's terms of service. Use at your own risk.",
    ] {
        lines.push(Line::from(step.to_string()));
    }
    for section in &app.sections {
        lines.push(Line::from(""));
        lines.push(heading(section.title));
        let width = section.rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0) + 2;
        for (k, v) in &section.rows {
            lines.push(Line::from(vec![Span::styled(format!("  {k:<width$}"), muted()), Span::raw(v.clone())]));
        }
    }
    let text = Text::from(lines);
    let max_scroll = (text.height() as u16).saturating_sub(area.height.saturating_sub(2));
    app.setup_scroll = app.setup_scroll.min(max_scroll);
    f.render_widget(Paragraph::new(text).block(block("Setup and diagnostics")).scroll((app.setup_scroll, 0)), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cube::app::tests::demo_app;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    use ratatui::Terminal;

    fn render(app: &mut App, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, f.area(), app)).unwrap();
        let buf = term.backend().buffer().clone();
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
    fn every_view_renders_at_common_sizes() {
        let mut app = demo_app(16);
        for tab in ['1', '2', '3', '4'] {
            app.on_key(KeyEvent::from(KeyCode::Char(tab)));
            for (w, h) in [(64, 18), (100, 30), (160, 45)] {
                let screen = render(&mut app, w, h);
                assert!(screen.contains("1 Radar"), "tab {tab} at {w}x{h}:\n{screen}");
            }
        }
    }

    #[test]
    fn the_radar_shows_the_alert_and_the_cubes() {
        let mut app = demo_app(16);
        let screen = render(&mut app, 140, 40);
        assert!(screen.contains("HIDDEN CUBE NEARBY"), "{screen}");
        assert!(screen.contains("Hidden Cubes: 3 still there"), "{screen}");
        assert!(screen.contains("in view"), "{screen}");
        assert!(screen.contains("template 1200039"), "the banner: {screen}");
        assert!(screen.contains("the 100 m send range"), "the legend: {screen}");
        assert!(screen.contains('●') && screen.contains("CUBE "), "the chart with a callout: {screen}");
        assert!(screen.contains("Seraphel"), "{screen}");
    }

    #[test]
    fn the_log_tells_what_happened() {
        let mut app = demo_app(40);
        app.on_key(KeyEvent::from(KeyCode::Char('3')));
        let screen = render(&mut app, 140, 40);
        for what in ["appeared", "taken", "opened by you", "left view", "entity 5709"] {
            assert!(screen.contains(what), "{what}: {screen}");
        }
    }

    #[test]
    fn the_radar_charts_you_the_cubes_and_the_spots() {
        let mut app = demo_app(16);
        let screen = render(&mut app, 140, 40);
        assert!(screen.contains('●'), "you: {screen}");
        assert!(["▲", "▼", "■"].iter().any(|g| screen.contains(g)), "the cubes with their height: {screen}");
        for badge in [" A ", " B ", " C "] {
            assert!(screen.contains(badge), "{badge}: {screen}");
        }
        assert!(screen.contains("100N"), "the chart and its axes: {screen}");
        assert!(!screen.contains("Around you"), "the chart's box has no title: {screen}");
        assert!(screen.contains("moving"), "the side panel: {screen}");
        assert!(screen.contains("out of view, still there"), "the legend: {screen}");
        let braille = screen.chars().filter(|c| ('\u{2801}'..='\u{28FF}').contains(c)).count();
        assert!(braille > 100, "{braille} braille cells: {screen}");
        let follow_m = app.map.half_m;
        app.on_key(KeyEvent::from(KeyCode::Char('a')));
        let fit = render(&mut app, 140, 40);
        assert!(fit.contains('●') && app.map.half_m != follow_m, "a zooms to show everything: {fit}");
    }

    #[test]
    fn the_3d_view_draws_the_scene_in_braille() {
        let mut app = demo_app(16);
        app.on_key(KeyEvent::from(KeyCode::Char('2')));
        let screen = render(&mut app, 140, 40);
        assert!(screen.contains("3D, looking"), "{screen}");
        assert!(screen.contains("you") && screen.contains("100 m"), "{screen}");
        assert!(screen.contains("Hidden Cubes"), "the side list: {screen}");
        let braille = screen.chars().filter(|c| ('\u{2801}'..='\u{28FF}').contains(c)).count();
        assert!(braille > 200, "{braille} braille cells: {screen}");
        app.on_key(KeyEvent::from(KeyCode::Char('v')));
        assert!(render(&mut app, 100, 30).contains("top view"));
    }

    #[test]
    fn the_radar_and_the_3d_view_say_when_there_is_nothing_yet() {
        let mut app = crate::cube::app::App::with_speaker(
            Box::new(crate::cube::demo::Demo::new(crate::cube::app::tests::START, cube_core::cubes::CubeBook::default())),
            crate::cube::app::ViewOptions { hide_name: false, sound: false, detector: true },
            crate::cube::app::tests::START - 1,
            crate::cube::app::Speaker::SILENT,
        );
        assert!(render(&mut app, 120, 30).contains("No position yet"));
        app.on_key(KeyEvent::from(KeyCode::Char('2')));
        assert!(render(&mut app, 120, 30).contains("Nothing to show yet"));
    }

    #[test]
    fn an_empty_radar_says_how_it_works() {
        let mut app = demo_app(0);
        let screen = render(&mut app, 120, 30);
        assert!(screen.contains("No cube in range"), "{screen}");
    }

    /// Prints every view as text, for looking at layout changes without a terminal:
    /// `cargo test -p cube-tui print_screens -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn print_screens() {
        let mut app = demo_app(16);
        for tab in ['1', '2', '3', '4'] {
            app.on_key(KeyEvent::from(KeyCode::Char(tab)));
            println!("{}", render(&mut app, 120, 34));
        }
    }

    /// Writes every view as cells (symbol, foreground, background) to the file in `AION2CUBE_CELLS`, one view per
    /// line, for turning into a picture of the screen: `cargo test -p cube-tui dump_cells -- --ignored`.
    #[test]
    #[ignore]
    fn dump_cells() {
        let Ok(path) = std::env::var("AION2CUBE_CELLS") else { return };
        let rgb = |c: Color, fallback: (u8, u8, u8)| match c {
            Color::Rgb(r, g, b) => (r, g, b),
            Color::Black => (12, 12, 12),
            Color::Red => (197, 15, 31),
            Color::Green => (19, 161, 14),
            Color::Yellow => (193, 156, 0),
            Color::Cyan => (58, 150, 221),
            Color::Gray => (204, 204, 204),
            Color::DarkGray => (118, 118, 118),
            Color::White => (242, 242, 242),
            _ => fallback,
        };
        let (w, h) = (140, 40);
        let mut app = demo_app(16);
        let mut out = String::new();
        for tab in ['1', '2'] {
            app.on_key(KeyEvent::from(KeyCode::Char(tab)));
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
            let buf = term.backend().buffer().clone();
            let cells: Vec<serde_json::Value> = (0..h)
                .flat_map(|y| (0..w).map(move |x| (x, y)))
                .map(|p| {
                    let c = &buf[p];
                    serde_json::json!([c.symbol(), rgb(c.fg, (204, 204, 204)), rgb(c.bg, (12, 12, 12))])
                })
                .collect();
            out.push_str(&serde_json::json!({ "w": w, "h": h, "cells": cells }).to_string());
            out.push('\n');
        }
        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn too_small_says_so() {
        let mut app = demo_app(0);
        assert!(render(&mut app, 40, 10).contains("needs at least"));
    }
}
