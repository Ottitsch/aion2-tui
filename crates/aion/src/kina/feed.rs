//! Where the kina views get their data: live capture, a replay of saved captures, or the demo generator.
//!
//! All three answer the same questions (the live session, the character, the balance, past sessions), so
//! the views never care which one runs. Only live capture writes the history to disk. Capture itself belongs
//! to the hub (`source.rs`); the live feed only ticks and reads the kina tracker in `Shared`.

use crate::fmt::{clock_secs, group};
use crate::kina::demo::Demo;
use crate::kina::history::History;
use crate::source::{transport_rows, Replay, Shared};
use aion_net::pipeline::PipelineStats;
use kina_core::parser::ParserStats;
use kina_core::snapshot::{Character, EndReason, SessionSnapshot, SessionSummary};
use kina_core::tracker::{AbyssPoints, DailyKina, Tracker, TrackerConfig};
use std::path::PathBuf;
use std::sync::Arc;

pub use crate::source::{Health, Section, Status};

/// How often the live session is written to disk, so a crash or a forced shutdown loses little.
const LIVE_SAVE_MS: u64 = 30_000;

/// The daily gain limit and the Abyss Points as the game last sent them, with the clock the daily reset is
/// judged by.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Limits {
    pub daily: Option<DailyKina>,
    pub abyss: Option<AbyssPoints>,
    /// Now on the feed's clock: the capture (or wall) clock live, the last packet in a replay.
    pub now_ms: u64,
}

/// The game resets the daily gain limit every day at this hour ("Resets every 08:00"), taken as local time.
pub const DAILY_RESET_HOUR: u32 = 8;

/// The first daily reset after `t_ms`, in Unix ms.
pub fn next_reset(t_ms: u64) -> u64 {
    use chrono::{Local, NaiveDate, TimeZone};
    let Some(t) = i64::try_from(t_ms).ok().and_then(|ms| Local.timestamp_millis_opt(ms).single()) else { return u64::MAX };
    let reset_on = |day: NaiveDate| day.and_hms_opt(DAILY_RESET_HOUR, 0, 0).and_then(|at| Local.from_local_datetime(&at).earliest());
    let next = reset_on(t.date_naive()).filter(|r| *r > t).or_else(|| t.date_naive().succ_opt().and_then(reset_on));
    next.map(|r| r.timestamp_millis() as u64).unwrap_or(u64::MAX)
}

impl Limits {
    fn of(tracker: &Tracker, now_ms: u64) -> Limits {
        Limits { daily: tracker.daily_kina(), abyss: tracker.abyss_points(), now_ms }
    }

    /// Kina counted toward today's limit: the last counter, or 0 once a daily reset passed since the game sent
    /// it. `None` until the game sent one (a loading screen or an open world kina drop).
    pub fn daily_counted(&self) -> Option<u64> {
        let d = self.daily?;
        Some(if self.now_ms >= next_reset(d.server_ms) { 0 } else { d.counted })
    }

    /// When the counter resets next.
    pub fn next_reset_ms(&self) -> u64 {
        next_reset(self.now_ms)
    }
}

pub trait Feed {
    /// Advances clocks and files finished sessions. Called every 250 ms with the wall clock.
    fn step(&mut self, now_ms: u64);
    /// Ends the session and starts a fresh one from the current balance.
    fn new_session(&mut self, now_ms: u64);
    /// The live session, or the last one while none runs.
    fn current(&self) -> Option<SessionSnapshot>;
    fn character(&self) -> Option<Character>;
    fn balance(&self) -> Option<u64>;
    fn limits(&self) -> Limits;
    /// Past sessions, newest first.
    fn history(&self) -> Vec<SessionSummary>;
    fn session(&self, id: u64) -> Option<SessionSnapshot>;
    fn delete(&mut self, id: u64) -> Result<(), String>;
    fn status(&self) -> Status;
    fn diagnostics(&self) -> Vec<Section>;
    /// Starts capture again (live only).
    fn retry(&mut self) {}
    /// Ends the live session on the way out and returns it.
    fn shutdown(&mut self) -> Option<SessionSnapshot>;
    /// Whether the history is kept on disk.
    fn persistent(&self) -> bool {
        false
    }
}

