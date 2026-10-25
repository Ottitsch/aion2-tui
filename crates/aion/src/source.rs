//! One capture for every module.
//!
//! The meter, the kina tracker and the cube radar each decode a different slice of the same game traffic, so
//! they share one set of capture threads, one TCP reassembly and one frame decoder. `Parsers` hands every
//! decoded packet to all three parsers, and each module reads its own tracker out of `Shared`.
//!
//! Modules get their data through their own `Feed` (live, replay or demo); this file holds what the live and
//! replay feeds have in common: the shared state, the capture status and the Setup view sections.

use crate::capture::{self, CaptureState, CaptureStatus};
use crate::fmt::{clock_secs, group};
use aion_net::capture_file::read_packets;
use aion_net::net::LinkType;
use aion_net::pipeline::{PacketParser, PipelineStats};
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// TCP flows silent this long are forgotten by the pipeline.
const FLOW_IDLE_MS: u64 = 120_000;

/// Every packet goes to all three parsers; each picks out the opcodes it knows.
#[derive(Default)]
pub struct Parsers {
    pub meter: meter_core::parser::Parser,
    pub kina: kina_core::parser::Parser,
    pub cube: cube_core::parser::Parser,
}

pub enum Event {
    Meter(meter_core::parser::Event),
    Kina(kina_core::parser::Event),
    Cube(cube_core::parser::Event),
}

impl PacketParser for Parsers {
    type Event = Event;

    fn packet(&mut self, t_ms: u64, opcode: u16, body: &[u8], out: &mut dyn FnMut(Event)) {
        self.meter.packet(t_ms, opcode, body, &mut |e| out(Event::Meter(e)));
        self.kina.packet(t_ms, opcode, body, &mut |e| out(Event::Kina(e)));
        self.cube.packet(t_ms, opcode, body, &mut |e| out(Event::Cube(e)));
    }
}

pub type Pipeline = aion_net::pipeline::Pipeline<Parsers>;

/// What the capture threads and the TUI loop share. Locks are held briefly and never nested; the restart
/// lock inside `capture` is the one exception and is always taken first.
pub struct Shared {
    pub ports: Vec<u16>,
    pub status: Mutex<CaptureStatus>,
    pub pipeline: Mutex<Pipeline>,
    pub meter: Mutex<meter_core::combat::Tracker>,
    pub kina: Mutex<kina_core::tracker::Tracker>,
    pub cube: Mutex<cube_core::tracker::Tracker>,
    pub capture: CaptureState,
    pub log_path: PathBuf,
    /// Set by `--record`: every captured frame also goes to this file.
    pub recorder: std::sync::OnceLock<crate::record::Recorder>,
}

impl Shared {
    pub fn new(
        ports: Vec<u16>,
        meter: meter_core::combat::Tracker,
        kina: kina_core::tracker::Tracker,
        cube: cube_core::tracker::Tracker,
        log_path: PathBuf,
    ) -> Arc<Shared> {
        Arc::new(Shared {
            pipeline: Mutex::new(Pipeline::new(ports.clone())),
            ports,
            status: Mutex::new(CaptureStatus::Off),
            meter: Mutex::new(meter),
            kina: Mutex::new(kina),
            cube: Mutex::new(cube),
            capture: CaptureState::default(),
            log_path,
            recorder: std::sync::OnceLock::new(),
        })
    }

    pub fn set_status(&self, status: CaptureStatus) {
        *self.status.lock() = status;
    }

    /// Hands decoded events to their trackers, each tracker locked once and in packet order.
    pub fn dispatch(&self, events: &mut Vec<Event>, self_hint: Option<meter_core::parser::EntityId>) {
        if events.iter().any(|e| matches!(e, Event::Meter(_))) {
            let mut tracker = self.meter.lock();
            for e in events.iter() {
                if let Event::Meter(e) = e {
                    tracker.push(e);
                }
            }
            // The party scope vote stands in for the local player until the first self record, so a meter
            // started mid dungeon still flags the right row.
            tracker.set_self_hint(self_hint);
        }
        if events.iter().any(|e| matches!(e, Event::Kina(_))) {
            let mut tracker = self.kina.lock();
            for e in events.iter() {
                if let Event::Kina(e) = e {
                    tracker.push(e);
                }
            }
        }
        if events.iter().any(|e| matches!(e, Event::Cube(_))) {
            let mut tracker = self.cube.lock();
            for e in events.iter() {
                if let Event::Cube(e) = e {
                    tracker.push(e);
                }
            }
        }
        events.clear();
    }

    /// Opens the adapters again on a thread of its own; the TUI keeps drawing meanwhile.
    pub fn restart(self: &Arc<Self>) {
        let shared = self.clone();
        let spawned = std::thread::Builder::new().name("capture start".into()).spawn(move || capture::start(&shared));
        if let Err(e) = spawned {
            self.set_status(CaptureStatus::Error { message: format!("could not start capture: {e}") });
        }
    }

