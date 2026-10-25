//! The aion2dps app, kept for its in game overlay.
//!
//! The DPS tab is the meter now, so when the app is started from here its own main window is hidden once it shows
//! (the overlay window stays); `w` brings it back. An aion2dps that already runs is adopted as it is: its windows
//! are left alone and it keeps running on exit. One started from here is closed with the TUI, through its main
//! window, which quits the app the way its own close button does. In demo and replay mode nothing is touched.
//!
//! When the overlay is off in the app's own settings (its Ctrl+Shift+O), the TUI does not start the app at all:
//! it would only flash its overlay (created visible, hidden once the settings load) and its main window.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// The titles from aion2dps' `tauri.conf.json`.
const MAIN_TITLE: &str = "Aion2 DPS";
const EXE_NAME: &str = "aion2dps.exe";
/// How often the process and its windows are looked at.
const POLL_MS: u64 = 1_000;
/// An aion2dps started outside is found by its main window on every poll, which takes microseconds; walking the
/// process list (about 10 ms) finds one without that window too, this often.
const SCAN_MS: u64 = 30_000;
/// How long the main window is waited for after a start before giving up on hiding it.
const HIDE_WAIT_MS: u64 = 20_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// No aion2dps.exe was found to start.
    Missing,
    /// Demo or replay: the real app is not looked at.
    Unmanaged,
    Stopped,
    Running { pid: u32, ours: bool },
    Failed(String),
}

pub struct Overlay {
    exe: Option<PathBuf>,
    /// Off in demo and replay mode (and tests): no process or window is ever looked at.
    managed: bool,
    child: Option<Child>,
    pid: Option<u32>,
    /// Whether the main window should be hidden when the app starts.
    hide_main: bool,
    /// The pid whose main window was already hidden (or shown again by hand), so it is not hidden twice.
    handled_pid: Option<u32>,
    started_ms: u64,
    last_poll_ms: u64,
    last_scan_ms: u64,
    failed: Option<String>,
    /// Not started because the overlay is off in the app's settings.
    off_in_app: bool,
}

impl Overlay {
    pub fn new(exe: Option<PathBuf>, hide_main: bool) -> Overlay {
        Overlay { exe, managed: true, child: None, pid: None, hide_main, handled_pid: None, started_ms: 0, last_poll_ms: 0, last_scan_ms: 0, failed: None, off_in_app: false }
    }

    /// For demo and replay mode: shows where the app is but never looks for, starts or touches it.
    pub fn unmanaged(exe: Option<PathBuf>) -> Overlay {
        Overlay { exe, managed: false, child: None, pid: None, hide_main: false, handled_pid: None, started_ms: 0, last_poll_ms: 0, last_scan_ms: 0, failed: None, off_in_app: false }
    }

    pub fn exe(&self) -> Option<&Path> {
        self.exe.as_deref()
    }

    pub fn state(&self) -> State {
        if !self.managed {
            return State::Unmanaged;
        }
        match (self.pid, &self.failed, &self.exe) {
            (Some(pid), _, _) => State::Running { pid, ours: self.child.is_some() },
            (None, Some(e), _) => State::Failed(e.clone()),
            (None, None, None) => State::Missing,
            (None, None, Some(_)) => State::Stopped,
        }
    }

    /// One line for the header and the Setup view.
    pub fn label(&self) -> String {
        match self.state() {
            State::Missing => "overlay app not found".into(),
            State::Unmanaged => "overlay app not used in demo or replay".into(),
            State::Stopped if self.off_in_app => "overlay off in aion2dps (o starts the app)".into(),
            State::Stopped => "overlay off".into(),
            State::Running { ours: true, .. } => "overlay on".into(),
            State::Running { ours: false, .. } => "overlay on (started outside)".into(),
            State::Failed(e) => format!("overlay failed: {e}"),
        }
    }

    /// At startup: adopts a running aion2dps, else starts one unless its overlay is off in its settings.
    pub fn start_if_wanted(&mut self, now_ms: u64) -> Result<(), String> {
        if !self.managed {
            return Ok(());
        }
        self.refresh(true);
        if self.pid.is_none() && !app_overlay_visible() {
            tracing::info!("the aion2dps overlay is off in its settings, so the app is not started");
            self.off_in_app = true;
            return Ok(());
        }
        self.start(now_ms)
    }

