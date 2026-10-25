//! The read model: what the tracker hands to the terminal app and the history files. Serialised camelCase,
//! so the session files stay readable by other tools.

use crate::gamedata::MapInfo;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Character {
    pub name: String,
    pub server: u16,
    /// 0 when the self record carried none.
    pub level: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// Reset by hotkey, overlay or main window.
    Manual,
    /// Another character logged in.
    CharacterChange,
    /// Nothing arrived from the game for a long time (game closed, disconnected, PC asleep).
    Idle,
    /// The tracker was closed.
    AppExit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Gain,
    Spend,
    /// The full inventory on a loading screen disagreed with the running balance (packets were missed, or
    /// the character changed). Shown, never counted as income or spending.
    Resync,
}

/// One line of the kina log. Changes of the same direction within a few seconds on the same map are merged
/// (selling ten items, buying a stack), `count` says how many.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KinaEntry {
    pub t_ms: u64,
    /// Time of the last change merged into this entry.
    pub last_ms: u64,
    pub delta: i64,
    pub count: u32,
    /// Balance after the entry.
    pub balance: u64,
    pub kind: EntryKind,
    pub map_id: Option<u32>,
    pub map_name: Option<String>,
}

/// One visit to a map: from its loading screen to the next one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStats {
    pub index: u32,
    /// `None` until the first loading screen of the session.
    pub map: Option<MapInfo>,
    pub started_ms: u64,
    pub ended_ms: u64,
    pub duration_ms: u64,
    pub earned: u64,
    pub spent: u64,
    pub net: i64,
    pub changes: u32,
    /// The run the character is in right now.
    pub current: bool,
}

/// All visits to one map in a session, for the "where it came from" table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapStats {
    pub map: Option<MapInfo>,
    pub visits: u32,
    pub time_ms: u64,
    pub earned: u64,
    pub spent: u64,
    pub net: i64,
    pub changes: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BalancePoint {
    pub t_ms: u64,
    pub balance: u64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshot {
    pub id: u64,
    pub active: bool,
    pub character: Option<Character>,
    pub started_ms: u64,
    /// Last packet seen while the session was live.
    pub last_ms: u64,
    /// Time connected to the game: gaps of more than a minute without packets are left out.
    pub duration_ms: u64,
    pub end_reason: Option<EndReason>,
    pub start_balance: Option<u64>,
    pub balance: Option<u64>,
    pub earned: u64,
    pub spent: u64,
    /// `earned - spent`.
    pub net: i64,
    pub net_per_hour: f64,
    pub earned_per_hour: f64,
    pub changes: u32,
    pub resyncs: u32,
    pub biggest_gain: u64,
    pub biggest_spend: u64,
    pub current_map: Option<MapInfo>,
    /// Oldest first.
    pub entries: Vec<KinaEntry>,
    pub runs: Vec<RunStats>,
    /// Most net kina first.
    pub maps: Vec<MapStats>,
    pub timeline: Vec<BalancePoint>,
}

/// The compact form kept in the history index and listed by the History view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: u64,
    pub character_name: Option<String>,
    pub started_ms: u64,
    pub last_ms: u64,
    pub duration_ms: u64,
    pub end_reason: Option<EndReason>,
    pub earned: u64,
    pub spent: u64,
    pub net: i64,
    pub net_per_hour: f64,
    pub end_balance: Option<u64>,
    pub changes: u32,
    /// Runs with at least one kina change.
    pub runs: u32,
    /// The map that brought the most net kina.
    pub top_map: Option<String>,
}

/// `amount` per hour over `duration_ms`; 0 for sessions shorter than a second.
pub fn per_hour(amount: f64, duration_ms: u64) -> f64 {
    if duration_ms < 1_000 {
        0.0
    } else {
        amount * 3_600_000.0 / duration_ms as f64
    }
}

impl SessionSnapshot {
    pub fn summary(&self) -> SessionSummary {
        SessionSummary {
            id: self.id,
            character_name: self.character.as_ref().map(|c| c.name.clone()),
            started_ms: self.started_ms,
            last_ms: self.last_ms,
            duration_ms: self.duration_ms,
            end_reason: self.end_reason,
            earned: self.earned,
            spent: self.spent,
            net: self.net,
            net_per_hour: self.net_per_hour,
            end_balance: self.balance,
            changes: self.changes,
            runs: self.runs.iter().filter(|r| r.changes > 0).count() as u32,
            top_map: self.maps.iter().find(|m| m.net > 0).and_then(|m| m.map.as_ref()).map(|m| m.name.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialises_camel_case_and_snake_case_enums() {
        let snap = SessionSnapshot { id: 3, end_reason: Some(EndReason::CharacterChange), net_per_hour: 1.5, ..Default::default() };
        let json = serde_json::to_value(&snap).unwrap();
        assert_eq!(json["endReason"], "character_change");
        assert_eq!(json["netPerHour"], 1.5);
        assert!(json.get("startBalance").is_some());
        let entry = KinaEntry { t_ms: 1, last_ms: 2, delta: -5, count: 1, balance: 10, kind: EntryKind::Resync, map_id: None, map_name: None };
        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["kind"], "resync");
        assert_eq!(json["lastMs"], 2);
    }

    #[test]
    fn per_hour_needs_a_second() {
        assert_eq!(per_hour(1_000.0, 999), 0.0);
        assert_eq!(per_hour(600_000.0, 1_800_000), 1_200_000.0);
    }
}
