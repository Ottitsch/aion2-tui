//! How much time the app spends where, measured on the saved captures through the live code path: every packet
//! goes through the shared pipeline and the trackers as the capture threads do, the hub steps every 100 ms of
//! capture time and draws a tab as the TUI loop does.
//!
//! `cargo test --release -p aion perf_ -- --ignored --nocapture --test-threads 1`
//!
//! On the four captures of 11 October 2026 (a Ryzen 9 3900X): a packet takes 2 us through all three decoders, a
//! step 55 us, a frame 0.4 to 0.9 ms (a quarter of that is ratatui's own cost for a 200 by 55 terminal, whatever
//! is on it), so drawing is most of the app's time, under 1% of one core.

use crate::hub::{self, Hub, Module, MODULES};
use crate::source::{Event, Shared};
use aion_net::capture_file::read_packets;
use aion_net::net::LinkType;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Durations of one kind of work.
#[derive(Default)]
struct Times(Vec<Duration>);

impl Times {
    fn time<T>(&mut self, f: impl FnOnce() -> T) -> T {
        let t = Instant::now();
        let out = f();
        self.0.push(t.elapsed());
        out
    }

    fn line(&mut self, what: &str) -> String {
        if self.0.is_empty() {
            return format!("{what:<28} none");
        }
        self.0.sort();
        let n = self.0.len();
        let total: Duration = self.0.iter().sum();
        let us = |d: Duration| d.as_secs_f64() * 1e6;
        format!(
            "{what:<28} n {n:>8}  mean {:>8.1} us  p50 {:>8.1}  p99 {:>8.1}  max {:>9.1}  total {:>8.1} ms",
            us(total) / n as f64,
            us(self.0[n / 2]),
            us(self.0[(n * 99 / 100).min(n - 1)]),
            us(self.0[n - 1]),
            total.as_secs_f64() * 1e3
        )
    }
}

fn captures() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../captures");
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "pcap")).collect();
    files.sort();
    files
}

/// A hub on live feeds over a `Shared` that nothing captures into, with its files in a fresh temp folder.
fn live_hub(dir: &std::path::Path, now: u64) -> (Hub, std::sync::Arc<Shared>) {
    let (meter, dps_history) = crate::dps::feed::open_history(&dir.join("dps"));
    let (kina, kina_history) = crate::kina::feed::open_history(Default::default(), &dir.join("kina"));
    let templates = crate::cube::store::TemplateStore::open(dir.join("cube").join("templates.json"));
    let mut book = cube_core::cubes::CubeBook::default();
    templates.load_into(&mut book);
    let spots = crate::cube::store::SpotStore::new(dir.join("cube").join("spots.json"));
    let cube = crate::cube::feed::tracker(book, &spots);
    let shared = Shared::new(vec![aion_net::pipeline::DEFAULT_PORT], meter, kina, cube, dir.join("aion.log"));
    let dps_options = crate::dps::app::ViewOptions { hide_names: false, party_only: false, boss_only: false };
    let cube_options = crate::cube::app::ViewOptions { hide_name: false, sound: true, detector: true };
    let kina_options = crate::kina::app::ViewOptions { hide_name: false, goal: 5_000_000 };
    let hub = Hub {
        module: Module::Overview,
        dps: crate::dps::app::App::new(
            Box::new(crate::dps::feed::LiveFeed::new(shared.clone(), dps_history)),
            crate::dps::overlay::Overlay::unmanaged(None),
            dps_options,
            now,
        ),
        cube: crate::cube::app::App::with_speaker(
            Box::new(crate::cube::feed::LiveFeed::new(shared.clone(), templates, spots, dir.join("cube"))),
            cube_options,
            now,
            crate::cube::app::Speaker::SILENT,
        ),
        kina: crate::kina::app::App::new(Box::new(crate::kina::feed::LiveFeed::new(shared.clone(), kina_history, dir.join("kina"), now)), kina_options, now),
        keys: crate::keys::App::new(crate::keys::hold::Engine::new("viiper.exe".into()), false, now),
        live: Some(shared.clone()),
        settings: None,
        game_audio: None,
        hotkeys: None,
        quit: false,
    };
    (hub, shared)
}

/// Counts the bytes the app writes to the terminal, which is what the terminal has to render.
#[derive(Clone, Default)]
struct Counter(std::sync::Arc<std::sync::atomic::AtomicU64>);

