//! Rendering. Every view reads the `App` and draws; nothing here changes state except clamping scroll
//! offsets to what a list holds.
//!
//! Colours: green and red mark gains and spends and always come with a `+` or `-`, so the sign carries
//! the meaning without colour; magenta marks resyncs; cyan is the one data series (the balance line);
//! labels, axes and borders stay grey so the numbers stand out.

use crate::kina::app::{App, Prompt, Tab, TABS};
use crate::kina::feed::{Health, DAILY_RESET_HOUR};
use kina_core::tracker::DAILY_KINA_LIMIT;
use crate::options::Opt;
use crate::fmt::{clock, clock_secs, day, format_duration, format_short, format_signed, group, group_signed, same_day};
use kina_core::snapshot::{EndReason, EntryKind, SessionSnapshot};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::symbols::{self, Marker};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Axis, Block, Cell, Chart, Dataset, GraphType, LineGauge, Padding, Paragraph, Row, Table, TableState, Tabs, Wrap};
use ratatui::Frame;

const MUTED: Color = Color::DarkGray;
const GAIN: Color = Color::Green;
const SPEND: Color = Color::Red;
const RESYNC: Color = Color::Magenta;
const SERIES: Color = Color::Cyan;
const BRAND: Color = Color::Yellow;
/// Rates are shown once a session has been connected this long; before that they swing wildly.
const RATE_AFTER_MS: u64 = 60_000;
/// A run needs this long before its rate means anything: a two minute town visit is not a farm.
const RUN_RATE_AFTER_MS: u64 = 5 * 60_000;
const MIN_WIDTH: u16 = 64;
const MIN_HEIGHT: u16 = 18;

fn muted() -> Style {
    Style::new().fg(MUTED)
}

fn block(title: &str) -> Block<'static> {
    Block::bordered().border_style(muted()).title(Span::styled(format!(" {title} "), Style::new().fg(Color::Gray)))
}

fn signed_color(n: i64) -> Color {
    match n {
        n if n > 0 => GAIN,
        n if n < 0 => SPEND,
        _ => Color::Gray,
    }
}

fn right(text: impl Into<String>) -> Cell<'static> {
    Cell::from(Line::from(text.into()).alignment(Alignment::Right))
}

fn right_styled(text: impl Into<String>, style: Style) -> Cell<'static> {
    Cell::from(Line::from(Span::styled(text.into(), style)).alignment(Alignment::Right))
}

fn rate(s: &SessionSnapshot) -> Option<f64> {
    (s.duration_ms >= RATE_AFTER_MS).then_some(s.net_per_hour)
}

fn end_label(reason: Option<EndReason>) -> &'static str {
    match reason {
        None => "live",
        Some(EndReason::Manual) => "new session",
        Some(EndReason::CharacterChange) => "character change",
        Some(EndReason::Idle) => "idle",
        Some(EndReason::AppExit) => "closed",
    }
}

