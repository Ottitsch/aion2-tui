//! What you picked on the tabs, kept for the next start in `%APPDATA%\aion-all\settings.json`: the meter's
//! filters, hidden names, the cube sound and map (radar or 3D), and the kina goal.
//!
//! Only what changes while the app runs is written back, so a command line option (`--hide-name`, `--no-sound`,
//! `--goal`) wins for the run it is given on without being saved.

use crate::cube::app::MapKind;
use crate::hub::Hub;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub hide_names: bool,
    pub party_only: bool,
    pub boss_only: bool,
    pub sound: bool,
    /// The cube detector's repeating beeps.
    pub detector: bool,
    pub cube_map: MapKind,
    pub kina_goal: u64,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { hide_names: false, party_only: false, boss_only: false, sound: true, detector: true, cube_map: MapKind::Radar, kina_goal: 0 }
    }
}

impl Settings {
    /// The saved settings, or the defaults when there are none or they cannot be read.
    pub fn load(path: &Path) -> Settings {
        let Ok(text) = std::fs::read_to_string(path) else { return Settings::default() };
        serde_json::from_str(&text).unwrap_or_else(|e| {
            tracing::warn!("{}: {e}; using the defaults", path.display());
            Settings::default()
        })
    }

    /// What the tabs are set to now.
    pub fn of(hub: &Hub) -> Settings {
        Settings {
            hide_names: hub.dps.options.hide_names,
            party_only: hub.dps.options.party_only,
            boss_only: hub.dps.options.boss_only,
            sound: hub.cube.options.sound,
            detector: hub.cube.options.detector,
            cube_map: hub.cube.map_kind,
            kina_goal: hub.kina.options.goal,
        }
    }

    fn fields(&self) -> Map<String, Value> {
        match serde_json::to_value(self) {
            Ok(Value::Object(fields)) => fields,
            _ => Map::new(),
        }
    }
}

/// The saved settings, and what the tabs were set to when last looked at, so only what you change gets saved.
pub struct Store {
    /// `None` in demo and replay: looking around there changes nothing for the next start.
    path: Option<PathBuf>,
    saved: Settings,
    seen: Settings,
}

impl Store {
    pub fn new(path: Option<PathBuf>, saved: Settings, now: Settings) -> Store {
        Store { path, saved, seen: now }
    }

    /// Takes every setting that changed since the last call into the saved ones, and writes them.
    pub fn update(&mut self, now: Settings) {
        if now == self.seen {
            return;
        }
        let seen = self.seen.fields();
        let mut saved = self.saved.fields();
        for (key, value) in now.fields() {
            if seen.get(&key) != Some(&value) {
                saved.insert(key, value);
            }
        }
        self.seen = now;
        let Ok(merged) = serde_json::from_value::<Settings>(Value::Object(saved)) else { return };
        if merged == self.saved {
            return;
        }
        self.saved = merged;
        let Some(path) = &self.path else { return };
        let written = serde_json::to_vec_pretty(&self.saved).map_err(std::io::Error::other).and_then(|json| crate::files::write_atomic(path, &json));
        if let Err(e) = written {
            tracing::warn!("could not save {}: {e}", path.display());
        }
    }

    #[cfg(test)]
    pub fn saved(&self) -> &Settings {
        &self.saved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_changes_is_saved() {
        // Started with --hide-name over saved settings that show names.
        let saved = Settings { party_only: true, ..Settings::default() };
        let started = Settings { hide_names: true, ..saved.clone() };
        let mut store = Store::new(None, saved, started.clone());
        store.update(started.clone());
        assert!(!store.saved().hide_names, "the command line option is not saved");
        store.update(Settings { cube_map: MapKind::View3d, ..started });
        assert_eq!(store.saved().cube_map, MapKind::View3d);
        assert!(store.saved().party_only && !store.saved().hide_names);
    }

    #[test]
    fn settings_round_trip_and_fill_gaps_with_defaults() {
        let dir = std::env::temp_dir().join(format!("aion-settings-{}", std::process::id()));
        let path = dir.join("settings.json");
        let mut store = Store::new(Some(path.clone()), Settings::default(), Settings::default());
        store.update(Settings { boss_only: true, kina_goal: 5_000_000, ..Settings::default() });
        let loaded = Settings::load(&path);
        assert!(loaded.boss_only && loaded.sound);
        assert_eq!(loaded.kina_goal, 5_000_000);
        std::fs::write(&path, r#"{"cube_map":"3d"}"#).unwrap();
        assert_eq!(Settings::load(&path), Settings { cube_map: MapKind::View3d, ..Settings::default() });
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
        let _ = std::fs::remove_dir_all(dir);
    }
}