/// Transport and kina parser counters, shared by the live and the replay feed.
fn decoder_section(st: &PipelineStats, ps: &ParserStats) -> Section {
    let n = |v: u64| group(v);
    let mut rows = transport_rows(st);
    rows.extend([
        ("heartbeats".into(), n(ps.heartbeats)),
        ("zone changes".into(), n(ps.zone_changes)),
        ("self records".into(), format!("{} ({} rejected)", ps.self_records, ps.self_rejected)),
        ("full inventories".into(), format!("{} ({} without kina)", ps.inventories, ps.inventories_without_kina)),
        ("inventory updates".into(), n(ps.item_updates)),
        ("kina records".into(), n(ps.kina_records)),
        ("other currency records".into(), n(ps.currency_records)),
        ("daily limit records".into(), format!("{} ({} rejected)", ps.daily_kina_records, ps.limit_rejected)),
        ("abyss point records".into(), n(ps.abyss_point_records)),
    ]);
    Section { title: "Decoder", rows }
}

fn tracker_section(tracker: &Tracker) -> Section {
    let mut rows = vec![
        ("character".into(), tracker.character().map(|c| format!("{} (server {}, level {})", c.name, c.server, c.level)).unwrap_or_else(|| "not identified yet".into())),
        ("kina".into(), tracker.balance().map(group).unwrap_or_else(|| "unknown until a loading screen".into())),
        (
            "daily gain limit".into(),
            tracker.daily_kina().map(|d| format!("{} counted at server time {}", group(d.counted), clock_secs(d.server_ms))).unwrap_or_else(|| "not sent yet".into()),
        ),
        ("abyss points".into(), tracker.abyss_points().map(|a| format!("{} of {}", group(a.points), group(a.cap))).unwrap_or_else(|| "not sent yet".into())),
    ];
    for (key, seen) in tracker.currencies() {
        rows.push((format!("currency 0x{key:08X}"), format!("{} at {}", group(seen.count), clock_secs(seen.t_ms))));
    }
    Section { title: "Inventory", rows }
}

/// Opens the session history (filing a session a crash left behind) and a tracker that numbers on from it.
pub fn open_history(config: TrackerConfig, data_dir: &std::path::Path) -> (Tracker, History) {
    let mut history = History::open(data_dir.join("sessions"));
    history.recover_live();
    let mut tracker = Tracker::new(config);
    tracker.set_next_session_id(history.max_id() + 1);
    tracing::info!("{} kina sessions in history, next id {}", history.len(), history.max_id() + 1);
    (tracker, history)
}

/// Live capture on the configured ports, with the history on disk.
pub struct LiveFeed {
    shared: Arc<Shared>,
    history: History,
    data_dir: PathBuf,
    last_tick_ms: u64,
    last_live_save_ms: u64,
}

impl LiveFeed {
    /// `history` is the one `open_history` returned with the tracker that went into `shared`.
    pub fn new(shared: Arc<Shared>, history: History, data_dir: PathBuf, now_ms: u64) -> LiveFeed {
        LiveFeed { shared, history, data_dir, last_tick_ms: 0, last_live_save_ms: now_ms }
    }

    fn store(&mut self, finished: Vec<SessionSnapshot>) {
        if finished.is_empty() {
            return;
        }
        for snap in finished {
            let who = snap.character.as_ref().map(|c| c.name.as_str()).unwrap_or("unidentified");
            match self.history.push(&snap) {
                Ok(true) => tracing::info!("kina session {} stored: {who}, {} min, net {}", snap.id, snap.duration_ms / 60_000, snap.net),
                Ok(false) => tracing::debug!("kina session {} skipped: {who}, no kina changes", snap.id),
                Err(e) => tracing::warn!("could not store kina session {}: {e}", snap.id),
            }
        }
        self.history.clear_live();
    }
}

