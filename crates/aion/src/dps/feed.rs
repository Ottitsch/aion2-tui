//! Where the DPS views get their data: live capture, a replay of saved captures, or the demo generator.
//!
//! Live works like the aion2dps shell's tick loop: the tracker is ticked with the capture clock and finished
//! encounters go into the history on disk. Capture itself belongs to the hub (`source.rs`).

use crate::dps::demo::Demo;
use crate::dps::history::History;
use crate::fmt::group;
use crate::source::{transport_rows, Replay, Shared};
use aion_net::pipeline::PipelineStats;
use meter_core::combat::{Tracker, TrackerConfig};
use meter_core::parser::ParserStats;
use meter_core::snapshot::{EncounterSummary, EndReason, Snapshot};
use std::sync::Arc;

pub use crate::source::{Health, Section, Status};

pub trait Feed {
    /// Advances clocks and files finished encounters. Called every 100 ms with the wall clock.
    fn step(&mut self, now_ms: u64);
    /// The live encounter, or the last one while none runs.
    fn current(&self) -> Option<Snapshot>;
    /// Past encounters, newest first.
    fn history(&self) -> Vec<EncounterSummary>;
    fn encounter(&self, id: u64) -> Option<Snapshot>;
    fn delete(&mut self, id: u64) -> Result<(), String>;
    /// Ends the running encounter by hand.
    fn reset(&mut self, now_ms: u64);
    fn self_name(&self) -> Option<String>;
    fn status(&self) -> Status;
    fn diagnostics(&self) -> Vec<Section>;
    /// Starts capture again (live only).
    fn retry(&mut self) {}
    /// Whether the history is kept on disk.
    fn persistent(&self) -> bool {
        false
    }
}

/// Transport and meter parser counters, shared by the live and the replay feed.
fn decoder_section(st: &PipelineStats, ps: &ParserStats) -> Section {
    let n = |v: u64| group(v);
    let mut rows = transport_rows(st);
    rows.extend([
        ("parsed packets".into(), n(ps.packets)),
        ("heartbeats".into(), n(ps.heartbeats)),
        ("damage records".into(), format!("{} ({} rejected)", n(ps.damage_records), n(ps.damage_rejected))),
        ("self hint".into(), ps.hint_self_id.map(|id| format!("entity {id}")).unwrap_or_else(|| "none yet".into())),
    ]);
    Section { title: "Decoder", rows }
}

fn tracker_section(tracker: &Tracker) -> Section {
    let snap = tracker.snapshot();
    let rows = vec![
        ("you".into(), tracker.self_name().map(str::to_owned).unwrap_or_else(|| "not identified yet".into())),
        ("map".into(), tracker.map_id().map(|id| id.to_string()).unwrap_or_else(|| "unknown until a loading screen".into())),
        ("encounter".into(), match &snap {
            Some(s) if s.active => format!("#{} live, {} actors", s.id, s.actors.len()),
            Some(s) => format!("#{} ended", s.id),
            None => "none yet".into(),
        }),
    ];
    Section { title: "Meter", rows }
}

/// Opens the encounter history and a tracker that numbers on from it.
pub fn open_history(data_dir: &std::path::Path) -> (Tracker, History) {
    let history = History::open(data_dir.join("encounters"));
    let mut tracker = Tracker::new(TrackerConfig::default());
    tracker.set_next_encounter_id(history.max_id() + 1);
    tracing::info!("{} encounters in history, next id {}", history.len(), history.max_id() + 1);
    (tracker, history)
}

/// Live capture on the configured ports, with the history on disk.
pub struct LiveFeed {
    shared: Arc<Shared>,
    history: History,
    last_tick_ms: u64,
}

impl LiveFeed {
    /// `history` is the one `open_history` returned with the tracker that went into `shared`.
    pub fn new(shared: Arc<Shared>, history: History) -> LiveFeed {
        LiveFeed { shared, history, last_tick_ms: 0 }
    }

    fn store(&mut self, finished: Vec<Snapshot>) {
        for snap in finished {
            let boss = snap.boss.as_ref().map(|b| b.name.as_str()).unwrap_or("no target");
            match self.history.push(&snap) {
                Ok(true) => tracing::info!("encounter {} stored: {boss}, {} s, {} damage", snap.id, snap.duration_ms / 1000, snap.total_damage),
                Ok(false) => tracing::debug!("encounter {} skipped: {boss}, {} ms, {} damage", snap.id, snap.duration_ms, snap.total_damage),
                Err(e) => tracing::warn!("could not store encounter {}: {e}", snap.id),
            }
        }
    }
}

impl Feed for LiveFeed {
    fn step(&mut self, wall_ms: u64) {
        let now = self.shared.capture.now(wall_ms).max(self.last_tick_ms);
        self.last_tick_ms = now;
        let finished = {
            let mut tracker = self.shared.meter.lock();
            tracker.tick(now);
            tracker.drain_finished()
        };
        self.store(finished);
    }

    fn current(&self) -> Option<Snapshot> {
        self.shared.meter.lock().snapshot()
    }

