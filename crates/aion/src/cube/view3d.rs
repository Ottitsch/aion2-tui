//! The 3D view: the radar's scene (your path, the 100 m send ring, the cubes, the spots) drawn in perspective
//! with Braille dots, so it runs in the terminal like every other view.
//!
//! A small software renderer: an orbit camera projects metres (x east, y north, z up) onto the dot grid of the
//! view (two dots per cell across, four down, so dots are about square), and everything is drawn far to near so
//! nearer things win a cell's colour. Cubes are shaded octahedra filled dot by dot, standing on a stem down to
//! your ground level so their height reads at a glance.

use crate::cube::app::cube_label;
use cube_core::geo::Vec3;
use cube_core::snapshot::{CubeState, CubeView, RadarSnapshot};
use ratatui::style::Color;

/// Vertical field of view, 50 degrees.
const FOV: f64 = 0.872_664_626;
const NEAR: f64 = 0.5;
const MIN_PITCH: f64 = 0.08;
/// Just short of straight down, where the camera's right vector is undefined.
const MAX_PITCH: f64 = 1.553;
const MIN_DIST: f64 = 8.0;
const MAX_DIST: f64 = 5_000.0;
/// The game sends cubes from about this far (planar).
const SEND_RADIUS_M: f64 = 100.0;
/// Radians per second of the auto orbit.
const SPIN_RATE: f64 = 0.3;

const GRID: Color = Color::Rgb(58, 66, 82);
const RING: Color = Color::Rgb(168, 132, 40);
const PATH: Color = Color::Rgb(45, 212, 191);
const SPOT: Color = Color::Rgb(167, 139, 250);
const SPOT_STEM: Color = Color::Rgb(96, 82, 150);
const ME: Color = Color::Rgb(34, 211, 238);
const TEXT: Color = Color::Rgb(226, 232, 240);
const MUTED: Color = Color::Rgb(139, 151, 171);

/// Metres east, north and up from game units (centimetres, north = -y).
pub fn metres(p: &Vec3) -> [f64; 3] {
    [f64::from(p.x) / 100.0, -f64::from(p.y) / 100.0, f64::from(p.z) / 100.0]
}

/// The orbit camera of the 3D view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera3d {
    /// Turn around the target; 0 looks north.
    pub yaw: f64,
    /// Elevation above the ground plane; `MAX_PITCH` looks straight down.
    pub pitch: f64,
    /// Distance from the target in metres.
    pub dist: f64,
    /// Where the camera looks; `None` follows you.
    pub target: Option<[f64; 3]>,
    pub top: bool,
    saved: (f64, f64),
    pub spin: bool,
    pub show_path: bool,
    pub show_spots: bool,
}

impl Default for Camera3d {
    fn default() -> Self {
        Camera3d { yaw: 0.5, pitch: 0.72, dist: 230.0, target: None, top: false, saved: (0.5, 0.72), spin: false, show_path: true, show_spots: true }
    }
}

impl Camera3d {
    pub fn rotate(&mut self, yaw: f64, pitch: f64) {
        self.top = false;
        self.yaw += yaw;
        self.pitch = (self.pitch + pitch).clamp(MIN_PITCH, MAX_PITCH);
    }

    pub fn zoom(&mut self, factor: f64) {
        self.dist = (self.dist * factor).clamp(MIN_DIST, MAX_DIST);
    }

    /// Moves the target along the ground: `right` and `ahead` in metres as seen from the camera.
    pub fn pan(&mut self, from: [f64; 3], right: f64, ahead: f64) {
        let (sy, cy) = self.yaw.sin_cos();
        // The camera looks along (-sin yaw, cos yaw) on the ground; right is (cos yaw, sin yaw).
        self.target = Some([from[0] + cy * right - sy * ahead, from[1] + sy * right + cy * ahead, from[2]]);
    }

    pub fn toggle_top(&mut self) {
        if self.top {
            (self.yaw, self.pitch) = self.saved;
            self.top = false;
        } else {
            self.saved = (self.yaw, self.pitch);
            (self.yaw, self.pitch) = (0.0, MAX_PITCH);
            self.top = true;
        }
    }

