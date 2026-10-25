//! Hold buttons, ported from key-mouse's `hold_buttons.py`.
//!
//! `^` (a global hotkey, swallowed so the game never sees it) holds the left and right mouse buttons down, `^`
//! again lets go. While holding it also presses 1 and f, each on its own random 8 to 12 second timer: 1 stays
//! down for 1 to 2 seconds, f for 60 to 140 ms. Every 85 to 95 seconds the mouse buttons are let go for 3 to 7
//! seconds. The TUI can toggle it too.
//!
//! Everything runs on one thread that owns the devices, so timing never waits for the screen. On the way out
//! (quit, a closed window, a panic) every key and button is let go and the devices are removed.

use crate::keys::viiper::{self, Devices, BUTTON_LEFT, BUTTON_RIGHT};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Child;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const HOTKEY: char = '^';
/// Key, and how long it stays down per press (seconds, random in the range).
pub const TAP_KEYS: [(char, (f64, f64)); 2] = [('1', (1.0, 2.0)), ('f', (0.06, 0.14))];
/// Seconds between presses, picked at random for each key every time.
pub const TAP_EVERY: (f64, f64) = (8.0, 12.0);
/// Seconds of holding the mouse buttons before a break, and how long a break lasts.
pub const BREAK_EVERY: (f64, f64) = (85.0, 95.0);
pub const BREAK_FOR: (f64, f64) = (3.0, 7.0);
const LOG_LINES: usize = 200;
const LOOP_SLEEP: Duration = Duration::from_millis(10);
/// How long a game brought to the front gets before the buttons go down: it lets go of every key and button
/// once it is active (Unreal does), which would drop a press that came first.
const SETTLE: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    /// Starting the server, registering the hotkey, creating the devices.
    Starting(String),
    /// Devices ready, buttons up.
    Ready,
    Holding,
    /// Holding, but the mouse buttons are up until `until`.
    Break { until: Instant },
    /// Not running (stopped by hand, or never started).
    Off,
    Failed(String),
}

/// What the TUI shows, written by the engine thread.
pub struct State {
    pub phase: Phase,
    /// When each tap key is next pressed (while holding), and until when it is down.
    pub next_press: Vec<(char, Instant)>,
    pub down_until: Vec<(char, Instant)>,
    pub next_break: Option<Instant>,
    pub presses: Vec<(char, u64)>,
    pub breaks: u64,
    pub holding_since: Option<Instant>,
    /// Whether the VIIPER server was started here (and is stopped again on the way out).
    pub own_server: bool,
    /// Newest last: `(wall clock ms, text)`.
    pub log: VecDeque<(u64, String)>,
}

impl State {
    fn new() -> State {
        State {
            phase: Phase::Off,
            next_press: Vec::new(),
            down_until: Vec::new(),
            next_break: None,
            presses: TAP_KEYS.iter().map(|(k, _)| (*k, 0)).collect(),
            breaks: 0,
            holding_since: None,
            own_server: false,
            log: VecDeque::new(),
        }
    }

    fn say(&mut self, text: impl Into<String>) {
        let text = text.into();
        tracing::info!("keys: {text}");
        self.log.push_back((crate::now_ms(), text));
        while self.log.len() > LOG_LINES {
            self.log.pop_front();
        }
    }
}

enum Cmd {
    Toggle,
    Quit,
}

/// A small xorshift generator: the timings only need to look random.
struct Rng(u64);

impl Rng {
    fn new() -> Rng {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
        Rng(seed | 1)
    }

    fn uniform(&mut self, (lo, hi): (f64, f64)) -> f64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        lo + (hi - lo) * ((x >> 11) as f64 / (1u64 << 53) as f64)
    }

    fn after(&mut self, now: Instant, range: (f64, f64)) -> Instant {
        now + Duration::from_secs_f64(self.uniform(range))
    }
}

/// The engine thread and its controls. Dropping it lets go of everything and removes the devices.
pub struct Engine {
    pub state: Arc<Mutex<State>>,
    tx: Option<Sender<Cmd>>,
    thread: Option<JoinHandle<()>>,
    viiper_exe: PathBuf,
}

impl Engine {
    /// An engine that is off until `start`.
    pub fn new(viiper_exe: PathBuf) -> Engine {
        Engine { state: Arc::new(Mutex::new(State::new())), tx: None, thread: None, viiper_exe }
    }

