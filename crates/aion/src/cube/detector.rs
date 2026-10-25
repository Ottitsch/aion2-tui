//! The cube detector: while a cube is still there, a beep repeats faster the closer you get, like a parking
//! sensor. The distance is the straight line to the cube with the height in it (a cube 20 m up a cliff right
//! above you is 20 m away, not 0), and the pitch says which way: higher when the cube is above you, lower when
//! below, in between when about level. It follows your position fixes, which come every 0.6 s in a fight and
//! every few seconds walking.

use crate::cube::chart::LEVEL_M;
use cube_core::parser::EntityId;
use cube_core::snapshot::{CubeState, RadarSnapshot};
use std::collections::HashSet;

/// Farther than this (straight line) the detector is quiet: the game sends cubes from about 100 m.
pub const RANGE_M: f32 = 150.0;
/// Beep start to start at 3 m or closer, how much longer per metre farther, and the slowest.
const FASTEST_MS: f32 = 120.0;
const MS_PER_M: f32 = 14.0;
const SLOWEST_MS: f32 = 2_200.0;
pub const PITCH_ABOVE: u32 = 1_760;
pub const PITCH_LEVEL: u32 = 1_320;
pub const PITCH_BELOW: u32 = 880;
/// What `t` plays after the alert: `(straight line metres, height metres, beeps)` from far to near, then above
/// and below.
pub const PREVIEW: [(f32, f32, u32); 7] = [(80.0, 0.0, 2), (40.0, 0.0, 2), (20.0, 0.0, 3), (8.0, 0.0, 4), (3.0, 0.0, 6), (8.0, 8.0, 4), (8.0, -8.0, 4)];

/// The cube the detector beeps for, how far it is in a straight line, and the beep.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tracked {
    pub id: EntityId,
    pub straight_m: f32,
    pub tone: Tone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tone {
    /// Beep start to beep start.
    pub interval_ms: u64,
    pub freq: u32,
}

impl Tone {
    pub fn at(distance_m: f32, height_m: f32) -> Tone {
        let interval = (FASTEST_MS + (distance_m - 3.0).max(0.0) * MS_PER_M).min(SLOWEST_MS);
        let freq = if height_m > LEVEL_M {
            PITCH_ABOVE
        } else if height_m < -LEVEL_M {
            PITCH_BELOW
        } else {
            PITCH_LEVEL
        };
        Tone { interval_ms: interval as u64, freq }
    }
}

/// The straight line distance to a cube, height included, when your position is known.
pub fn straight_m(planar_m: Option<f32>, height_m: Option<f32>) -> Option<f32> {
    planar_m.map(|d| d.hypot(height_m.unwrap_or(0.0)))
}

/// The nearest cube still there (in view or out of view, not being opened) that was not silenced, within range.
pub fn nearest(snap: &RadarSnapshot, quiet: &HashSet<EntityId>) -> Option<Tracked> {
    snap.cubes
        .iter()
        .filter(|c| matches!(c.state, CubeState::Visible | CubeState::OutOfView) && !quiet.contains(&c.id))
        .filter_map(|c| {
            let straight_m = straight_m(c.distance_m, c.height_m)?;
            Some(Tracked { id: c.id, straight_m, tone: Tone::at(straight_m, c.height_m.unwrap_or(0.0)) })
        })
        .min_by(|a, b| a.straight_m.total_cmp(&b.straight_m))
        .filter(|t| t.straight_m <= RANGE_M)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cube_core::geo::Vec3;
    use cube_core::snapshot::CubeView;

    fn cube(id: EntityId, state: CubeState, distance_m: Option<f32>, height_m: Option<f32>) -> CubeView {
        CubeView {
            id,
            template: 1_200_039,
            zone: None,
            evidence: None,
            pos: Vec3::default(),
            state,
            opener: None,
            opened_by_you: false,
            first_seen_ms: 0,
            state_ms: 0,
            age_ms: None,
            distance_m,
            height_m,
            bearing: None,
            estimated: false,
        }
    }

    fn snap(cubes: Vec<CubeView>) -> RadarSnapshot {
        RadarSnapshot { cubes, ..Default::default() }
    }

    #[test]
    fn closer_beeps_faster_and_height_counts() {
        assert_eq!(Tone::at(2.0, 0.0), Tone { interval_ms: 120, freq: PITCH_LEVEL });
        assert!(Tone::at(10.0, 0.0).interval_ms < Tone::at(50.0, 0.0).interval_ms);
        assert_eq!(Tone::at(500.0, 0.0).interval_ms, 2_200, "the slowest");
        // Right below a cube 20 m up: 20 m away, and the high tone says up.
        let d = straight_m(Some(0.0), Some(20.0)).unwrap();
        assert_eq!(d, 20.0);
        assert_eq!(Tone::at(d, 20.0).freq, PITCH_ABOVE);
        assert_eq!(Tone::at(5.0, -6.0).freq, PITCH_BELOW);
        assert_eq!(Tone::at(5.0, 1.5).freq, PITCH_LEVEL, "within 2 m is level");
    }

    #[test]
    fn the_nearest_cube_still_there_is_tracked() {
        let cubes = vec![
            cube(1, CubeState::Visible, Some(60.0), Some(0.0)),
            cube(2, CubeState::OutOfView, Some(10.0), Some(-5.0)),
            cube(3, CubeState::Opening, Some(2.0), Some(0.0)),
            cube(4, CubeState::Opened, Some(1.0), Some(0.0)),
            cube(5, CubeState::Visible, None, None),
        ];
        let quiet = HashSet::new();
        let t = nearest(&snap(cubes.clone()), &quiet).unwrap();
        assert_eq!((t.id, (t.straight_m * 10.0).round(), t.tone), (2, 112.0, Tone::at(11.18, -5.0)), "cube 2: 11.2 m, below");
        let quiet: HashSet<EntityId> = [2].into();
        assert_eq!(nearest(&snap(cubes.clone()), &quiet).map(|t| t.id), Some(1), "silenced, so the next one");
        assert_eq!(nearest(&snap(vec![cube(1, CubeState::Visible, Some(160.0), Some(0.0))]), &HashSet::new()), None, "out of range");
        assert_eq!(nearest(&snap(vec![cube(5, CubeState::Visible, None, None)]), &HashSet::new()), None, "no position, no distance");
    }
}
