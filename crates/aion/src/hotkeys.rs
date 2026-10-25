//! Global hotkeys, pressed in the game or anywhere else: Ctrl+R starts a new fight on the meter (as the aion2dps
//! app's reset did), Ctrl+M mutes or unmutes the game in the Windows mixer. Like `^` for hold buttons they are
//! registered with Windows for as long as the app runs, so while it runs those keys do nothing else in any program.
//!
//! One thread registers them and waits for presses; the hub picks the presses up on its next step.

use parking_lot::Mutex;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hotkey {
    NewFight,
    GameSound,
}

/// Each hotkey and how the screens name it.
pub const HOTKEYS: [(Hotkey, &str); 2] = [(Hotkey::NewFight, "ctrl+r"), (Hotkey::GameSound, "ctrl+m")];

pub fn label(key: Hotkey) -> &'static str {
    HOTKEYS.iter().find(|(k, _)| *k == key).map(|(_, l)| *l).unwrap_or("")
}

pub struct Hotkeys {
    rx: Receiver<Hotkey>,
    /// Whether each hotkey could be registered (another program may hold it); empty until the thread tried.
    registered: Arc<Mutex<Vec<(Hotkey, bool)>>>,
    thread_id: Arc<std::sync::atomic::AtomicU32>,
}

impl Hotkeys {
    /// Registers the hotkeys on their own thread; `None` when the thread could not be started.
    pub fn start() -> Option<Hotkeys> {
        let (tx, rx) = mpsc::channel();
        let registered = Arc::new(Mutex::new(Vec::new()));
        let thread_id = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let (r, t) = (registered.clone(), thread_id.clone());
        match std::thread::Builder::new().name("hotkeys".into()).spawn(move || sys::run(tx, r, t)) {
            Ok(_) => Some(Hotkeys { rx, registered, thread_id }),
            Err(e) => {
                tracing::warn!("could not start the hotkey thread: {e}");
                None
            }
        }
    }

    /// The presses since the last call.
    pub fn pressed(&self) -> Vec<Hotkey> {
        self.rx.try_iter().collect()
    }

    /// `Some(false)` when another program holds the key, `None` before the thread tried.
    pub fn registered(&self, key: Hotkey) -> Option<bool> {
        self.registered.lock().iter().find(|(k, _)| *k == key).map(|(_, ok)| *ok)
    }
}

impl Drop for Hotkeys {
    fn drop(&mut self) {
        sys::stop(self.thread_id.load(std::sync::atomic::Ordering::SeqCst));
    }
}

#[cfg(windows)]
mod sys {
    use super::{Hotkey, HOTKEYS};
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::mpsc::Sender;
    use std::sync::Arc;
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, MOD_CONTROL, MOD_NOREPEAT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetMessageW, PostThreadMessageW, MSG, WM_HOTKEY, WM_QUIT};

    /// Ids on this thread; hold buttons registers `^` on its own thread.
    const FIRST_ID: i32 = 10;

    fn key_of(key: Hotkey) -> u32 {
        match key {
            Hotkey::NewFight => u32::from(b'R'),
            Hotkey::GameSound => u32::from(b'M'),
        }
    }

    pub fn run(tx: Sender<Hotkey>, registered: Arc<Mutex<Vec<(Hotkey, bool)>>>, thread_id: Arc<AtomicU32>) {
        // SAFETY: plain Win32 calls; the hotkeys belong to this thread, whose queue `GetMessageW` reads until
        // `stop` posts WM_QUIT to it.
        unsafe {
            thread_id.store(GetCurrentThreadId(), Ordering::SeqCst);
            for (i, &(key, label)) in HOTKEYS.iter().enumerate() {
                let ok = RegisterHotKey(std::ptr::null_mut(), FIRST_ID + i as i32, MOD_CONTROL | MOD_NOREPEAT, key_of(key)) != 0;
                if ok {
                    tracing::info!("hotkey {label} registered");
                } else {
                    tracing::warn!("could not register {label} ({}); another program holds it", std::io::Error::last_os_error());
                }
                registered.lock().push((key, ok));
            }
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                if msg.message == WM_HOTKEY {
                    let index = (msg.wParam as i32 - FIRST_ID) as usize;
                    match HOTKEYS.get(index) {
                        Some(&(key, _)) => {
                            if tx.send(key).is_err() {
                                break;
                            }
                        }
                        None => tracing::warn!("the hotkey thread got hotkey {}, which is not one of its own", msg.wParam),
                    }
                }
            }
            for i in 0..HOTKEYS.len() {
                UnregisterHotKey(std::ptr::null_mut(), FIRST_ID + i as i32);
            }
        }
    }

    pub fn stop(thread_id: u32) {
        if thread_id != 0 {
            // SAFETY: posts WM_QUIT to the hotkey thread's own queue.
            unsafe { PostThreadMessageW(thread_id, WM_QUIT, 0, 0) };
        }
    }
}

#[cfg(not(windows))]
mod sys {
    use super::Hotkey;
    use parking_lot::Mutex;
    use std::sync::atomic::AtomicU32;
    use std::sync::mpsc::Sender;
    use std::sync::Arc;
    pub fn run(_tx: Sender<Hotkey>, _registered: Arc<Mutex<Vec<(Hotkey, bool)>>>, _thread_id: Arc<AtomicU32>) {}
    pub fn stop(_thread_id: u32) {}
}
