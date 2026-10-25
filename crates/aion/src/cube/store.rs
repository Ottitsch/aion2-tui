//! What the app keeps between runs, in the data dir: cube templates learned from key openings
//! (`templates.json`, so the next run knows them from the first spawn) and the spots where cubes were seen
//! (`spots.json`, drawn on the maps).

use cube_core::cubes::{CubeBook, CubeTemplate};
use cube_core::spots::Spot;
use std::path::{Path, PathBuf};

/// Writes `text` to a temporary file next to `path` first, so a failed write keeps the old file.
fn write_atomic(path: &Path, text: &str) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// The spots file. Reading a missing or broken file gives no spots (the reason is logged).
pub struct SpotStore {
    path: PathBuf,
}

impl SpotStore {
    pub fn new(path: PathBuf) -> SpotStore {
        SpotStore { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Vec<Spot> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!("ignoring {}: {e}", self.path.display());
                Vec::new()
            }),
            Err(_) => Vec::new(),
        }
    }

    pub fn save(&self, spots: &[Spot]) -> anyhow::Result<()> {
        write_atomic(&self.path, &serde_json::to_string_pretty(spots)?)
    }
}

pub struct TemplateStore {
    path: PathBuf,
    templates: Vec<CubeTemplate>,
}

impl TemplateStore {
    /// Reads the file; a missing or unreadable file is an empty store (the reason is logged).
    pub fn open(path: PathBuf) -> TemplateStore {
        let templates = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!("ignoring {}: {e}", path.display());
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };
        TemplateStore { path, templates }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn len(&self) -> usize {
        self.templates.len()
    }

    /// Adds the saved templates to `book`.
    pub fn load_into(&self, book: &mut CubeBook) {
        for t in &self.templates {
            book.add(t.clone());
        }
    }

    /// Saves one more template.
    pub fn add(&mut self, template: CubeTemplate) -> anyhow::Result<()> {
        if self.templates.iter().any(|t| t.template == template.template) {
            return Ok(());
        }
        self.templates.push(template);
        write_atomic(&self.path, &serde_json::to_string_pretty(&self.templates)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cube_core::cubes::Evidence;

    #[test]
    fn saves_and_loads_learned_templates() {
        let dir = std::env::temp_dir().join(format!("aion2cube-store-{}", std::process::id()));
        let path = dir.join("templates.json");
        let mut store = TemplateStore::open(path.clone());
        assert_eq!(store.len(), 0);
        let learned = CubeTemplate { template: 1_400_123, zone: Some("Verteron".into()), evidence: Evidence::Learned };
        store.add(learned.clone()).unwrap();
        store.add(learned.clone()).unwrap();
        let again = TemplateStore::open(path);
        assert_eq!(again.len(), 1);
        let mut book = CubeBook::default();
        again.load_into(&mut book);
        assert_eq!(book.get(1_400_123), Some(&learned));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saves_and_loads_spots() {
        let dir = std::env::temp_dir().join(format!("aion2cube-spots-{}", std::process::id()));
        let store = SpotStore::new(dir.join("spots.json"));
        assert!(store.load().is_empty());
        let spot = Spot { map_id: 1010, template: 1_200_039, pos: cube_core::geo::Vec3::new(1.0, 2.0, 3.0), first_seen_ms: 1, last_seen_ms: 2, sightings: 1, opened_by_you: 0 };
        store.save(std::slice::from_ref(&spot)).unwrap();
        assert_eq!(store.load(), vec![spot]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_file_is_an_empty_store() {
        let dir = std::env::temp_dir().join(format!("aion2cube-broken-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("templates.json");
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(TemplateStore::open(path).len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
