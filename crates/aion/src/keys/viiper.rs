//! A virtual USB keyboard and mouse through a VIIPER server, ported from key-mouse's `keymouse.py`.
//!
//! VIIPER (`viiper.exe`) emulates the USB devices and usbip-win2 attaches them, so Windows sees real hardware.
//! Its API is a line protocol on 127.0.0.1:3242: one request (`command payload\0`) per connection, a JSON reply.
//! Each device then gets a stream connection that takes binary input reports.

use serde_json::Value;
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const API_ADDR: &str = "127.0.0.1:3242";
/// VIIPER keeps only the newest state until Windows polls the device (every 2 ms), so each state has to stay
/// put for a moment or Windows never sees it.
const KEY_HOLD: Duration = Duration::from_millis(20);
const MOUSE_STEP: Duration = Duration::from_millis(8);

pub const BUTTON_LEFT: u8 = 0x01;
pub const BUTTON_RIGHT: u8 = 0x02;

#[derive(Debug)]
pub enum Error {
    /// Nothing listens on the API port.
    NoServer,
    Io(std::io::Error),
    /// VIIPER answered with an error object.
    Api(String),
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NoServer => write!(f, "no VIIPER server on {API_ADDR}"),
            Error::Io(e) => write!(f, "{e}"),
            Error::Api(e) | Error::Other(e) => write!(f, "{e}"),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::ConnectionRefused {
            Error::NoServer
        } else {
            Error::Io(e)
        }
    }
}

fn addr() -> SocketAddr {
    API_ADDR.parse().expect("valid API address")
}

/// Sends one request to the VIIPER API and returns its JSON reply.
pub fn api(command: &str, payload: Option<&str>, timeout: Duration) -> Result<Value, Error> {
    let mut stream = TcpStream::connect_timeout(&addr(), timeout.min(Duration::from_secs(5)))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let line = match payload {
        Some(p) => format!("{command} {p}\0"),
        None => format!("{command}\0"),
    };
    stream.write_all(line.as_bytes())?;
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply)?;
    let text = String::from_utf8_lossy(&reply);
    let text = text.trim_end_matches(['\0', '\r', '\n', ' ']);
    if text.is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    let value: Value = serde_json::from_str(text).map_err(|e| Error::Other(format!("{command}: bad reply {text:?}: {e}")))?;
    if let (Some(status), Some(title)) = (value.get("status"), value.get("title")) {
        let detail = value.get("detail").and_then(Value::as_str).unwrap_or("");
        return Err(Error::Api(format!("{command}: {status} {title}: {detail}")));
    }
    Ok(value)
}

pub fn ping() -> bool {
    api("ping", None, Duration::from_secs(2)).is_ok()
}