/// Draws the module into `area`, which the hub leaves below its own top bar.
pub fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let text = format!("The terminal is {}x{}; the kina tracker needs at least {MIN_WIDTH}x{MIN_HEIGHT}.", area.width, area.height);
        f.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), area);
        return;
    }
    let banner = app.opened.is_some() as u16;
    let [header, options, banner_area, body, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Length(banner), Constraint::Min(0), Constraint::Length(1)]).areas(area);
    draw_header(f, app, header);
    let goal = if app.options.goal > 0 { format_short(app.options.goal as f64) } else { "none".into() };
    crate::options::draw(f, options, &[Opt::Value { key: "g", label: "goal", value: goal }, Opt::Toggle { key: "h", label: "hide name", on: app.options.hide_name }]);
    if let Some(s) = &app.opened {
        let who = app.character_label(s.character.as_ref());
        let text = format!(" Viewing session #{} from {} {} ({who}). Esc returns to the live session. ", s.id, day(s.started_ms, app.now_ms), clock(s.started_ms));
        f.render_widget(Paragraph::new(text).style(Style::new().fg(Color::Black).bg(BRAND)), banner_area);
    }
    match app.tab {
        Tab::Live => draw_live(f, app, body),
        Tab::Runs => draw_runs(f, app, body),
        Tab::History => draw_history(f, app, body),
        Tab::Setup => draw_setup(f, app, body),
    }
    draw_footer(f, app, footer);
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let tabs_width = TABS.iter().map(|(_, l)| l.len() as u16 + 5).sum::<u16>();
    let [left, tabs_area] = Layout::horizontal([Constraint::Min(0), Constraint::Length(tabs_width)]).areas(area);
    let (dot, dot_color) = match app.status.health {
        Health::Good => ("●", GAIN),
        Health::Waiting => ("●", Color::Yellow),
        Health::Bad => ("●", SPEND),
    };
    let mut spans = vec![Span::raw(" ")];
    if let Some(c) = &app.character {
        spans.push(Span::styled(app.character_label(Some(c)), Style::new().bold()));
        let level = if c.level > 0 { format!(" · Lv {}", c.level) } else { String::new() };
        spans.push(Span::styled(format!(" · server {}{level}", c.server), muted()));
        spans.push(Span::styled("  │  ", muted()));
    }
    spans.push(Span::styled(format!("{dot} "), Style::new().fg(dot_color)));
    spans.push(Span::raw(app.status.text.clone()));
    f.render_widget(Paragraph::new(Line::from(spans)), left);
    let titles: Vec<Line> = TABS.iter().enumerate().map(|(i, (_, l))| Line::from(format!("{} {l}", i + 1))).collect();
    let tabs = Tabs::new(titles).select(app.tab.index()).style(muted()).highlight_style(Style::new().fg(Color::Black).bg(Color::Gray).bold()).divider(" ");
    f.render_widget(tabs, tabs_area);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    match &app.prompt {
        Some(Prompt::Delete(id)) => {
            let line = Line::from(vec![Span::styled(format!(" Delete session #{id}? "), Style::new().fg(SPEND).bold()), Span::raw("y deletes, any other key keeps it")]);
            f.render_widget(Paragraph::new(line), area);
            return;
        }
        Some(Prompt::Goal(text)) => {
            let line = Line::from(vec![
                Span::styled(" Net kina goal: ", Style::new().fg(BRAND).bold()),
                Span::styled(format!("{text}▏"), Style::new().bold()),
                Span::styled("   like 5m or 750k, enter sets it, empty or 0 clears it, esc keeps the old one", muted()),
            ]);
            f.render_widget(Paragraph::new(line), area);
            return;
        }
        None => {}
    }
    if let Some((text, _)) = &app.message {
        f.render_widget(Paragraph::new(Line::from(vec![Span::raw(" "), Span::styled(text.clone(), Style::new().fg(BRAND))])), area);
        return;
    }
    let mut keys: Vec<(&str, &str)> = vec![("q", "quit"), ("1-4", "views"), ("n", "new session"), ("c", "copy summary")];
    match app.tab {
        Tab::Live => keys.push(("↑↓", "scroll")),
        Tab::Runs => keys.extend([("↑↓", "scroll"), ("a", "all visits")]),
        Tab::History => keys.extend([("↑↓", "select"), ("enter", "open"), ("d", "delete")]),
        Tab::Setup => keys.extend([("↑↓", "scroll"), ("r", "retry capture")]),
    }
    if app.opened.is_some() {
        keys.push(("esc", "back to live"));
    }
    let mut spans = vec![Span::raw(" ")];
    for (k, label) in keys {
        spans.push(Span::styled(k, Style::new().fg(Color::Gray).bold()));
        spans.push(Span::styled(format!(" {label}   "), muted()));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The overview's part: balance, net, rate and goal, the live session, and as many of the newest changes as fit.
pub fn draw_overview(f: &mut Frame, area: Rect, app: &App) {
    let Some(s) = app.current.as_ref() else {
        let lines = vec![
            Line::from(Span::styled("Waiting for your kina balance.", Style::new().fg(BRAND))),
            Line::from(Span::styled("Any loading screen starts the session.", muted())),
        ];
        f.render_widget(Paragraph::new(lines).block(block("Kina").padding(Padding::horizontal(1))), area);
        return;
    };
    let mut top = vec![
        Span::styled(s.balance.map(group).unwrap_or_else(|| "unknown".into()), Style::new().bold()),
        Span::styled("  net ", muted()),
        Span::styled(group_signed(s.net), Style::new().fg(signed_color(s.net)).bold()),
    ];
    match rate(s) {
        Some(r) => top.extend([Span::raw("  "), Span::styled(format_signed(r), Style::new().fg(signed_color(r.round() as i64))), Span::styled(" per hour", muted())]),
        None => top.push(Span::styled("  rate after 1 min", muted())),
    }
    let mut lines = vec![Line::from(top)];
    if let Some(counted) = app.limits.daily_counted() {
        let short = area.width < 50;
        let mut daily = vec![Span::styled("daily ", muted())];
        daily.extend(limit_spans(counted, DAILY_KINA_LIMIT, area.width.saturating_sub(if short { 26 } else { 36 }).min(24), short));
        lines.push(Line::from(daily));
    }
    if app.options.goal > 0 {
        let ratio = (s.net.max(0) as f64 / app.options.goal as f64).clamp(0.0, 1.0);
        let label = format!(" {:.0}% of {}", ratio * 100.0, format_short(app.options.goal as f64));
        let width = area.width.saturating_sub(4 + 5 + label.len() as u16).min(30);
        let mut goal = vec![Span::styled("goal ", muted())];
        goal.extend(crate::dps::ui::bar(width, ratio, BRAND));
        goal.push(Span::styled(label, muted()));
        lines.push(Line::from(goal));
    }
    lines.push(Line::from(vec![
        Span::styled(group_signed(s.earned as i64), Style::new().fg(GAIN)),
        Span::styled(" earned  ", muted()),
        Span::styled(group_signed(-(s.spent as i64)), Style::new().fg(SPEND)),
        Span::styled(" spent", muted()),
    ]));
    let place = s.current_map.as_ref().map(|m| m.name.clone()).unwrap_or_else(|| "map unknown".into());
    lines.push(Line::from(Span::styled(format!("{} connected · {place}", format_duration(s.duration_ms)), muted())));
    let room = (area.height.saturating_sub(2) as usize).saturating_sub(lines.len());
    for e in s.entries.iter().rev().take(room) {
        let color = match e.kind {
            EntryKind::Gain => GAIN,
            EntryKind::Spend => SPEND,
            EntryKind::Resync => RESYNC,
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", clock_secs(e.t_ms)), muted()),
            Span::styled(format!("{:>11}", group_signed(e.delta)), Style::new().fg(color).bold()),
            Span::styled(if e.count > 1 { format!(" x{:<3}", e.count) } else { "     ".into() }, muted()),
            match (&e.map_name, e.kind) {
                (_, EntryKind::Resync) => Span::styled("resync, not counted", Style::new().fg(RESYNC)),
                (Some(m), _) => Span::raw(m.clone()),
                (None, _) => Span::styled("map unknown", muted()),
            },
        ]));
    }
    let title = if s.active { format!("Kina · session #{}", s.id) } else { format!("Kina · session #{} ended: {}", s.id, end_label(s.end_reason)) };
    f.render_widget(Paragraph::new(lines).block(block(&title).padding(Padding::horizontal(1))), area);
}