    /// Once per TUI tick: forgets idle flows and drops back to waiting when the game went quiet. The trackers
    /// are ticked by their modules.
    pub fn step(&self, wall_ms: u64) {
        let now = self.capture.now(wall_ms);
        let has_flow = {
            let mut pipe = self.pipeline.lock();
            pipe.expire_flows(now, FLOW_IDLE_MS);
            pipe.has_game_flow()
        };
        // The game disconnected or went quiet: back to waiting, and the next flow announces itself again.
        if !has_flow && self.capture.clear_live() {
            let adapters = self.capture.open_adapter_count();
            let mut status = self.status.lock();
            if matches!(*status, CaptureStatus::Capturing { .. }) {
                tracing::info!("game traffic stopped");
                *status = CaptureStatus::Waiting { adapters };
            }
        }
    }

    pub fn status_line(&self) -> Status {
        capture_status(&self.status.lock())
    }

    /// The Capture and Network adapters sections every module's Setup view starts with.
    pub fn capture_sections(&self) -> Vec<Section> {
        let state = &self.capture;
        let open = state.open_adapters.lock().clone();
        let last = state.last_packet_ms();
        let capture = Section {
            title: "Capture",
            rows: vec![
                ("status".into(), self.status_line().text),
                ("Npcap found".into(), if state.npcap_found.load(Ordering::Relaxed) { "yes".into() } else { "no".into() }),
                ("administrator".into(), if capture::is_admin() { "yes".into() } else { "no (fine unless Npcap is restricted to admins)".into() }),
                ("ports".into(), self.ports.iter().map(u16::to_string).collect::<Vec<_>>().join(", ")),
                ("open adapters".into(), if open.is_empty() { "none".into() } else { open.join(", ") }),
                ("last packet".into(), if last > 0 { clock_secs(last) } else { "none yet".into() }),
                ("log".into(), self.log_path.display().to_string()),
            ],
        };
        let adapters = Section {
            title: "Network adapters",
            rows: state
                .cached_adapters()
                .into_iter()
                .map(|a| {
                    let label = if a.description.trim().is_empty() { a.name.clone() } else { a.description.trim().to_string() };
                    let used = if a.usable { "listening" } else if a.connected { "skipped, no address" } else { "skipped, disconnected" };
                    (label, format!("{used}{}", if a.is_loopback { ", loopback" } else { "" }))
                })
                .collect(),
        };
        vec![capture, adapters]
    }
}

/// How the status line is coloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Good,
    Waiting,
    Bad,
}

#[derive(Debug, Clone)]
pub struct Status {
    pub text: String,
    pub health: Health,
}

/// A titled block of label and value rows for the Setup views.
pub struct Section {
    pub title: &'static str,
    pub rows: Vec<(String, String)>,
}

pub fn capture_status(status: &CaptureStatus) -> Status {
    match status {
        CaptureStatus::Capturing { adapter } => Status { text: format!("capturing on {adapter}"), health: Health::Good },
        CaptureStatus::Waiting { adapters } => Status {
            text: format!("waiting for game traffic on {adapters} adapter{}", if *adapters == 1 { "" } else { "s" }),
            health: Health::Waiting,
        },
        CaptureStatus::NpcapMissing => Status { text: "Npcap is not installed".into(), health: Health::Bad },
        CaptureStatus::Error { message } => Status { text: message.clone(), health: Health::Bad },
        CaptureStatus::Off => Status { text: "starting capture".into(), health: Health::Waiting },
    }
}

/// The transport rows of a module's Decoder section, followed by the module's own parser rows.
pub fn transport_rows(st: &PipelineStats) -> Vec<(String, String)> {
    vec![
        ("packets seen".into(), group(st.packets_seen)),
        ("game flows".into(), st.flows.to_string()),
        ("frames / bundles".into(), format!("{} / {}", group(st.frames.frames), group(st.frames.bundles))),
        ("resyncs / tcp gaps".into(), format!("{} / {}", st.frames.resyncs, st.tcp_gaps)),
    ]
}

/// Saved captures played through the one pipeline and all three trackers in one go at start. The modules take
/// their tracker (and what it finished) from here; nothing is written to disk.
pub struct Replay {
    pub stats: PipelineStats,
    pub parsers: Parsers,
    pub meter: meter_core::combat::Tracker,
    pub meter_finished: Vec<meter_core::snapshot::Snapshot>,
    pub kina: kina_core::tracker::Tracker,
    pub kina_finished: Vec<kina_core::snapshot::SessionSnapshot>,
    pub cube: cube_core::tracker::Tracker,
    pub files: usize,
    pub span_ms: u64,
    pub last_ms: u64,
}

