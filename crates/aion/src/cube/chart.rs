//! The radar chart, drawn in terminal cells in the style of a Bloomberg chart: a dotted grid in metres with its
//! labels on the right and bottom edges (north and south, west and east of you), the 100 m send range as a dashed
//! amber ring, your path in grey fading with age, and you as an amber dot with an arrow the way you are travelling.
//! The game never sends where your camera looks, only where you are (every 0.6 s in a fight, every few seconds
//! walking), so the arrow shows only once you have gone 15 m along a fairly straight line.
//!
//! Lines are Braille dots, two by four per cell, so a dot is about square; each cell takes the colour of the most
//! important line through it. Everything else is cell text. A cube is ▲ when it is more than 2 m above you, ▼
//! when more than 2 m below and ■ when about level. Cubes still there get a callout with distance, bearing,
//! height and what is happening to them, the nearest gets brackets and a dashed line tagged with its distance,
//! and cubes past the edge get an arrow on it. Drawing takes well under a millisecond, so the chart can follow
//! your dot gliding to each new position at full frame rate.

use crate::cube::app::cube_label;
use crate::cube::view3d::metres;
use cube_core::geo::{arrow, compass, Vec3, UNITS_PER_METRE};
use cube_core::snapshot::{CubeState, CubeView, RadarSnapshot, TrailPoint};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style, Stylize};

/// The game sends cubes from about this far (planar).
pub const SEND_RADIUS_M: f32 = 100.0;
/// A cube within this many metres up or down is about level with you.
pub const LEVEL_M: f32 = 2.0;
/// The trend compares your distance to a cube now with the distance from where you stood this long ago.
pub const TREND_MS: u64 = 30_000;
/// Without a fix that old, the oldest one will do if it is at least this old.
const TREND_MIN_MS: u64 = 10_000;
/// You are travelling once your path goes this far (game units) within this long, its two halves within
/// `HEADING_MAX_BEND` degrees of each other. In the four October 2026 captures that direction swung by more than
/// 90 degrees 2 to 8 times a session; the old rule (3 m in 10 s, any shape) swung 38 to 83 times, as every step
/// in a fight re-aimed it.
const HEADING_MIN_UNITS: f32 = 1_500.0;
const HEADING_WINDOW_MS: u64 = 15_000;
const HEADING_MAX_BEND: f32 = 45.0;
/// A heading older than this is not shown.
const HEADING_FRESH_MS: u64 = 20_000;
/// At most this many callouts, nearest cubes first.
const MAX_CALLOUTS: usize = 4;
/// Columns on the right for the north and south labels; the bottom row holds west and east.
const RIGHT_MARGIN: u16 = 6;
/// Grid lines are at least this many dots apart.
const GRID_MIN_DOTS: f32 = 10.0;
/// The arrow the way you travel reaches this many dots; its head's barbs are this long and this many radians off.
const ARROW_DOTS: f32 = 14.0;
const ARROW_BARB_DOTS: f32 = 5.0;
const ARROW_BARB: f32 = 0.5;

pub const AMBER: Color = Color::Rgb(255, 160, 40);
const AMBER_DIM: Color = Color::Rgb(168, 112, 30);
const RING: Color = Color::Rgb(140, 88, 22);
const WHITE: Color = Color::Rgb(230, 230, 230);
pub const GREEN: Color = Color::Rgb(47, 210, 122);
pub const RED: Color = Color::Rgb(255, 77, 77);
const RED_DIM: Color = Color::Rgb(120, 34, 34);
pub const YELLOW: Color = Color::Rgb(255, 224, 74);
pub const GREY: Color = Color::Rgb(125, 125, 125);
const MUTED: Color = Color::Rgb(154, 154, 154);
const GRID_MAJOR: Color = Color::Rgb(72, 72, 72);
const GRID_MINOR: Color = Color::Rgb(44, 44, 44);
const LEADER: Color = Color::Rgb(90, 90, 90);
const BOX: Color = Color::Rgb(24, 24, 24);
/// Your path: the oldest part and the newest.
const PATH_OLD: (u8, u8, u8) = (56, 56, 56);
const PATH_NEW: (u8, u8, u8) = (185, 185, 185);
pub const PATH: Color = Color::Rgb(PATH_NEW.0, PATH_NEW.1, PATH_NEW.2);

/// Line layers: a cell takes the colour of the highest layer that put a dot in it.
const GRID: u8 = 1;
const LEADERS: u8 = 2;
const RANGE: u8 = 3;
const TRAIL: u8 = 4;
const VIEW: u8 = 5;
const TARGET: u8 = 6;

/// Green in view, red when someone else is opening it, amber when you are, yellow out of view but still there,
/// grey once opened or gone.
pub fn cube_color(c: &CubeView) -> Color {
    match c.state {
        CubeState::Visible => GREEN,
        CubeState::Opening if c.opened_by_you => AMBER,
        CubeState::Opening => RED,
        CubeState::OutOfView => YELLOW,
        CubeState::Opened | CubeState::Gone => GREY,
    }
}

/// Where a cube is against your height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Elevation {
    Above,
    Below,
    Level,
}

impl Elevation {
    pub fn of(height_m: Option<f32>) -> Option<Elevation> {
        height_m.map(|h| match h {
            h if h > LEVEL_M => Elevation::Above,
            h if h < -LEVEL_M => Elevation::Below,
            _ => Elevation::Level,
        })
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Elevation::Above => "▲",
            Elevation::Below => "▼",
            Elevation::Level => "■",
        }
    }
}