impl Feed for LiveFeed {
    fn step(&mut self, wall_ms: u64) {
        let now = self.shared.capture.now(wall_ms).max(self.last_tick_ms);
        self.last_tick_ms = now;
        let due = wall_ms.saturating_sub(self.last_live_save_ms) >= LIVE_SAVE_MS;
        let (finished, live) = {
            let mut tracker = self.shared.kina.lock();
            tracker.tick(now);
            let live = if due { tracker.snapshot().filter(|s| s.active && s.changes > 0) } else { None };
            (tracker.drain_finished(), live)
        };
        self.store(finished);
        if let Some(live) = live {
            self.last_live_save_ms = wall_ms;
            if let Err(e) = self.history.save_live(&live) {
                tracing::warn!("could not save the live kina session: {e}");
            }
        }
    }

    fn new_session(&mut self, wall_ms: u64) {
        let now = self.shared.capture.now(wall_ms);
        let finished = {
            let mut tracker = self.shared.kina.lock();
            tracker.reset(now);
            tracker.drain_finished()
        };
        self.store(finished);
        tracing::info!("new kina session started by hand");
    }

    fn current(&self) -> Option<SessionSnapshot> {
        self.shared.kina.lock().snapshot()
    }

    fn character(&self) -> Option<Character> {
        self.shared.kina.lock().character().cloned()
    }

    fn balance(&self) -> Option<u64> {
        self.shared.kina.lock().balance()
    }

    fn limits(&self) -> Limits {
        Limits::of(&self.shared.kina.lock(), self.last_tick_ms)
    }

    fn history(&self) -> Vec<SessionSummary> {
        self.history.summaries(usize::MAX)
    }

    /// The live or last session first (it may not be on disk yet), then the files.
    fn session(&self, id: u64) -> Option<SessionSnapshot> {
        self.current().filter(|s| s.id == id).or_else(|| self.history.get(id))
    }

    fn delete(&mut self, id: u64) -> Result<(), String> {
        self.history.delete(id).map_err(|e| e.to_string())
    }

    fn status(&self) -> Status {
        self.shared.status_line()
    }

    fn diagnostics(&self) -> Vec<Section> {
        let mut sections = self.shared.capture_sections();
        {
            let pipe = self.shared.pipeline.lock();
            sections.push(decoder_section(&pipe.stats, &pipe.parser.kina.stats));
        }
        sections.push(tracker_section(&self.shared.kina.lock()));
        sections.push(Section {
            title: "Files",
            rows: vec![
                ("sessions".into(), format!("{} ({} saved)", self.history.dir().display(), self.history.len())),
                ("data dir".into(), self.data_dir.display().to_string()),
            ],
        });
        sections
    }

    fn retry(&mut self) {
        self.shared.restart();
    }

    fn shutdown(&mut self) -> Option<SessionSnapshot> {
        let finished = {
            let mut tracker = self.shared.kina.lock();
            tracker.finish(EndReason::AppExit);
            tracker.drain_finished()
        };
        let last = finished.last().cloned();
        self.store(finished);
        last
    }

    fn persistent(&self) -> bool {
        true
    }
}

/// Saved captures, already played through the tracker by `Replay::load`. Nothing is written to disk.
pub struct ReplayFeed {
    tracker: Tracker,
    finished: Vec<SessionSnapshot>,
    stats: PipelineStats,
    parser: ParserStats,
    status: Status,
}

impl ReplayFeed {
    /// Takes the kina tracker out of the replay.
    pub fn new(replay: &mut Replay) -> ReplayFeed {
        let mut finished = std::mem::take(&mut replay.kina_finished);
        finished.retain(History::worth_keeping);
        let tracker = std::mem::replace(&mut replay.kina, Tracker::new(TrackerConfig::default()));
        ReplayFeed { tracker, finished, stats: replay.stats.clone(), parser: replay.parsers.kina.stats.clone(), status: replay.status() }
    }
}

impl Feed for ReplayFeed {
    fn step(&mut self, _now_ms: u64) {}

