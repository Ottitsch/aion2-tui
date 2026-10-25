//! Finished encounters on disk: `encounters/<id>.json` per fight plus `encounters/index.json` holding the
//! compact summaries, newest first. The index is what the UI lists; a full snapshot is loaded on demand.

use crate::files::write_atomic;
use meter_core::snapshot::{EncounterSummary, Snapshot};
use std::path::{Path, PathBuf};

/// Oldest files are deleted past this many encounters.
pub const MAX_ENCOUNTERS: usize = 500;
/// Encounters shorter than this are not worth keeping (a stray hit on a mob).
pub const MIN_DURATION_MS: u64 = 5_000;

const INDEX_FILE: &str = "index.json";

pub struct History {
    dir: PathBuf,
    /// Newest first.
    index: Vec<EncounterSummary>,
}

impl History {
    /// Creates the directory if needed and loads the index. A missing or unreadable index is rebuilt from
    /// the encounter files that are present.
    pub fn open(dir: PathBuf) -> History {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!("could not create {}: {e}", dir.display());
        }
        let index_path = dir.join(INDEX_FILE);
        let index = match std::fs::read_to_string(&index_path) {
            Ok(text) => match serde_json::from_str::<Vec<EncounterSummary>>(&text) {
                Ok(mut index) => {
                    sort_newest_first(&mut index);
                    Some(index)
                }
                Err(e) => {
                    tracing::warn!("encounter index {} is not valid, rebuilding: {e}", index_path.display());
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

    /// Highest encounter id on disk, 0 when empty. New encounters continue after it.
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
    pub fn summaries(&self, limit: usize) -> Vec<EncounterSummary> {
        self.index.iter().take(limit).cloned().collect()
    }

    /// Fights that are too short or had no player damage are dropped rather than stored.
    pub fn worth_keeping(snap: &Snapshot) -> bool {
        let player_damage = snap.total_damage > 0 || snap.actors.iter().any(|a| a.is_player && a.damage_all_targets > 0);
        snap.duration_ms >= MIN_DURATION_MS && player_damage
    }

    /// Stores a finished encounter. Returns `Ok(false)` when it was skipped by `worth_keeping`.
    pub fn push(&mut self, snap: &Snapshot) -> std::io::Result<bool> {
        if !Self::worth_keeping(snap) {
            return Ok(false);
        }
        let bytes = serde_json::to_vec(snap).map_err(std::io::Error::other)?;
        write_atomic(&self.file_path(snap.id), &bytes)?;
        self.index.retain(|s| s.id != snap.id);
        self.index.insert(0, snap.summary());
        sort_newest_first(&mut self.index);
        while self.index.len() > MAX_ENCOUNTERS {
            if let Some(old) = self.index.pop() {
                remove_if_present(&self.file_path(old.id));
            }
        }
        self.save_index()?;
        Ok(true)
    }

    pub fn get(&self, id: u64) -> Option<Snapshot> {
        let path = self.file_path(id);
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(snap) => Some(snap),
                Err(e) => {
                    tracing::warn!("encounter file {} is not valid: {e}", path.display());
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
                if let Ok(snap) = serde_json::from_str::<Snapshot>(&text) {
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
            tracing::warn!("could not write the rebuilt encounter index: {e}");
        } else {
            tracing::info!("rebuilt the encounter index from {} files", self.index.len());
        }
    }
}

fn sort_newest_first(index: &mut [EncounterSummary]) {
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
    use meter_core::snapshot::{ActorStats, EndReason, TargetInfo};
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A fresh directory per test under the system temp dir, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let dir = std::env::temp_dir().join(format!("aion2dps-history-{}-{n}", std::process::id()));
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

    fn fight(id: u64, started_ms: u64) -> Snapshot {
        Snapshot {
            id,
            started_ms,
            last_event_ms: started_ms + 30_000,
            duration_ms: 30_000,
            end_reason: Some(EndReason::BossDied),
            boss: Some(TargetInfo { name: format!("Boss {id}"), is_boss: true, ..Default::default() }),
            total_damage: 1_000 * id,
            actors: vec![ActorStats {
                name: "Seraphel".into(),
                is_player: true,
                damage: 1_000 * id,
                damage_all_targets: 1_000 * id,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn round_trips_through_the_index() {
        let tmp = TempDir::new();
        let dir = tmp.0.join("encounters");
        {
            let mut h = History::open(dir.clone());
            assert!(h.is_empty());
            assert_eq!(h.max_id(), 0);
            assert!(h.push(&fight(1, 1_000)).unwrap());
            assert!(h.push(&fight(3, 3_000)).unwrap());
            assert!(h.push(&fight(2, 2_000)).unwrap());
            assert_eq!(h.len(), 3);
            assert_eq!(h.max_id(), 3);
            let ids: Vec<u64> = h.summaries(10).iter().map(|s| s.id).collect();
            assert_eq!(ids, vec![3, 2, 1]);
            assert_eq!(h.summaries(2).len(), 2);
        }
        let h = History::open(dir.clone());
        let ids: Vec<u64> = h.summaries(10).iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![3, 2, 1]);
        assert_eq!(h.summaries(10)[0].boss_name.as_deref(), Some("Boss 3"));
        let loaded = h.get(2).unwrap();
        assert_eq!(loaded, fight(2, 2_000));
        assert!(h.get(99).is_none());
        assert!(dir.join("index.json").exists());
        assert!(dir.join("2.json").exists());
    }

    #[test]
    fn delete_removes_file_and_entry() {
        let tmp = TempDir::new();
        let mut h = History::open(tmp.0.clone());
        h.push(&fight(5, 5_000)).unwrap();
        h.push(&fight(6, 6_000)).unwrap();
        h.delete(5).unwrap();
        assert!(!tmp.0.join("5.json").exists());
        assert_eq!(h.summaries(10).len(), 1);
        h.delete(5).unwrap();
        let again = History::open(tmp.0.clone());
        assert_eq!(again.summaries(10).len(), 1);
        assert_eq!(again.summaries(10)[0].id, 6);
    }

    #[test]
    fn skips_short_or_empty_fights() {
        let tmp = TempDir::new();
        let mut h = History::open(tmp.0.clone());
        let mut short = fight(1, 1_000);
        short.duration_ms = 4_999;
        assert!(!h.push(&short).unwrap());
        let mut empty = fight(2, 2_000);
        empty.total_damage = 0;
        empty.actors[0].damage_all_targets = 0;
        assert!(!h.push(&empty).unwrap());
        assert!(h.is_empty());
        assert!(!tmp.0.join("1.json").exists());
    }

    #[test]
    fn caps_and_drops_oldest_files() {
        let tmp = TempDir::new();
        let mut h = History::open(tmp.0.clone());
        for id in 1..=(MAX_ENCOUNTERS as u64 + 3) {
            h.push(&fight(id, id * 1_000)).unwrap();
        }
        assert_eq!(h.len(), MAX_ENCOUNTERS);
        assert!(!tmp.0.join("1.json").exists());
        assert!(!tmp.0.join("3.json").exists());
        assert!(tmp.0.join("4.json").exists());
        assert_eq!(h.summaries(1)[0].id, MAX_ENCOUNTERS as u64 + 3);
    }

    #[test]
    fn rebuilds_index_from_files() {
        let tmp = TempDir::new();
        {
            let mut h = History::open(tmp.0.clone());
            h.push(&fight(7, 7_000)).unwrap();
            h.push(&fight(8, 8_000)).unwrap();
        }
        std::fs::remove_file(tmp.0.join("index.json")).unwrap();
        let h = History::open(tmp.0.clone());
        let ids: Vec<u64> = h.summaries(10).iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![8, 7]);
        std::fs::write(tmp.0.join("index.json"), "garbage").unwrap();
        let h = History::open(tmp.0.clone());
        assert_eq!(h.len(), 2);
    }
}
