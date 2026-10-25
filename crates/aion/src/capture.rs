//! Live capture: one read only pcap handle per usable adapter, all feeding the one shared pipeline (and
//! through it the meter, the kina tracker and the cube radar).
//!
//! Packets are only ever read, never sent. Every adapter that is connected and has an address is opened,
//! plus the Npcap loopback adapter (where the traffic shows up when a VPN or ping reducer relays it
//! locally). A generation counter lets a restart retire the old threads: each one re checks it after
//! every packet or read timeout and exits when it no longer matches.

use crate::source::{Event, Shared};
use aion_net::net::LinkType;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum CaptureStatus {
    /// Npcap is not installed (Windows).
    NpcapMissing,
    /// Capture not started yet.
    Off,
    /// Adapters are open but no game traffic has been seen yet.
    Waiting { adapters: usize },
    Capturing { adapter: String },
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AdapterInfo {
    pub name: String,
    pub description: String,
    pub connected: bool,
    pub has_addresses: bool,
    pub is_loopback: bool,
    /// Whether `start` opens this adapter.
    pub usable: bool,
}

/// Capture side state kept in `Shared`.
#[derive(Default)]
pub struct CaptureState {
    /// Held for the whole of a start or stop, so two restarts never interleave and a dying capture
    /// thread never races a restart. It is the one lock that is held while others are taken; it is
    /// always taken first and never while another lock is held.
    restart: parking_lot::Mutex<()>,
    /// Bumped on every restart; threads exit when it no longer matches theirs.
    generation: AtomicU64,
    /// Set once the status has switched to `Capturing` for the current generation.
    live_announced: AtomicBool,
    pub npcap_found: AtomicBool,
    /// Timestamp of the latest captured packet (capture clock, Unix ms).
    last_packet_ms: AtomicU64,
    /// Wall clock when that packet arrived.
    last_packet_wall_ms: AtomicU64,
    /// Labels of the adapters opened by the current generation.
    pub open_adapters: parking_lot::Mutex<Vec<String>>,
    /// Every adapter pcap listed the last time `start` ran. Enumerating adapters
    /// is slow on Windows, so the setup view reads this instead of asking pcap again.
    adapters: parking_lot::Mutex<Vec<AdapterInfo>>,
}

/// How long after the last packet the capture clock still drives the tracker.
const CLOCK_FRESH_MS: u64 = 1_000;

impl CaptureState {
    /// Serialises restarts; see the `restart` field.
    pub fn lock_restart(&self) -> parking_lot::MutexGuard<'_, ()> {
        self.restart.lock()
    }

    pub fn stop(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// The adapter list from the last `start`; empty before it ran.
    pub fn cached_adapters(&self) -> Vec<AdapterInfo> {
        self.adapters.lock().clone()
    }

    /// Forgets that game traffic was announced, so the next flow announces again. Returns whether the
    /// status had been `Capturing`.
    pub fn clear_live(&self) -> bool {
        self.live_announced.swap(false, Ordering::SeqCst)
    }

    /// Number of adapters the current generation opened.
    pub fn open_adapter_count(&self) -> usize {
        self.open_adapters.lock().len()
    }

    pub(crate) fn record_packet(&self, t_ms: u64, wall_ms: u64) {
        self.last_packet_ms.fetch_max(t_ms, Ordering::Relaxed);
        self.last_packet_wall_ms.store(wall_ms, Ordering::Relaxed);
    }

    /// Latest packet timestamp, 0 before any packet.
    pub fn last_packet_ms(&self) -> u64 {
        self.last_packet_ms.load(Ordering::Relaxed)
    }

    /// The clock the tracker should tick with: the capture clock while packets are flowing, the wall
    /// clock otherwise (so idle timeouts still fire when the game goes quiet).
    pub fn now(&self, wall_ms: u64) -> u64 {
        let last = self.last_packet_ms();
        let last_wall = self.last_packet_wall_ms.load(Ordering::Relaxed);
        if last > 0 && wall_ms.saturating_sub(last_wall) <= CLOCK_FRESH_MS {
            last
        } else {
            wall_ms
        }
    }
}

#[cfg(windows)]
fn system_root() -> std::path::PathBuf {
    std::env::var_os("SystemRoot").map(std::path::PathBuf::from).unwrap_or_else(|| r"C:\Windows".into())
}

/// True when wpcap.dll can be found. Npcap installs it under `System32\Npcap`, outside the normal DLL
/// search path, so that folder is added with `SetDllDirectoryW` before the delay loaded import resolves.
/// A WinPcap compatible install puts a copy in `System32` itself, which needs nothing.
#[cfg(windows)]
pub fn npcap_available() -> bool {
    use std::os::windows::ffi::OsStrExt;
    static DLL_DIR_SET: std::sync::Once = std::sync::Once::new();
    let system32 = system_root().join("System32");
    let npcap_dir = system32.join("Npcap");
    if npcap_dir.join("wpcap.dll").is_file() {
        DLL_DIR_SET.call_once(|| {
            let wide: Vec<u16> = npcap_dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
            // SAFETY: `wide` is a NUL terminated path that outlives the call.
            let ok = unsafe { windows_sys::Win32::System::LibraryLoader::SetDllDirectoryW(wide.as_ptr()) };
            if ok == 0 {
                tracing::warn!("SetDllDirectoryW({}) failed", npcap_dir.display());
            } else {
                tracing::info!("using Npcap from {}", npcap_dir.display());
            }
        });
        return true;
    }
    system32.join("wpcap.dll").is_file()
}

#[cfg(not(windows))]
pub fn npcap_available() -> bool {
    true
}

/// Whether the process token is elevated. Only used for the error text: without elevation Npcap may
/// refuse to open adapters, depending on how it was installed.
#[cfg(windows)]
pub fn is_admin() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    // SAFETY: plain Win32 calls with valid out pointers; the token handle is closed before returning.
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut TOKEN_ELEVATION as *mut core::ffi::c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

#[cfg(not(windows))]
pub fn is_admin() -> bool {
    true
}

fn adapter_info(dev: &pcap::Device) -> AdapterInfo {
    let description = dev.desc.clone().unwrap_or_default();
    let lower_name = dev.name.to_ascii_lowercase();
    let lower_desc = description.to_ascii_lowercase();
    let is_loopback = dev.flags.is_loopback() || lower_name.contains("loopback") || lower_desc.contains("loopback");
    let connected = matches!(dev.flags.connection_status, pcap::ConnectionStatus::Connected);
    let disconnected = matches!(dev.flags.connection_status, pcap::ConnectionStatus::Disconnected);
    let has_addresses = !dev.addresses.is_empty();
    AdapterInfo {
        name: dev.name.clone(),
        description,
        connected,
        has_addresses,
        is_loopback,
        usable: is_loopback || (has_addresses && !disconnected),
    }
}

/// Builds the BPF filter for the configured ports.
pub fn bpf_filter(ports: &[u16]) -> String {
    ports.iter().map(|p| format!("tcp port {p}")).collect::<Vec<_>>().join(" or ")
}

/// Stops the previous generation and opens every usable adapter with the configured ports.
/// The resulting status is written to `shared.status`.
///
/// Every handle is opened before any thread starts, and the status and the open adapter list are
/// written before that too: a thread that sees game traffic at once must find `Waiting` already in
/// place, or its `Capturing` would be overwritten and never announced again.
pub fn start(shared: &Arc<Shared>) {
    let state = &shared.capture;
    let _serialised = state.lock_restart();
    state.stop();
    let generation = state.generation();
    state.live_announced.store(false, Ordering::SeqCst);
    state.open_adapters.lock().clear();
    let ports = shared.ports.clone();

    if !npcap_available() {
        state.npcap_found.store(false, Ordering::Relaxed);
        shared.set_status(CaptureStatus::NpcapMissing);
        tracing::warn!("Npcap not found");
        return;
    }
    state.npcap_found.store(true, Ordering::Relaxed);

    let devices = match pcap::Device::list() {
        Ok(d) => d,
        Err(e) => {
            shared.set_status(CaptureStatus::Error { message: format!("Could not list network adapters: {e}") });
            tracing::warn!("pcap device list failed: {e}");
            return;
        }
    };
    *state.adapters.lock() = devices.iter().map(adapter_info).collect();

    let filter = bpf_filter(&ports);
    let mut opened: Vec<(pcap::Capture<pcap::Active>, LinkType, u32, String)> = Vec::new();
    let mut failures = Vec::new();
    for dev in devices {
        let info = adapter_info(&dev);
        if !info.usable {
            tracing::debug!("skipping adapter {} ({})", info.name, info.description);
            continue;
        }
        let label = if info.description.is_empty() { info.name.clone() } else { info.description.clone() };
        let capture = pcap::Capture::from_device(dev)
            .and_then(|c| c.promisc(false).snaplen(65_535).timeout(250).immediate_mode(true).open());
        let mut capture = match capture {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("could not open adapter {label}: {e}");
                failures.push(format!("{label}: {e}"));
                continue;
            }
        };
        if let Err(e) = capture.filter(&filter, true) {
            tracing::warn!("could not set filter on {label}: {e}");
            failures.push(format!("{label}: {e}"));
            continue;
        }
        let link_code = capture.get_datalink().0;
        let Some(link) = LinkType::from_pcap(link_code as u32) else {
            tracing::warn!("adapter {label} has unsupported link type {link_code}");
            continue;
        };
        opened.push((capture, link, link_code as u32, label));
    }

    if opened.is_empty() {
        let mut message = String::from("Could not open any network adapter.");
        if !is_admin() {
            message.push_str(" Try running aion as administrator.");
        }
        if let Some(first) = failures.first() {
            message.push_str(&format!(" ({first})"));
        }
        tracing::warn!("{message}");
        shared.set_status(CaptureStatus::Error { message });
        return;
    }

    let labels: Vec<String> = opened.iter().map(|(_, _, _, label)| label.clone()).collect();
    tracing::info!("capturing on {} adapter(s) with filter \"{filter}\": {}", labels.len(), labels.join(", "));
    *state.open_adapters.lock() = labels;
    shared.set_status(CaptureStatus::Waiting { adapters: opened.len() });

    for (capture, link, link_code, label) in opened {
        let thread_shared = shared.clone();
        let thread_label = label.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("capture {label}"))
            .spawn(move || capture_loop(thread_shared, capture, link, link_code, thread_label, generation));
        if let Err(e) = spawned {
            tracing::warn!("could not start capture thread for {label}: {e}");
            retire_adapter(&shared.capture.open_adapters, &shared.status, &label, &e.to_string());
        }
    }
}

