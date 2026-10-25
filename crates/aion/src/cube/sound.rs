//! Alert sounds and the detector's beeps. The game usually has the focus, so they have to be heard: on Windows
//! tones through `Beep` (it plays even when the terminal is behind the game), elsewhere the terminal bell.
//!
//! Everything plays on one thread, so an alert and the detector never talk over each other: the thread plays
//! queued alerts as they come and beeps for the tracked cube in between.

use crate::cube::detector::{Tone, PREVIEW};
use cube_core::snapshot::AlertKind;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// The detector's beep: short, so even the fastest rate leaves a gap.
const BEEP_MS: u32 = 70;

/// `(frequency Hz, duration ms)` steps for an alert; empty for alerts that only show.
pub fn tones(kind: AlertKind) -> &'static [(u32, u32)] {
    match kind {
        // Rising two tone, the same idea as the dr89mh radar and the BeardBear33 detector.
        AlertKind::Appeared => &[(1_200, 140), (1_600, 200)],
        AlertKind::StillThere => &[(1_200, 120)],
        AlertKind::OpenedByOther => &[(500, 160)],
        AlertKind::OpenedByYou | AlertKind::Learned => &[],
    }
}

enum Msg {
    Alert(&'static [(u32, u32)]),
    Track(Option<Tone>),
    Preview,
}

/// The sound thread, started on first use; `None` when it could not be started.
fn speaker() -> Option<&'static Sender<Msg>> {
    static SPEAKER: OnceLock<Option<Sender<Msg>>> = OnceLock::new();
    SPEAKER
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel();
            match std::thread::Builder::new().name("cube sounds".into()).spawn(move || run(rx, beep)) {
                Ok(_) => Some(tx),
                Err(e) => {
                    tracing::warn!("could not start the sound thread: {e}");
                    None
                }
            }
        })
        .as_ref()
}

fn send(msg: Msg) {
    if let Some(tx) = speaker() {
        let _ = tx.send(msg);
    }
}

/// Plays the alert.
pub fn play(kind: AlertKind) {
    let steps = tones(kind);
    if !steps.is_empty() {
        send(Msg::Alert(steps));
    }
}

/// Beeps at this rate and pitch from now on, or stops.
pub fn track(tone: Option<Tone>) {
    send(Msg::Track(tone));
}

/// The detector from far to near, then above and below, so it can be learnt without a cube.
pub fn preview() {
    send(Msg::Preview);
}

/// The sound thread: `beep` plays tones and blocks while they play.
fn run(rx: Receiver<Msg>, beep: fn(&[(u32, u32)])) {
    let mut target: Option<Tone> = None;
    let mut last = Instant::now();
    loop {
        let wait = match target {
            Some(t) => (last + Duration::from_millis(t.interval_ms)).saturating_duration_since(Instant::now()),
            None => Duration::from_secs(3_600),
        };
        match rx.recv_timeout(wait) {
            Ok(Msg::Alert(steps)) => {
                beep(steps);
                last = Instant::now();
            }
            Ok(Msg::Track(tone)) => {
                // A cube to track just came up: the first beep comes at once (after any alert before it).
                if target.is_none() {
                    if let Some(t) = tone {
                        last = Instant::now().checked_sub(Duration::from_millis(t.interval_ms)).unwrap_or_else(Instant::now);
                    }
                }
                target = tone;
            }
            Ok(Msg::Preview) => {
                for (distance, height, beeps) in PREVIEW {
                    let t = Tone::at(distance, height);
                    for _ in 0..beeps {
                        beep(&[(t.freq, BEEP_MS)]);
                        std::thread::sleep(Duration::from_millis(t.interval_ms.saturating_sub(u64::from(BEEP_MS))));
                    }
                }
                last = Instant::now();
            }
            Err(RecvTimeoutError::Timeout) => {
                if let Some(t) = target {
                    last = Instant::now();
                    beep(&[(t.freq, BEEP_MS)]);
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(windows)]
fn beep(steps: &[(u32, u32)]) {
    for &(freq, ms) in steps {
        // SAFETY: plain Win32 call with value arguments; it blocks this thread for `ms`.
        unsafe { windows_sys::Win32::System::Diagnostics::Debug::Beep(freq, ms) };
    }
}

#[cfg(not(windows))]
fn beep(_steps: &[(u32, u32)]) {
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x07").and_then(|_| out.flush());
}

#[cfg(test)]
mod tests {
    use super::*;

    static HEARD: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

    fn listen(steps: &[(u32, u32)]) {
        HEARD.lock().unwrap().extend(steps.iter().map(|s| s.0));
    }

    #[test]
    fn the_sound_thread_beeps_at_the_tracked_rate_and_stops() {
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::spawn(move || run(rx, listen));
        tx.send(Msg::Alert(tones(AlertKind::Appeared))).unwrap();
        tx.send(Msg::Track(Some(Tone { interval_ms: 100, freq: 1_760 }))).unwrap();
        std::thread::sleep(Duration::from_millis(550));
        tx.send(Msg::Track(None)).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let heard = HEARD.lock().unwrap().clone();
        assert_eq!(heard[..2], [1_200, 1_600], "the alert first: {heard:?}");
        let beeps = heard[2..].iter().filter(|f| **f == 1_760).count();
        assert!((4..=7).contains(&beeps), "about one beep per 100 ms: {heard:?}");
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(HEARD.lock().unwrap().len(), heard.len(), "nothing after it stopped");
        drop(tx);
        thread.join().unwrap();
    }

    #[test]
    fn only_cubes_to_go_for_make_noise() {
        assert_eq!(tones(AlertKind::Appeared).len(), 2);
        assert!(tones(AlertKind::OpenedByYou).is_empty());
        assert!(tones(AlertKind::Learned).is_empty());
    }
}
