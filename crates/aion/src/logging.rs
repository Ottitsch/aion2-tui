//! Logging: `tracing` into a log file in the data dir. Nothing goes to the terminal, which belongs to the
//! TUI. The file is rotated once when it grows past a few megabytes, keeping one old copy.

use std::fs::File;
use std::path::Path;
use std::sync::Mutex;

const ROTATE_BYTES: u64 = 4 * 1024 * 1024;

/// Installs the global subscriber writing to `path`. `RUST_LOG` overrides the default filter. Without a
/// usable file nothing is logged.
pub fn init(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() > ROTATE_BYTES {
            let old = path.with_extension("old.log");
            let _ = std::fs::remove_file(&old);
            let _ = std::fs::rename(path, &old);
        }
    }
    let Ok(file) = File::options().create(true).append(true).open(path) else { return };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_ansi(false).with_target(false).with_writer(Mutex::new(file)).try_init();
}