    /// Back to the starting view, following you; what is shown stays as chosen.
    pub fn reset(&mut self) {
        *self = Camera3d { show_path: self.show_path, show_spots: self.show_spots, ..Camera3d::default() };
    }

    /// Advances the auto orbit by `dt` seconds.
    pub fn animate(&mut self, dt: f64) {
        if self.spin {
            self.yaw += SPIN_RATE * dt;
        }
    }

    /// Compass bearing the camera looks along, in degrees.
    pub fn heading(&self) -> f64 {
        (-self.yaw.to_degrees()).rem_euclid(360.0)
    }

    /// Tilt above the horizon in degrees.
    pub fn tilt(&self) -> f64 {
        self.pitch.to_degrees()
    }
}

/// Projects metres onto the dot grid of a `w` by `h` dot view, y up.
pub struct Projector {
    eye: [f64; 3],
    f: [f64; 3],
    r: [f64; 3],
    u: [f64; 3],
    focal: f64,
    w: f64,
    h: f64,
}

fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

impl Projector {
    pub fn new(cam: &Camera3d, target: [f64; 3], w: f64, h: f64) -> Projector {
        let (sp, cp) = cam.pitch.sin_cos();
        let (sy, cy) = cam.yaw.sin_cos();
        let o = [cp * sy, -cp * cy, sp];
        let eye = [target[0] + o[0] * cam.dist, target[1] + o[1] * cam.dist, target[2] + o[2] * cam.dist];
        let f = [-o[0], -o[1], -o[2]];
        let rl = f[0].hypot(f[1]).max(1e-9);
        let r = [f[1] / rl, -f[0] / rl, 0.0];
        let u = [r[1] * f[2] - r[2] * f[1], r[2] * f[0] - r[0] * f[2], r[0] * f[1] - r[1] * f[0]];
        Projector { eye, f, r, u, focal: (h / 2.0) / (FOV / 2.0).tan(), w, h }
    }

    /// Dot position (x right, y up) and depth, or `None` behind the camera.
    pub fn project(&self, p: [f64; 3]) -> Option<(f64, f64, f64)> {
        let v = [p[0] - self.eye[0], p[1] - self.eye[1], p[2] - self.eye[2]];
        let z = dot3(v, self.f);
        if z < NEAR {
            return None;
        }
        Some(((self.w - 1.0) / 2.0 + self.focal * dot3(v, self.r) / z, (self.h - 1.0) / 2.0 + self.focal * dot3(v, self.u) / z, z))
    }
}

/// A text label at a dot position.
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub x: f64,
    pub y: f64,
    pub text: String,
    pub color: Color,
}

/// What the 3D view draws: dot batches in drawing order, then labels on top.
#[derive(Debug, Default)]
pub struct Scene {
    pub batches: Vec<(Color, Vec<(f64, f64)>)>,
    pub labels: Vec<Label>,
    /// Width and height in dots.
    pub w: f64,
    pub h: f64,
    pub empty: bool,
}

impl Scene {
    fn dot(&mut self, color: Color, x: f64, y: f64) {
        if !(0.0..=self.w - 1.0).contains(&x) || !(0.0..=self.h - 1.0).contains(&y) {
            return;
        }
        match self.batches.last_mut() {
            Some((c, points)) if *c == color => points.push((x, y)),
            _ => self.batches.push((color, vec![(x, y)])),
        }
    }

    /// A 2D line in dots; `every` > 1 leaves gaps (dotted).
    fn line2d(&mut self, color: Color, a: (f64, f64), b: (f64, f64), every: usize) {
        let n = (b.0 - a.0).abs().max((b.1 - a.1).abs()).ceil().clamp(1.0, 4_000.0) as usize;
        for i in (0..=n).step_by(every.max(1)) {
            let t = i as f64 / n as f64;
            self.dot(color, (a.0 + (b.0 - a.0) * t).round(), (a.1 + (b.1 - a.1) * t).round());
        }
    }

    /// A 3D line, cut in `pieces` so a piece behind the camera only hides itself.
    fn line3d(&mut self, p: &Projector, color: Color, a: [f64; 3], b: [f64; 3], pieces: usize, every: usize) {
        let at = |t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
        for i in 0..pieces {
            let (t0, t1) = (i as f64 / pieces as f64, (i + 1) as f64 / pieces as f64);
            if let (Some(s), Some(e)) = (p.project(at(t0)), p.project(at(t1))) {
                self.line2d(color, (s.0, s.1), (e.0, e.1), every);
            }
        }
    }