    /// Adopts a running aion2dps, else starts one.
    pub fn start(&mut self, now_ms: u64) -> Result<(), String> {
        if !self.managed {
            return Err("the overlay app only runs with live capture".into());
        }
        self.refresh(true);
        if self.pid.is_some() {
            return Ok(());
        }
        let exe = self.exe.clone().ok_or_else(|| format!("{EXE_NAME} not found, pass --dps-exe"))?;
        let mut cmd = Command::new(&exe);
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        if let Some(dir) = exe.parent() {
            cmd.current_dir(dir);
        }
        match cmd.spawn() {
            Ok(child) => {
                tracing::info!("started {} (pid {})", exe.display(), child.id());
                self.pid = Some(child.id());
                self.child = Some(child);
                self.started_ms = now_ms;
                self.failed = None;
                self.off_in_app = false;
                Ok(())
            }
            Err(e) => {
                let msg = format!("could not start {}: {e}", exe.display());
                tracing::warn!("{msg}");
                self.failed = Some(msg.clone());
                Err(msg)
            }
        }
    }

    /// Closes the app through its main window, the way its own close button quits it. A started one that does
    /// not go within a few seconds is ended.
    pub fn stop(&mut self) {
        let Some(pid) = self.pid else { return };
        let closed = sys::close_main_window(pid, MAIN_TITLE);
        if let Some(mut child) = self.child.take() {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while closed && std::time::Instant::now() < deadline {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            if !matches!(child.try_wait(), Ok(Some(_))) {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        tracing::info!("aion2dps (pid {pid}) closed");
        self.pid = None;
        self.handled_pid = None;
    }

    /// Ends the app if this TUI started it; an adopted one keeps running.
    pub fn shutdown(&mut self) {
        if self.child.is_some() {
            self.stop();
        }
    }

    /// Shows the main window if it is hidden, hides it otherwise. Returns what it did.
    pub fn toggle_main_window(&mut self) -> Result<&'static str, String> {
        if !self.managed {
            return Err("the overlay app only runs with live capture".into());
        }
        let pid = self.pid.ok_or("the overlay app is not running")?;
        self.handled_pid = Some(pid);
        match sys::main_window_visible(pid, MAIN_TITLE) {
            Some(true) => {
                sys::show_main_window(pid, MAIN_TITLE, false);
                Ok("aion2dps window hidden, the overlay stays")
            }
            Some(false) => {
                sys::show_main_window(pid, MAIN_TITLE, true);
                Ok("aion2dps window shown")
            }
            None => Err("the aion2dps window was not found".into()),
        }
    }

    /// Notices when the app quits or another one starts, and hides a fresh main window once.
    pub fn poll(&mut self, now_ms: u64) {
        if !self.managed || now_ms.saturating_sub(self.last_poll_ms) < POLL_MS {
            return;
        }
        self.last_poll_ms = now_ms;
        let scan = now_ms.saturating_sub(self.last_scan_ms) >= SCAN_MS;
        if scan {
            self.last_scan_ms = now_ms;
        }
        self.refresh(scan);
        let Some(pid) = self.pid else { return };
        // Only an app started from here gets its main window hidden; one you started keeps its windows.
        if !self.hide_main || self.child.is_none() || self.handled_pid == Some(pid) {
            return;
        }
        if sys::main_window_visible(pid, MAIN_TITLE) == Some(true) {
            sys::show_main_window(pid, MAIN_TITLE, false);
            self.handled_pid = Some(pid);
            tracing::info!("aion2dps main window hidden, its overlay stays");
        } else if now_ms.saturating_sub(self.started_ms) > HIDE_WAIT_MS {
            // It never showed one (or it was already hidden): leave it be.
            self.handled_pid = Some(pid);
        }
    }

    /// `scan`: also walk the whole process list for an aion2dps started outside (about 10 ms), which `poll` only
    /// does now and then. One already found is checked by its pid, a new one looked for by its main window.
    fn refresh(&mut self, scan: bool) {
        if let Some(child) = &mut self.child {
            if let Ok(Some(status)) = child.try_wait() {
                tracing::info!("aion2dps exited ({status})");
                self.child = None;
                self.pid = None;
            }
            return;
        }
        // Not ours: whatever aion2dps runs (or none).
        if let Some(pid) = self.pid {
            if sys::is_running(pid, EXE_NAME) {
                return;
            }
            self.pid = None;
        }
        self.pid = sys::find_by_window(MAIN_TITLE, EXE_NAME).or_else(|| scan.then(|| sys::find_process(EXE_NAME)).flatten());
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Whether the app shows its overlay when it starts: `overlayVisible` in its settings, which its Ctrl+Shift+O
/// toggles (`%APPDATA%\dev.ottitsch.aion2dps\settings.json`, after its Tauri identifier). Without settings it does.
pub fn app_overlay_visible() -> bool {
    let Some(appdata) = std::env::var_os("APPDATA") else { return true };
    let path = PathBuf::from(appdata).join("dev.ottitsch.aion2dps").join("settings.json");
    std::fs::read_to_string(path).map(|text| overlay_visible_in(&text)).unwrap_or(true)
}

fn overlay_visible_in(settings: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(settings).ok().and_then(|v| v.get("overlayVisible")?.as_bool()).unwrap_or(true)
}

/// The first aion2dps.exe that exists: `--dps-exe`, next to this exe, the installed app, then the sibling
/// aion2dps checkout this workspace was built beside.
pub fn find_exe(explicit: Option<PathBuf>) -> Option<PathBuf> {
    if explicit.is_some() {
        return explicit.filter(|p| p.is_file());
    }
    let mut candidates = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        candidates.push(dir.join(EXE_NAME));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(&local).join("Aion2 DPS").join(EXE_NAME));
        candidates.push(PathBuf::from(&local).join("Programs").join("Aion2 DPS").join(EXE_NAME));
    }
    candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../aion2dps/target/release").join(EXE_NAME));
    candidates.into_iter().find(|p| p.is_file()).map(|p| p.canonicalize().map(strip_verbatim).unwrap_or(p))
}

/// `\\?\C:\x` to `C:\x`, for display.
fn strip_verbatim(p: PathBuf) -> PathBuf {
    match p.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(rest) => PathBuf::from(rest),
        None => p,
    }
}

#[cfg(windows)]
mod sys {
    use windows_sys::Win32::Foundation::{CloseHandle, HWND, INVALID_HANDLE_VALUE, LPARAM};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, FindWindowExW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, ShowWindow, SW_HIDE, SW_SHOW, WM_CLOSE,
    };