fn draw_waiting(f: &mut Frame, app: &App, area: Rect) {
    let mut lines = vec![
        Line::from(Span::styled("Waiting for your kina balance", Style::new().fg(BRAND).bold())),
        Line::from(""),
        Line::from("The game sends the whole inventory on every loading screen. Teleport, enter or leave a dungeon,"),
        Line::from("change channel, or log in with the tracker already running, and the session starts."),
        Line::from(""),
        Line::from(vec![Span::styled("status  ", muted()), Span::raw(app.status.text.clone())]),
    ];
    if let Some(c) = &app.character {
        lines.push(Line::from(vec![Span::styled("seen    ", muted()), Span::raw(app.character_label(Some(c)))]));
    }
    if app.status.health == Health::Bad {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("Capture is not running. Open Setup (4) for what to do.", Style::new().fg(SPEND))));
    }
    let [_, mid, _] = Layout::vertical([Constraint::Fill(1), Constraint::Length(lines.len() as u16 + 2), Constraint::Fill(1)]).areas(area);
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center).block(block("Live")), mid);
}

fn draw_live(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(s) = app.shown().cloned() else {
        draw_waiting(f, app, area);
        return;
    };
    let chart_height = if area.height >= 38 { 14 } else if area.height >= 30 { 10 } else { 7 };
    let [tiles, limits, middle, log] =
        Layout::vertical([Constraint::Length(5), Constraint::Length(4), Constraint::Length(chart_height), Constraint::Min(4)]).areas(area);
    draw_tiles(f, app, &s, tiles);
    draw_limits(f, app, limits);
    let maps_width = (area.width * 2 / 5).clamp(36, 60);
    let [chart_area, maps_area] = Layout::horizontal([Constraint::Min(20), Constraint::Length(maps_width)]).areas(middle);
    draw_chart(f, &s, chart_area);
    draw_maps(f, &s, maps_area);
    draw_log(f, app, &s, log);
}