/// How much farther (positive) or nearer (negative) a cube is than from where you stood about 30 s ago, in
/// metres. `None` until your path goes back far enough.
pub fn trend_m(snap: &RadarSnapshot, c: &CubeView) -> Option<f32> {
    let me = snap.me?;
    let target = me.t_ms.saturating_sub(TREND_MS);
    let then = snap.trail.iter().rev().find(|t| t.t_ms <= target && me.t_ms >= t.t_ms + TREND_MIN_MS);
    let then = then.or_else(|| snap.trail.first().filter(|t| me.t_ms >= t.t_ms + TREND_MIN_MS))?;
    Some((me.pos.planar_distance(&c.pos) - then.pos.planar_distance(&c.pos)) / UNITS_PER_METRE)
}

/// `CLOSER 12`, `AWAY 3` or `STEADY`, with its colour.
pub fn trend_text(t: f32) -> (String, Color) {
    match t {
        t if t <= -1.0 => (format!("CLOSER {:.0}", -t), GREEN),
        t if t >= 1.0 => (format!("AWAY {t:.0}"), RED),
        _ => ("STEADY".into(), MUTED),
    }
}

/// Time since the cube spawned (by the server clock when the spawn said), else since it was first seen: `0:12`,
/// `14:05`, `3 H`.
fn age_text(c: &CubeView, now_ms: u64) -> String {
    let s = c.age_ms.unwrap_or_else(|| now_ms.saturating_sub(c.first_seen_ms)) / 1_000;
    if s < 3_600 {
        format!("{}:{:02}", s / 60, s % 60)
    } else {
        format!("{} H", s / 3_600)
    }
}

fn distance_text(c: &CubeView) -> String {
    match c.distance_m {
        Some(d) if c.estimated => format!("~{d:.1} M"),
        Some(d) => format!("{d:.1} M"),
        None => "? M".into(),
    }
}

/// `100N`, `50W`, `0`: metres from you along an axis.
fn axis_label(v: f32, pos: char, neg: char) -> String {
    match v.round() as i64 {
        0 => "0".into(),
        n if n > 0 => format!("{n}{pos}"),
        n => format!("{}{neg}", -n),
    }
}

/// The way you are travelling, in compass degrees: from the latest point of your path at least 15 m back to the
/// newest one, when the newest is recent and the stretch between them is fairly straight. Standing, fighting or
/// turning gives no direction rather than a wrong one.
pub fn heading(snap: &RadarSnapshot) -> Option<f32> {
    let last = snap.trail.last()?;
    let fresh = snap.me.is_some_and(|m| m.t_ms.saturating_sub(last.t_ms) <= HEADING_FRESH_MS) && snap.now_ms.saturating_sub(last.t_ms) <= HEADING_FRESH_MS;
    if !fresh {
        return None;
    }
    let back: Vec<&TrailPoint> = snap.trail.iter().rev().skip(1).take_while(|p| last.t_ms.saturating_sub(p.t_ms) <= HEADING_WINDOW_MS).collect();
    let at = back.iter().position(|p| p.pos.planar_distance(&last.pos) >= HEADING_MIN_UNITS)?;
    let start = back[at];
    // The point nearest halfway along: both halves have to point about the same way.
    let halfway = |p: &&&TrailPoint| (p.pos.planar_distance(&start.pos) - p.pos.planar_distance(&last.pos)).abs();
    if let Some(mid) = back[..at].iter().min_by(|a, b| halfway(a).total_cmp(&halfway(b))) {
        let bend = (start.pos.bearing_to(&mid.pos) - mid.pos.bearing_to(&last.pos)).rem_euclid(360.0);
        if bend.min(360.0 - bend) > HEADING_MAX_BEND {
            return None;
        }
    }
    Some(start.pos.bearing_to(&last.pos))
}

fn east_north(p: &Vec3) -> (f32, f32) {
    let m = metres(p);
    (m[0] as f32, m[1] as f32)
}

/// The view: world metres east and north at the centre, and metres from the centre to the nearer edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapFrame {
    pub center: (f32, f32),
    pub radius_m: f32,
}

/// Braille dots over a grid of cells, each cell coloured by the highest layer that drew into it.
struct Dots {
    cols: usize,
    rows: usize,
    bits: Vec<u8>,
    layer: Vec<u8>,
    color: Vec<Color>,
}

/// The Braille bit of the dot at column `x` (0 or 1) and row `y` (0 to 3) of a cell.
const BRAILLE: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

impl Dots {
    fn new(cols: usize, rows: usize) -> Dots {
        Dots { cols, rows, bits: vec![0; cols * rows], layer: vec![0; cols * rows], color: vec![Color::Reset; cols * rows] }
    }

    fn width(&self) -> f32 {
        (self.cols * 2) as f32
    }

    fn height(&self) -> f32 {
        (self.rows * 4) as f32
    }

    fn dot(&mut self, x: f32, y: f32, layer: u8, color: Color) {
        if x < 0.0 || y < 0.0 || x >= self.width() || y >= self.height() {
            return;
        }
        let (x, y) = (x as usize, y as usize);
        let i = (y / 4) * self.cols + x / 2;
        self.bits[i] |= BRAILLE[x % 2][y % 4];
        if layer >= self.layer[i] {
            self.layer[i] = layer;
            self.color[i] = color;
        }
    }