    /// The pid of the first process with this image name (case insensitive).
    pub fn find_process(exe: &str) -> Option<u32> {
        // SAFETY: the snapshot handle is checked and closed; the entry is sized as the API requires.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return None;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut found = None;
            let mut ok = Process32FirstW(snap, &mut entry) != 0;
            while ok {
                let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                if String::from_utf16_lossy(&entry.szExeFile[..len]).eq_ignore_ascii_case(exe) {
                    found = Some(entry.th32ProcessID);
                    break;
                }
                ok = Process32NextW(snap, &mut entry) != 0;
            }
            CloseHandle(snap);
            found
        }
    }

    /// Whether `pid` still runs and is `exe`: the pid of an app that quit can be reused by another process.
    pub fn is_running(pid: u32, exe: &str) -> bool {
        use windows_sys::Win32::Foundation::STILL_ACTIVE;
        use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION};
        // SAFETY: the process handle is checked and closed; the name buffer's length is passed along.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let mut code = 0u32;
            let alive = GetExitCodeProcess(handle, &mut code) != 0 && code == STILL_ACTIVE as u32;
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let named = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut len) != 0;
            CloseHandle(handle);
            let path = String::from_utf16_lossy(&buf[..len as usize]);
            let name = path.rsplit(['\\', '/']).next().unwrap_or_default();
            alive && named && name.eq_ignore_ascii_case(exe)
        }
    }

    /// The pid of `exe` owning a top level window titled `title`, hidden or not. Another program's window may carry
    /// the same title (a folder of that name in Explorer), so every one is checked.
    pub fn find_by_window(title: &str, exe: &str) -> Option<u32> {
        let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        let mut hwnd: HWND = std::ptr::null_mut();
        loop {
            // SAFETY: `wide` is a NUL terminated title that outlives the call; `hwnd` is null or a window it found.
            hwnd = unsafe { FindWindowExW(std::ptr::null_mut(), hwnd, std::ptr::null(), wide.as_ptr()) };
            if hwnd.is_null() {
                return None;
            }
            let mut pid = 0u32;
            // SAFETY: a window handle just found.
            unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
            if pid != 0 && is_running(pid, exe) {
                return Some(pid);
            }
        }
    }

    struct Search {
        pid: u32,
        title: Vec<u16>,
        found: HWND,
    }

    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> windows_sys::core::BOOL {
        // SAFETY: `lparam` is the `Search` passed to `EnumWindows` below, alive for the whole enumeration.
        let search = unsafe { &mut *(lparam as *mut Search) };
        let mut pid = 0u32;
        // SAFETY: plain Win32 calls on a window handle the enumeration handed us.
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid != search.pid {
            return 1;
        }
        let mut buf = [0u16; 256];
        // SAFETY: the buffer length is passed along.
        let len = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        if len > 0 && buf[..len as usize] == search.title[..] {
            search.found = hwnd;
            return 0;
        }
        1
    }

    fn main_window(pid: u32, title: &str) -> Option<HWND> {
        let mut search = Search { pid, title: title.encode_utf16().collect(), found: std::ptr::null_mut() };
        // SAFETY: `visit` only reads the `Search` through `lparam` while `EnumWindows` runs.
        unsafe { EnumWindows(Some(visit), &mut search as *mut Search as LPARAM) };
        (!search.found.is_null()).then_some(search.found)
    }

    pub fn main_window_visible(pid: u32, title: &str) -> Option<bool> {
        // SAFETY: a window handle just found by enumeration.
        main_window(pid, title).map(|hwnd| unsafe { IsWindowVisible(hwnd) } != 0)
    }

    pub fn show_main_window(pid: u32, title: &str, show: bool) {
        if let Some(hwnd) = main_window(pid, title) {
            // SAFETY: a window handle just found by enumeration.
            unsafe { ShowWindow(hwnd, if show { SW_SHOW } else { SW_HIDE }) };
        }
    }

    /// Asks the main window to close; aion2dps quits when it does. Returns whether the window was there.
    pub fn close_main_window(pid: u32, title: &str) -> bool {
        match main_window(pid, title) {
            // SAFETY: posting a message to a window handle just found by enumeration.
            Some(hwnd) => unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) != 0 },
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_overlay_setting_reads_like_aion2dps_writes_it() {
        assert!(!overlay_visible_in(r#"{"ports":[13328],"overlayLocked":false,"overlayVisible":false}"#));
        assert!(overlay_visible_in(r#"{"overlayVisible":true}"#));
        assert!(overlay_visible_in(r#"{"ports":[13328]}"#), "the app's default shows it");
        assert!(overlay_visible_in("not json"));
    }

    #[test]
    fn an_overlay_left_off_says_how_to_start_it() {
        let mut o = Overlay::new(Some(PathBuf::from("aion2dps.exe")), true);
        o.off_in_app = true;
        assert_eq!(o.label(), "overlay off in aion2dps (o starts the app)");
        o.off_in_app = false;
        assert_eq!(o.label(), "overlay off");
        assert!(Overlay::unmanaged(None).start_if_wanted(0).is_ok(), "demo and replay never look");
    }

    #[cfg(windows)]
    #[test]
    fn a_found_app_is_checked_by_its_pid_and_name() {
        let me = std::process::id();
        let exe = std::env::current_exe().unwrap();
        let name = exe.file_name().unwrap().to_str().unwrap().to_ascii_uppercase();
        assert!(sys::is_running(me, &name), "this test runs, whatever the case of its name");
        assert!(!sys::is_running(me, EXE_NAME), "a pid now used by another program is not the app");
        assert!(!sys::is_running(u32::MAX - 3, EXE_NAME), "no such process");
        assert_eq!(sys::find_by_window("no window has this title 7f3a", EXE_NAME), None);
        let t = std::time::Instant::now();
        for _ in 0..100 {
            let _ = sys::find_by_window(MAIN_TITLE, EXE_NAME);
        }
        assert!(t.elapsed() < std::time::Duration::from_millis(100), "a lookup is far cheaper than a process walk: {:?}", t.elapsed() / 100);
    }
}

#[cfg(not(windows))]
mod sys {
    pub fn find_process(_exe: &str) -> Option<u32> {
        None
    }
    pub fn is_running(_pid: u32, _exe: &str) -> bool {
        false
    }
    pub fn find_by_window(_title: &str, _exe: &str) -> Option<u32> {
        None
    }
    pub fn main_window_visible(_pid: u32, _title: &str) -> Option<bool> {
        None
    }
    pub fn show_main_window(_pid: u32, _title: &str, _show: bool) {}
    pub fn close_main_window(_pid: u32, _title: &str) -> bool {
        false
    }
}