    /// A filled triangle in dots.
    fn triangle(&mut self, color: Color, a: (f64, f64), b: (f64, f64), c: (f64, f64)) {
        let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
        if area.abs() < 1e-6 {
            self.line2d(color, a, b, 1);
            self.line2d(color, b, c, 1);
            return;
        }
        let (x0, x1) = (a.0.min(b.0).min(c.0).floor().max(0.0), a.0.max(b.0).max(c.0).ceil().min(self.w - 1.0));
        let (y0, y1) = (a.1.min(b.1).min(c.1).floor().max(0.0), a.1.max(b.1).max(c.1).ceil().min(self.h - 1.0));
        let edge = |p: (f64, f64), q: (f64, f64), x: f64, y: f64| ((q.0 - p.0) * (y - p.1) - (q.1 - p.1) * (x - p.0)) * area.signum();
        let mut y = y0;
        while y <= y1 {
            let mut x = x0;
            while x <= x1 {
                if edge(a, b, x, y) >= -0.5 && edge(b, c, x, y) >= -0.5 && edge(c, a, x, y) >= -0.5 {
                    self.dot(color, x, y);
                }
                x += 1.0;
            }
            y += 1.0;
        }
        // The outline closes the gaps a thin sliver leaves between dots.
        self.line2d(color, a, b, 1);
        self.line2d(color, b, c, 1);
        self.line2d(color, c, a, 1);
    }

    fn disc(&mut self, color: Color, c: (f64, f64), r: f64) {
        let r2 = r * r;
        let mut y = -r.ceil();
        while y <= r.ceil() {
            let mut x = -r.ceil();
            while x <= r.ceil() {
                if x * x + y * y <= r2 {
                    self.dot(color, (c.0 + x).round(), (c.1 + y).round());
                }
                x += 1.0;
            }
            y += 1.0;
        }
    }

    fn label(&mut self, x: f64, y: f64, text: impl Into<String>, color: Color) {
        self.labels.push(Label { x, y, text: text.into(), color });
    }
}

fn base_color(c: &CubeView) -> (u8, u8, u8) {
    match c.state {
        CubeState::Visible => (245, 197, 66),
        CubeState::OutOfView => (148, 163, 184),
        CubeState::Opening | CubeState::Opened if c.opened_by_you => (74, 222, 128),
        _ => (248, 113, 113),
    }
}

fn shade((r, g, b): (u8, u8, u8), k: f64) -> Color {
    let s = |v: u8| (f64::from(v) * k).round().clamp(0.0, 255.0) as u8;
    Color::Rgb(s(r), s(g), s(b))
}

/// A grid step that draws about eight lines across `span` metres.
fn grid_step(span: f64) -> f64 {
    [5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1_000.0, 2_500.0].into_iter().find(|s| span / s <= 8.0).unwrap_or(5_000.0)
}

/// `81m ▲9` for a cube label.
fn cube_text(i: usize, c: &CubeView) -> String {
    let mut text = cube_label(i);
    if let Some(d) = c.distance_m {
        text.push_str(&format!(" {}{d:.0}m", if c.estimated { "~" } else { "" }));
    }
    match c.height_m {
        Some(h) if h > 3.0 => text.push_str(&format!(" ▲{h:.0}")),
        Some(h) if h < -3.0 => text.push_str(&format!(" ▼{:.0}", -h)),
        _ => {}
    }
    text
}

/// Everything at a depth, drawn far to near.
enum Thing<'a> {
    Spot([f64; 3], u32),
    Cube(usize, &'a CubeView, [f64; 3]),
    Me([f64; 3]),
}