impl Replay {
    pub fn load(
        paths: &[PathBuf],
        ports: Vec<u16>,
        mut meter: meter_core::combat::Tracker,
        mut kina: kina_core::tracker::Tracker,
        mut cube: cube_core::tracker::Tracker,
    ) -> anyhow::Result<Replay> {
        let mut pipeline = Pipeline::new(ports);
        let (mut meter_finished, mut kina_finished) = (Vec::new(), Vec::new());
        let mut events: Vec<Event> = Vec::new();
        let (mut first_ms, mut last_ms, mut last_tick) = (u64::MAX, 0u64, 0u64);
        for path in paths {
            let bytes = std::fs::read(path).map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
            read_packets(&bytes, |link_code, t_ms, data| {
                let Some(link) = LinkType::from_pcap(link_code) else { return };
                first_ms = first_ms.min(t_ms);
                last_ms = last_ms.max(t_ms);
                pipeline.push(link, t_ms, data, &mut |e| events.push(e));
                // A tick every 250 ms of capture time, as live capture does.
                if t_ms.saturating_sub(last_tick) >= 250 {
                    meter.tick(t_ms);
                    kina.tick(t_ms);
                    cube.tick(t_ms);
                    last_tick = t_ms;
                    meter_finished.extend(meter.drain_finished());
                    kina_finished.extend(kina.drain_finished());
                }
                let hint = pipeline.parser.meter.stats.hint_self_id;
                for e in events.drain(..) {
                    match e {
                        Event::Meter(e) => {
                            meter.push(&e);
                            meter.set_self_hint(hint);
                        }
                        Event::Kina(e) => kina.push(&e),
                        Event::Cube(e) => cube.push(&e),
                    }
                }
            })?;
        }
        meter_finished.extend(meter.drain_finished());
        kina_finished.extend(kina.drain_finished());
        // The alerts happened in the past; the cube log keeps them.
        cube.drain_alerts();
        let span_ms = last_ms.saturating_sub(first_ms.min(last_ms));
        let stats = pipeline.stats.clone();
        Ok(Replay {
            stats,
            parsers: std::mem::take(&mut pipeline.parser),
            meter,
            meter_finished,
            kina,
            kina_finished,
            cube,
            files: paths.len(),
            span_ms,
            last_ms,
        })
    }

    /// A replay on the default port with fresh default trackers, for tests.
    #[cfg(test)]
    pub fn with_defaults(paths: &[PathBuf]) -> anyhow::Result<Replay> {
        Replay::load(
            paths,
            vec![aion_net::pipeline::DEFAULT_PORT],
            meter_core::combat::Tracker::default(),
            kina_core::tracker::Tracker::new(Default::default()),
            cube_core::tracker::Tracker::new(Default::default(), Default::default()),
        )
    }