    /// A line from `a` to `b` in dots; with `dash` = (on, off) dots it is dashed.
    fn line(&mut self, a: (f32, f32), b: (f32, f32), layer: u8, color: Color, dash: Option<(usize, usize)>) {
        let (w, h) = (self.width(), self.height());
        if (a.0 < 0.0 && b.0 < 0.0) || (a.1 < 0.0 && b.1 < 0.0) || (a.0 >= w && b.0 >= w) || (a.1 >= h && b.1 >= h) {
            return;
        }
        let steps = (b.0 - a.0).abs().max((b.1 - a.1).abs()).ceil().clamp(1.0, 20_000.0) as usize;
        for i in 0..=steps {
            if dash.is_some_and(|(on, off)| i % (on + off) >= on) {
                continue;
            }
            let t = i as f32 / steps as f32;
            self.dot(a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, layer, color);
        }
    }

    fn write(&self, area: Rect, buf: &mut Buffer) {
        for row in 0..self.rows {
            for col in 0..self.cols {
                let i = row * self.cols + col;
                if self.bits[i] == 0 {
                    continue;
                }
                let ch = char::from_u32(0x2800 + u32::from(self.bits[i])).unwrap_or(' ');
                if let Some(cell) = buf.cell_mut((area.x + col as u16, area.y + row as u16)) {
                    cell.set_char(ch).set_fg(self.color[i]);
                }
            }
        }
    }
}

/// Metres to dots: the dot at the frame's centre and dots per metre.
#[derive(Debug, Clone, Copy)]
struct Mapping {
    c: (f32, f32),
    scale: f32,
    center: (f32, f32),
}

impl Mapping {
    fn dot(self, m: (f32, f32)) -> (f32, f32) {
        (self.c.0 + (m.0 - self.center.0) * self.scale, self.c.1 - (m.1 - self.center.1) * self.scale)
    }

    fn world(self, d: (f32, f32)) -> (f32, f32) {
        (self.center.0 + (d.0 - self.c.0) / self.scale, self.center.1 - (d.1 - self.c.1) / self.scale)
    }
}

/// The cell a dot falls in, relative to the plot.
fn cell_of(d: (f32, f32)) -> (i32, i32) {
    ((d.0 / 2.0).floor() as i32, (d.1 / 4.0).floor() as i32)
}

/// A box of cells relative to the plot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cells {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl Cells {
    fn overlaps(self, o: Cells) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }

    fn inside(self, cols: i32, rows: i32) -> bool {
        self.x >= 0 && self.y >= 0 && self.x + self.w <= cols && self.y + self.h <= rows
    }
}

/// What the chart placed where, in screen cells.
#[derive(Debug, Default)]
pub struct Layout {
    /// Cube index and cell of every cube inside the plot.
    pub cubes: Vec<(usize, (u16, u16))>,
    /// Cube index and box of every callout.
    pub callouts: Vec<(usize, Rect)>,
    /// Cube index of every cube shown by an arrow on the edge.
    pub arrows: Vec<usize>,
}

/// One line of a callout: spans from the left, and a span set against the right edge.
struct CalloutLine {
    left: Vec<(String, Style)>,
    right: Option<(String, Style)>,
}

impl CalloutLine {
    fn width(&self) -> i32 {
        let left: usize = self.left.iter().map(|(s, _)| s.chars().count()).sum();
        let right = self.right.as_ref().map_or(0, |(s, _)| s.chars().count() + 2);
        (left + right) as i32
    }
}

fn callout_lines(snap: &RadarSnapshot, i: usize, c: &CubeView, blink: bool) -> Vec<CalloutLine> {
    let fg = |c: Color| Style::new().fg(c);
    let name = CalloutLine { left: vec![(format!("CUBE {}", cube_label(i)), fg(WHITE).bold())], right: Some((distance_text(c), fg(WHITE).bold())) };
    let mut place = Vec::new();
    if let Some(b) = c.bearing {
        place.push((format!("{} {:03}", compass(b), (b.round() as i32).rem_euclid(360)), fg(AMBER)));
    }
    match (Elevation::of(c.height_m), c.height_m) {
        (Some(Elevation::Level), _) => place.push((" ■ LEVEL".into(), fg(WHITE))),
        (Some(e), Some(h)) => {
            place.push((format!(" {} {h:+.1} M", e.glyph()), fg(WHITE)));
            place.push((if e == Elevation::Above { " ABOVE" } else { " BELOW" }.into(), fg(MUTED)));
        }
        _ => {}
    }
    let status = match c.state {
        CubeState::Opening if c.opened_by_you => vec![(" YOU ARE OPENING IT ".into(), Style::new().fg(Color::Black).bg(AMBER).bold())],
        CubeState::Opening => vec![(" OPENING BY OTHER ".into(), Style::new().fg(Color::Black).bg(if blink { RED } else { RED_DIM }).bold())],
        CubeState::OutOfView => vec![("OUT OF VIEW, STILL THERE".into(), fg(YELLOW))],
        _ => {
            let mut spans = Vec::new();
            if let Some(t) = trend_m(snap, c) {
                let (s, col) = trend_text(t);
                spans.push((format!("{s}  "), fg(col)));
            }
            spans.push(("AGE ".into(), fg(AMBER)));
            spans.push((age_text(c, snap.now_ms), fg(WHITE)));
            spans
        }
    };
    let mut lines = vec![name];
    if !place.is_empty() {
        lines.push(CalloutLine { left: place, right: None });
    }
    lines.push(CalloutLine { left: status, right: None });
    lines
}

