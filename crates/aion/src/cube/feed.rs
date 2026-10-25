//! Where the cube views get their data: live capture, a replay of saved captures, or the demo generator.
//!
//! All three answer the same questions (the radar, the alerts since the last step, the status), so the views
//! never care which one runs. Only live capture saves learned cube templates and spots. Capture itself belongs
//! to the hub (`source.rs`); the live feed only ticks and reads the cube tracker in `Shared`.

use crate::cube::store::{SpotStore, TemplateStore};
use crate::fmt::group;
use crate::source::{transport_rows, Replay, Shared};
use aion_net::pipeline::PipelineStats;
use cube_core::cubes::{CubeBook, Evidence};
use cube_core::parser::ParserStats;
use cube_core::snapshot::{Alert, RadarSnapshot};
use cube_core::tracker::{Tracker, TrackerConfig};
use std::path::PathBuf;
use std::sync::Arc;

pub use crate::source::{Health, Section, Status};

pub trait Feed {
    /// Advances clocks and collects alerts. Called every 100 ms with the wall clock.
    fn step(&mut self, now_ms: u64);
    fn snapshot(&self) -> RadarSnapshot;
    /// Alerts raised since the last call, oldest first.
    fn drain_alerts(&mut self) -> Vec<Alert>;
    fn status(&self) -> Status;
    fn diagnostics(&self) -> Vec<Section>;
    /// Starts capture again (live only).
    fn retry(&mut self) {}
    fn shutdown(&mut self) {}
}

/// Transport and cube parser counters, shared by the live and the replay feed.
fn decoder_section(st: &PipelineStats, ps: &ParserStats) -> Section {
    let n = |v: u64| group(v);
    let mut rows = transport_rows(st);
    rows.extend([
        ("heartbeats".into(), n(ps.heartbeats)),
        ("zone changes".into(), n(ps.zone_changes)),
        ("self records".into(), format!("{} ({} rejected)", ps.self_records, ps.self_rejected)),
        ("object spawns".into(), n(ps.object_spawns)),
        ("object removals / openings".into(), format!("{} / {}", n(ps.object_removals), n(ps.opens_done))),
        ("object records rejected".into(), n(ps.object_rejected)),
        ("position fixes / teleports".into(), format!("{} / {}", n(ps.position_fixes), n(ps.teleports))),
        ("buff records without a position".into(), n(ps.buffs_without_position)),
    ]);
    Section { title: "Decoder", rows }
}

fn radar_section(tracker: &Tracker, now_ms: u64) -> Section {
    let s = tracker.snapshot(now_ms);
    let you = match (s.self_id, &s.character) {
        (Some(id), Some(c)) => format!("{} (entity {id})", c.name),
        (Some(id), None) => format!("entity {id}{}", if s.self_guessed { ", guessed from buff records" } else { "" }),
        (None, _) => "not identified yet (needs a loading screen or a few buffs)".into(),
    };
    let fix = match s.me {
        Some(m) => format!("{} s old, from {:?}", now_ms.saturating_sub(m.t_ms) / 1000, m.source),
        None => "none yet".into(),
    };
    let mut rows = vec![
        ("map".into(), s.map.map(|m| format!("{} ({})", m.name, m.id)).unwrap_or_else(|| "unknown until a loading screen".into())),
        ("you".into(), you),
        ("last position fix".into(), fix),
        ("objects in view".into(), s.objects_in_view.to_string()),
        (
            "cubes this run".into(),
            format!("{} seen, {} opened by you, {} by others", s.counts.cubes_seen, s.counts.opened_by_you, s.counts.opened_by_others),
        ),
        ("key openings of unknown objects".into(), s.counts.key_opens_unknown.to_string()),
    ];
    for t in tracker.book().all() {
        let evidence = match t.evidence {
            Evidence::Verified => "verified",
            Evidence::Hypothesis => "hypothesis",
            Evidence::Learned => "learned",
            Evidence::Manual => "--template",
        };
        rows.push((format!("cube template {}", t.template), format!("{} ({evidence})", t.zone.as_deref().unwrap_or("zone unknown"))));
    }
    Section { title: "Radar", rows }
}