impl std::io::Write for Counter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.fetch_add(buf.len() as u64, std::sync::atomic::Ordering::Relaxed);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Each tab for one whole capture on the TUI loop's own schedule (a step every 100 ms, frames every 40 ms while
/// the map animates): how often it draws, what that costs here, and how many bytes go to the terminal.
#[test]
#[ignore]
fn perf_terminal() {
    let Some(path) = captures().pop() else {
        eprintln!("no captures found, skipping");
        return;
    };
    let bytes = std::fs::read(&path).unwrap();
    let (w, h) = (200u16, 55u16);
    let tabs: [(&str, Module, Option<crate::cube::app::MapKind>); 6] = [
        ("Overview, radar", Module::Overview, Some(crate::cube::app::MapKind::Radar)),
        ("Overview, 3D", Module::Overview, Some(crate::cube::app::MapKind::View3d)),
        ("DPS", Module::Dps, None),
        ("Cube, radar", Module::Cube, Some(crate::cube::app::MapKind::Radar)),
        ("Kina", Module::Kina, None),
        ("Keys", Module::Keys, None),
    ];
    for (label, module, map) in tabs {
        let dir = std::env::temp_dir().join(format!("aion-perf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut first = 0u64;
        read_packets(&bytes, |_, t, _| {
            if first == 0 {
                first = t;
            }
        })
        .unwrap();
        let (mut hub, shared) = live_hub(&dir, first);
        hub.module = module;
        if let Some(map) = map {
            hub.cube.open_map(map);
        }
        let counter = Counter::default();
        let backend = ratatui::backend::CrosstermBackend::new(counter.clone());
        let viewport = ratatui::Viewport::Fixed(ratatui::layout::Rect::new(0, 0, w, h));
        let mut term = Terminal::with_options(backend, ratatui::TerminalOptions { viewport }).unwrap();
        let mut draws = Times::default();
        let mut idle = 0usize;
        let mut events: Vec<Event> = Vec::new();
        // `run` in main.rs on a virtual clock: a step when 100 ms passed, a frame every pass, and the next pass
        // after 40 ms while the map animates, else at the next step.
        let (mut clock, mut last_step, mut last_frame, mut last) = (first, 0u64, first, first);
        read_packets(&bytes, |code, t_ms, data| {
            let Some(link) = LinkType::from_pcap(code) else { return };
            while clock <= t_ms {
                if clock.saturating_sub(last_step) >= 100 {
                    hub.step(clock);
                    last_step = clock;
                }
                hub.animate((clock - last_frame) as f64 / 1_000.0);
                last_frame = clock;
                let before = counter.0.load(std::sync::atomic::Ordering::Relaxed);
                draws.time(|| term.draw(|f| hub::draw(f, &mut hub)).unwrap());
                // A frame that changed nothing writes only the colour resets and the cursor hide (26 bytes).
                if counter.0.load(std::sync::atomic::Ordering::Relaxed) - before <= 26 {
                    idle += 1;
                }
                clock += if hub.animating() { 40 } else { 100u64.saturating_sub(clock - last_step).max(1) };
            }
            let hint = {
                let mut pipe = shared.pipeline.lock();
                pipe.push(link, t_ms, data, &mut |e| events.push(e));
                pipe.parser.meter.stats.hint_self_id
            };
            shared.capture.record_packet(t_ms, t_ms);
            if !events.is_empty() {
                shared.dispatch(&mut events, hint);
            }
            last = t_ms;
        })
        .unwrap();
        hub.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
        let secs = (last - first) as f64 / 1_000.0;
        let n = draws.0.len() as f64;
        let cpu: f64 = draws.0.iter().map(Duration::as_secs_f64).sum();
        let written = counter.0.load(std::sync::atomic::Ordering::Relaxed) as f64;
        println!(
            "{label:<16} {:>5.1} frames/s ({:>3.0}% change nothing)  {:>6.2} ms CPU per s ({:>4.2}% of a core)  {:>7.1} KB/s to the terminal",
            n / secs,
            idle as f64 * 100.0 / n,
            cpu * 1e3 / secs,
            cpu * 100.0 / secs,
            written / secs / 1e3
        );
    }
}

#[test]
#[ignore]
fn perf_report() {
    let files = captures();
    if files.is_empty() {
        eprintln!("no captures found, skipping");
        return;
    }
    let (w, h) = (200, 55);
    let mut packets = Times::default();
    let mut steps = Times::default();
    let mut meter_snap = Times::default();
    let mut cube_snap = Times::default();
    let mut kina_snap = Times::default();
    let mut draws: Vec<(Module, Times)> = MODULES.iter().map(|(m, _)| (*m, Times::default())).collect();
    let mut capture_ms = 0u64;
    for path in files {
        let dir = std::env::temp_dir().join(format!("aion-perf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let bytes = std::fs::read(&path).unwrap();
        let mut first = 0u64;
        read_packets(&bytes, |_, t, _| {
            if first == 0 {
                first = t;
            }
        })
        .unwrap();
        let (mut hub, shared) = live_hub(&dir, first);
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let mut events: Vec<Event> = Vec::new();
        let (mut next_step, mut frame, mut last) = (first, 0usize, first);
        read_packets(&bytes, |code, t_ms, data| {
            let Some(link) = LinkType::from_pcap(code) else { return };
            // What a capture thread does with one packet.
            packets.time(|| {
                let hint = {
                    let mut pipe = shared.pipeline.lock();
                    pipe.push(link, t_ms, data, &mut |e| events.push(e));
                    pipe.parser.meter.stats.hint_self_id
                };
                shared.capture.record_packet(t_ms, t_ms);
                if !events.is_empty() {
                    shared.dispatch(&mut events, hint);
                }
            });
            last = t_ms;
            // What the TUI loop does every 100 ms: step every module, draw the open tab.
            while t_ms >= next_step {
                steps.time(|| hub.step(next_step));
                meter_snap.time(|| shared.meter.lock().snapshot());
                cube_snap.time(|| shared.cube.lock().snapshot(next_step));
                kina_snap.time(|| shared.kina.lock().snapshot());
                let slot = frame % draws.len();
                hub.module = draws[slot].0;
                draws[slot].1.time(|| term.draw(|f| hub::draw(f, &mut hub)).unwrap());
                frame += 1;
                next_step += 100;
            }
        })
        .unwrap();
        capture_ms += last - first;
        hub.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }
    println!("{:.0} min of game traffic", capture_ms as f64 / 60_000.0);
    println!("{}", packets.line("packet (pipeline+trackers)"));
    println!("{}", steps.line("hub step"));
    println!("{}", meter_snap.line("  meter snapshot"));
    println!("{}", cube_snap.line("  cube snapshot"));
    println!("{}", kina_snap.line("  kina snapshot"));
    for (module, times) in &mut draws {
        println!("{}", times.line(&format!("draw {module:?}")));
    }
}