fn draw_tiles(f: &mut Frame, app: &App, s: &SessionSnapshot, area: Rect) {
    let cols = Layout::horizontal([Constraint::Ratio(1, 4); 4]).split(area);
    // Balance
    let balance = s.balance.map(group).unwrap_or_else(|| "unknown".into());
    let start = s.start_balance.map(|b| format!("from {}", group(b))).unwrap_or_default();
    let tile = Paragraph::new(vec![
        Line::from(Span::styled(balance, Style::new().bold())),
        Line::from(Span::styled(start, muted())),
        Line::from(Span::styled(if s.biggest_gain > 0 { format!("best gain {}", group_signed(s.biggest_gain as i64)) } else { String::new() }, muted())),
    ])
    .block(block("Balance"));
    f.render_widget(tile, cols[0]);
    // Net and rate, with the goal when one is set
    let rate_line = match rate(s) {
        Some(r) => Line::from(vec![Span::styled(format_signed(r), Style::new().fg(signed_color(r.round() as i64))), Span::styled(" per hour", muted())]),
        None => Line::from(Span::styled("rate after 1 min", muted())),
    };
    let net_block = block("Net");
    let inner = net_block.inner(cols[1]);
    f.render_widget(net_block, cols[1]);
    let [l1, l2, l3] = Layout::vertical([Constraint::Length(1); 3]).areas(inner);
    f.render_widget(Paragraph::new(Span::styled(group_signed(s.net), Style::new().fg(signed_color(s.net)).bold())), l1);
    f.render_widget(Paragraph::new(rate_line), l2);
    if app.options.goal > 0 {
        let ratio = (s.net.max(0) as f64 / app.options.goal as f64).clamp(0.0, 1.0);
        let gauge = LineGauge::default()
            .ratio(ratio)
            .label(Span::styled(format!("{:>3.0}% of {} ", ratio * 100.0, format_short(app.options.goal as f64)), muted()))
            .filled_style(Style::new().fg(BRAND))
            .unfilled_style(muted())
            .line_set(symbols::line::THICK);
        f.render_widget(gauge, l3);
    } else {
        f.render_widget(Paragraph::new(Span::styled(format!("{} changes", s.changes), muted())), l3);
    }
    // Earned and spent
    let tile = Paragraph::new(vec![
        Line::from(vec![Span::styled(group_signed(s.earned as i64), Style::new().fg(GAIN)), Span::styled(" earned", muted())]),
        Line::from(vec![Span::styled(group_signed(-(s.spent as i64)), Style::new().fg(SPEND)), Span::styled(" spent", muted())]),
        Line::from(Span::styled(if s.resyncs > 0 { format!("{} resync{} not counted", s.resyncs, if s.resyncs == 1 { "" } else { "s" }) } else { String::new() }, Style::new().fg(RESYNC))),
    ])
    .block(block("In and out"));
    f.render_widget(tile, cols[2]);
    // Session
    let place = s.current_map.as_ref().map(|m| m.name.clone()).unwrap_or_else(|| "map unknown".into());
    let state = if s.active { format!("since {}", clock(s.started_ms)) } else { format!("ended: {}", end_label(s.end_reason)) };
    let tile = Paragraph::new(vec![
        Line::from(vec![Span::styled(format_duration(s.duration_ms), Style::new().bold()), Span::styled(" connected", muted())]),
        Line::from(Span::raw(place)),
        Line::from(Span::styled(state, muted())),
    ])
    .block(block(&format!("Session #{}", s.id)));
    f.render_widget(tile, cols[3]);
}

/// `in 16 h 12 min`, `in 12 min`.
fn countdown(ms: u64) -> String {
    let min = ms.div_ceil(60_000);
    if min >= 60 {
        format!("in {} h {} min", min / 60, min % 60)
    } else {
        format!("in {min} min")
    }
}

/// One limit as the game's window shows it: a bar, `192,339 / 1,000,000`, and the whole percent reached.
fn limit_spans(value: u64, cap: u64, bar_width: u16, short: bool) -> Vec<Span<'static>> {
    let ratio = if cap > 0 { value as f64 / cap as f64 } else { 0.0 };
    let mut spans = crate::dps::ui::bar(bar_width, ratio, SERIES);
    let numbers = if short { format!(" {} / {}", format_short(value as f64), format_short(cap as f64)) } else { format!(" {} / {}", group(value), group(cap)) };
    spans.push(Span::styled(numbers, Style::new().bold()));
    spans.push(Span::styled(format!(" {:>3}%", (ratio * 100.0).floor() as u64), muted()));
    spans
}

