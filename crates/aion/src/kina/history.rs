//! Finished sessions on disk: `sessions/<id>.json` per session plus `sessions/index.json` holding the compact
//! summaries, newest first. The index is what the UI lists; a full session is loaded on demand.

use kina_core::snapshot::{EndReason, SessionSnapshot, SessionSummary};
use crate::files::write_atomic;
use std::path::{Path, PathBuf};

/// Oldest files are deleted past this many sessions.
pub const MAX_SESSIONS: usize = 1_000;

const INDEX_FILE: &str = "index.json";
/// The running session, rewritten every half minute. Left behind only by a crash or a forced shutdown.
const LIVE_FILE: &str = "live.json";

pub struct History {
    dir: PathBuf,
    /// Newest first.
    index: Vec<SessionSummary>,
}

impl History {
    /// Creates the directory if needed and loads the index. A missing or unreadable index is rebuilt from
    /// the session files that are present.
    pub fn open(dir: PathBuf) -> History {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!("could not create {}: {e}", dir.display());
        }
        let index_path = dir.join(INDEX_FILE);
        let index = match std::fs::read_to_string(&index_path) {
            Ok(text) => match serde_json::from_str::<Vec<SessionSummary>>(&text) {
                Ok(mut index) => {
                    sort_newest_first(&mut index);
                    Some(index)
                }
                Err(e) => {
                    tracing::warn!("session index {} is not valid, rebuilding: {e}", index_path.display());
                    None
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                tracing::warn!("could not read {}: {e}, rebuilding", index_path.display());
                None
            }
        };
        let mut history = History { dir, index: index.unwrap_or_default() };
        if history.index.is_empty() {
            history.rebuild_index();
        }
        history
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Highest session id on disk, 0 when empty. New sessions continue after it.
    pub fn max_id(&self) -> u64 {
        self.index.iter().map(|s| s.id).max().unwrap_or(0)
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// Newest first, at most `limit` entries.
    pub fn summaries(&self, limit: usize) -> Vec<SessionSummary> {
        self.index.iter().take(limit).cloned().collect()
    }

    /// A session where the balance never moved is not worth a file.
    pub fn worth_keeping(snap: &SessionSnapshot) -> bool {
        snap.changes > 0
    }

    /// Stores a finished session. Returns `Ok(false)` when it was skipped by `worth_keeping`.
    pub fn push(&mut self, snap: &SessionSnapshot) -> std::io::Result<bool> {
        if !Self::worth_keeping(snap) {
            return Ok(false);
        }
        let bytes = serde_json::to_vec(snap).map_err(std::io::Error::other)?;
        write_atomic(&self.file_path(snap.id), &bytes)?;
        self.index.retain(|s| s.id != snap.id);
        self.index.insert(0, snap.summary());
        sort_newest_first(&mut self.index);
        while self.index.len() > MAX_SESSIONS {
            if let Some(old) = self.index.pop() {
                remove_if_present(&self.file_path(old.id));
            }
        }
        self.save_index()?;
        Ok(true)
    }

    pub fn get(&self, id: u64) -> Option<SessionSnapshot> {
        let path = self.file_path(id);
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(snap) => Some(snap),
                Err(e) => {
                    tracing::warn!("session file {} is not valid: {e}", path.display());
                    None
                }
            },
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!("could not read {}: {e}", path.display());
                }
                None
            }
        }
    }

    pub fn delete(&mut self, id: u64) -> std::io::Result<()> {
        remove_if_present(&self.file_path(id));
        let before = self.index.len();
        self.index.retain(|s| s.id != id);
        if self.index.len() != before {
            self.save_index()?;
        }
        Ok(())
    }

    /// Writes the running session so a crash loses at most the last half minute.
    pub fn save_live(&self, snap: &SessionSnapshot) -> std::io::Result<()> {
        let bytes = serde_json::to_vec(snap).map_err(std::io::Error::other)?;
        write_atomic(&self.dir.join(LIVE_FILE), &bytes)
    }

    /// The running session ended and was filed (or was never worth keeping).
    pub fn clear_live(&self) {
        remove_if_present(&self.dir.join(LIVE_FILE));
    }

    /// Files a live copy left behind by a crash or a forced shutdown as a finished session.
    pub fn recover_live(&mut self) {
        let path = self.dir.join(LIVE_FILE);
        let Ok(text) = std::fs::read_to_string(&path) else { return };
        match serde_json::from_str::<SessionSnapshot>(&text) {
            Ok(mut snap) => {
                snap.active = false;
                snap.end_reason.get_or_insert(EndReason::AppExit);
                for run in &mut snap.runs {
                    run.current = false;
                }
                if self.index.iter().any(|s| s.id == snap.id) {
                    tracing::info!("live session {} is already in the history", snap.id);
                } else {
                    match self.push(&snap) {
                        Ok(_) => tracing::info!("recovered session {} left behind by the last run", snap.id),
                        Err(e) => tracing::warn!("could not recover session {}: {e}", snap.id),
                    }
                }
            }
            Err(e) => tracing::warn!("live session file {} is not valid: {e}", path.display()),
        }
        remove_if_present(&path);
    }

    fn file_path(&self, id: u64) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    fn save_index(&self) -> std::io::Result<()> {
        let bytes = serde_json::to_vec(&self.index).map_err(std::io::Error::other)?;
        write_atomic(&self.dir.join(INDEX_FILE), &bytes)
    }

    /// Reads every `<id>.json` in the directory and rebuilds the index from their summaries.
    fn rebuild_index(&mut self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return };
        let mut index = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let numeric_stem = path.file_stem().and_then(|s| s.to_str()).map(|s| s.parse::<u64>().is_ok()).unwrap_or(false);
            if !numeric_stem || path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Ok(snap) = serde_json::from_str::<SessionSnapshot>(&text) {
                    index.push(snap.summary());
                }
            }
        }
        if index.is_empty() {
            return;
        }
        sort_newest_first(&mut index);
        self.index = index;
        if let Err(e) = self.save_index() {
            tracing::warn!("could not write the rebuilt session index: {e}");
        } else {
            tracing::info!("rebuilt the session index from {} files", self.index.len());
        }
    }
}