    fn history(&self) -> Vec<EncounterSummary> {
        self.history.summaries(usize::MAX)
    }

    /// The live or last encounter first (it may not be on disk yet), then the files.
    fn encounter(&self, id: u64) -> Option<Snapshot> {
        self.current().filter(|s| s.id == id).or_else(|| self.history.get(id))
    }

    fn delete(&mut self, id: u64) -> Result<(), String> {
        self.history.delete(id).map_err(|e| e.to_string())
    }

    fn reset(&mut self, _now_ms: u64) {
        let finished = {
            let mut tracker = self.shared.meter.lock();
            tracker.reset();
            tracker.drain_finished()
        };
        self.store(finished);
        tracing::info!("encounter reset by hand");
    }

    fn self_name(&self) -> Option<String> {
        self.shared.meter.lock().self_name().map(str::to_owned)
    }

    fn status(&self) -> Status {
        self.shared.status_line()
    }

    fn diagnostics(&self) -> Vec<Section> {
        let mut sections = self.shared.capture_sections();
        {
            let pipe = self.shared.pipeline.lock();
            sections.push(decoder_section(&pipe.stats, &pipe.parser.meter.stats));
        }
        sections.push(tracker_section(&self.shared.meter.lock()));
        sections.push(Section {
            title: "Files",
            rows: vec![("encounters".into(), format!("{} ({} saved)", self.history.dir().display(), self.history.len()))],
        });
        sections
    }

    fn retry(&mut self) {
        self.shared.restart();
    }

    fn persistent(&self) -> bool {
        true
    }
}

/// Saved captures, already played through the tracker by `Replay::load`. Nothing is written to disk.
pub struct ReplayFeed {
    tracker: Tracker,
    /// Newest first.
    finished: Vec<Snapshot>,
    stats: PipelineStats,
    parser: ParserStats,
    status: Status,
}

impl ReplayFeed {
    /// Takes the meter tracker out of the replay. The fight still running at the end of the files is closed,
    /// as an idle meter would.
    pub fn new(replay: &mut Replay) -> ReplayFeed {
        let mut tracker = std::mem::replace(&mut replay.meter, Tracker::new(TrackerConfig::default()));
        let mut finished = std::mem::take(&mut replay.meter_finished);
        tracker.tick(replay.last_ms + 10 * 60_000);
        finished.extend(tracker.drain_finished());
        finished.retain(History::worth_keeping);
        finished.reverse();
        ReplayFeed { tracker, finished, stats: replay.stats.clone(), parser: replay.parsers.meter.stats.clone(), status: replay.status() }
    }
}

impl Feed for ReplayFeed {
    fn step(&mut self, _now_ms: u64) {}

    fn current(&self) -> Option<Snapshot> {
        self.tracker.snapshot().or_else(|| self.finished.first().cloned())
    }

    fn history(&self) -> Vec<EncounterSummary> {
        self.finished.iter().map(Snapshot::summary).collect()
    }

    fn encounter(&self, id: u64) -> Option<Snapshot> {
        self.finished.iter().find(|s| s.id == id).cloned()
    }

    fn delete(&mut self, id: u64) -> Result<(), String> {
        self.finished.retain(|s| s.id != id);
        Ok(())
    }

    fn reset(&mut self, _now_ms: u64) {}

    fn self_name(&self) -> Option<String> {
        self.tracker.self_name().map(str::to_owned)
    }

    fn status(&self) -> Status {
        self.status.clone()
    }

    fn diagnostics(&self) -> Vec<Section> {
        vec![decoder_section(&self.stats, &self.parser), tracker_section(&self.tracker)]
    }
}

impl Feed for Demo {
    fn step(&mut self, now_ms: u64) {
        Demo::step(self, now_ms);
    }

    fn current(&self) -> Option<Snapshot> {
        Demo::current(self)
    }

    fn history(&self) -> Vec<EncounterSummary> {
        self.summaries()
    }

    fn encounter(&self, id: u64) -> Option<Snapshot> {
        self.get(id)
    }

    fn delete(&mut self, id: u64) -> Result<(), String> {
        Demo::delete(self, id);
        Ok(())
    }

    fn reset(&mut self, now_ms: u64) {
        Demo::reset(self, now_ms);
    }

    fn self_name(&self) -> Option<String> {
        Some(Demo::self_name(self).to_string())
    }

    fn status(&self) -> Status {
        Status { text: "demo data".into(), health: Health::Good }
    }

    fn diagnostics(&self) -> Vec<Section> {
        vec![Section { title: "Demo", rows: vec![("source".into(), "a generated boss fight with five players, see src/dps/demo.rs".into())] }]
    }
}

/// Why an encounter ended, as the overlay labels it.
pub fn end_label(reason: Option<EndReason>) -> &'static str {
    match reason {
        None => "live",
        Some(EndReason::BossDied) => "kill",
        Some(EndReason::Wipe) => "wipe",
        Some(EndReason::Idle) => "idle",
        Some(EndReason::ZoneChange) => "zone",
        Some(EndReason::Manual) => "manual",
        Some(EndReason::DummyCap) => "dummy",
    }
}