/// The scene for a view of `cols` by `rows` cells.
pub fn render(cam: &Camera3d, snap: &RadarSnapshot, cols: u16, rows: u16) -> Scene {
    let (w, h) = (f64::from(cols.max(1)) * 2.0, f64::from(rows.max(1)) * 4.0);
    let mut scene = Scene { w, h, ..Default::default() };
    let me = snap.me.map(|m| metres(&m.pos));
    let target = cam
        .target
        .or(me)
        .or_else(|| snap.cubes.first().map(|c| metres(&c.pos)))
        .or_else(|| snap.spots.first().map(|s| metres(&s.pos)))
        .or_else(|| snap.trail.last().map(|t| metres(&t.pos)));
    let Some(target) = target else {
        scene.empty = true;
        return scene;
    };
    let p = Projector::new(cam, target, w, h);
    let gz = me.map_or(target[2], |m| m[2]);

    // Ground grid: sparse dots, sparser with distance, inside a disc around the target so its edge does not
    // run off as a busy horizon.
    let extent = (cam.dist * 1.1).max(50.0);
    let step = grid_step(extent * 2.0);
    let (x0, x1) = (((target[0] - extent) / step).ceil() * step, ((target[0] + extent) / step).floor() * step);
    let (y0, y1) = (((target[1] - extent) / step).ceil() * step, ((target[1] + extent) / step).floor() * step);
    let chord = |offset: f64| (extent * extent - offset * offset).max(0.0).sqrt();
    let mut x = x0;
    while x <= x1 {
        let half = chord(x - target[0]);
        grid_line(&mut scene, &p, [x, target[1] - half, gz], [x, target[1] + half, gz], cam.dist);
        x += step;
    }
    let mut y = y0;
    while y <= y1 {
        let half = chord(y - target[1]);
        grid_line(&mut scene, &p, [target[0] - half, y, gz], [target[0] + half, y, gz], cam.dist);
        y += step;
    }
    if let Some((nx, ny, _)) = p.project([target[0], target[1] + extent, gz]) {
        scene.label(nx, ny, "N", TEXT);
    }

    // The ring inside which the game sends cubes, dashed.
    if let Some(m) = me {
        let segments = 96;
        for i in (0..segments).step_by(2) {
            let (a0, a1) = (i as f64 / segments as f64 * std::f64::consts::TAU, (i + 1) as f64 / segments as f64 * std::f64::consts::TAU);
            let at = |a: f64| [m[0] + a.cos() * SEND_RADIUS_M, m[1] + a.sin() * SEND_RADIUS_M, gz];
            scene.line3d(&p, RING, at(a0), at(a1), 1, 1);
        }
        if let Some((rx, ry, _)) = p.project([m[0], m[1] + SEND_RADIUS_M, gz]) {
            scene.label(rx, ry + 2.0, "100 m", RING);
        }
    }

    if cam.show_path {
        for pair in snap.trail.windows(2) {
            scene.line3d(&p, PATH, metres(&pair[0].pos), metres(&pair[1].pos), 2, 1);
        }
    }

    // Spots, cubes and you, far to near.
    let size = (cam.dist * 0.035).max(1.5);
    let mut things: Vec<(f64, Thing)> = Vec::new();
    if cam.show_spots {
        for s in &snap.spots {
            let near_cube = snap.cubes.iter().any(|c| c.pos.planar_distance(&s.pos) < cube_core::spots::SAME_SPOT_UNITS);
            let at = metres(&s.pos);
            if let (false, Some((_, _, z))) = (near_cube, p.project(at)) {
                things.push((z, Thing::Spot(at, s.sightings)));
            }
        }
    }
    for (i, c) in snap.cubes.iter().enumerate() {
        let at = metres(&c.pos);
        let lifted = [at[0], at[1], at[2] + size * 1.6];
        if let Some((_, _, z)) = p.project(lifted) {
            things.push((z, Thing::Cube(i, c, lifted)));
        }
    }
    if let Some(m) = me {
        if let Some((_, _, z)) = p.project(m) {
            things.push((z, Thing::Me(m)));
        }
    }
    things.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (_, thing) in things {
        match thing {
            Thing::Spot(at, sightings) => {
                scene.line3d(&p, SPOT_STEM, at, [at[0], at[1], gz], 1, 2);
                if let Some((sx, sy, _)) = p.project(at) {
                    scene.triangle(SPOT, (sx - 2.0, sy), (sx, sy + 2.0), (sx + 2.0, sy));
                    scene.triangle(SPOT, (sx - 2.0, sy), (sx, sy - 2.0), (sx + 2.0, sy));
                    if sightings > 1 {
                        scene.label(sx + 3.0, sy + 3.0, format!("×{sightings}"), SPOT);
                    }
                }
            }
            Thing::Cube(i, c, at) => {
                let base = base_color(c);
                scene.line3d(&p, shade(base, 0.55), at, [at[0], at[1], gz], 1, 2);
                draw_octahedron(&mut scene, &p, at, size, base);
                if let Some((lx, ly, _)) = p.project([at[0], at[1], at[2] + size * 2.6]) {
                    let open = c.state.is_open_to_take();
                    scene.label(lx, ly, cube_text(i, c), if open { shade(base, 1.0) } else { MUTED });
                }
            }
            Thing::Me(m) => {
                if let Some((mx, my, z)) = p.project(m) {
                    scene.disc(ME, (mx, my), (p.focal * 1.2 / z).clamp(1.5, 4.0));
                    scene.label(mx, my - 5.0, "you", ME);
                }
            }
        }
    }
    scene
}

