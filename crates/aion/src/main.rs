//! aion: the Aion 2 tools in one terminal window.
//!
//! One passive capture feeds the DPS meter, the Hidden Cube alert and the kina tracker; key-mouse's hold buttons
//! run alongside, and the aion2dps app is kept for its in game overlay. Nothing is ever sent to the game.

mod capture;
mod clipboard;
mod cube;
mod dps;
mod files;
mod fmt;
mod game_audio;
mod hotkeys;
mod hub;
mod keys;
mod kina;
mod logging;
mod options;
mod overview;
#[cfg(test)]
mod perf;
mod record;
mod settings;
mod source;

use anyhow::{anyhow, Context};
use aion_net::pipeline::DEFAULT_PORT;
use cube_core::cubes::{CubeBook, CubeTemplate, Evidence};
use fmt::parse_amount;
use hub::{Hub, Module};
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, MouseEventKind};
use ratatui::crossterm::execute;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How often the feeds are read: the cube radar's position arrives about every 0.6 s, so this keeps up with it.
const TICK: Duration = Duration::from_millis(100);
/// Redraw interval while the cube radar's dot glides or its 3D view turns.
const FRAME: Duration = Duration::from_millis(40);

const USAGE: &str = "\
aion: the Aion 2 tools in one terminal: DPS meter, Hidden Cube alert, kina tracker and hold buttons

usage: aion [options]

  --module <name>     start on overview, dps, cube, kina or keys (default overview)
  --port <n>          game server port, repeatable (default 13328, the Global client)
  --hide-name         mask character names (streaming)
  --demo              generated data in every tab, nothing is captured, no keys, no overlay
  --replay <file>...  play saved captures (pcap or pcapng) through every tab instead of capturing
  --record <file>     also save the game traffic to this pcapng file (for finding new data in it)

  DPS                 --no-overlay        do not start the aion2dps overlay app
                      --show-dps-window   leave the aion2dps main window open (hidden by default)
                      --dps-exe <path>    the aion2dps.exe to start for the overlay
  Cube                --no-sound          start with the alert muted (m toggles)
                      --template <id>     treat this object template as a Hidden Cube too, repeatable
  Kina                --goal <amount>     net kina goal for the session, for example 5m or 750k
                      --idle-minutes <n>  end the session after this long without game traffic (default 15)
  Keys                --no-keys           do not start hold buttons (the virtual keyboard and mouse)
                      --viiper <path>     the viiper.exe to use

  -h, --help          this text

keys: F1-F5 (or Alt+1-5) switch between Overview, DPS, Cube, Kina and Keys; q quits everything.
      Inside a module tab, 1-4 or tab switch its views; the footer lists the rest.
";

struct Options {
    module: Module,
    ports: Vec<u16>,
    hide_name: bool,
    demo: bool,
    replay: Vec<PathBuf>,
    overlay: bool,
    show_dps_window: bool,
    dps_exe: Option<PathBuf>,
    sound: bool,
    templates: Vec<u32>,
    goal: u64,
    idle_minutes: u32,
    keys: bool,
    viiper: Option<PathBuf>,
    record: Option<PathBuf>,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn parse_args(args: &[String]) -> anyhow::Result<Option<Options>> {
    let mut o = Options {
        module: Module::Overview,
        ports: Vec::new(),
        hide_name: false,
        demo: false,
        replay: Vec::new(),
        overlay: true,
        show_dps_window: false,
        dps_exe: None,
        sound: true,
        templates: Vec::new(),
        goal: 0,
        idle_minutes: 15,
        keys: true,
        viiper: None,
        record: None,
    };
    let mut i = 0;
    let value = |i: usize, flag: &str| args.get(i).cloned().ok_or_else(|| anyhow!("{flag} needs a value"));
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => return Ok(None),
            "--module" => {
                i += 1;
                let v = value(i, "--module")?;
                o.module = Module::from_name(&v).ok_or_else(|| anyhow!("--module takes overview, dps, cube, kina or keys"))?;
            }
            "--port" => {
                i += 1;
                let p: u16 = value(i, "--port")?.parse().context("--port takes a number from 1 to 65535")?;
                if p == 0 {
                    return Err(anyhow!("--port takes a number from 1 to 65535"));
                }
                o.ports.push(p);
            }
            "--hide-name" => o.hide_name = true,
            "--demo" => o.demo = true,
            "--replay" => {
                while let Some(next) = args.get(i + 1).filter(|a| !a.starts_with("--")) {
                    o.replay.push(PathBuf::from(next));
                    i += 1;
                }
                if o.replay.is_empty() {
                    return Err(anyhow!("--replay needs at least one capture file"));
                }
            }
            "--no-overlay" => o.overlay = false,
            "--show-dps-window" => o.show_dps_window = true,
            "--dps-exe" => {
                i += 1;
                o.dps_exe = Some(PathBuf::from(value(i, "--dps-exe")?));
            }
            "--no-sound" => o.sound = false,
            "--template" => {
                i += 1;
                let t: u32 = value(i, "--template")?.parse().context("--template takes an object template id such as 1200039")?;
                o.templates.push(t);
            }
            "--goal" => {
                i += 1;
                let v = value(i, "--goal")?;
                o.goal = parse_amount(&v).ok_or_else(|| anyhow!("--goal {v:?} is not an amount like 5m or 750000"))?;
            }
            "--idle-minutes" => {
                i += 1;
                o.idle_minutes = value(i, "--idle-minutes")?.parse::<u32>().context("--idle-minutes takes whole minutes")?.clamp(5, 240);
            }
            "--no-keys" => o.keys = false,
            "--record" => {
                i += 1;
                o.record = Some(PathBuf::from(value(i, "--record")?));
            }
            "--viiper" => {
                i += 1;
                o.viiper = Some(PathBuf::from(value(i, "--viiper")?));
            }
            other => return Err(anyhow!("unknown argument {other:?}\n\n{USAGE}")),
        }
        i += 1;
    }
    if o.ports.is_empty() {
        o.ports.push(DEFAULT_PORT);
    }
    if o.demo && !o.replay.is_empty() {
        return Err(anyhow!("--demo and --replay do not go together"));
    }
    if o.record.is_some() && (o.demo || !o.replay.is_empty()) {
        return Err(anyhow!("--record saves live traffic; it does not go with --demo or --replay"));
    }
    Ok(Some(o))
}