    pub fn viiper_exe(&self) -> &std::path::Path {
        &self.viiper_exe
    }

    pub fn running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    pub fn start(&mut self) {
        if self.running() {
            return;
        }
        self.join();
        let (tx, rx) = mpsc::channel();
        let state = self.state.clone();
        let exe = self.viiper_exe.clone();
        state.lock().phase = Phase::Starting("starting".into());
        match std::thread::Builder::new().name("hold buttons".into()).spawn(move || run(state, rx, exe)) {
            Ok(handle) => {
                self.tx = Some(tx);
                self.thread = Some(handle);
            }
            Err(e) => self.state.lock().phase = Phase::Failed(format!("could not start the key thread: {e}")),
        }
    }

    /// Lets go of everything, removes the devices and waits for the thread.
    pub fn stop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Cmd::Quit);
        }
        self.join();
    }

    fn join(&mut self) {
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
        self.tx = None;
    }

    /// The same as pressing `^`.
    pub fn toggle(&self) -> bool {
        self.tx.as_ref().is_some_and(|tx| tx.send(Cmd::Toggle).is_ok())
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Everything the engine thread owns. Its drop order matters: devices first (let go, remove), then the hotkey,
/// then the server this thread started.
struct Owned {
    devices: Option<Devices>,
    hotkey: Option<hotkey::Registration>,
    server: Option<Child>,
}

impl Drop for Owned {
    fn drop(&mut self) {
        self.devices.take();
        self.hotkey.take();
        if let Some(mut server) = self.server.take() {
            let _ = server.kill();
            let _ = server.wait();
        }
    }
}

fn run(state: Arc<Mutex<State>>, rx: Receiver<Cmd>, viiper_exe: PathBuf) {
    let started = Instant::now();
    let mut owned = Owned { devices: None, hotkey: None, server: None };
    let set_phase = |phase: Phase| state.lock().phase = phase;

    set_phase(Phase::Starting("starting the VIIPER server".into()));
    match viiper::ensure_server(&viiper_exe, Duration::from_secs(10)) {
        Ok(child) => {
            let mut s = state.lock();
            s.own_server = child.is_some();
            s.say(if child.is_some() { "VIIPER server started" } else { "VIIPER server already running" });
            owned.server = child;
        }
        Err(e) => {
            let mut s = state.lock();
            s.say(format!("VIIPER: {e}"));
            s.phase = Phase::Failed(format!("VIIPER: {e}"));
            return;
        }
    }

    set_phase(Phase::Starting(format!("registering {HOTKEY}")));
    match hotkey::register(HOTKEY) {
        Ok(reg) => owned.hotkey = Some(reg),
        Err(e) => {
            let mut s = state.lock();
            s.say(e.clone());
            s.phase = Phase::Failed(e);
            return;
        }
    }

    set_phase(Phase::Starting("adding the virtual keyboard and mouse".into()));
    let usages: Vec<(char, u8)> = match TAP_KEYS.iter().map(|(k, _)| viiper::usage_for(*k).map(|u| (*k, u))).collect() {
        Ok(u) => u,
        Err(e) => {
            set_phase(Phase::Failed(e));
            return;
        }
    };
    match Devices::create(Duration::from_secs(10)) {
        Ok(d) => owned.devices = Some(d),
        Err(e) => {
            let mut s = state.lock();
            s.say(format!("devices: {e}"));
            s.phase = Phase::Failed(format!("devices: {e}"));
            return;
        }
    }
    {
        let mut s = state.lock();
        s.phase = Phase::Ready;
        s.say(format!("ready in {:.1} s: {HOTKEY} holds left + right mouse button, {HOTKEY} again lets go", started.elapsed().as_secs_f64()));
        if !crate::capture::is_admin() {
            // Windows keeps the hotkey from a program without them while one with them (the game) is in front.
            s.say(format!("not running as administrator: {HOTKEY} does not reach a game that runs as administrator; start AION with AION.lnk"));
        }
    }

    let mut rng = Rng::new();
    let mut holding = false;
    let mut next_press: Vec<(char, Instant)> = Vec::new();
    let mut release_at: Vec<(char, Instant)> = Vec::new();
    let mut next_break = Instant::now();
    let mut resume_at: Option<Instant> = None;
    // The game's processes while holding; empty when it is not running, and then the taps go wherever.
    let mut game = std::collections::HashSet::new();
    let usage = |k: char| usages.iter().find(|(c, _)| *c == k).map(|(_, u)| *u).unwrap_or(0);

    loop {
        let mut toggles = owned.hotkey.as_ref().map(hotkey::Registration::drain).unwrap_or(0);
        loop {
            match rx.try_recv() {
                Ok(Cmd::Toggle) => toggles += 1,
                Ok(Cmd::Quit) | Err(TryRecvError::Disconnected) => {
                    if holding {
                        state.lock().say("released (quitting)");
                    }
                    let mut s = state.lock();
                    s.phase = Phase::Off;
                    s.next_press.clear();
                    s.down_until.clear();
                    s.next_break = None;
                    s.holding_since = None;
                    drop(s);
                    // `owned` lets go of everything and removes the devices on drop.
                    return;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        let Some(dev) = owned.devices.as_mut() else { return };
        for _ in 0..toggles {
            holding = !holding;
            // A hold is meant for the game: when it runs behind another window (AION's own, which is in front
            // after every start), it is brought to the front first, so the buttons and the taps reach it.
            let start = if holding { Some(front::start()) } else { None };
            if start.as_ref().is_some_and(|s| s.ok && s.brought) {
                std::thread::sleep(SETTLE);
            }
            let now = Instant::now();
            let mut s = state.lock();
            if let Some(start) = start {
                if !start.ok {
                    holding = false;
                    s.say(format!("not holding: {}", start.note));
                    continue;
                }
                game = start.game;
                press_both(dev);
                next_press = TAP_KEYS.iter().map(|(k, _)| (*k, rng.after(now, TAP_EVERY))).collect();
                next_break = rng.after(now, BREAK_EVERY);
                s.phase = Phase::Holding;
                s.holding_since = Some(now);
                s.say(format!("holding ({})", start.note));
            } else {
                dev.release_all();
                release_at.clear();
                resume_at = None;
                s.phase = Phase::Ready;
                s.holding_since = None;
                s.say("released");
            }
        }

        let now = Instant::now();
        if holding && resume_at.is_none() && now >= next_break {
            let _ = dev.mouse.release_all();
            let pause = rng.uniform(BREAK_FOR);
            let until = now + Duration::from_secs_f64(pause);
            resume_at = Some(until);
            let mut s = state.lock();
            s.phase = Phase::Break { until };
            s.breaks += 1;
            s.say(format!("break: mouse buttons up for {pause:.1} s"));
        } else if holding && resume_at.is_some_and(|until| now >= until) {
            press_both(dev);
            resume_at = None;
            next_break = rng.after(now, BREAK_EVERY);
            let mut s = state.lock();
            s.phase = Phase::Holding;
            s.say("break over, holding again");
        }
        let mut released = Vec::new();
        release_at.retain(|(k, at)| {
            if now >= *at {
                released.push(*k);
                false
            } else {
                true
            }
        });
        for k in released {
            let _ = dev.keyboard.release(usage(k));
        }
        if holding {
            for (key, due) in next_press.iter_mut() {
                if now >= *due && !release_at.iter().any(|(k, _)| k == key) {
                    // A tap goes to the window in front; with the game behind another one it would type there.
                    if !front::game_in_front(&game) {
                        *due = now + Duration::from_secs_f64(rng.uniform(TAP_EVERY));
                        state.lock().say(format!("skipped {key}: the game is not in front"));
                        continue;
                    }
                    let range = TAP_KEYS.iter().find(|(k, _)| k == key).map(|(_, r)| *r).unwrap_or((0.1, 0.1));
                    let hold = rng.uniform(range);
                    let _ = dev.keyboard.press(usage(*key));
                    release_at.push((*key, now + Duration::from_secs_f64(hold)));
                    let wait = rng.uniform(TAP_EVERY);
                    *due = now + Duration::from_secs_f64(wait);
                    let mut s = state.lock();
                    if let Some(count) = s.presses.iter_mut().find(|(k, _)| k == key) {
                        count.1 += 1;
                    }
                    s.say(format!("pressed {key} for {:.0} ms, next {key} in {wait:.1} s", hold * 1000.0));
                }
            }
        }
        {
            let mut s = state.lock();
            s.next_press = if holding { next_press.clone() } else { Vec::new() };
            s.down_until = release_at.clone();
            s.next_break = (holding && resume_at.is_none()).then_some(next_break);
        }
        std::thread::sleep(LOOP_SLEEP);
    }
}

/// Left, then right, as two reports, the way hold_buttons.py pressed them.
fn press_both(dev: &mut Devices) {
    let _ = dev.mouse.press(BUTTON_LEFT);
    let _ = dev.mouse.press(BUTTON_RIGHT);
}

/// The global hotkey. `RegisterHotKey` with no window posts `WM_HOTKEY` to the registering thread's queue, so
/// the engine thread registers it and drains its own queue every loop.
#[cfg(windows)]
mod hotkey {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyboardLayout, RegisterHotKey, UnregisterHotKey, VkKeyScanExW, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{PeekMessageW, MSG, PM_REMOVE, WM_HOTKEY};

    const ID: i32 = 1;

    pub struct Registration {
        _not_send: std::marker::PhantomData<*const ()>,
    }

    /// Registers the key that types `ch` on the current layout, with the modifiers it needs.
    pub fn register(ch: char) -> Result<Registration, String> {
        let mut units = [0u16; 2];
        if ch.encode_utf16(&mut units).len() != 1 {
            return Err(format!("{ch:?} cannot be a hotkey"));
        }
        // SAFETY: plain Win32 calls with value arguments; the hotkey belongs to this thread.
        unsafe {
            let res = VkKeyScanExW(units[0], GetKeyboardLayout(0));
            if res == -1 {
                return Err(format!("{ch:?} is not on the current keyboard layout"));
            }
            let (vk, shift) = ((res as u16 & 0xFF) as u32, (res as u16 >> 8) & 0xFF);
            let mut mods = MOD_NOREPEAT;
            if shift & 1 != 0 {
                mods |= MOD_SHIFT;
            }
            if shift & 2 != 0 {
                mods |= MOD_CONTROL;
            }
            if shift & 4 != 0 {
                mods |= MOD_ALT;
            }
            if RegisterHotKey(std::ptr::null_mut(), ID, mods, vk) == 0 {
                let err = std::io::Error::last_os_error();
                return Err(format!("could not register {ch} ({err}); another program already uses it, is the old hold-buttons.cmd still running?"));
            }
        }
        Ok(Registration { _not_send: std::marker::PhantomData })
    }

    impl Registration {
        /// How many times the hotkey was pressed since the last call.
        pub fn drain(&self) -> usize {
            let mut n = 0;
            // SAFETY: `msg` is a valid out pointer; only WM_HOTKEY messages of this thread are removed.
            unsafe {
                let mut msg: MSG = std::mem::zeroed();
                while PeekMessageW(&mut msg, std::ptr::null_mut(), WM_HOTKEY, WM_HOTKEY, PM_REMOVE) != 0 {
                    if msg.wParam == ID as usize {
                        n += 1;
                    } else {
                        tracing::warn!("hold buttons got hotkey {} instead of {ID}", msg.wParam);
                    }
                }
            }
            n
        }
    }

    impl Drop for Registration {
        fn drop(&mut self) {
            // SAFETY: unregisters the hotkey this thread registered.
            unsafe { UnregisterHotKey(std::ptr::null_mut(), ID) };
        }
    }
}

/// The game's window, which the held buttons and the taps are meant for.
#[cfg(windows)]
mod front {
    use std::collections::HashSet;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{CloseHandle, HWND, INVALID_HANDLE_VALUE, LPARAM, POINT, RECT};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetAncestor, GetCursorPos, GetForegroundWindow, GetWindow, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        SetCursorPos, SetForegroundWindow, ShowWindow, WindowFromPoint, GA_ROOT, GW_OWNER, SW_RESTORE,
    };

    const GAME_EXE: &str = "AION2.exe";
    /// How long the game's window gets to come to the front.
    const ACTIVATE_WAIT: Duration = Duration::from_millis(500);

    /// How a hold starts.
    pub struct Start {
        /// The game's processes; empty when it is not running.
        pub game: HashSet<u32>,
        /// False when the game runs but stayed behind another window: nothing is held then.
        pub ok: bool,
        /// Whether the game was just brought to the front.
        pub brought: bool,
        /// Where the buttons go, for the log.
        pub note: String,
    }

    /// Brings the game to the front when it runs behind another window, and its window under the pointer, since
    /// held buttons land on the window under the pointer (and a click there would make that window active). The
    /// game is only ever looked at through the process list and its windows; no handle to it is opened.
    pub fn start() -> Start {
        let procs = processes();
        let name = |pid: u32| procs.iter().find(|(p, _)| *p == pid).map_or_else(|| "another program".to_string(), |(_, n)| n.clone());
        let game: HashSet<u32> = procs.iter().filter(|(_, n)| n.eq_ignore_ascii_case(GAME_EXE)).map(|(p, _)| *p).collect();
        let before = foreground_pid();
        if game.is_empty() {
            return Start { note: format!("{} in front, the game is not running", name(before)), ok: true, brought: false, game };
        }
        let Some(window) = game_window(&game) else {
            return Start { note: format!("{} in front, no game window found", name(before)), ok: true, brought: false, game };
        };
        let mut note = format!("{GAME_EXE} in front");
        let brought = !game.contains(&before);
        if brought {
            if !bring_to_front(window, &game) {
                let note = format!("{} is in front and the game did not come before it; click into the game and press ^ again", name(before));
                return Start { game, ok: false, brought: false, note };
            }
            note = format!("brought {GAME_EXE} to the front, {} was", name(before));
        }
        if move_pointer_into(window, &game) {
            note.push_str(", pointer moved into it");
        }
        Start { game, ok: true, brought, note }
    }

    /// Whether the window in front is the game's; true when the game is not running, as then any window is meant.
    pub fn game_in_front(game: &HashSet<u32>) -> bool {
        game.is_empty() || game.contains(&foreground_pid())
    }

    fn foreground_pid() -> u32 {
        // SAFETY: a plain Win32 call; a null window gives pid 0.
        window_pid(unsafe { GetForegroundWindow() })
    }

    fn window_pid(hwnd: HWND) -> u32 {
        let mut pid = 0u32;
        // SAFETY: a plain Win32 call on a window handle (or null, which gives 0).
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        pid
    }

    /// Every process as (pid, exe name).
    fn processes() -> Vec<(u32, String)> {
        let mut out = Vec::new();
        // SAFETY: the snapshot is checked and closed; the entry is sized as the API requires.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return out;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut more = Process32FirstW(snap, &mut entry) != 0;
            while more {
                let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                out.push((entry.th32ProcessID, String::from_utf16_lossy(&entry.szExeFile[..len])));
                more = Process32NextW(snap, &mut entry) != 0;
            }
            CloseHandle(snap);
        }
        out
    }

    struct Search<'a> {
        game: &'a HashSet<u32>,
        best: Option<(HWND, i64)>,
    }

    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> windows_sys::core::BOOL {
        // SAFETY: `lparam` is the `Search` passed to `EnumWindows` below, alive for the whole enumeration.
        let search = unsafe { &mut *(lparam as *mut Search) };
        // SAFETY: plain Win32 calls on a window handle the enumeration handed us.
        unsafe {
            if !search.game.contains(&window_pid(hwnd)) || IsWindowVisible(hwnd) == 0 || !GetWindow(hwnd, GW_OWNER).is_null() {
                return 1;
            }
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut r);
            let area = i64::from(r.right - r.left) * i64::from(r.bottom - r.top);
            if search.best.is_none_or(|(_, a)| area > a) {
                search.best = Some((hwnd, area));
            }
        }
        1
    }

    /// The game's main window: its largest visible top level window.
    fn game_window(game: &HashSet<u32>) -> Option<HWND> {
        let mut search = Search { game, best: None };
        // SAFETY: `visit` only uses the `Search` through `lparam` while `EnumWindows` runs.
        unsafe { EnumWindows(Some(visit), &mut search as *mut Search as LPARAM) };
        search.best.map(|(hwnd, _)| hwnd)
    }

    /// Makes the game's window the active one. The `^` press gives this thread the right to; the switch itself
    /// happens asynchronously, so it is waited for.
    fn bring_to_front(window: HWND, game: &HashSet<u32>) -> bool {
        // SAFETY: plain Win32 calls on the game's window handle.
        unsafe {
            if IsIconic(window) != 0 {
                ShowWindow(window, SW_RESTORE);
            }
            SetForegroundWindow(window);
        }
        let deadline = Instant::now() + ACTIVATE_WAIT;
        loop {
            if game.contains(&foreground_pid()) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Moves the pointer to the middle of the game's window when it is over another one. Returns whether it moved.
    fn move_pointer_into(window: HWND, game: &HashSet<u32>) -> bool {
        // SAFETY: plain Win32 calls with valid out pointers.
        unsafe {
            let mut at = POINT { x: 0, y: 0 };
            if GetCursorPos(&mut at) == 0 {
                return false;
            }
            let under = GetAncestor(WindowFromPoint(at), GA_ROOT);
            if !under.is_null() && game.contains(&window_pid(under)) {
                return false;
            }
            let mut r: RECT = std::mem::zeroed();
            if GetWindowRect(window, &mut r) == 0 {
                return false;
            }
            SetCursorPos((r.left + r.right) / 2, (r.top + r.bottom) / 2) != 0
        }
    }

    /// Finds the game's window and says what is in front, changing nothing:
    /// `cargo test -p aion finds_the_game_window -- --ignored --nocapture`.
    #[cfg(test)]
    #[test]
    #[ignore]
    fn finds_the_game_window() {
        let procs = processes();
        let game: HashSet<u32> = procs.iter().filter(|(_, n)| n.eq_ignore_ascii_case(GAME_EXE)).map(|(p, _)| *p).collect();
        let front = foreground_pid();
        let window = game_window(&game);
        let mut r: RECT = unsafe { std::mem::zeroed() };
        if let Some(w) = window {
            // SAFETY: a window handle just found.
            unsafe { GetWindowRect(w, &mut r) };
        }
        let name = procs.iter().find(|(p, _)| *p == front).map(|(_, n)| n.as_str());
        println!("game pids {game:?}, window {window:?} at {},{} to {},{}, in front: {name:?}", r.left, r.top, r.right, r.bottom);
    }
}

#[cfg(not(windows))]
mod front {
    use std::collections::HashSet;

    pub struct Start {
        pub game: HashSet<u32>,
        pub ok: bool,
        pub brought: bool,
        pub note: String,
    }

    pub fn start() -> Start {
        Start { game: HashSet::new(), ok: true, brought: false, note: "no game window on this system".into() }
    }

    pub fn game_in_front(_game: &HashSet<u32>) -> bool {
        true
    }
}

#[cfg(not(windows))]
mod hotkey {
    pub struct Registration;
    pub fn register(_ch: char) -> Result<Registration, String> {
        Err("global hotkeys need Windows".into())
    }
    impl Registration {
        pub fn drain(&self) -> usize {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_the_game_any_window_is_meant() {
        assert!(front::game_in_front(&std::collections::HashSet::new()), "the taps go wherever, as before");
    }

    /// Starts the whole engine (VIIPER, `^`, the devices), waits until it is ready and stops it again without
    /// holding anything: `cargo test -p aion engine_starts_and_stops -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn engine_starts_and_stops() {
        let exe = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../bin/viiper.exe");
        let server_before = viiper::ping();
        let devices_before = viiper::sys::input_devices();
        let mut engine = Engine::new(exe);
        engine.start();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let phase = engine.state.lock().phase.clone();
            match phase {
                Phase::Ready => break,
                Phase::Failed(e) => panic!("engine failed: {e}"),
                _ => assert!(Instant::now() < deadline, "not ready after 30 s: {phase:?}"),
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        engine.stop();
        let state = engine.state.lock();
        for (_, line) in &state.log {
            println!("log: {line}");
        }
        assert_eq!(state.phase, Phase::Off);
        assert_eq!(state.presses.iter().map(|(_, n)| n).sum::<u64>(), 0, "nothing may be pressed");
        drop(state);
        let deadline = Instant::now() + Duration::from_secs(10);
        while viiper::sys::input_devices().difference(&devices_before).next().is_some() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(viiper::sys::input_devices().difference(&devices_before).count(), 0, "devices left behind");
        assert_eq!(viiper::ping(), server_before, "the server should be as it was before");
        // `^` is free again once the engine is gone.
        drop(hotkey::register(HOTKEY).expect("^ released"));
    }

    #[test]
    fn random_timings_stay_in_their_ranges() {
        let mut rng = Rng::new();
        for _ in 0..1_000 {
            let v = rng.uniform(TAP_EVERY);
            assert!((TAP_EVERY.0..TAP_EVERY.1).contains(&v), "{v}");
        }
    }
}
