//! The one line chat summary of a session, for pasting into the game chat.

use crate::fmt::{format_duration, format_short, format_signed};
use kina_core::snapshot::SessionSnapshot;

/// Example: `Kina 1:59:47 | net +1.51M (757K/h) | earned 3.03M, spent 1.52M | best Vakron Sky Island +565K`.
pub fn chat_summary(s: &SessionSnapshot) -> String {
    let mut parts = vec![format!("Kina {}", format_duration(s.duration_ms))];
    parts.push(format!("net {} ({}/h)", format_signed(s.net as f64), format_signed(s.net_per_hour)));
    parts.push(format!("earned {}, spent {}", format_short(s.earned as f64), format_short(s.spent as f64)));
    if let Some(best) = s.maps.iter().find(|m| m.net > 0) {
        if let Some(map) = &best.map {
            parts.push(format!("best {} {}", map.name, format_signed(best.net as f64)));
        }
    }
    parts.join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use kina_core::gamedata::GameData;
    use kina_core::snapshot::MapStats;

    #[test]
    fn summarises_a_session() {
        let s = SessionSnapshot {
            duration_ms: 7_187_000,
            earned: 3_027_497,
            spent: 1_516_259,
            net: 1_511_238,
            net_per_hour: 756_946.0,
            maps: vec![MapStats { map: Some(GameData::get().map(600_072)), visits: 1, time_ms: 0, earned: 600_000, spent: 35_000, net: 565_000, changes: 2 }],
            ..Default::default()
        };
        assert_eq!(chat_summary(&s), "Kina 1:59:47 | net +1.51M (+757K/h) | earned 3.03M, spent 1.52M | best Vakron Sky Island +565K");
        let empty = SessionSnapshot::default();
        assert_eq!(chat_summary(&empty), "Kina 0:00 | net 0 (0/h) | earned 0, spent 0");
    }
}