/// Forgets an adapter the current generation can no longer read from. The waiting count follows, and
/// when it was the last one the status becomes an error so the setup view offers a retry.
fn retire_adapter(open_adapters: &parking_lot::Mutex<Vec<String>>, status: &parking_lot::Mutex<CaptureStatus>, label: &str, reason: &str) {
    let remaining = {
        let mut open = open_adapters.lock();
        open.retain(|l| l != label);
        open.len()
    };
    let mut status = status.lock();
    if remaining == 0 {
        *status = CaptureStatus::Error { message: format!("Capture stopped on {label}: {reason}. Retry capture to open the adapters again.") };
    } else if matches!(*status, CaptureStatus::Waiting { .. }) {
        *status = CaptureStatus::Waiting { adapters: remaining };
    }
}

fn capture_loop(shared: Arc<Shared>, mut capture: pcap::Capture<pcap::Active>, link: LinkType, link_code: u32, label: String, generation: u64) {
    let mut events: Vec<Event> = Vec::new();
    while shared.capture.generation() == generation {
        match capture.next_packet() {
            Ok(packet) => {
                let ts = &packet.header.ts;
                let t_ms = (ts.tv_sec as i64 * 1000 + ts.tv_usec as i64 / 1000).max(1) as u64;
                if let Some(recorder) = shared.recorder.get() {
                    let at = std::time::Duration::new(ts.tv_sec.max(0) as u64, (ts.tv_usec.clamp(0, 999_999) as u32) * 1_000);
                    recorder.write(&label, link_code, at, packet.data, packet.header.len);
                }
                let (has_flow, self_hint) = {
                    let mut pipe = shared.pipeline.lock();
                    pipe.push(link, t_ms, packet.data, &mut |event| events.push(event));
                    (pipe.has_game_flow(), pipe.parser.meter.stats.hint_self_id)
                };
                shared.capture.record_packet(t_ms, crate::now_ms());
                if has_flow
                    && shared.capture.generation() == generation
                    && !shared.capture.live_announced.swap(true, Ordering::SeqCst)
                {
                    tracing::info!("game traffic seen on {label}");
                    shared.set_status(CaptureStatus::Capturing { adapter: label.clone() });
                }
                if !events.is_empty() {
                    shared.dispatch(&mut events, self_hint);
                }
            }
            Err(pcap::Error::TimeoutExpired) => {}
            Err(e) => {
                tracing::warn!("capture on {label} stopped: {e}");
                // The adapter went away (unplugged, driver reset). Under the restart lock a restart
                // cannot slip in between the generation check and the bookkeeping.
                let _serialised = shared.capture.lock_restart();
                if shared.capture.generation() == generation {
                    retire_adapter(&shared.capture.open_adapters, &shared.status, &label, &e.to_string());
                }
                break;
            }
        }
    }
    tracing::debug!("capture thread for {label} (generation {generation}) exited");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_joins_ports() {
        assert_eq!(bpf_filter(&[13328]), "tcp port 13328");
        assert_eq!(bpf_filter(&[13328, 7777]), "tcp port 13328 or tcp port 7777");
    }

    #[test]
    fn capture_clock_falls_back_to_wall_time() {
        let state = CaptureState::default();
        assert_eq!(state.now(5_000), 5_000);
        state.record_packet(4_200, 5_000);
        assert_eq!(state.now(5_500), 4_200);
        assert_eq!(state.now(6_100), 6_100);
        state.record_packet(4_100, 6_200);
        assert_eq!(state.last_packet_ms(), 4_200, "the clock never runs backwards");
    }

    #[test]
    fn live_flag_is_announced_once_per_generation() {
        let state = CaptureState::default();
        assert!(!state.clear_live(), "nothing announced yet");
        assert!(!state.live_announced.swap(true, Ordering::SeqCst));
        assert!(state.live_announced.swap(true, Ordering::SeqCst), "second flow does not announce again");
        assert!(state.clear_live(), "the game went quiet");
        assert!(!state.live_announced.swap(true, Ordering::SeqCst), "and the next flow announces");
        let before = state.generation();
        state.stop();
        assert_eq!(state.generation(), before + 1);
    }

    #[test]
    fn retiring_adapters_updates_the_status() {
        let open = parking_lot::Mutex::new(vec!["Wi-Fi".to_string(), "Ethernet".to_string()]);
        let status = parking_lot::Mutex::new(CaptureStatus::Waiting { adapters: 2 });
        retire_adapter(&open, &status, "Wi-Fi", "device removed");
        assert_eq!(*open.lock(), vec!["Ethernet".to_string()]);
        assert_eq!(*status.lock(), CaptureStatus::Waiting { adapters: 1 });
        *status.lock() = CaptureStatus::Capturing { adapter: "Ethernet".into() };
        retire_adapter(&open, &status, "not open", "ignored");
        assert_eq!(*status.lock(), CaptureStatus::Capturing { adapter: "Ethernet".into() }, "a stray label changes nothing");
        retire_adapter(&open, &status, "Ethernet", "driver reset");
        assert!(open.lock().is_empty());
        let last = status.lock().clone();
        match last {
            CaptureStatus::Error { message } => assert!(message.contains("Ethernet") && message.contains("driver reset"), "{message}"),
            other => panic!("expected an error status, got {other:?}"),
        }
    }

    #[test]
    fn status_serialises_with_a_state_tag() {
        let json = serde_json::to_string(&CaptureStatus::Waiting { adapters: 2 }).unwrap();
        assert_eq!(json, r#"{"state":"waiting","adapters":2}"#);
        let json = serde_json::to_string(&CaptureStatus::NpcapMissing).unwrap();
        assert_eq!(json, r#"{"state":"npcapMissing"}"#);
    }
}