/// One grid line in pieces: a dot every 4 near the target, every 8 farther back, none past twice its distance.
fn grid_line(scene: &mut Scene, p: &Projector, a: [f64; 3], b: [f64; 3], dist: f64) {
    const PIECES: usize = 16;
    let at = |t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
    for i in 0..PIECES {
        let (t0, t1) = (i as f64 / PIECES as f64, (i + 1) as f64 / PIECES as f64);
        if let (Some(s), Some(e)) = (p.project(at(t0)), p.project(at(t1))) {
            let depth = (s.2 + e.2) / 2.0;
            if depth < dist * 2.0 {
                scene.line2d(GRID, (s.0, s.1), (e.0, e.1), if depth < dist * 1.2 { 4 } else { 8 });
            }
        }
    }
}

/// A shaded octahedron (the cube marker) at `c`, `s` metres across its waist.
fn draw_octahedron(scene: &mut Scene, p: &Projector, c: [f64; 3], s: f64, base: (u8, u8, u8)) {
    let h = s * 1.4;
    let ring = |k: f64| {
        let a = std::f64::consts::FRAC_PI_4 + k * std::f64::consts::FRAC_PI_2;
        [c[0] + a.cos() * s, c[1] + a.sin() * s, c[2]]
    };
    let v = [[c[0], c[1], c[2] + h], [c[0], c[1], c[2] - h], ring(0.0), ring(1.0), ring(2.0), ring(3.0)];
    let Some(pv) = v.iter().map(|&q| p.project(q)).collect::<Option<Vec<_>>>() else { return };
    let mut faces: Vec<([usize; 3], f64)> = Vec::new();
    for i in 0..4 {
        let (a, b) = (2 + i, 2 + (i + 1) % 4);
        faces.push(([0, a, b], 1.05 - i as f64 * 0.1));
        faces.push(([1, a, b], 0.6 - i as f64 * 0.06));
    }
    faces.sort_by(|x, y| {
        let depth = |f: &[usize; 3]| f.iter().map(|&i| pv[i].2).sum::<f64>();
        depth(&y.0).total_cmp(&depth(&x.0))
    });
    for (f, light) in faces {
        let pt = |i: usize| (pv[i].0, pv[i].1);
        scene.triangle(shade(base, light), pt(f[0]), pt(f[1]), pt(f[2]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cube_core::parser::PositionSource;
    use cube_core::snapshot::Fix;

    fn cube(x: f32, y: f32, state: CubeState) -> CubeView {
        CubeView {
            id: 1,
            template: 1_200_039,
            zone: None,
            evidence: None,
            pos: Vec3::new(x, y, 1_000.0),
            state,
            opener: None,
            opened_by_you: false,
            first_seen_ms: 0,
            state_ms: 0,
            age_ms: None,
            distance_m: Some(50.0),
            height_m: Some(9.0),
            bearing: Some(0.0),
            estimated: false,
        }
    }

    #[test]
    fn the_target_sits_in_the_middle_and_behind_is_hidden() {
        let cam = Camera3d::default();
        let p = Projector::new(&cam, [100.0, 200.0, 10.0], 201.0, 101.0);
        let (x, y, z) = p.project([100.0, 200.0, 10.0]).unwrap();
        assert!((x - 100.0).abs() < 1e-6 && (y - 50.0).abs() < 1e-6, "({x}, {y})");
        assert!((z - cam.dist).abs() < 1e-6);
        let (sy, cy) = cam.yaw.sin_cos();
        let behind = [100.0 + sy * cam.dist * 3.0, 200.0 - cy * cam.dist * 3.0, 10.0 + cam.pitch.sin() * cam.dist * 3.0];
        assert!(p.project(behind).is_none());
    }

    #[test]
    fn top_view_is_north_up_east_right() {
        let mut cam = Camera3d::default();
        cam.toggle_top();
        let p = Projector::new(&cam, [0.0, 0.0, 0.0], 200.0, 100.0);
        let (cx, cy, _) = p.project([0.0, 0.0, 0.0]).unwrap();
        let (_, ny, _) = p.project([0.0, 50.0, 0.0]).unwrap();
        let (ex, _, _) = p.project([50.0, 0.0, 0.0]).unwrap();
        assert!(ny > cy + 10.0, "north is up");
        assert!(ex > cx + 10.0, "east is right");
        cam.toggle_top();
        assert_eq!((cam.yaw, cam.pitch), (Camera3d::default().yaw, Camera3d::default().pitch), "back to the saved view");
    }

    #[test]
    fn the_camera_turns_tilts_zooms_and_pans() {
        let mut cam = Camera3d::default();
        cam.rotate(0.0, 10.0);
        assert_eq!(cam.pitch, MAX_PITCH);
        cam.zoom(1e9);
        assert_eq!(cam.dist, MAX_DIST);
        cam.yaw = 0.0;
        assert_eq!(cam.heading(), 0.0, "yaw 0 looks north");
        cam.pan([0.0, 0.0, 5.0], 10.0, 20.0);
        let t = cam.target.unwrap();
        assert!((t[0] - 10.0).abs() < 1e-9 && (t[1] - 20.0).abs() < 1e-9 && t[2] == 5.0, "{t:?}");
        cam.spin = true;
        cam.animate(2.0);
        assert!((cam.yaw - 0.6).abs() < 1e-9);
        cam.show_path = false;
        cam.reset();
        assert!(!cam.show_path && cam.target.is_none() && cam.dist == Camera3d::default().dist);
    }

    #[test]
    fn draws_you_the_cube_the_ring_and_the_grid() {
        let snap = RadarSnapshot {
            me: Some(Fix { pos: Vec3::new(0.0, 0.0, 1_000.0), t_ms: 0, source: PositionSource::Buff }),
            cubes: vec![cube(0.0, -5_000.0, CubeState::Visible)],
            ..Default::default()
        };
        let scene = render(&Camera3d::default(), &snap, 80, 30);
        assert!(!scene.empty);
        let colors: Vec<Color> = scene.batches.iter().map(|b| b.0).collect();
        for want in [GRID, RING, ME] {
            assert!(colors.contains(&want), "{want:?} in {colors:?}");
        }
        assert!(colors.iter().any(|c| matches!(c, Color::Rgb(r, g, _) if *r > 200 && *g > 150)), "a yellow face");
        let texts: Vec<&str> = scene.labels.iter().map(|l| l.text.as_str()).collect();
        for want in ["N", "100 m", "you", "A 50m ▲9"] {
            assert!(texts.contains(&want), "{want} in {texts:?}");
        }
        assert!(scene.batches.iter().all(|(_, pts)| pts.iter().all(|&(x, y)| (0.0..160.0).contains(&x) && (0.0..120.0).contains(&y))));
    }

    #[test]
    fn nothing_to_look_at_is_an_empty_scene() {
        assert!(render(&Camera3d::default(), &RadarSnapshot::default(), 80, 30).empty);
    }

    #[test]
    fn triangles_fill() {
        let mut scene = Scene { w: 50.0, h: 50.0, ..Default::default() };
        scene.triangle(ME, (10.0, 10.0), (30.0, 10.0), (10.0, 30.0));
        let n: usize = scene.batches.iter().map(|b| b.1.len()).sum();
        assert!(n > 150, "about 200 dots of area, got {n}");
    }
}
