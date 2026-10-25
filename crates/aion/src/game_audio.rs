//! Ctrl+M (a global hotkey, `hotkeys.rs`): mutes and unmutes the game in the Windows volume mixer, the way the
//! mixer's own speaker button does, on every audio session of AION2.exe on every playback device. Only that mixer
//! mute is touched, nothing in the game.
//!
//! The Windows audio calls run on their own thread, so the screen never waits for them. Finding the game's
//! sessions walks the process list and every playback device (about 15 ms), so it is done every 30 s and on every
//! press; in between only their mute is read (well under a millisecond), every couple of seconds, so a mute set in
//! the mixer itself shows here too.

use parking_lot::Mutex;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

const GAME_EXE: &str = "AION2.exe";
const REFRESH: Duration = Duration::from_secs(2);
/// How often the game's sessions are looked up again: a new playback device or a restarted game brings new ones.
const RESCAN: Duration = Duration::from_secs(30);
/// The same while the game has none (it is not running, or has not played a sound yet).
const RESCAN_IDLE: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameSound {
    /// The game has no audio session: it is not running, or has not played a sound yet.
    NotPlaying,
    On,
    Muted,
}

enum Cmd {
    Toggle,
}

pub struct GameAudio {
    state: Arc<Mutex<GameSound>>,
    tx: Sender<Cmd>,
}

impl GameAudio {
    /// Starts the audio thread; `None` when it could not be started.
    pub fn start() -> Option<GameAudio> {
        let state = Arc::new(Mutex::new(GameSound::NotPlaying));
        let (tx, rx) = mpsc::channel();
        let thread_state = state.clone();
        match std::thread::Builder::new().name("game audio".into()).spawn(move || run(rx, thread_state)) {
            Ok(_) => Some(GameAudio { state, tx }),
            Err(e) => {
                tracing::warn!("could not start the game audio thread: {e}");
                None
            }
        }
    }

    pub fn state(&self) -> GameSound {
        *self.state.lock()
    }

    /// Mutes the game when it plays, unmutes it when muted.
    pub fn toggle(&self) {
        let _ = self.tx.send(Cmd::Toggle);
    }
}

fn run(rx: Receiver<Cmd>, state: Arc<Mutex<GameSound>>) {
    sys::init();
    let mut sessions = sys::Sessions::default();
    let mut found_at: Option<Instant> = None;
    loop {
        let toggle = match rx.recv_timeout(REFRESH) {
            Ok(Cmd::Toggle) => true,
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let every = if sessions.is_empty() { RESCAN_IDLE } else { RESCAN };
        // A press looks them up first, so it reaches every stream the game has right now.
        if toggle || found_at.is_none_or(|t| t.elapsed() >= every) || sessions.expired() {
            sessions = sys::Sessions::find();
            found_at = Some(Instant::now());
        }
        let now = match sessions.read() {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!("game audio: {e}");
                sessions = sys::Sessions::default();
                GameSound::NotPlaying
            }
        };
        let now = match (toggle, now) {
            (true, GameSound::On | GameSound::Muted) => {
                let mute = now == GameSound::On;
                match sessions.set_muted(mute) {
                    Ok(n) => {
                        tracing::info!("game sound {} ({n} audio session{})", if mute { "muted" } else { "on again" }, if n == 1 { "" } else { "s" });
                        if mute {
                            GameSound::Muted
                        } else {
                            GameSound::On
                        }
                    }
                    Err(e) => {
                        tracing::warn!("could not {} the game: {e}", if mute { "mute" } else { "unmute" });
                        now
                    }
                }
            }
            _ => now,
        };
        *state.lock() = now;
    }
}

/// Muted when any session of the game is muted (the mixer shows the game muted then), on when sessions exist and
/// none is.
fn summarise(mutes: &[bool]) -> GameSound {
    match mutes {
        [] => GameSound::NotPlaying,
        m if m.iter().any(|&muted| muted) => GameSound::Muted,
        _ => GameSound::On,
    }
}

#[cfg(windows)]
mod sys {
    use super::{summarise, GameSound, GAME_EXE};
    use std::collections::HashSet;
    use windows::core::Interface;
    use windows::Win32::Media::Audio::{
        eRender, AudioSessionStateExpired, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator,
        DEVICE_STATE_ACTIVE,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};