/// What each tab reads, and the shared capture behind them when it is live.
struct Feeds {
    dps: Box<dyn dps::feed::Feed>,
    cube: Box<dyn cube::feed::Feed>,
    kina: Box<dyn kina::feed::Feed>,
    shared: Option<std::sync::Arc<source::Shared>>,
}

/// `%APPDATA%\<name>` on Windows, `$XDG_DATA_HOME/<name>` or `~/.local/share/<name>` elsewhere.
fn data_dir(name: &str) -> PathBuf {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return PathBuf::from(appdata).join(name);
    }
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(xdg).join(name);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".local").join("share").join(name)
}

/// `--viiper`, else `bin\viiper.exe` next to this exe or in this workspace, else key-mouse's copy beside it.
fn find_viiper(explicit: Option<PathBuf>) -> PathBuf {
    if let Some(p) = explicit {
        return p;
    }
    let mut candidates = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)) {
        candidates.push(dir.join("viiper.exe"));
        candidates.push(dir.join("bin").join("viiper.exe"));
    }
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    candidates.push(workspace.join("bin").join("viiper.exe"));
    candidates.push(workspace.join("../key-mouse/bin/viiper.exe"));
    candidates.iter().find(|p| p.is_file()).cloned().unwrap_or_else(|| workspace.join("bin").join("viiper.exe"))
}

/// Set when the console window is closed (or Windows logs off): the loop then quits the normal way, so the mouse
/// buttons are let go and the kina session is saved before Windows ends the process.
static CLOSE_REQUESTED: AtomicBool = AtomicBool::new(false);
static SHUTDOWN_DONE: AtomicBool = AtomicBool::new(false);

#[cfg(windows)]
fn watch_console_close() {
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
    unsafe extern "system" fn handler(event: u32) -> windows_sys::core::BOOL {
        // CTRL_C_EVENT and CTRL_BREAK_EVENT: raw mode turns Ctrl+C into a key, so these only come from outside.
        CLOSE_REQUESTED.store(true, Ordering::SeqCst);
        if event <= 1 {
            return 1;
        }
        // Windows ends the process when this returns (or after about 5 s), so wait for the cleanup.
        let deadline = Instant::now() + Duration::from_millis(4_500);
        while !SHUTDOWN_DONE.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        1
    }
    // SAFETY: registers a handler with the right signature for the life of the process.
    unsafe { SetConsoleCtrlHandler(Some(handler), 1) };
}

#[cfg(not(windows))]
fn watch_console_close() {}

fn set_mouse_capture(on: bool) -> std::io::Result<()> {
    if on {
        execute!(std::io::stdout(), EnableMouseCapture)
    } else {
        execute!(std::io::stdout(), DisableMouseCapture)
    }
}

