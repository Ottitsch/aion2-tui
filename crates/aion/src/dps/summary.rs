//! The one line party chat summary and the number formatting it uses.

use meter_core::snapshot::Snapshot;

/// Players included in the chat line; the game's chat input is short.
const MAX_PLAYERS: usize = 10;

/// Example: `Kromede 1:59 | Seraphel 23.4K (31.8%) | Nyxara 15.6K (21.2%)`. The number is DPS on the
/// main target, the percentage is the share of the party's damage.
pub fn chat_summary(snap: &Snapshot) -> String {
    let title = snap
        .boss
        .as_ref()
        .map(|b| b.name.clone())
        .filter(|n| !n.is_empty())
        .or_else(|| snap.map_name.clone())
        .unwrap_or_else(|| "Encounter".into());
    let mut parts = vec![format!("{title} {}", format_duration(snap.duration_ms))];
    let mut players: Vec<_> = snap.actors.iter().filter(|a| a.is_player && a.damage > 0).collect();
    players.sort_by_key(|a| std::cmp::Reverse(a.damage));
    for actor in players.iter().take(MAX_PLAYERS) {
        parts.push(format!("{} {} ({:.1}%)", actor.name, format_short(actor.dps), actor.share * 100.0));
    }
    parts.join(" | ")
}

/// `m:ss`, or `h:mm:ss` past an hour.
pub fn format_duration(ms: u64) -> String {
    let total = ms / 1000;
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// `950`, `23.4K`, `1.25M`, `2.10B`.
pub fn format_short(value: f64) -> String {
    if !value.is_finite() || value < 0.0 {
        return "0".into();
    }
    let units = [(1e9, "B"), (1e6, "M"), (1e3, "K")];
    for (scale, suffix) in units {
        if value >= scale {
            let scaled = value / scale;
            let digits = if scaled >= 100.0 {
                0
            } else if scaled >= 10.0 {
                1
            } else {
                2
            };
            return format!("{scaled:.digits$}{suffix}");
        }
    }
    format!("{}", value.round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use meter_core::snapshot::{ActorStats, TargetInfo};

    fn actor(name: &str, damage: u64, dps: f64, share: f64, is_player: bool) -> ActorStats {
        ActorStats { name: name.into(), damage, dps, share, is_player, ..Default::default() }
    }

    #[test]
    fn formats_numbers() {
        assert_eq!(format_short(950.0), "950");
        assert_eq!(format_short(23_400.0), "23.4K");
        assert_eq!(format_short(123_456.0), "123K");
        assert_eq!(format_short(1_250_000.0), "1.25M");
        assert_eq!(format_short(2_100_000_000.0), "2.10B");
        assert_eq!(format_short(f64::NAN), "0");
    }

    #[test]
    fn formats_durations() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(119_400), "1:59");
        assert_eq!(format_duration(3_661_000), "1:01:01");
    }

    #[test]
    fn summary_line_matches_design() {
        let snap = Snapshot {
            duration_ms: 119_000,
            boss: Some(TargetInfo { name: "Kromede".into(), is_boss: true, ..Default::default() }),
            actors: vec![
                actor("Nyxara", 1_856_000, 15_600.0, 0.212, true),
                actor("Seraphel", 2_784_000, 23_400.0, 0.312, true),
                actor("Fire Spirit", 300_000, 2_500.0, 0.03, false),
                actor("Idle", 0, 0.0, 0.0, true),
            ],
            ..Default::default()
        };
        assert_eq!(chat_summary(&snap), "Kromede 1:59 | Seraphel 23.4K (31.2%) | Nyxara 15.6K (21.2%)");
    }

    #[test]
    fn summary_falls_back_without_a_boss() {
        let snap = Snapshot { duration_ms: 5_000, map_name: Some("Elysea".into()), ..Default::default() };
        assert_eq!(chat_summary(&snap), "Elysea 0:05");
        let snap = Snapshot::default();
        assert_eq!(chat_summary(&snap), "Encounter 0:00");
    }
}