    /// COM for this thread, once.
    pub fn init() {
        // SAFETY: initialises COM on the calling thread (the audio thread), which then makes every call below.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    }

    /// Every running AION2.exe (the game runs as two processes).
    pub fn game_pids() -> HashSet<u32> {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
        let mut pids = HashSet::new();
        // SAFETY: a process snapshot read with a correctly sized entry, closed before returning.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return pids;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut more = Process32FirstW(snap, &mut entry) != 0;
            while more {
                let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                if String::from_utf16_lossy(&entry.szExeFile[..len]).eq_ignore_ascii_case(GAME_EXE) {
                    pids.insert(entry.th32ProcessID);
                }
                more = Process32NextW(snap, &mut entry) != 0;
            }
            CloseHandle(snap);
        }
        pids
    }

    /// The game's audio sessions on every active playback device, found once and then read or set directly.
    #[derive(Default)]
    pub struct Sessions(Vec<(IAudioSessionControl2, ISimpleAudioVolume)>);

    impl Sessions {
        pub fn find() -> Sessions {
            match find(&game_pids()) {
                Ok(found) => Sessions(found),
                Err(e) => {
                    tracing::debug!("game audio sessions: {e}");
                    Sessions::default()
                }
            }
        }

        pub fn is_empty(&self) -> bool {
            self.0.is_empty()
        }

        /// Whether one of them ended: the game quit, or its device went away.
        pub fn expired(&self) -> bool {
            // SAFETY: a session control `find` obtained.
            self.0.iter().any(|(control, _)| unsafe { control.GetState() }.is_ok_and(|s| s == AudioSessionStateExpired))
        }

        pub fn read(&self) -> Result<GameSound, String> {
            let mut mutes = Vec::with_capacity(self.0.len());
            for (_, volume) in &self.0 {
                // SAFETY: a volume control `find` obtained.
                mutes.push(unsafe { volume.GetMute() }.map_err(|e| e.to_string())?.as_bool());
            }
            Ok(summarise(&mutes))
        }

        pub fn set_muted(&self, mute: bool) -> Result<usize, String> {
            for (_, volume) in &self.0 {
                // SAFETY: a volume control `find` obtained.
                unsafe { volume.SetMute(mute, std::ptr::null()) }.map_err(|e| e.to_string())?;
            }
            Ok(self.0.len())
        }
    }

    /// The audio sessions of these processes, on every active playback device.
    fn find(pids: &HashSet<u32>) -> windows::core::Result<Vec<(IAudioSessionControl2, ISimpleAudioVolume)>> {
        let mut found = Vec::new();
        if pids.is_empty() {
            return Ok(found);
        }
        // SAFETY: COM calls on interfaces this function obtained; each is released when it goes out of scope.
        unsafe {
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let devices = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
            for i in 0..devices.GetCount()? {
                let Ok(manager) = devices.Item(i).and_then(|d| d.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None)) else { continue };
                let sessions = manager.GetSessionEnumerator()?;
                for j in 0..sessions.GetCount()? {
                    let Ok(control) = sessions.GetSession(j).and_then(|s| s.cast::<IAudioSessionControl2>()) else { continue };
                    // A session shared by several processes has no single pid: not the game's.
                    if control.GetProcessId().is_ok_and(|pid| pids.contains(&pid)) {
                        let volume = control.cast::<ISimpleAudioVolume>()?;
                        found.push((control, volume));
                    }
                }
            }
        }
        Ok(found)
    }
}

#[cfg(not(windows))]
mod sys {
    use super::GameSound;

    pub fn init() {}

    #[derive(Default)]
    pub struct Sessions;

    impl Sessions {
        pub fn find() -> Sessions {
            Sessions
        }
        pub fn is_empty(&self) -> bool {
            true
        }
        pub fn expired(&self) -> bool {
            false
        }
        pub fn read(&self) -> Result<GameSound, String> {
            Ok(GameSound::NotPlaying)
        }
        pub fn set_muted(&self, _mute: bool) -> Result<usize, String> {
            Ok(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_muted_session_reads_as_muted() {
        assert_eq!(summarise(&[]), GameSound::NotPlaying);
        assert_eq!(summarise(&[false, false]), GameSound::On);
        assert_eq!(summarise(&[false, true]), GameSound::Muted);
    }

    /// Reads the game's mixer state without changing it, and how long finding and reading take:
    /// `cargo test --release -p aion read_game_sound -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn read_game_sound() {
        sys::init();
        let t = Instant::now();
        let sessions = sys::Sessions::find();
        let find_ms = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        let state = sessions.read();
        let read_ms = t.elapsed().as_secs_f64() * 1e3;
        println!("{state:?}, expired {}: finding took {find_ms:.2} ms, reading {read_ms:.3} ms", sessions.expired());
    }
}