    fn new_session(&mut self, _now_ms: u64) {
        let now = self.tracker.last_seen_ms();
        self.tracker.reset(now);
        self.finished.extend(self.tracker.drain_finished().into_iter().filter(History::worth_keeping));
    }

    fn current(&self) -> Option<SessionSnapshot> {
        self.tracker.snapshot()
    }

    fn character(&self) -> Option<Character> {
        self.tracker.character().cloned()
    }

    fn balance(&self) -> Option<u64> {
        self.tracker.balance()
    }

    fn limits(&self) -> Limits {
        Limits::of(&self.tracker, self.tracker.last_seen_ms())
    }

    fn history(&self) -> Vec<SessionSummary> {
        self.finished.iter().rev().map(SessionSnapshot::summary).collect()
    }

    fn session(&self, id: u64) -> Option<SessionSnapshot> {
        self.finished.iter().find(|s| s.id == id).cloned().or_else(|| self.current().filter(|s| s.id == id))
    }

    fn delete(&mut self, id: u64) -> Result<(), String> {
        self.finished.retain(|s| s.id != id);
        Ok(())
    }

    fn status(&self) -> Status {
        self.status.clone()
    }

    fn diagnostics(&self) -> Vec<Section> {
        vec![decoder_section(&self.stats, &self.parser), tracker_section(&self.tracker)]
    }

    fn shutdown(&mut self) -> Option<SessionSnapshot> {
        self.tracker.snapshot()
    }
}

impl Feed for Demo {
    fn step(&mut self, now_ms: u64) {
        Demo::step(self, now_ms);
    }

    fn new_session(&mut self, now_ms: u64) {
        self.reset(now_ms);
    }

    fn current(&self) -> Option<SessionSnapshot> {
        Demo::current(self)
    }

    fn character(&self) -> Option<Character> {
        Demo::character(self)
    }

    fn balance(&self) -> Option<u64> {
        Demo::balance(self)
    }

    fn limits(&self) -> Limits {
        Limits::of(self.tracker(), self.tracker().last_seen_ms())
    }

    fn history(&self) -> Vec<SessionSummary> {
        self.summaries()
    }

    fn session(&self, id: u64) -> Option<SessionSnapshot> {
        self.get(id)
    }

    fn delete(&mut self, id: u64) -> Result<(), String> {
        Demo::delete(self, id);
        Ok(())
    }

    fn status(&self) -> Status {
        Status { text: "demo data".into(), health: Health::Good }
    }

    fn diagnostics(&self) -> Vec<Section> {
        vec![Section { title: "Demo", rows: vec![("source".into(), "a scripted farming loop, see src/kina/demo.rs".into())] }]
    }

    fn shutdown(&mut self) -> Option<SessionSnapshot> {
        Demo::current(self)
    }
}

#[cfg(test)]
mod limit_tests {
    use super::*;
    use chrono::{Local, TimeZone};

    fn local(d: u32, h: u32, m: u32) -> u64 {
        Local.with_ymd_and_hms(2026, 10, d, h, m, 0).earliest().unwrap().timestamp_millis() as u64
    }

    #[test]
    fn the_daily_counter_resets_at_eight() {
        assert_eq!(next_reset(local(26, 7, 59)), local(26, 8, 0));
        assert_eq!(next_reset(local(26, 8, 0)), local(27, 8, 0), "at eight the next reset is tomorrow's");
        assert_eq!(next_reset(local(26, 23, 30)), local(27, 8, 0));
        let sent = Limits { daily: Some(DailyKina { counted: 192_339, server_ms: local(25, 15, 50) }), abyss: None, now_ms: local(25, 16, 0) };
        assert_eq!(sent.daily_counted(), Some(192_339));
        assert_eq!(Limits { now_ms: local(26, 7, 59), ..sent }.daily_counted(), Some(192_339), "still yesterday's day");
        assert_eq!(Limits { now_ms: local(26, 8, 1), ..sent }.daily_counted(), Some(0), "reset at eight");
        assert_eq!(Limits::default().daily_counted(), None);
    }
}