fn sort_newest_first(index: &mut [SessionSummary]) {
    index.sort_by(|a, b| b.started_ms.cmp(&a.started_ms).then(b.id.cmp(&a.id)));
}

fn remove_if_present(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!("could not delete {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kina_core::snapshot::Character;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A fresh directory per test under the system temp dir, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let dir = std::env::temp_dir().join(format!("aion2kina-history-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn session(id: u64, started_ms: u64) -> SessionSnapshot {
        SessionSnapshot {
            id,
            started_ms,
            last_ms: started_ms + 3_600_000,
            duration_ms: 3_600_000,
            end_reason: Some(EndReason::Manual),
            character: Some(Character { name: "Seraphel".into(), server: 2314, level: 45 }),
            start_balance: Some(1_000_000),
            balance: Some(1_000_000 + 100_000 * id),
            earned: 100_000 * id,
            net: 100_000 * id as i64,
            net_per_hour: 100_000.0 * id as f64,
            changes: 3,
            ..Default::default()
        }
    }

    #[test]
    fn round_trips_through_the_index() {
        let tmp = TempDir::new();
        let dir = tmp.0.join("sessions");
        {
            let mut h = History::open(dir.clone());
            assert!(h.is_empty());
            assert_eq!(h.max_id(), 0);
            assert!(h.push(&session(1, 1_000)).unwrap());
            assert!(h.push(&session(3, 3_000)).unwrap());
            assert!(h.push(&session(2, 2_000)).unwrap());
            assert_eq!(h.len(), 3);
            assert_eq!(h.max_id(), 3);
            let ids: Vec<u64> = h.summaries(10).iter().map(|s| s.id).collect();
            assert_eq!(ids, vec![3, 2, 1]);
        }
        let h = History::open(dir.clone());
        let ids: Vec<u64> = h.summaries(10).iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![3, 2, 1]);
        assert_eq!(h.summaries(10)[0].character_name.as_deref(), Some("Seraphel"));
        assert_eq!(h.summaries(10)[0].net, 300_000);
        assert_eq!(h.get(2).unwrap(), session(2, 2_000));
        assert!(h.get(99).is_none());
        assert!(dir.join("index.json").exists());
    }

    #[test]
    fn delete_removes_file_and_entry() {
        let tmp = TempDir::new();
        let mut h = History::open(tmp.0.clone());
        h.push(&session(5, 5_000)).unwrap();
        h.push(&session(6, 6_000)).unwrap();
        h.delete(5).unwrap();
        assert!(!tmp.0.join("5.json").exists());
        assert_eq!(h.summaries(10).len(), 1);
        h.delete(5).unwrap();
        let again = History::open(tmp.0.clone());
        assert_eq!(again.summaries(10)[0].id, 6);
    }

    #[test]
    fn skips_sessions_without_changes() {
        let tmp = TempDir::new();
        let mut h = History::open(tmp.0.clone());
        let mut idle = session(1, 1_000);
        idle.changes = 0;
        assert!(!h.push(&idle).unwrap());
        assert!(h.is_empty());
    }

    #[test]
    fn caps_and_drops_oldest_files() {
        let tmp = TempDir::new();
        let mut h = History::open(tmp.0.clone());
        for id in 1..=(MAX_SESSIONS as u64 + 2) {
            h.push(&session(id, id * 1_000)).unwrap();
        }
        assert_eq!(h.len(), MAX_SESSIONS);
        assert!(!tmp.0.join("1.json").exists());
        assert!(!tmp.0.join("2.json").exists());
        assert!(tmp.0.join("3.json").exists());
    }

    #[test]
    fn recovers_a_live_session_left_behind() {
        let tmp = TempDir::new();
        let mut live = session(4, 4_000);
        live.active = true;
        live.end_reason = None;
        {
            let h = History::open(tmp.0.clone());
            h.save_live(&live).unwrap();
        }
        let mut h = History::open(tmp.0.clone());
        assert!(h.is_empty(), "live.json is not a session file");
        h.recover_live();
        assert_eq!(h.len(), 1);
        let back = h.get(4).unwrap();
        assert!(!back.active);
        assert_eq!(back.end_reason, Some(EndReason::AppExit));
        assert!(!tmp.0.join(LIVE_FILE).exists());
        h.save_live(&live).unwrap();
        h.recover_live();
        assert_eq!(h.len(), 1, "a session already filed is not filed twice");
        h.save_live(&live).unwrap();
        h.clear_live();
        assert!(!tmp.0.join(LIVE_FILE).exists());
    }

    #[test]
    fn rebuilds_index_from_files() {
        let tmp = TempDir::new();
        {
            let mut h = History::open(tmp.0.clone());
            h.push(&session(7, 7_000)).unwrap();
            h.push(&session(8, 8_000)).unwrap();
        }
        std::fs::remove_file(tmp.0.join("index.json")).unwrap();
        let h = History::open(tmp.0.clone());
        let ids: Vec<u64> = h.summaries(10).iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![8, 7]);
        std::fs::write(tmp.0.join("index.json"), "garbage").unwrap();
        assert_eq!(History::open(tmp.0.clone()).len(), 2);
    }
}