/// Writes `s` from a cell on, clipped to `clip`.
fn put(buf: &mut Buffer, clip: Rect, x: i32, y: i32, s: &str, style: Style) {
    if y < i32::from(clip.y) || y >= i32::from(clip.bottom()) {
        return;
    }
    for (i, ch) in s.chars().enumerate() {
        let cx = x + i as i32;
        if cx < i32::from(clip.x) || cx >= i32::from(clip.right()) {
            continue;
        }
        if let Some(cell) = buf.cell_mut((cx as u16, y as u16)) {
            cell.set_char(ch).set_style(style);
        }
    }
}

/// Draws the chart into `area`, centred and zoomed as `frame` says, with you at `you` (metres east and north,
/// which may lie between two fixes while your dot glides). `blink` is the phase of anything that blinks.
pub fn draw(snap: &RadarSnapshot, frame: MapFrame, you: Option<(f32, f32)>, blink: bool, area: Rect, buf: &mut Buffer) -> Layout {
    let plot = Rect { width: area.width.saturating_sub(RIGHT_MARGIN), height: area.height.saturating_sub(1), ..area };
    let mut layout = Layout::default();
    if plot.width < 4 || plot.height < 2 {
        return layout;
    }
    let (cols, rows) = (i32::from(plot.width), i32::from(plot.height));
    let mut dots = Dots::new(plot.width as usize, plot.height as usize);
    let (dw, dh) = (dots.width(), dots.height());
    let map = Mapping { c: (dw / 2.0, dh / 2.0), scale: dw.min(dh) / 2.0 / frame.radius_m.max(1.0), center: frame.center };
    let abs = |(x, y): (i32, i32)| (i32::from(plot.x) + x, i32::from(plot.y) + y);

    // The grid, through you, every `step` metres; every second line brighter and labelled.
    let anchor = you.unwrap_or(frame.center);
    let step = [5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1_000.0, 2_500.0, 5_000.0, 10_000.0].into_iter().find(|s| s * map.scale >= GRID_MIN_DOTS).unwrap_or(25_000.0);
    let (west, north) = map.world((0.0, 0.0));
    let (east, south) = map.world((dw, dh));
    let lines = |lo: f32, hi: f32, at: f32| ((lo - at) / step).ceil() as i64..=((hi - at) / step).floor() as i64;
    let xs: Vec<(i64, f32)> = lines(west, east, anchor.0).map(|k| (k, map.dot((anchor.0 + k as f32 * step, 0.0)).0)).collect();
    let ys: Vec<(i64, f32)> = lines(south, north, anchor.1).map(|k| (k, map.dot((0.0, anchor.1 + k as f32 * step)).1)).collect();
    for &(k, x) in &xs {
        dots.line((x, 0.0), (x, dh - 1.0), GRID, if k % 2 == 0 { GRID_MAJOR } else { GRID_MINOR }, Some((1, 3)));
    }
    for &(k, y) in &ys {
        dots.line((0.0, y), (dw - 1.0, y), GRID, if k % 2 == 0 { GRID_MAJOR } else { GRID_MINOR }, Some((1, 3)));
    }

    // The send range, your path (older parts fainter) and the arrow the way you travel.
    let me_dot = you.map(|p| map.dot(p));
    let reach = SEND_RADIUS_M * map.scale;
    if let Some(c) = me_dot {
        let n = (std::f32::consts::TAU * reach).ceil().max(8.0) as usize;
        for i in (0..n).filter(|i| i % 6 < 3) {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            dots.dot(c.0 + a.cos() * reach, c.1 + a.sin() * reach, RANGE, RING);
        }
    }
    let mut path: Vec<(f32, f32)> = snap.trail.iter().map(|t| east_north(&t.pos)).collect();
    if let Some(p) = you {
        // The newest fix is where your dot is heading; the path ends where the dot is now.
        let fix = snap.me.map(|m| east_north(&m.pos));
        if path.last().zip(fix).is_some_and(|(a, b)| (a.0 - b.0).abs() < 0.05 && (a.1 - b.1).abs() < 0.05) {
            path.pop();
        }
        path.push(p);
    }
    let n = path.len().max(2) as f32;
    for (i, pair) in path.windows(2).enumerate() {
        let f = (i + 1) as f32 / (n - 1.0);
        let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * f).round() as u8;
        let color = Color::Rgb(mix(PATH_OLD.0, PATH_NEW.0), mix(PATH_OLD.1, PATH_NEW.1), mix(PATH_OLD.2, PATH_NEW.2));
        dots.line(map.dot(pair[0]), map.dot(pair[1]), TRAIL, color, None);
    }
    if let (Some(c), Some(deg)) = (me_dot, heading(snap)) {
        // Dots run down the screen, so north (0 degrees) is -y.
        let toward = |deg: f32, len: f32| (deg.to_radians().sin() * len, -deg.to_radians().cos() * len);
        let (sx, sy) = toward(deg, 3.0);
        let (tx, ty) = toward(deg, ARROW_DOTS);
        let tip = (c.0 + tx, c.1 + ty);
        dots.line((c.0 + sx, c.1 + sy), tip, VIEW, AMBER, None);
        for side in [-1.0, 1.0] {
            let (bx, by) = toward(deg + 180.0 + side * ARROW_BARB.to_degrees(), ARROW_BARB_DOTS);
            dots.line(tip, (tip.0 + bx, tip.1 + by), VIEW, AMBER, None);
        }
    }

    // Where the cubes are, and the nearest still there.
    let in_plot = |(x, y): (i32, i32)| x >= 0 && y >= 0 && x < cols && y < rows;
    let cube_cells: Vec<(usize, (i32, i32))> = snap.cubes.iter().enumerate().map(|(i, c)| (i, cell_of(map.dot(east_north(&c.pos))))).collect();
    let nearest = snap
        .cubes
        .iter()
        .enumerate()
        .filter(|(_, c)| c.state.is_open_to_take() && c.distance_m.is_some())
        .min_by(|a, b| a.1.distance_m.unwrap_or(f32::MAX).total_cmp(&b.1.distance_m.unwrap_or(f32::MAX)))
        .map(|(i, _)| i)
        .filter(|&i| in_plot(cube_cells[i].1));
    if let (Some(i), Some(me)) = (nearest, me_dot) {
        dots.line(me, map.dot(east_north(&snap.cubes[i].pos)), TARGET, AMBER, Some((2, 2)));
    }

    // Callouts, nearest first, where they cover nothing else; their leader lines go in with the dots.
    let me_cell = me_dot.map(cell_of);
    let mut taken: Vec<Cells> = me_cell.map(|(x, y)| Cells { x: x - 2, y: y - 1, w: 5, h: 3 }).into_iter().collect();
    for &(i, (x, y)) in &cube_cells {
        if snap.cubes[i].state.is_open_to_take() && in_plot((x, y)) {
            taken.push(Cells { x: x - 2, y, w: 5, h: 1 });
        }
    }
    let mut wanted: Vec<usize> = cube_cells.iter().filter(|(i, cell)| snap.cubes[*i].state.is_open_to_take() && in_plot(*cell)).map(|(i, _)| *i).collect();
    wanted.sort_by(|a, b| snap.cubes[*a].distance_m.unwrap_or(f32::MAX).total_cmp(&snap.cubes[*b].distance_m.unwrap_or(f32::MAX)));
    let mut boxes = Vec::new();
    for i in wanted.into_iter().take(MAX_CALLOUTS) {
        let lines = callout_lines(snap, i, &snap.cubes[i], blink);
        let (w, h) = (lines.iter().map(CalloutLine::width).max().unwrap_or(0) + 3, lines.len() as i32);
        let (mx, my) = cube_cells[i].1;
        let candidates = [(mx + 3, my - h - 1), (mx - 3 - w, my - h - 1), (mx + 3, my + 2), (mx - 3 - w, my + 2), (mx + 3, my - h / 2), (mx - 3 - w, my - h / 2)];
        let marker = Cells { x: mx - 2, y: my, w: 5, h: 1 };
        let free = candidates.iter().map(|&(x, y)| Cells { x, y, w, h }).find(|b| b.inside(cols, rows) && taken.iter().filter(|t| **t != marker).all(|t| !b.overlaps(Cells { x: t.x - 1, w: t.w + 2, ..*t })));
        let Some(b) = free else { continue };
        taken.push(b);
        // The leader runs from the cube to the nearest point of the box.
        let from = map.dot(east_north(&snap.cubes[i].pos));
        let to = (from.0.clamp(b.x as f32 * 2.0, (b.x + b.w) as f32 * 2.0 - 1.0), from.1.clamp(b.y as f32 * 4.0, (b.y + b.h) as f32 * 4.0 - 1.0));
        dots.line(from, to, LEADERS, LEADER, None);
        boxes.push((i, b, lines));
    }

    dots.write(plot, buf);
    let fg = |c: Color| Style::new().fg(c);

    // The send range label, outside the ring to the north east.
    if let Some(c) = me_dot {
        let label = "SEND RANGE 100 M";
        let (x, y) = cell_of((c.0 + reach * 0.72 + 2.0, c.1 - reach * 0.72 - 2.0));
        let b = Cells { x, y, w: label.chars().count() as i32, h: 1 };
        if b.inside(cols, rows) && taken.iter().all(|t| !b.overlaps(*t)) {
            let (ax, ay) = abs((x, y));
            put(buf, plot, ax, ay, label, fg(AMBER_DIM));
        }
    }

    // Spots where cubes were seen before, unless a cube stands there now.
    for s in &snap.spots {
        if snap.cubes.iter().any(|cube| cube.pos.planar_distance(&s.pos) < cube_core::spots::SAME_SPOT_UNITS) {
            continue;
        }
        let cell = cell_of(map.dot(east_north(&s.pos)));
        if in_plot(cell) {
            let (x, y) = abs(cell);
            put(buf, plot, x, y, "▪", fg(GREY));
        }
    }

    // Cubes: opened ones first, then the ones still there, nearest last so it stays on top.
    let mut order: Vec<usize> = (0..snap.cubes.len()).collect();
    order.sort_by_key(|&i| (snap.cubes[i].state.is_open_to_take(), std::cmp::Reverse((snap.cubes[i].distance_m.unwrap_or(f32::MAX) * 10.0) as i64)));
    let mut off_plot = Vec::new();
    for i in order {
        let cube = &snap.cubes[i];
        let cell = cube_cells[i].1;
        if !in_plot(cell) {
            if cube.state.is_open_to_take() {
                off_plot.push(i);
            }
            continue;
        }
        let glyph = Elevation::of(cube.height_m).unwrap_or(Elevation::Level).glyph();
        let (x, y) = abs(cell);
        put(buf, plot, x, y, glyph, fg(cube_color(cube)).add_modifier(Modifier::BOLD));
        layout.cubes.push((i, (x as u16, y as u16)));
    }
    if let Some((x, y)) = me_cell.filter(|c| in_plot(*c)) {
        let (x, y) = abs((x, y));
        put(buf, plot, x, y, "●", fg(AMBER).add_modifier(Modifier::BOLD));
    }

    // Brackets around the nearest cube and its distance on the line to it.
    if let Some(i) = nearest {
        let (x, y) = abs(cube_cells[i].1);
        for (dx, dy, s) in [(-2, -1, "┌"), (2, -1, "┐"), (-2, 1, "└"), (2, 1, "┘")] {
            put(buf, plot, x + dx, y + dy, s, fg(AMBER));
        }
        if let (Some(me), Some(d)) = (me_cell, snap.cubes[i].distance_m) {
            let tag = format!(" {d:.1} ");
            let (cx, cy) = cube_cells[i].1;
            let mid = ((me.0 + cx) / 2 + 1, (me.1 + cy) / 2);
            let b = Cells { x: mid.0, y: mid.1, w: tag.chars().count() as i32, h: 1 };
            let clear = |b: Cells| b.inside(cols, rows) && !b.overlaps(Cells { x: cx - 2, y: cy - 1, w: 5, h: 3 }) && !b.overlaps(Cells { x: me.0, y: me.1, w: 1, h: 1 });
            let b = if clear(b) { Some(b) } else { Some(Cells { x: mid.0 - b.w - 1, ..b }).filter(|b| clear(*b)) };
            if let Some(b) = b {
                let (ax, ay) = abs((b.x, b.y));
                put(buf, plot, ax, ay, &tag, Style::new().fg(Color::Black).bg(AMBER).bold());
            }
        }
    }

    // The callout boxes.
    for (i, b, lines) in &boxes {
        let (x, y) = abs((b.x, b.y));
        for (row, line) in lines.iter().enumerate() {
            let ry = y + row as i32;
            put(buf, plot, x, ry, &" ".repeat(b.w as usize), Style::new().bg(BOX));
            put(buf, plot, x, ry, "▌", Style::new().fg(cube_color(&snap.cubes[*i])).bg(BOX));
            let mut cx = x + 2;
            for (s, style) in &line.left {
                put(buf, plot, cx, ry, s, style.bg(style.bg.unwrap_or(BOX)));
                cx += s.chars().count() as i32;
            }
            if let Some((s, style)) = &line.right {
                put(buf, plot, x + b.w - 1 - s.chars().count() as i32, ry, s, style.bg(BOX));
            }
        }
        layout.callouts.push((*i, Rect::new(x as u16, y as u16, b.w as u16, b.h as u16)));
    }

    // Arrows on the edge toward cubes past it, with what lies there.
    for i in off_plot {
        let cube = &snap.cubes[i];
        let d = map.dot(east_north(&cube.pos));
        let (dx, dy) = (d.0 - dw / 2.0, d.1 - dh / 2.0);
        let len = dx.hypot(dy).max(1e-3);
        let (ux, uy) = (dx / len, dy / len);
        let k = ((dw / 2.0 - 2.0) / ux.abs().max(1e-6)).min((dh / 2.0 - 4.0) / uy.abs().max(1e-6));
        let (ex, ey) = cell_of((dw / 2.0 + ux * k, dh / 2.0 + uy * k));
        let (ex, ey) = (ex.clamp(0, cols - 1), ey.clamp(0, rows - 1));
        let style = fg(cube_color(cube)).add_modifier(Modifier::BOLD);
        let (ax, ay) = abs((ex, ey));
        put(buf, plot, ax, ay, arrow(ux.atan2(-uy).to_degrees()), style);
        let mut label = format!("CUBE {} {}", cube_label(i), distance_text(cube));
        if let Some(b) = cube.bearing {
            label.push_str(&format!(" {}", compass(b)));
        }
        if cube.state == CubeState::OutOfView {
            label.push_str(", STILL THERE");
        }
        let w = label.chars().count() as i32;
        let lx = if ex > cols / 2 { ex - 1 - w } else { ex + 2 };
        let b = Cells { x: lx.clamp(0, (cols - w).max(0)), y: ey, w, h: 1 };
        if boxes.iter().all(|(_, o, _)| !b.overlaps(*o)) {
            let (ax, ay) = abs((b.x, b.y));
            put(buf, plot, ax, ay, &label, style);
        }
        layout.arrows.push(i);
    }

    // The labels in the margins, every major line unless they would crowd.
    for &(k, y) in &ys {
        let row = (y / 4.0).floor() as i32;
        if k % 2 == 0 && (0..rows).contains(&row) {
            let (_, ay) = abs((0, row));
            put(buf, area, i32::from(plot.right()) + 1, ay, &axis_label(k as f32 * step, 'N', 'S'), fg(AMBER));
        }
    }
    let mut free_from = i32::MIN;
    for &(k, x) in &xs {
        let label = axis_label(k as f32 * step, 'E', 'W');
        let w = label.chars().count() as i32;
        let col = (x / 2.0).floor() as i32 - w / 2;
        if k % 2 == 0 && col >= 0 && col + w <= cols && col > free_from {
            put(buf, area, i32::from(plot.x) + col, i32::from(plot.bottom()), &label, fg(AMBER));
            free_from = col + w + 1;
        }
    }
    layout
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use cube_core::parser::PositionSource;
    use cube_core::snapshot::{Fix, TrailPoint};

    fn view(x: f32, y: f32, state: CubeState) -> CubeView {
        CubeView {
            id: 1,
            template: 1_200_039,
            zone: None,
            evidence: None,
            pos: Vec3::new(x, y, 1_000.0),
            state,
            opener: None,
            opened_by_you: false,
            first_seen_ms: 28_000,
            state_ms: 0,
            age_ms: None,
            distance_m: Some(58.3),
            height_m: Some(6.0),
            bearing: Some(31.0),
            estimated: false,
        }
    }

    pub(crate) fn walking_north() -> RadarSnapshot {
        RadarSnapshot {
            now_ms: 40_000,
            me: Some(Fix { pos: Vec3::new(0.0, 0.0, 1_000.0), t_ms: 40_000, source: PositionSource::Buff }),
            trail: vec![
                TrailPoint { t_ms: 4_000, pos: Vec3::new(0.0, 4_000.0, 1_000.0) },
                TrailPoint { t_ms: 38_000, pos: Vec3::new(0.0, 500.0, 1_000.0) },
                TrailPoint { t_ms: 40_000, pos: Vec3::new(0.0, 0.0, 1_000.0) },
            ],
            cubes: vec![view(3_000.0, -5_000.0, CubeState::Visible)],
            ..Default::default()
        }
    }

    fn render(snap: &RadarSnapshot, w: u16, h: u16) -> (Buffer, Layout) {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        let you = snap.me.map(|m| east_north(&m.pos));
        let layout = draw(snap, MapFrame { center: you.unwrap_or_default(), radius_m: 150.0 }, you, true, area, &mut buf);
        (buf, layout)
    }

    fn text(buf: &Buffer) -> String {
        let a = buf.area;
        (a.y..a.bottom()).map(|y| (a.x..a.right()).map(|x| buf[(x, y)].symbol()).collect::<String>() + "\n").collect()
    }

    #[test]
    fn height_and_trend_read_plainly() {
        assert_eq!(Elevation::of(Some(6.0)), Some(Elevation::Above));
        assert_eq!(Elevation::of(Some(-2.5)), Some(Elevation::Below));
        assert_eq!(Elevation::of(Some(1.5)), Some(Elevation::Level));
        assert_eq!(Elevation::of(None), None);
        let snap = walking_north();
        // 36 s ago you stood 40 m south: the cube (30 m east, 50 m north) was 94.9 m away, now 58.3 m.
        let t = trend_m(&snap, &snap.cubes[0]).unwrap();
        assert!((t + 36.6).abs() < 0.2, "{t}");
        assert_eq!(trend_text(t), ("CLOSER 37".to_string(), GREEN));
        assert_eq!(trend_text(3.2).0, "AWAY 3");
        assert_eq!(trend_text(0.4).0, "STEADY");
        let mut young = walking_north();
        young.trail.remove(0);
        assert_eq!(trend_m(&young, &young.cubes[0]), None, "two seconds of path is no trend");
        assert_eq!(axis_label(100.0, 'N', 'S'), "100N");
        assert_eq!(axis_label(-50.0, 'E', 'W'), "50W");
        assert_eq!(age_text(&snap.cubes[0], 40_000), "0:12");
    }

    fn trail(points: &[(u64, f32, f32)]) -> RadarSnapshot {
        let (t, x, y) = *points.last().unwrap();
        RadarSnapshot {
            now_ms: t,
            me: Some(Fix { pos: Vec3::new(x, y, 1_000.0), t_ms: t, source: PositionSource::Buff }),
            trail: points.iter().map(|&(t_ms, x, y)| TrailPoint { t_ms, pos: Vec3::new(x, y, 1_000.0) }).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn heading_is_the_way_you_travel() {
        // 20 m north in 6 s, a fix every 2 s.
        let north = trail(&[(30_000, 0.0, 2_000.0), (32_000, 0.0, 1_300.0), (34_000, 0.0, 600.0), (36_000, 0.0, 0.0)]);
        assert_eq!(heading(&north).map(|h| h.round()), Some(0.0), "moving to smaller y is north");
        let mut stale = north.clone();
        stale.now_ms = 90_000;
        assert_eq!(heading(&stale), None);
        // Stepping around in a fight: 3 to 8 m moves back and forth, never 15 m from where it ended.
        let fight = trail(&[(30_000, 0.0, 0.0), (31_000, 500.0, 300.0), (32_000, -300.0, 200.0), (33_000, 400.0, -500.0), (34_000, 0.0, 300.0)]);
        assert_eq!(heading(&fight), None, "steps in a fight are no direction");
        // 12 m north, then 12 m east round a corner: the halves point 90 degrees apart.
        let corner = trail(&[(30_000, 0.0, 0.0), (32_000, 0.0, -600.0), (34_000, 0.0, -1_200.0), (36_000, 600.0, -1_200.0), (38_000, 1_200.0, -1_200.0)]);
        assert_eq!(heading(&corner), None, "a sharp turn is no direction yet");
        // An arc of 30 degrees still reads as the way you go.
        let arc = trail(&[(30_000, 0.0, 0.0), (33_000, 500.0, -870.0), (36_000, 1_200.0, -1_570.0)]);
        assert!(heading(&arc).is_some_and(|h| (h - 37.0).abs() < 3.0), "{:?}", heading(&arc));
    }

    #[test]
    fn the_arrow_points_the_way_you_travel() {
        let north = trail(&[(30_000, 0.0, 2_000.0), (32_000, 0.0, 1_300.0), (34_000, 0.0, 600.0), (36_000, 0.0, 0.0)]);
        let (buf, _) = render(&north, 80, 30);
        assert_eq!(buf[(37, 14)].symbol(), "●", "{}", text(&buf));
        let amber_dots = |y: u16| (30..45).any(|x| buf[(x, y)].fg == AMBER && buf[(x, y)].symbol().chars().all(|c| ('\u{2801}'..='\u{28FF}').contains(&c)));
        assert!(amber_dots(12) && amber_dots(13), "an arrow above you, north:\n{}", text(&buf));
        assert!(!amber_dots(15) && !amber_dots(16), "and nothing amber below:\n{}", text(&buf));
        let mut standing = north.clone();
        standing.trail.drain(..3);
        let (still, _) = render(&standing, 80, 30);
        assert!(!(11..14).any(|y| (30..45).any(|x| still[(x, y)].fg == AMBER && still[(x, y)].symbol() != "●")), "no arrow without a move:\n{}", text(&still));
    }

    #[test]
    fn draws_you_the_cube_its_callout_and_the_axes() {
        let (buf, layout) = render(&walking_north(), 80, 30);
        let screen = text(&buf);
        // You in the middle of the plot, amber.
        let me = &buf[(37, 14)];
        assert_eq!((me.symbol(), me.fg), ("●", AMBER), "{screen}");
        // The cube north east, a green triangle (6 m above you), with its callout.
        assert_eq!(layout.cubes.len(), 1);
        let (x, y) = layout.cubes[0].1;
        assert_eq!((buf[(x, y)].symbol(), buf[(x, y)].fg), ("▲", GREEN));
        assert_eq!(layout.callouts.len(), 1);
        for want in ["CUBE A", "58.3 M", "NE 031", "▲ +6.0 M", "ABOVE", "CLOSER 37", "AGE 0:12", "100N", "100S", "100W", "100E"] {
            assert!(screen.contains(want), "{want}:\n{screen}");
        }
        assert!(screen.contains('┌') && screen.contains('┘'), "brackets on the nearest");
        assert!(screen.chars().any(|c| ('\u{2801}'..='\u{28FF}').contains(&c)), "Braille lines");
    }

    #[test]
    fn callouts_say_who_opens_and_keep_apart() {
        let mut snap = walking_north();
        let mut other = view(-6_000.0, -3_000.0, CubeState::Opening);
        other.height_m = Some(-4.0);
        other.distance_m = Some(67.1);
        snap.cubes.push(other);
        let (buf, layout) = render(&snap, 110, 36);
        let screen = text(&buf);
        assert_eq!(layout.callouts.len(), 2, "{screen}");
        let (a, b) = (layout.callouts[0].1, layout.callouts[1].1);
        assert!(a.intersection(b).is_empty(), "{a:?} {b:?}");
        assert!(screen.contains("OPENING BY OTHER") && screen.contains("▼ -4.0 M"), "{screen}");
        let red = (b.y..b.bottom()).any(|y| (b.x..b.right()).any(|x| buf[(x, y)].bg == RED));
        assert!(red, "on red");
    }

    #[test]
    fn cubes_past_the_edge_get_an_arrow() {
        let mut snap = walking_north();
        snap.cubes[0].pos = Vec3::new(40_000.0, 0.0, 1_000.0);
        snap.cubes[0].state = CubeState::OutOfView;
        snap.cubes[0].bearing = Some(90.0);
        let (buf, layout) = render(&snap, 80, 30);
        let screen = text(&buf);
        assert!(layout.cubes.is_empty());
        assert_eq!(layout.arrows, vec![0]);
        assert!(screen.contains('→') && screen.contains("CUBE A 58.3 M E, STILL THERE"), "{screen}");
        assert!(screen.contains("SEND RANGE 100 M"), "with no callout in the way: {screen}");
    }

    #[test]
    fn your_dot_glides_and_the_path_follows_it() {
        let snap = walking_north();
        let area = Rect::new(0, 0, 80, 30);
        let mut buf = Buffer::empty(area);
        // Halfway between the last two fixes (5 m south of the newest), the view still centred on the newest.
        draw(&snap, MapFrame { center: (0.0, 0.0), radius_m: 150.0 }, Some((0.0, -2.5)), true, area, &mut buf);
        let dot = (0..30).flat_map(|y| (0..74).map(move |x| (x, y))).find(|&p| buf[p].symbol() == "●").unwrap();
        assert!(dot.1 >= 14, "the dot is not ahead of where it is drawn: {dot:?}");
    }

    #[test]
    fn small_areas_draw_nothing_and_do_not_panic() {
        for (w, h) in [(0, 0), (5, 2), (9, 3), (12, 4)] {
            render(&walking_north(), w, h);
        }
    }
}