/// Starts `viiper.exe` without a window unless a server already answers. Returns the process when it was
/// started here, so it can be stopped again on the way out.
pub fn ensure_server(exe: &Path, timeout: Duration) -> Result<Option<Child>, Error> {
    if ping() {
        return Ok(None);
    }
    if !exe.is_file() {
        return Err(Error::Other(format!("viiper.exe not found at {}", exe.display())));
    }
    let mut cmd = Command::new(exe);
    // USBIP on [::1] because usbip-win2 dials "localhost", which tries ::1 first and waits 2 s for it to be
    // refused before falling back to 127.0.0.1, on every device add (key-mouse's start-server.cmd).
    cmd.args(["server", "--update-notify=none", "--usb.addr=[::1]:3241", &format!("--api.addr={API_ADDR}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(dir) = exe.parent() {
        cmd.current_dir(dir);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| Error::Other(format!("could not start {}: {e}", exe.display())))?;
    let deadline = Instant::now() + timeout;
    loop {
        if ping() {
            return Ok(Some(child));
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(Error::Other(format!("viiper.exe exited at once ({status}); is another server holding port 3241?")));
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            return Err(Error::Other("VIIPER did not answer within 10 s".into()));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// A JSON id as VIIPER wants it back in a path: numbers bare, strings as they are.
fn id_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// One device on a bus and its input stream.
struct Device {
    bus: String,
    dev: String,
    stream: TcpStream,
}

impl Device {
    fn add(bus: &str, kind: &str) -> Result<Device, Error> {
        // Adding waits for Windows to attach the device, which can take a while when other virtual devices are
        // being detached at the same time.
        let info = api(&format!("bus/{bus}/add"), Some(&format!("{{\"type\":\"{kind}\"}}")), Duration::from_secs(30))?;
        let (Some(bus), Some(dev)) = (info.get("busId"), info.get("devId")) else {
            return Err(Error::Other(format!("bus/add {kind}: unexpected reply {info}")));
        };
        let (bus, dev) = (id_text(bus), id_text(dev));
        // VIIPER drops the device if this stream is not opened within 5 s.
        let mut stream = TcpStream::connect_timeout(&addr(), Duration::from_secs(5))?;
        stream.set_nodelay(true)?;
        stream.write_all(format!("bus/{bus}/{dev}\0").as_bytes())?;
        Ok(Device { bus, dev, stream })
    }

    fn close(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
        let _ = api(&format!("bus/{}/remove", self.bus), Some(&self.dev), Duration::from_secs(5));
    }
}

pub struct Keyboard {
    device: Device,
    mods: u8,
    keys: BTreeSet<u8>,
    /// Only the device test reads it back.
    #[cfg_attr(not(test), allow(dead_code))]
    leds: Arc<Leds>,
}

/// The lock key LEDs as Windows last set them on this keyboard: bit 0 NumLock, bit 1 CapsLock, bit 2 ScrollLock.
#[derive(Default)]
struct Leds {
    state: AtomicU8,
    reports: AtomicUsize,
}

impl Keyboard {
    fn add(bus: &str) -> Result<Keyboard, Error> {
        let device = Device::add(bus, "keyboard")?;
        let leds = Arc::new(Leds::default());
        // Windows writes the lock key LEDs back on this stream; read them so the socket never fills up.
        if let Ok(mut reader) = device.stream.try_clone() {
            let leds = leds.clone();
            let _ = std::thread::Builder::new().name("keyboard leds".into()).spawn(move || {
                let mut buf = [0u8; 64];
                while let Ok(n @ 1..) = reader.read(&mut buf) {
                    leds.state.store(buf[n - 1], Ordering::Relaxed);
                    leds.reports.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
        Ok(Keyboard { device, mods: 0, keys: BTreeSet::new(), leds })
    }

    /// The LED state and how many LED reports Windows has sent so far.
    #[cfg(test)]
    fn leds(&self) -> (u8, usize) {
        (self.leds.state.load(Ordering::Relaxed), self.leds.reports.load(Ordering::Relaxed))
    }

    fn send(&mut self) -> std::io::Result<()> {
        let mut report = vec![self.mods, self.keys.len() as u8];
        report.extend(self.keys.iter());
        self.device.stream.write_all(&report)?;
        std::thread::sleep(KEY_HOLD);
        Ok(())
    }

    /// Holds a key (a HID usage) down until `release`.
    pub fn press(&mut self, usage: u8) -> std::io::Result<()> {
        self.keys.insert(usage);
        self.send()
    }

    pub fn release(&mut self, usage: u8) -> std::io::Result<()> {
        self.keys.remove(&usage);
        self.send()
    }

    pub fn release_all(&mut self) -> std::io::Result<()> {
        self.mods = 0;
        self.keys.clear();
        self.send()
    }
}

pub struct Mouse {
    device: Device,
    buttons: u8,
}

impl Mouse {
    fn add(bus: &str) -> Result<Mouse, Error> {
        Ok(Mouse { device: Device::add(bus, "mouse")?, buttons: 0 })
    }

    fn send(&mut self) -> std::io::Result<()> {
        // buttons, dx, dy, wheel, pan: little endian, the movement fields zero.
        let mut report = [0u8; 9];
        report[0] = self.buttons;
        self.device.stream.write_all(&report)?;
        std::thread::sleep(MOUSE_STEP);
        Ok(())
    }

    pub fn press(&mut self, buttons: u8) -> std::io::Result<()> {
        self.buttons |= buttons;
        self.send()
    }

    pub fn release_all(&mut self) -> std::io::Result<()> {
        self.buttons = 0;
        self.send()
    }
}

/// One virtual bus holding a keyboard and a mouse. They vanish again on drop.
pub struct Devices {
    bus: String,
    pub keyboard: Keyboard,
    pub mouse: Mouse,
}

impl Devices {
    /// Creates the bus and both devices, then waits until Windows has loaded drivers for them: input sent before
    /// that is lost.
    pub fn create(timeout: Duration) -> Result<Devices, Error> {
        let before = sys::input_devices();
        let created = api("bus/create", None, Duration::from_secs(10))?;
        let bus = created.get("busId").map(id_text).ok_or_else(|| Error::Other(format!("bus/create: unexpected reply {created}")))?;
        let remove_bus = |bus: &str| {
            let _ = api("bus/remove", Some(bus), Duration::from_secs(5));
        };
        let mut keyboard = match Keyboard::add(&bus) {
            Ok(k) => k,
            Err(e) => {
                remove_bus(&bus);
                return Err(e);
            }
        };
        let mouse = match Mouse::add(&bus) {
            Ok(m) => m,
            Err(e) => {
                keyboard.device.close();
                remove_bus(&bus);
                return Err(e);
            }
        };
        let devices = Devices { bus, keyboard, mouse };
        let deadline = Instant::now() + timeout;
        loop {
            let new: Vec<u32> = sys::input_devices().difference(&before).map(|&(_, kind)| kind).collect();
            if new.contains(&sys::RIM_TYPEMOUSE) && new.contains(&sys::RIM_TYPEKEYBOARD) {
                break;
            }
            if Instant::now() > deadline {
                return Err(Error::Other("Windows did not pick up the virtual devices, is usbip-win2 installed?".into()));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_millis(200));
        Ok(devices)
    }

    /// Lets go of every key and button.
    pub fn release_all(&mut self) {
        let _ = self.mouse.release_all();
        let _ = self.keyboard.release_all();
    }
}

impl Drop for Devices {
    fn drop(&mut self) {
        self.release_all();
        self.keyboard.device.close();
        self.mouse.device.close();
        let _ = api("bus/remove", Some(&self.bus), Duration::from_secs(5));
    }
}

/// Set 1 scan code to HID usage for the keys that produce characters: Windows reports keys by scan code, USB
/// by HID usage.
fn scan_to_hid(scan: u32) -> Option<u8> {
    Some(match scan {
        0x02..=0x0B => 0x1E + (scan - 0x02) as u8, // 1 to 0
        0x0C => 0x2D,
        0x0D => 0x2E,
        0x10 => 0x14,
        0x11 => 0x1A,
        0x12 => 0x08,
        0x13 => 0x15,
        0x14 => 0x17,
        0x15 => 0x1C,
        0x16 => 0x18,
        0x17 => 0x0C,
        0x18 => 0x12,
        0x19 => 0x13,
        0x1A => 0x2F,
        0x1B => 0x30,
        0x1E => 0x04,
        0x1F => 0x16,
        0x20 => 0x07,
        0x21 => 0x09,
        0x22 => 0x0A,
        0x23 => 0x0B,
        0x24 => 0x0D,
        0x25 => 0x0E,
        0x26 => 0x0F,
        0x27 => 0x33,
        0x28 => 0x34,
        0x29 => 0x35,
        0x2B => 0x31,
        0x2C => 0x1D,
        0x2D => 0x1B,
        0x2E => 0x06,
        0x2F => 0x19,
        0x30 => 0x05,
        0x31 => 0x11,
        0x32 => 0x10,
        0x33 => 0x36,
        0x34 => 0x37,
        0x35 => 0x38,
        0x39 => 0x2C,
        0x56 => 0x64, // the extra key left of Y/Z on ISO keyboards (< > |)
        _ => return None,
    })
}

/// The HID usage of the key that types `ch` on the keyboard layout of the foreground window, which is where the
/// key lands.
pub fn usage_for(ch: char) -> Result<u8, String> {
    let scan = sys::scan_code(ch).ok_or_else(|| format!("{ch:?} cannot be typed with the current keyboard layout"))?;
    scan_to_hid(scan).ok_or_else(|| format!("{ch:?} is not on a character key"))
}

#[cfg(windows)]
pub mod sys {
    use std::collections::HashSet;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, MapVirtualKeyExW, VkKeyScanExW, MAPVK_VK_TO_VSC};
    use windows_sys::Win32::UI::Input::{GetRawInputDeviceList, RAWINPUTDEVICELIST};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    pub const RIM_TYPEMOUSE: u32 = 0;
    pub const RIM_TYPEKEYBOARD: u32 = 1;

    /// `(handle, type)` of every input device Windows has drivers loaded for.
    pub fn input_devices() -> HashSet<(usize, u32)> {
        let size = std::mem::size_of::<RAWINPUTDEVICELIST>() as u32;
        let mut count = 0u32;
        // SAFETY: a null list asks for the count only.
        unsafe { GetRawInputDeviceList(std::ptr::null_mut(), &mut count, size) };
        let mut list: Vec<RAWINPUTDEVICELIST> = Vec::with_capacity(count as usize + 8);
        let mut room = list.capacity() as u32;
        // SAFETY: `list` has room for `room` entries; the call writes at most that many and returns how many.
        let n = unsafe { GetRawInputDeviceList(list.as_mut_ptr(), &mut room, size) };
        if n == u32::MAX {
            return HashSet::new();
        }
        // SAFETY: the call initialised the first `n` entries.
        unsafe { list.set_len(n as usize) };
        list.iter().map(|d| (d.hDevice as usize, d.dwType)).collect()
    }

    /// The set 1 scan code of the key that types `ch` on the foreground window's layout.
    pub fn scan_code(ch: char) -> Option<u32> {
        let mut units = [0u16; 2];
        if ch.encode_utf16(&mut units).len() != 1 {
            return None;
        }
        // SAFETY: plain Win32 calls with value arguments.
        unsafe {
            let thread = GetWindowThreadProcessId(GetForegroundWindow(), std::ptr::null_mut());
            let layout = GetKeyboardLayout(thread);
            let res = VkKeyScanExW(units[0], layout);
            if res == -1 {
                return None;
            }
            let vk = (res as u16 & 0xFF) as u32;
            Some(MapVirtualKeyExW(vk, MAPVK_VK_TO_VSC, layout))
        }
    }
}

#[cfg(not(windows))]
pub mod sys {
    use std::collections::HashSet;
    pub const RIM_TYPEMOUSE: u32 = 0;
    pub const RIM_TYPEKEYBOARD: u32 = 1;
    pub fn input_devices() -> HashSet<(usize, u32)> {
        HashSet::new()
    }
    pub fn scan_code(_ch: char) -> Option<u32> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_codes_map_to_hid_usages() {
        assert_eq!(scan_to_hid(0x02), Some(0x1E), "1");
        assert_eq!(scan_to_hid(0x0B), Some(0x27), "0");
        assert_eq!(scan_to_hid(0x21), Some(0x09), "f");
        assert_eq!(scan_to_hid(0x01), None, "escape is not a character key");
    }

    /// Asks a running VIIPER server for `ping`; adds no device: `cargo test -p aion viiper_ping -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn viiper_ping() {
        match api("ping", None, Duration::from_secs(2)) {
            Ok(reply) => println!("VIIPER answered: {reply}"),
            Err(e) => println!("no answer: {e}"),
        }
        println!("'1' is HID usage {:?}, 'f' is {:?} on this layout", usage_for('1'), usage_for('f'));
    }

    /// Adds the keyboard and mouse, taps Scroll Lock twice (Windows answers each tap with the new LED state, which
    /// works whatever window is in front) and moves the pointer right and back, then removes both. Clicks and
    /// types nothing: `cargo test -p aion devices_reach_windows -- --ignored --nocapture`.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn devices_reach_windows() {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
        const SCROLL_LOCK: u8 = 0x47;
        const SCROLL_LED: u8 = 0x04;
        let cursor = || {
            let mut p = POINT { x: 0, y: 0 };
            // SAFETY: `p` is a valid out pointer.
            unsafe { GetCursorPos(&mut p) };
            (p.x, p.y)
        };

        let exe = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../bin/viiper.exe");
        let mut server = ensure_server(&exe, Duration::from_secs(10)).expect("VIIPER server");
        println!("server: {}", if server.is_some() { "started here" } else { "already running" });
        let before = sys::input_devices();
        let started = Instant::now();
        let mut devices = Devices::create(Duration::from_secs(15)).expect("devices");
        println!("devices ready in {:.1} s on bus {}", started.elapsed().as_secs_f64(), devices.bus);
        println!("keyboard: LEDs on arrival {:?} (state, reports)", devices.keyboard.leds());

        // Each tap should bring one more LED report with Scroll Lock flipped; two taps leave it as it was.
        let mut scroll = Vec::new();
        for _ in 0..2 {
            let (_, reports) = devices.keyboard.leds();
            devices.keyboard.press(SCROLL_LOCK).unwrap();
            devices.keyboard.release_all().unwrap();
            let deadline = Instant::now() + Duration::from_secs(1);
            while devices.keyboard.leds().1 == reports && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            let (state, now) = devices.keyboard.leds();
            scroll.push((now > reports).then_some(state & SCROLL_LED != 0));
        }
        println!("keyboard: Scroll Lock LED after each tap {scroll:?}");

        let mut nudge = |dx: i16| {
            let from = cursor();
            let mut report = [0u8; 9];
            report[1..3].copy_from_slice(&dx.to_le_bytes());
            devices.mouse.device.stream.write_all(&report).unwrap();
            std::thread::sleep(Duration::from_millis(40));
            cursor().0 - from.0
        };
        let home = cursor();
        let (right, left) = (nudge(20), nudge(-20));
        println!("mouse: pointer at {home:?} moved {right:+} px then {left:+} px, now at {:?}", cursor());

        drop(devices);
        let deadline = Instant::now() + Duration::from_secs(10);
        while sys::input_devices().difference(&before).next().is_some() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        let devices_left = sys::input_devices().difference(&before).count();
        println!("devices left after removal: {devices_left}");
        if let Some(child) = server.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        match scroll[..] {
            [Some(a), Some(b)] => assert_ne!(a, b, "Scroll Lock did not toggle back"),
            _ => panic!("Windows did not answer the virtual keyboard's Scroll Lock taps"),
        }
        assert!(right > 0 && left < 0, "the virtual mouse did not move the pointer right and back");
        assert_eq!(devices_left, 0, "the virtual devices were not removed");
    }

    #[test]
    fn ids_go_back_into_paths_bare() {
        assert_eq!(id_text(&serde_json::json!(3)), "3");
        assert_eq!(id_text(&serde_json::json!("1-2")), "1-2");
    }
}