/// The daily gain limit (only kina from open world mobs counts) and the Abyss Points, as in the game's window.
fn draw_limits(f: &mut Frame, app: &App, area: Rect) {
    let l = &app.limits;
    let bar_width = (area.width / 4).clamp(10, 36);
    let label = |t: &str| Span::styled(format!("{t:<14}"), muted());
    let daily = match l.daily_counted() {
        Some(counted) => {
            let mut spans = vec![label("daily gain")];
            spans.extend(limit_spans(counted, DAILY_KINA_LIMIT, bar_width, false));
            spans.push(Span::styled(
                format!(
                    "   {} left · resets {DAILY_RESET_HOUR:02}:00, {}",
                    group(DAILY_KINA_LIMIT.saturating_sub(counted)),
                    countdown(l.next_reset_ms().saturating_sub(l.now_ms))
                ),
                muted(),
            ));
            Line::from(spans)
        }
        None => Line::from(vec![label("daily gain"), Span::styled("comes with the next loading screen or open world kina drop", muted())]),
    };
    let abyss = match l.abyss {
        Some(a) => {
            let mut spans = vec![label("abyss points")];
            spans.extend(limit_spans(a.points, a.cap, bar_width, false));
            Line::from(spans)
        }
        None => Line::from(vec![label("abyss points"), Span::styled("comes with the next loading screen", muted())]),
    };
    f.render_widget(Paragraph::new(vec![daily, abyss]).block(block("Daily limits · only kina from open world mobs counts").padding(Padding::horizontal(1))), area);
}

/// The balance as a step line: it holds its value until the next change, then jumps.
fn step_points(s: &SessionSnapshot) -> Vec<(f64, f64)> {
    let x = |t: u64| t.saturating_sub(s.started_ms) as f64 / 1000.0;
    let mut pts: Vec<(f64, f64)> = Vec::with_capacity(s.timeline.len() * 2 + 1);
    for p in &s.timeline {
        if let Some(&(_, prev)) = pts.last() {
            pts.push((x(p.t_ms), prev));
        }
        pts.push((x(p.t_ms), p.balance as f64));
    }
    if let Some(&(_, last)) = pts.last() {
        pts.push((x(s.last_ms.max(s.started_ms)), last));
    }
    pts
}

fn draw_chart(f: &mut Frame, s: &SessionSnapshot, area: Rect) {
    let pts = step_points(s);
    let span = (s.last_ms.saturating_sub(s.started_ms) as f64 / 1000.0).max(60.0);
    let (lo, hi) = pts.iter().fold((f64::MAX, f64::MIN), |(lo, hi), &(_, y)| (lo.min(y), hi.max(y)));
    let (lo, hi) = if lo > hi { (0.0, 1.0) } else { (lo, hi) };
    let pad = ((hi - lo) * 0.08).max(1_000.0);
    let (y0, y1) = ((lo - pad).max(0.0), hi + pad);
    let mid_ms = s.started_ms + (span * 500.0) as u64;
    let x_labels = vec![Span::styled(clock(s.started_ms), muted()), Span::styled(clock(mid_ms), muted()), Span::styled(clock(s.started_ms + (span * 1000.0) as u64), muted())];
    let y_labels = vec![Span::styled(format_short(y0), muted()), Span::styled(format_short((y0 + y1) / 2.0), muted()), Span::styled(format_short(y1), muted())];
    let data = Dataset::default().marker(Marker::Braille).graph_type(GraphType::Line).style(Style::new().fg(SERIES)).data(&pts);
    let chart = Chart::new(vec![data])
        .block(block("Balance over the session"))
        .x_axis(Axis::default().bounds([0.0, span]).labels(x_labels).style(muted()))
        .y_axis(Axis::default().bounds([y0, y1]).labels(y_labels).style(muted()))
        .legend_position(None);
    f.render_widget(chart, area);
}

fn map_line(m: Option<&kina_core::gamedata::MapInfo>) -> Line<'static> {
    match m {
        Some(m) => {
            let mut spans = vec![Span::raw(m.name.clone())];
            if let Some(t) = &m.tier {
                spans.push(Span::styled(format!(" {t}"), muted()));
            }
            Line::from(spans)
        }
        None => Line::from(Span::styled("before the first loading screen", muted())),
    }
}

fn draw_maps(f: &mut Frame, s: &SessionSnapshot, area: Rect) {
    let rows: Vec<Row> = s
        .maps
        .iter()
        .filter(|m| m.changes > 0)
        .map(|m| {
            Row::new(vec![
                Cell::from(map_line(m.map.as_ref())),
                right(m.visits.to_string()),
                right(format_duration(m.time_ms)),
                right_styled(format_signed(m.net as f64), Style::new().fg(signed_color(m.net))),
            ])
        })
        .collect();
    let empty = rows.is_empty();
    let header = Row::new(vec![Cell::from("Map"), right("Runs"), right("Time"), right("Net")]).style(muted());
    let table = Table::new(rows, [Constraint::Min(10), Constraint::Length(4), Constraint::Length(8), Constraint::Length(8)]).header(header).block(block("Where it came from"));
    f.render_widget(table, area);
    if empty {
        let inner = Rect { x: area.x + 2, y: area.y + 2, width: area.width.saturating_sub(4), height: 1 };
        f.render_widget(Paragraph::new(Span::styled("no kina changes yet", muted())), inner);
    }
}