    pub fn status(&self) -> Status {
        let minutes = self.span_ms / 60_000;
        Status { text: format!("replay: {} file{}, {minutes} min", self.files, if self.files == 1 { "" } else { "s" }), health: Health::Good }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The local captures (git ignored, so the tests skip without them).
    fn captures() -> Vec<PathBuf> {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../captures");
        let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
        let mut files: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "pcap")).collect();
        files.sort();
        files
    }

    /// One shared reassembly must give each tracker exactly what its own pipeline would: the same fights, the same
    /// kina sessions, the same radar.
    #[test]
    fn one_pipeline_matches_three_separate_ones() {
        let files = captures();
        if files.is_empty() {
            eprintln!("no captures found, skipping");
            return;
        }
        for path in files {
            let combined = Replay::with_defaults(std::slice::from_ref(&path)).unwrap();

            let mut meter_pipe = meter_core::pipeline::Pipeline::new(vec![aion_net::pipeline::DEFAULT_PORT]);
            let mut kina_pipe = kina_core::pipeline::Pipeline::new(vec![aion_net::pipeline::DEFAULT_PORT]);
            let mut cube_pipe = cube_core::pipeline::Pipeline::new(vec![aion_net::pipeline::DEFAULT_PORT]);
            let mut meter = meter_core::combat::Tracker::default();
            let mut kina = kina_core::tracker::Tracker::new(Default::default());
            let mut cube = cube_core::tracker::Tracker::new(Default::default(), Default::default());
            let (mut meter_done, mut kina_done) = (Vec::new(), Vec::new());
            let (mut last_tick, mut last_ms) = (0u64, 0u64);
            let bytes = std::fs::read(&path).unwrap();
            read_packets(&bytes, |code, t_ms, data| {
                let Some(link) = LinkType::from_pcap(code) else { return };
                last_ms = last_ms.max(t_ms);
                let (mut me, mut ke, mut ce) = (Vec::new(), Vec::new(), Vec::new());
                meter_pipe.push(link, t_ms, data, &mut |e| me.push(e));
                kina_pipe.push(link, t_ms, data, &mut |e| ke.push(e));
                cube_pipe.push(link, t_ms, data, &mut |e| ce.push(e));
                if t_ms.saturating_sub(last_tick) >= 250 {
                    meter.tick(t_ms);
                    kina.tick(t_ms);
                    cube.tick(t_ms);
                    last_tick = t_ms;
                    meter_done.extend(meter.drain_finished());
                    kina_done.extend(kina.drain_finished());
                }
                let hint = meter_pipe.parser.stats.hint_self_id;
                for e in me {
                    meter.push(&e);
                    meter.set_self_hint(hint);
                }
                ke.iter().for_each(|e| kina.push(e));
                ce.iter().for_each(|e| cube.push(e));
            })
            .unwrap();
            meter_done.extend(meter.drain_finished());
            kina_done.extend(kina.drain_finished());

            let name = path.display();
            assert_eq!(combined.meter_finished, meter_done, "{name}: meter encounters");
            assert_eq!(combined.meter.snapshot(), meter.snapshot(), "{name}: live meter encounter");
            assert_eq!(combined.kina_finished, kina_done, "{name}: kina sessions");
            assert_eq!(combined.kina.snapshot(), kina.snapshot(), "{name}: live kina session");
            assert_eq!(combined.cube.snapshot(combined.last_ms), cube.snapshot(last_ms), "{name}: radar");
            assert_eq!(combined.stats.frames.frames, meter_pipe.stats.frames.frames, "{name}: frames");
            assert_eq!(combined.parsers.kina.stats.kina_records, kina_pipe.parser.stats.kina_records, "{name}: kina records");
        }
    }

    /// Opens the real adapters for a few seconds and prints what the Setup views would show. Needs Npcap; only
    /// reads: `cargo test -p aion live_capture_smoke -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_capture_smoke() {
        let shared = Shared::new(
            vec![aion_net::pipeline::DEFAULT_PORT],
            meter_core::combat::Tracker::default(),
            kina_core::tracker::Tracker::new(Default::default()),
            cube_core::tracker::Tracker::new(Default::default(), Default::default()),
            PathBuf::from("aion.log"),
        );
        shared.restart();
        for _ in 0..12 {
            std::thread::sleep(std::time::Duration::from_millis(250));
            shared.step(crate::now_ms());
        }
        println!("status: {}", shared.status_line().text);
        for section in shared.capture_sections() {
            println!("[{}]", section.title);
            for (k, v) in section.rows {
                println!("  {k}: {v}");
            }
        }
        let pipe = shared.pipeline.lock();
        println!("packets {}, frames {}, meter damage records {}, kina records {}, cube object spawns {}", pipe.stats.packets_seen, pipe.stats.frames.frames, pipe.parser.meter.stats.damage_records, pipe.parser.kina.stats.kina_records, pipe.parser.cube.stats.object_spawns);
        drop(pipe);
        println!("meter self: {:?}, kina character: {:?}", shared.meter.lock().self_name(), shared.kina.lock().character().map(|c| c.name.clone()));
        shared.capture.stop();
    }

    /// The numbers the standalone tools were verified against on the four Global EU captures of 11 October 2026
    /// (their READMEs): every fight, the kina balance, and the object that looks like an Altgard cube.
    #[test]
    fn all_captures_give_the_verified_numbers() {
        let files = captures();
        if files.len() < 4 {
            eprintln!("the four local captures are not there, skipping");
            return;
        }
        for (path, fights) in files.iter().zip([4, 2, 9, 9]) {
            let mut replay = Replay::with_defaults(std::slice::from_ref(path)).unwrap();
            replay.meter.tick(replay.last_ms + 10 * 60_000);
            let mut all = std::mem::take(&mut replay.meter_finished);
            all.extend(replay.meter.drain_finished());
            assert_eq!(all.len(), fights, "{}: encounters", path.display());
        }

        let replay = Replay::with_defaults(&files).unwrap();
        let mut sessions = replay.kina_finished.clone();
        sessions.extend(replay.kina.snapshot());
        let net: i64 = sessions.iter().map(|s| s.net).sum();
        let first = sessions.iter().find_map(|s| s.start_balance);
        let last = sessions.iter().rev().find_map(|s| s.balance);
        assert_eq!((first, last, net), (Some(6_796_955), Some(8_308_193), 1_511_238), "kina over the four captures");

        let radar = replay.cube.snapshot(replay.last_ms);
        assert!(radar.counts.cubes_seen >= 1, "the 1301199 object counts as a cube: {:?}", radar.counts);
        assert!(radar.log.iter().any(|l| l.template == 1_301_199 && l.map_id == Some(1110)), "{:?}", radar.log);
    }
}