fn run(hub: &mut Hub) -> anyhow::Result<()> {
    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), ratatui::crossterm::terminal::SetTitle("AION"));
    let mut captured = false;
    let result = (|| -> anyhow::Result<()> {
        let mut last_step = Instant::now() - TICK;
        let mut last_frame = Instant::now();
        let mut redraw = true;
        while !hub.quit && !CLOSE_REQUESTED.load(Ordering::SeqCst) {
            if last_step.elapsed() >= TICK {
                hub.step(now_ms());
                last_step = Instant::now();
                redraw = true;
            }
            let want_mouse = hub.wants_mouse();
            if want_mouse != captured {
                set_mouse_capture(want_mouse)?;
                captured = want_mouse;
            }
            hub.animate(last_frame.elapsed().as_secs_f64());
            last_frame = Instant::now();
            if redraw || hub.animating() {
                terminal.draw(|f| hub::draw(f, hub))?;
                redraw = false;
            }
            let wait = if hub.animating() { FRAME } else { TICK.saturating_sub(last_step.elapsed()) };
            if event::poll(wait)? {
                match event::read()? {
                    // Windows reports presses and releases; only presses act.
                    Event::Key(key) => {
                        if key.kind == KeyEventKind::Press {
                            hub.on_key(key);
                            redraw = true;
                        }
                    }
                    // A bare move changes nothing; with the mouse captured every move over the window comes here.
                    Event::Mouse(m) => {
                        if m.kind != MouseEventKind::Moved {
                            hub.on_mouse(m);
                            redraw = true;
                        }
                    }
                    _ => redraw = true,
                }
            }
        }
        Ok(())
    })();
    if captured {
        let _ = set_mouse_capture(false);
    }
    ratatui::restore();
    result
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(opts) = parse_args(&args)? else {
        print!("{USAGE}");
        return Ok(());
    };
    let base_dir = data_dir("aion-all");
    // Cube and kina keep the folders of the standalone tools, so learned templates, cube spots and past
    // sessions carry over. The meter files its own fights: the aion2dps app still runs (for the overlay) and
    // writes to its own folder.
    let cube_dir = data_dir("aion2cube");
    let kina_dir = data_dir("aion2kina");
    let dps_dir = base_dir.join("dps");
    let log_path = base_dir.join("aion.log");
    logging::init(&log_path);
    tracing::info!("aion {} starting{}", env!("CARGO_PKG_VERSION"), if capture::is_admin() { " as administrator" } else { ", not as administrator" });
    watch_console_close();

    let templates = cube::store::TemplateStore::open(cube_dir.join("templates.json"));
    let mut book = CubeBook::default();
    templates.load_into(&mut book);
    for &template in &opts.templates {
        book.add(CubeTemplate { template, zone: None, evidence: Evidence::Manual });
    }
    let spots = cube::store::SpotStore::new(cube_dir.join("spots.json"));
    let kina_config = kina_core::tracker::TrackerConfig { idle_end_ms: u64::from(opts.idle_minutes) * 60_000, ..Default::default() };

    let now = now_ms();
    let live = !opts.demo && opts.replay.is_empty();
    let feeds = if opts.demo {
        Feeds {
            dps: Box::new(dps::demo::Demo::new(now)),
            cube: Box::new(cube::demo::Demo::new(now, book)),
            kina: Box::new(kina::demo::Demo::new(now)),
            shared: None,
        }
    } else if !opts.replay.is_empty() {
        eprintln!("reading {} capture file(s)...", opts.replay.len());
        let mut replay = source::Replay::load(
            &opts.replay,
            opts.ports.clone(),
            meter_core::combat::Tracker::default(),
            kina_core::tracker::Tracker::new(kina_config),
            cube_core::tracker::Tracker::new(cube_core::tracker::TrackerConfig::default(), book),
        )?;
        Feeds {
            dps: Box::new(dps::feed::ReplayFeed::new(&mut replay)),
            cube: Box::new(cube::feed::ReplayFeed::new(&mut replay)),
            kina: Box::new(kina::feed::ReplayFeed::new(&mut replay)),
            shared: None,
        }
    } else {
        let (meter, dps_history) = dps::feed::open_history(&dps_dir);
        let (kina, kina_history) = kina::feed::open_history(kina_config, &kina_dir);
        let cube = cube::feed::tracker(book, &spots);
        let shared = source::Shared::new(opts.ports.clone(), meter, kina, cube, log_path.clone());
        if let Some(path) = &opts.record {
            let recorder = record::Recorder::create(path).with_context(|| format!("--record {}", path.display()))?;
            let _ = shared.recorder.set(recorder);
        }
        shared.restart();
        Feeds {
            dps: Box::new(dps::feed::LiveFeed::new(shared.clone(), dps_history)),
            cube: Box::new(cube::feed::LiveFeed::new(shared.clone(), templates, spots, cube_dir.clone())),
            kina: Box::new(kina::feed::LiveFeed::new(shared.clone(), kina_history, kina_dir.clone(), now)),
            shared: Some(shared),
        }
    };

    let dps_exe = dps::overlay::find_exe(opts.dps_exe.clone());
    let mut overlay = if live { dps::overlay::Overlay::new(dps_exe, !opts.show_dps_window) } else { dps::overlay::Overlay::unmanaged(dps_exe) };
    if live && opts.overlay {
        if let Err(e) = overlay.start_if_wanted(now) {
            tracing::warn!("overlay: {e}");
        }
    }
    // What was picked on the tabs last time; the command line wins for this run.
    let settings_path = base_dir.join("settings.json");
    let saved = settings::Settings::load(&settings_path);
    let hide = opts.hide_name || saved.hide_names;
    let goal = if opts.goal > 0 { opts.goal } else { saved.kina_goal };
    let dps_options = dps::app::ViewOptions { hide_names: hide, party_only: saved.party_only, boss_only: saved.boss_only };
    let cube_options = cube::app::ViewOptions { hide_name: hide, sound: opts.sound && saved.sound, detector: saved.detector };
    let kina_options = kina::app::ViewOptions { hide_name: hide, goal };
    let engine = keys::hold::Engine::new(find_viiper(opts.viiper.clone()));
    let mut hub = Hub {
        module: opts.module,
        dps: dps::app::App::new(feeds.dps, overlay, dps_options, now),
        cube: cube::app::App::new(feeds.cube, cube_options, now),
        kina: kina::app::App::new(feeds.kina, kina_options, now),
        keys: keys::App::new(engine, live && opts.keys, now),
        live: feeds.shared,
        settings: None,
        game_audio: game_audio::GameAudio::start(),
        hotkeys: hotkeys::Hotkeys::start(),
        quit: false,
    };
    hub.cube.open_map(saved.cube_map);
    // Demo and replay show the saved settings but do not change them.
    hub.settings = Some(settings::Store::new(live.then_some(settings_path), saved, settings::Settings::of(&hub)));
    if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        hub.cube.say("Colours are off because NO_COLOR is set; start aion without it to see them");
    }

    let result = run(&mut hub);

    let last = hub.shutdown();
    SHUTDOWN_DONE.store(true, Ordering::SeqCst);
    if let Some(s) = last.filter(|s| s.changes > 0) {
        println!("{}", kina::summary::chat_summary(&s));
        if hub.kina.feed.persistent() {
            println!("kina session #{} saved in {}", s.id, kina_dir.join("sessions").display());
        }
    }
    if let Some(r) = hub.live.as_ref().and_then(|s| s.recorder.get()) {
        println!("game traffic recorded to {} ({:.1} MB)", r.path().display(), r.bytes() as f64 / 1e6);
    }
    let counts = &hub.cube.snap.counts;
    if counts.cubes_seen > 0 {
        println!("cubes this run: {} seen, {} opened by you, {} by others", counts.cubes_seen, counts.opened_by_you, counts.opened_by_others);
    }
    tracing::info!("aion exiting");
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_the_flags() {
        let o = parse_args(&args(&["--port", "7777", "--goal", "5m", "--hide-name", "--no-keys", "--module", "Cube", "--template", "1400123"])).unwrap().unwrap();
        assert_eq!(o.ports, vec![7777]);
        assert_eq!(o.goal, 5_000_000);
        assert!(o.hide_name && !o.keys && o.overlay);
        assert_eq!(o.module, Module::Cube);
        assert_eq!(o.templates, vec![1_400_123]);
        let o = parse_args(&args(&["--replay", "a.pcap", "b.pcapng", "--no-overlay"])).unwrap().unwrap();
        assert_eq!(o.replay, vec![PathBuf::from("a.pcap"), PathBuf::from("b.pcapng")]);
        assert!(!o.overlay);
        assert_eq!(o.ports, vec![DEFAULT_PORT]);
        assert!(parse_args(&args(&["--help"])).unwrap().is_none());
        assert!(parse_args(&args(&["--module", "map"])).is_err());
        assert!(parse_args(&args(&["--replay"])).is_err());
        assert!(parse_args(&args(&["--demo", "--replay", "x"])).is_err());
        assert!(parse_args(&args(&["--frobnicate"])).is_err());
    }
}