fn entry_row(e: &kina_core::snapshot::KinaEntry) -> Row<'static> {
    let (label, color) = match e.kind {
        EntryKind::Gain => ("gain", GAIN),
        EntryKind::Spend => ("spend", SPEND),
        EntryKind::Resync => ("resync", RESYNC),
    };
    let times = if e.count > 1 { format!("x{}", e.count) } else { String::new() };
    let map = match (&e.map_name, e.kind) {
        (_, EntryKind::Resync) => Line::from(Span::styled("inventory check disagreed, not counted", Style::new().fg(RESYNC))),
        (Some(m), _) => Line::from(m.clone()),
        (None, _) => Line::from(Span::styled("map unknown", muted())),
    };
    Row::new(vec![
        Cell::from(Span::styled(clock_secs(e.t_ms), muted())),
        Cell::from(Span::styled(label, Style::new().fg(color))),
        right_styled(group_signed(e.delta), Style::new().fg(color).add_modifier(Modifier::BOLD)),
        Cell::from(Span::styled(times, muted())),
        right(group(e.balance)),
        Cell::from(map),
    ])
}

fn draw_log(f: &mut Frame, app: &mut App, s: &SessionSnapshot, area: Rect) {
    let visible = area.height.saturating_sub(3) as usize;
    let total = s.entries.len();
    app.log_scroll = app.log_scroll.min(total.saturating_sub(visible.max(1)));
    let rows: Vec<Row> = s.entries.iter().rev().skip(app.log_scroll).take(visible).map(entry_row).collect();
    let title = if total > visible { format!("Kina log, {} of {total}, newest first", (app.log_scroll + 1)) } else { "Kina log, newest first".into() };
    let header = Row::new(vec![Cell::from("Time"), Cell::from("Kind"), right("Change"), Cell::from(""), right("Balance"), Cell::from("Where")]).style(muted());
    let widths = [Constraint::Length(8), Constraint::Length(6), Constraint::Length(12), Constraint::Length(4), Constraint::Length(12), Constraint::Min(10)];
    f.render_widget(Table::new(rows, widths).header(header).block(block(&title)), area);
    if total == 0 {
        let inner = Rect { x: area.x + 2, y: area.y + 2, width: area.width.saturating_sub(4), height: 1 };
        f.render_widget(Paragraph::new(Span::styled("Nothing yet. Sales, rewards, repairs and purchases show up here as they happen.", muted())), inner);
    }
}

fn draw_runs(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(s) = app.shown().cloned() else {
        draw_waiting(f, app, area);
        return;
    };
    let visible = area.height.saturating_sub(3) as usize;
    let shown: Vec<&kina_core::snapshot::RunStats> = s.runs.iter().rev().filter(|r| app.runs_all || r.changes > 0 || r.current).collect();
    app.runs_scroll = app.runs_scroll.min(shown.len().saturating_sub(visible.max(1)));
    let rows: Vec<Row> = shown
        .iter()
        .skip(app.runs_scroll)
        .take(visible)
        .map(|r| {
            let quiet = r.changes == 0 && !r.current;
            let per_hour = if r.duration_ms >= RUN_RATE_AFTER_MS && r.changes > 0 { format_signed(r.net as f64 * 3_600_000.0 / r.duration_ms as f64) } else { String::new() };
            let mut map = map_line(r.map.as_ref());
            if r.current {
                map.spans.insert(0, Span::styled("▶ ", Style::new().fg(BRAND)));
            }
            let row = Row::new(vec![
                right(r.index.to_string()),
                Cell::from(clock_secs(r.started_ms)),
                right(format_duration(r.duration_ms)),
                Cell::from(map),
                right_styled(if r.earned > 0 { group_signed(r.earned as i64) } else { String::new() }, Style::new().fg(GAIN)),
                right_styled(if r.spent > 0 { group_signed(-(r.spent as i64)) } else { String::new() }, Style::new().fg(SPEND)),
                right_styled(if r.changes > 0 { group_signed(r.net) } else { "0".into() }, Style::new().fg(signed_color(r.net)).bold()),
                right_styled(per_hour, Style::new().fg(signed_color(r.net))),
            ]);
            if quiet {
                row.style(muted())
            } else {
                row
            }
        })
        .collect();
    let with_kina = s.runs.iter().filter(|r| r.changes > 0).count();
    let which = if app.runs_all { "all shown" } else { "a shows all" };
    let title = format!("Runs of session #{}: {} map visits, {with_kina} with kina ({which}), newest first", s.id, s.runs.len());
    let header = Row::new(vec![right("#"), Cell::from("Start"), right("Time"), Cell::from("Map"), right("Earned"), right("Spent"), right("Net"), right("Net/h")]).style(muted());
    let widths = [
        Constraint::Length(4),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Min(16),
        Constraint::Length(11),
        Constraint::Length(11),
        Constraint::Length(11),
        Constraint::Length(8),
    ];
    f.render_widget(Table::new(rows, widths).header(header).block(block(&title)), area);
}

