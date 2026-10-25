//! Server to client opcodes, written as the two bytes appear on the wire: `0xAABB` means the bytes `AA BB`.
//!
//! Heartbeat, map load and self record are community findings shared with the aion2dps meter. The two
//! inventory opcodes were found for this tracker on the Global client captures of 11 October 2026 (see
//! docs/PROTOCOL.md). They move when the game patches; `replay --opcodes` on a fresh capture is the way to
//! find them again.

pub const HEARTBEAT: u16 = 0x0036;
pub const MAP_LOAD: u16 = 0x2136;
pub const SELF_INFO: u16 = 0x3336;
/// The whole inventory, sent on every loading screen right before the map load.
pub const INVENTORY: u16 = 0x1156;
/// Changed inventory entries: one record per item or currency whose count changed.
pub const ITEM_UPDATE: u16 = 0x1B56;
/// The kina daily gain limit counter, `<u64 counted> <u64 server ms>`, on every loading screen just before
/// the map load (found on a recording of 25 October 2026, also in the captures of 11 October).
pub const DAILY_KINA: u16 = 0xA5FF;
/// The same counter right after a kina gain that counts toward it (open world mobs, not the Abyss or dungeons).
pub const DAILY_KINA_GAIN: u16 = 0xA6FF;
/// Abyss Points and their cap, `<u64 points> <u64 cap>`, on loading screens and whenever they change.
pub const ABYSS_POINTS: u16 = 0xA9FF;

/// LZ4 bundle marker in the opcode position: the frame holds more frames, compressed.
pub const BUNDLE: u16 = 0xFFFF;

pub fn name(op: u16) -> &'static str {
    match op {
        HEARTBEAT => "Heartbeat",
        MAP_LOAD => "MapLoad",
        SELF_INFO => "SelfInfo",
        INVENTORY => "Inventory",
        ITEM_UPDATE => "ItemUpdate",
        DAILY_KINA => "DailyKina",
        DAILY_KINA_GAIN => "DailyKinaGain",
        ABYSS_POINTS => "AbyssPoints",
        _ => "",
    }
}

/// `1B 56` style rendering of an opcode.
pub fn hex(op: u16) -> String {
    format!("{:02X} {:02X}", op >> 8, op & 0xFF)
}

/// Parses `1b56`, `1B 56` or `1b-56`.
pub fn parse(s: &str) -> Option<u16> {
    let clean: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if clean.len() != 4 {
        return None;
    }
    u16::from_str_radix(&clean, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_order_round_trips() {
        assert_eq!(hex(ITEM_UPDATE), "1B 56");
        assert_eq!(parse("1b 56"), Some(ITEM_UPDATE));
        assert_eq!(parse("1156"), Some(INVENTORY));
        assert_eq!(parse("11-56"), Some(INVENTORY));
        assert_eq!(parse("156"), None);
        assert_eq!(name(INVENTORY), "Inventory");
    }
}