/// A tracker that knows the given templates and the spots saved earlier.
pub fn tracker(book: CubeBook, spots: &SpotStore) -> Tracker {
    let mut tracker = Tracker::new(TrackerConfig::default(), book);
    let saved = spots.load();
    tracing::info!("{} cube spots saved earlier", saved.len());
    tracker.set_spots(saved);
    tracker
}

/// Live capture on the configured ports. Learned templates and cube spots are saved to the data dir.
pub struct LiveFeed {
    shared: Arc<Shared>,
    store: TemplateStore,
    spots: SpotStore,
    data_dir: PathBuf,
    last_tick_ms: u64,
    alerts: Vec<Alert>,
}

impl LiveFeed {
    /// The tracker in `shared` should come from `tracker` with the same spot store.
    pub fn new(shared: Arc<Shared>, store: TemplateStore, spots: SpotStore, data_dir: PathBuf) -> LiveFeed {
        LiveFeed { shared, store, spots, data_dir, last_tick_ms: 0, alerts: Vec::new() }
    }
}

impl Feed for LiveFeed {
    fn step(&mut self, wall_ms: u64) {
        let now = self.shared.capture.now(wall_ms).max(self.last_tick_ms);
        self.last_tick_ms = now;
        let (alerts, learned, spots) = {
            let mut tracker = self.shared.cube.lock();
            tracker.tick(now);
            (tracker.drain_alerts(), tracker.drain_learned(), tracker.take_spots_changed())
        };
        if let Some(spots) = spots {
            if let Err(e) = self.spots.save(&spots) {
                tracing::warn!("could not save the cube spots: {e}");
            }
        }
        for a in &alerts {
            tracing::info!("alert {:?}: cube {} template {} at {:?}, {:?} m", a.kind, a.cube.id, a.cube.template, a.cube.pos, a.cube.distance_m);
        }
        self.alerts.extend(alerts);
        for t in learned {
            tracing::info!("learned cube template {} ({:?})", t.template, t.zone);
            if let Err(e) = self.store.add(t) {
                tracing::warn!("could not save the learned template: {e}");
            }
        }
    }

    fn snapshot(&self) -> RadarSnapshot {
        self.shared.cube.lock().snapshot(self.last_tick_ms)
    }

    fn drain_alerts(&mut self) -> Vec<Alert> {
        std::mem::take(&mut self.alerts)
    }

    fn status(&self) -> Status {
        self.shared.status_line()
    }

    fn diagnostics(&self) -> Vec<Section> {
        let mut sections = self.shared.capture_sections();
        {
            let pipe = self.shared.pipeline.lock();
            sections.push(decoder_section(&pipe.stats, &pipe.parser.cube.stats));
        }
        sections.push(radar_section(&self.shared.cube.lock(), self.last_tick_ms));
        sections.push(Section {
            title: "Files",
            rows: vec![
                ("learned templates".into(), format!("{} ({} saved)", self.store.path().display(), self.store.len())),
                ("cube spots".into(), self.spots.path().display().to_string()),
                ("data dir".into(), self.data_dir.display().to_string()),
            ],
        });
        sections
    }

    fn retry(&mut self) {
        self.shared.restart();
    }
}

/// Saved captures, already played through the tracker by `Replay::load`. Nothing is written to disk.
pub struct ReplayFeed {
    tracker: Tracker,
    stats: PipelineStats,
    parser: ParserStats,
    status: Status,
    last_ms: u64,
}

impl ReplayFeed {
    /// Takes the cube tracker out of the replay.
    pub fn new(replay: &mut Replay) -> ReplayFeed {
        let tracker = std::mem::replace(&mut replay.cube, Tracker::new(TrackerConfig::default(), CubeBook::default()));
        ReplayFeed { tracker, stats: replay.stats.clone(), parser: replay.parsers.cube.stats.clone(), status: replay.status(), last_ms: replay.last_ms }
    }
}

impl Feed for ReplayFeed {
    fn step(&mut self, _now_ms: u64) {}

    fn snapshot(&self) -> RadarSnapshot {
        self.tracker.snapshot(self.last_ms)
    }

    fn drain_alerts(&mut self) -> Vec<Alert> {
        Vec::new()
    }

    fn status(&self) -> Status {
        self.status.clone()
    }

    fn diagnostics(&self) -> Vec<Section> {
        vec![decoder_section(&self.stats, &self.parser), radar_section(&self.tracker, self.last_ms)]
    }
}
