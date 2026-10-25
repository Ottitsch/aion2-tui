//! Map names, embedded at compile time from `data/` (the same tables as the aion2dps meter, see
//! docs/DATA_SOURCES.md).

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

#[derive(Debug, Clone, Default, Deserialize)]
struct DungeonEntry {
    name: String,
    #[serde(default)]
    tier: Option<String>,
    #[serde(default)]
    category: Option<String>,
}

/// What the TUI shows for a map.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapInfo {
    pub id: u32,
    pub name: String,
    /// Difficulty or stage, for example "Conquest [Normal]".
    pub tier: Option<String>,
    /// "Dungeon", "Raid", "Transcendence", ... for instances in the table.
    pub category: Option<String>,
    pub is_instance: bool,
}

/// Maps named from play rather than from the tables: on 25 October 2026 the character went to the Abyss and the
/// map load said 20 (and Abyss kills moved the Abyss Points, not the daily kina limit).
const NAMED_MAPS: &[(u32, &str)] = &[(20, "Abyss")];

pub struct GameData {
    dungeons: HashMap<u32, DungeonEntry>,
    open_world: HashSet<u32>,
}

impl GameData {
    pub fn get() -> &'static GameData {
        static DATA: OnceLock<GameData> = OnceLock::new();
        DATA.get_or_init(|| {
            let raw: HashMap<String, DungeonEntry> = serde_json::from_str(include_str!("../data/dungeons.json")).expect("embedded table is valid JSON");
            let dungeons = raw.into_iter().filter_map(|(k, v)| k.parse::<u32>().ok().map(|k| (k, v))).collect();
            let ids: Vec<i64> = serde_json::from_str(include_str!("../data/open_world_maps.json")).expect("embedded table is valid JSON");
            let open_world = ids.into_iter().filter_map(|v| u32::try_from(v).ok()).collect();
            GameData { dungeons, open_world }
        })
    }

    /// Instance (dungeon) map ids live in 600000..699999; Nightmare, Raid and Ascension ids are instances too.
    pub fn is_instance(&self, map_id: u32) -> bool {
        (600_000..=699_999).contains(&map_id)
            || map_id == 200_003
            || (910_000..=919_999).contains(&map_id)
            || (self.dungeons.contains_key(&map_id) && !self.open_world.contains(&map_id))
    }

    pub fn map(&self, map_id: u32) -> MapInfo {
        let is_instance = self.is_instance(map_id);
        match self.dungeons.get(&map_id) {
            Some(d) => MapInfo { id: map_id, name: d.name.clone(), tier: d.tier.clone(), category: d.category.clone(), is_instance },
            None => {
                let name = if let Some((_, name)) = NAMED_MAPS.iter().find(|(id, _)| *id == map_id) {
                    (*name).to_string()
                } else if self.open_world.contains(&map_id) {
                    format!("Open world {map_id}")
                } else if is_instance {
                    format!("Instance {map_id}")
                } else {
                    format!("Map {map_id}")
                };
                MapInfo { id: map_id, name, tier: None, category: None, is_instance }
            }
        }
    }

    /// `Vakron Sky Island (Conquest [Normal])`, or just the name.
    pub fn map_label(&self, map_id: u32) -> String {
        let m = self.map(map_id);
        match m.tier {
            Some(tier) if !tier.is_empty() => format!("{} ({tier})", m.name),
            _ => m.name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_dungeons_from_the_captures() {
        let gd = GameData::get();
        let m = gd.map(600_072);
        assert_eq!(m.name, "Vakron Sky Island");
        assert_eq!(m.tier.as_deref(), Some("Conquest [Normal]"));
        assert!(m.is_instance);
        assert_eq!(gd.map_label(600_072), "Vakron Sky Island (Conquest [Normal])");
        let town = gd.map(1110);
        assert!(!town.is_instance);
        assert_eq!(town.name, "Open world 1110");
        assert_eq!(gd.map(4242).name, "Map 4242");
    }
}
