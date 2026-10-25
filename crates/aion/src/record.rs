//! `--record <file>`: every captured game packet, from every adapter, written to a pcapng file as it arrives, for
//! finding new fields in the traffic (a daily limit, anything that moved after a patch). The file is flushed every
//! second, so a crash or a closed window loses at most that much. It holds your own traffic, character names
//! included; the replay tools read it like any other capture (`kina-core`'s `replay <file> --find <value>`).

use pcap_file::pcapng::blocks::enhanced_packet::EnhancedPacketBlock;
use pcap_file::pcapng::blocks::interface_description::{InterfaceDescriptionBlock, InterfaceDescriptionOption};
use pcap_file::pcapng::PcapNgWriter;
use pcap_file::DataLink;
use parking_lot::Mutex;
use std::borrow::Cow;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const FLUSH_EVERY: Duration = Duration::from_secs(1);

pub struct Recorder {
    path: PathBuf,
    inner: Mutex<Option<Inner>>,
    bytes: AtomicU64,
}

struct Inner {
    writer: PcapNgWriter<BufWriter<File>>,
    /// Adapter labels by pcapng interface id.
    interfaces: Vec<String>,
    last_flush: Instant,
}

impl Recorder {
    pub fn create(path: &Path) -> anyhow::Result<Recorder> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let file = BufWriter::new(File::create(path)?);
        let writer = PcapNgWriter::new(file)?;
        tracing::info!("recording the game traffic to {}", path.display());
        Ok(Recorder {
            path: path.to_path_buf(),
            inner: Mutex::new(Some(Inner { writer, interfaces: Vec::new(), last_flush: Instant::now() })),
            bytes: AtomicU64::new(0),
        })
    }

    /// Appends one captured frame. The first frame of an adapter declares it in the file. A write error stops
    /// the recording (and says so in the log) without touching the capture.
    pub fn write(&self, adapter: &str, link: u32, timestamp: Duration, data: &[u8], original_len: u32) {
        let mut guard = self.inner.lock();
        let Some(inner) = guard.as_mut() else { return };
        let written = (|| -> anyhow::Result<()> {
            let id = match inner.interfaces.iter().position(|a| a == adapter) {
                Some(id) => id,
                None => {
                    inner.writer.write_pcapng_block(InterfaceDescriptionBlock {
                        linktype: DataLink::from(link),
                        snaplen: 65_535,
                        // pcap-file writes nanoseconds; the option makes Wireshark read them as such too.
                        options: vec![InterfaceDescriptionOption::IfName(Cow::Owned(adapter.to_owned())), InterfaceDescriptionOption::IfTsResol(9)],
                    })?;
                    inner.interfaces.push(adapter.to_owned());
                    inner.interfaces.len() - 1
                }
            };
            let n = inner.writer.write_pcapng_block(EnhancedPacketBlock {
                interface_id: id as u32,
                timestamp,
                original_len,
                data: Cow::Borrowed(data),
                options: Vec::new(),
            })?;
            self.bytes.fetch_add(n as u64, Ordering::Relaxed);
            if inner.last_flush.elapsed() >= FLUSH_EVERY {
                inner.writer.get_mut().flush()?;
                inner.last_flush = Instant::now();
            }
            Ok(())
        })();
        if let Err(e) = written {
            tracing::warn!("recording to {} stopped: {e}", self.path.display());
            *guard = None;
        }
    }

    pub fn flush(&self) {
        if let Some(inner) = self.inner.lock().as_mut() {
            let _ = inner.writer.get_mut().flush();
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Bytes written so far.
    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    /// False once a write failed.
    pub fn is_recording(&self) -> bool {
        self.inner.lock().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recording_reads_back_with_its_adapters_and_times() {
        let dir = std::env::temp_dir().join(format!("aion-record-{}", std::process::id()));
        let path = dir.join("test.pcapng");
        let rec = Recorder::create(&path).unwrap();
        rec.write("Ethernet", 1, Duration::from_millis(1_761_400_000_123), &[1, 2, 3], 3);
        rec.write("Loopback", 0, Duration::from_millis(1_761_400_000_456), &[4, 5], 2);
        rec.write("Ethernet", 1, Duration::from_millis(1_761_400_000_789), &[6], 1);
        rec.flush();
        assert!(rec.bytes() > 0 && rec.is_recording());
        let mut seen = Vec::new();
        aion_net::capture_file::read_packets(&std::fs::read(&path).unwrap(), |link, t, data| seen.push((link, t, data.to_vec()))).unwrap();
        assert_eq!(seen, vec![(1, 1_761_400_000_123, vec![1, 2, 3]), (0, 1_761_400_000_456, vec![4, 5]), (1, 1_761_400_000_789, vec![6])]);
        drop(rec);
        let _ = std::fs::remove_dir_all(dir);
    }
}