fn draw_history(f: &mut Frame, app: &mut App, area: Rect) {
    let [totals_area, list_area] = Layout::vertical([Constraint::Length(4), Constraint::Min(3)]).areas(area);
    // Today counts the live session too, since that is what the player is in the middle of.
    let live = app.current.as_ref().filter(|s| s.active);
    let today: Vec<(i64, u64)> = app
        .history
        .iter()
        .filter(|h| same_day(h.started_ms, app.now_ms))
        .map(|h| (h.net, h.duration_ms))
        .chain(live.filter(|s| same_day(s.started_ms, app.now_ms)).map(|s| (s.net, s.duration_ms)))
        .collect();
    let all: Vec<(i64, u64)> = app.history.iter().map(|h| (h.net, h.duration_ms)).chain(live.map(|s| (s.net, s.duration_ms))).collect();
    let line = |label: &str, rows: &[(i64, u64)]| {
        let net: i64 = rows.iter().map(|r| r.0).sum();
        let ms: u64 = rows.iter().map(|r| r.1).sum();
        let per_hour = if ms >= RATE_AFTER_MS { format!("{}/h", format_signed(net as f64 * 3_600_000.0 / ms as f64)) } else { "--".into() };
        Line::from(vec![
            Span::styled(format!("{label:<10}"), muted()),
            Span::styled(format!("{:>14}", group_signed(net)), Style::new().fg(signed_color(net)).bold()),
            Span::styled(format!("   {per_hour:>9}   "), Style::new().fg(signed_color(net))),
            Span::styled(format!("{} session{}, {} connected", rows.len(), if rows.len() == 1 { "" } else { "s" }, format_duration(ms)), muted()),
        ])
    };
    let title = if app.feed.persistent() { "Totals (live session included)" } else { "Totals (not saved: demo or replay)" };
    f.render_widget(Paragraph::new(vec![line("Today", &today), line("All saved", &all)]).block(block(title)), totals_area);

    let rows: Vec<Row> = app
        .history
        .iter()
        .map(|h| {
            let per_hour = if h.duration_ms >= RATE_AFTER_MS { format_signed(h.net_per_hour) } else { String::new() };
            Row::new(vec![
                Cell::from(day(h.started_ms, app.now_ms)),
                Cell::from(clock(h.started_ms)),
                Cell::from(app.summary_name(h.character_name.as_deref()).to_string()),
                right(format_duration(h.duration_ms)),
                right_styled(group_signed(h.net), Style::new().fg(signed_color(h.net)).bold()),
                right_styled(per_hour, Style::new().fg(signed_color(h.net))),
                right(h.end_balance.map(group).unwrap_or_default()),
                Cell::from(h.top_map.clone().unwrap_or_default()),
                Cell::from(Span::styled(end_label(h.end_reason), muted())),
            ])
        })
        .collect();
    let empty = rows.is_empty();
    let header = Row::new(vec![
        Cell::from("Day"),
        Cell::from("Start"),
        Cell::from("Character"),
        right("Time"),
        right("Net"),
        right("Net/h"),
        right("Balance"),
        Cell::from("Best map"),
        Cell::from("Ended"),
    ])
    .style(muted());
    let widths = [
        Constraint::Length(11),
        Constraint::Length(5),
        Constraint::Length(12),
        Constraint::Length(8),
        Constraint::Length(12),
        Constraint::Length(8),
        Constraint::Length(12),
        Constraint::Min(10),
        Constraint::Length(16),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .block(block(&format!("Sessions ({})", app.history.len())))
        .row_highlight_style(Style::new().bg(Color::Indexed(236)).bold())
        .highlight_symbol("› ");
    let mut state = TableState::default().with_selected((!empty).then_some(app.history_selected));
    f.render_stateful_widget(table, list_area, &mut state);
    if empty {
        let inner = Rect { x: list_area.x + 2, y: list_area.y + 2, width: list_area.width.saturating_sub(4), height: 1 };
        let text = "No finished sessions yet. A session is saved when you start a new one (n), switch character, go idle or quit.";
        f.render_widget(Paragraph::new(Span::styled(text, muted())), inner);
    }
}

fn draw_setup(f: &mut Frame, app: &mut App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    let heading = |t: &str| Line::from(Span::styled(t.to_string(), Style::new().fg(BRAND).bold()));
    lines.push(heading("Getting started"));
    for step in [
        "1. Install Npcap from https://npcap.com (default options are fine). The tracker only reads traffic, it never sends",
        "   or changes packets and never touches the game's memory.",
        "2. Start the tracker before you log in, or take any loading screen afterwards: the game sends the whole",
        "   inventory, including kina, on every loading screen.",
        "3. Only live inventory changes count as income or spending. If a loading screen shows a different balance",
        "   (packets were lost), the difference is logged as a resync and not counted.",
        "4. Global client (NA/EU) uses port 13328. Other regions: --port <n>.",
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
    use crate::kina::app::ViewOptions;
    use crate::kina::demo::Demo;
    use kina_core::snapshot::BalancePoint;
    use ratatui::backend::TestBackend;
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

    fn demo_app() -> App {
        let now = 1_791_722_000_000;
        App::new(Box::new(Demo::new(now)), ViewOptions { hide_name: false, goal: 5_000_000 }, now)
    }

    #[test]
    fn every_view_renders_at_common_sizes() {
        let mut app = demo_app();
        for tab in ['1', '2', '3', '4'] {
            app.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char(tab)));
            for (w, h) in [(64, 18), (100, 30), (160, 45)] {
                let screen = render(&mut app, w, h);
                assert!(screen.contains("1 Live"), "tab {tab} at {w}x{h}:\n{screen}");
            }
        }
    }

    #[test]
    fn live_view_shows_the_numbers() {
        let mut app = demo_app();
        let s = app.current.clone().unwrap();
        let screen = render(&mut app, 140, 40);
        assert!(screen.contains("Seraphel"), "{screen}");
        assert!(screen.contains(&group(s.balance.unwrap())), "{screen}");
        assert!(screen.contains(&group_signed(s.net)), "{screen}");
        assert!(screen.contains("Where it came from"));
        assert!(screen.contains("of 5.00M"), "the goal gauge: {screen}");
        assert!(screen.contains("daily gain") && screen.contains(" / 1,000,000") && screen.contains("resets 08:00"), "the daily limit: {screen}");
        assert!(screen.contains("abyss points") && screen.contains("138,120 / 2,000,000") && screen.contains("6%"), "the abyss points: {screen}");
    }

    /// Prints every view as text, for looking at layout changes without a terminal:
    /// `cargo test -p kina-tui print_screens -- --ignored --nocapture`. With `KINA_REPLAY` set to capture
    /// files separated by `;` it shows those instead of the demo.
    #[test]
    #[ignore]
    fn print_screens() {
        let now = 1_791_722_000_000;
        let feed: Box<dyn crate::kina::feed::Feed> = match std::env::var("KINA_REPLAY") {
            Ok(paths) => {
                let paths: Vec<std::path::PathBuf> = paths.split(';').map(Into::into).collect();
                Box::new(crate::kina::feed::ReplayFeed::new(&mut crate::source::Replay::with_defaults(&paths).unwrap()))
            }
            Err(_) => Box::new(Demo::new(now)),
        };
        let mut app = App::new(feed, ViewOptions { hide_name: false, goal: 5_000_000 }, now);
        let (w, h) = (120, 36);
        for tab in ['1', '2', '3', '4'] {
            app.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char(tab)));
            println!("{}", render(&mut app, w, h));
        }
    }

    #[test]
    fn too_small_says_so() {
        let mut app = demo_app();
        assert!(render(&mut app, 40, 10).contains("needs at least"));
    }

    #[test]
    fn step_points_hold_until_the_next_change() {
        let s = SessionSnapshot {
            started_ms: 1_000,
            last_ms: 31_000,
            timeline: vec![BalancePoint { t_ms: 1_000, balance: 100 }, BalancePoint { t_ms: 11_000, balance: 160 }],
            ..Default::default()
        };
        assert_eq!(step_points(&s), vec![(0.0, 100.0), (10.0, 100.0), (10.0, 160.0), (30.0, 160.0)]);
    }
}
