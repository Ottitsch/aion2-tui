//! Game packets to tracker events.
//!
//! Only four packets matter for kina: the heartbeat (a server clock and proof the game is connected), the
//! map load, the self record (who is playing) and the two inventory packets. The inventory layouts were
//! worked out on the Global client captures of 11 October 2026; see docs/PROTOCOL.md for the evidence.
//!
//! ```text
//! currency record   <u32 uid> 00 <u16 slot> 00 <u32 key> <u64 count> ...
//! kina              key 0x3770333F (wire bytes 3F 33 70 37)
//! ```
//! Every currency key seen so far ends in the bytes `33 70 37`; only kina is identified. A record is
//! accepted when the two guard bytes around the slot are zero and the count fits in 40 bits, which every
//! one of the 77 kina records in the captures does and random body bytes almost never do.

use crate::opcodes as op;
use crate::wire::Cursor;
use serde::Serialize;
use std::collections::BTreeMap;

/// Item key of kina, as a little endian u32 (wire bytes `3F 33 70 37`).
pub const KINA_KEY: u32 = 0x3770_333F;
/// Last three wire bytes of every currency key seen so far.
const CURRENCY_FAMILY: [u8; 3] = [0x33, 0x70, 0x37];
/// Counts at or above this are misreads (about 1.1 trillion).
pub const MAX_COUNT: u64 = 1 << 40;
/// Bytes between the start of a currency record and its key.
const RECORD_HEAD: usize = 8;

pub type EntityId = u32;
pub const MAX_ENTITY_ID: EntityId = 9_999_999;

/// Where a currency count came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CountSource {
    /// The full inventory sent on a loading screen: a checkpoint, never a change by itself.
    Inventory,
    /// A live inventory change: the count moved because something happened.
    Update,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    /// The server heartbeat, about twenty per second while connected.
    Heartbeat { t_ms: u64, server_ms: u64 },
    /// A loading screen: `map_id` is the map now loaded.
    ZoneChange { t_ms: u64, map_id: u32 },
    /// The local character.
    SelfInfo { t_ms: u64, id: EntityId, name: String, server: u16, level: u32 },
    /// The count of one currency. `key` is the item key; kina is `KINA_KEY`.
    Currency { t_ms: u64, key: u32, count: u64, source: CountSource },
    /// Kina counted toward today's gain limit, and the server time it stood there. Only kina from open world
    /// mobs counts; the limit itself is not in the packet (`DAILY_KINA_LIMIT`).
    DailyKina { t_ms: u64, counted: u64, server_ms: u64, source: CountSource },
    /// Abyss Points and their cap.
    AbyssPoints { t_ms: u64, points: u64, cap: u64 },
}

impl Event {
    pub fn t_ms(&self) -> u64 {
        match self {
            Event::Heartbeat { t_ms, .. }
            | Event::ZoneChange { t_ms, .. }
            | Event::SelfInfo { t_ms, .. }
            | Event::Currency { t_ms, .. }
            | Event::DailyKina { t_ms, .. }
            | Event::AbyssPoints { t_ms, .. } => *t_ms,
        }
    }

    /// Short lower case name of the variant, as used by `replay --events <type>`.
    pub fn kind(&self) -> &'static str {
        match self {
            Event::Heartbeat { .. } => "heartbeat",
            Event::ZoneChange { .. } => "zone",
            Event::SelfInfo { .. } => "self",
            Event::Currency { key, .. } if *key == KINA_KEY => "kina",
            Event::Currency { .. } => "currency",
            Event::DailyKina { .. } => "daily",
            Event::AbyssPoints { .. } => "abyss",
        }
    }
}

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParserStats {
    pub packets: u64,
    pub heartbeats: u64,
    pub first_heartbeat_ms: Option<u64>,
    pub last_heartbeat_ms: Option<u64>,
    /// Server clock from the latest heartbeat.
    pub server_time_ms: Option<u64>,
    pub zone_changes: u64,
    pub map_rejected: u64,
    pub self_records: u64,
    pub self_rejected: u64,
    pub inventories: u64,
    /// Full inventories with no kina record: either the layout moved or the character has no kina entry.
    pub inventories_without_kina: u64,
    pub item_updates: u64,
    pub kina_records: u64,
    /// Currency records other than kina.
    pub currency_records: u64,
    pub daily_kina_records: u64,
    pub abyss_point_records: u64,
    /// Daily limit or Abyss Points packets that did not read as two counts.
    pub limit_rejected: u64,
    #[serde(skip)]
    pub opcode_counts: BTreeMap<u16, u64>,
    #[serde(skip)]
    pub opcode_bytes: BTreeMap<u16, u64>,
}

#[derive(Default)]
pub struct Parser {
    pub stats: ParserStats,
}

impl Parser {
    /// Decodes one packet. `t_ms` is the capture time in Unix milliseconds.
    pub fn packet(&mut self, t_ms: u64, opcode: u16, body: &[u8], out: &mut dyn FnMut(Event)) {
        let s = &mut self.stats;
        s.packets += 1;
        *s.opcode_counts.entry(opcode).or_default() += 1;
        *s.opcode_bytes.entry(opcode).or_default() += body.len() as u64;
        match opcode {
            op::HEARTBEAT => {
                s.heartbeats += 1;
                s.first_heartbeat_ms.get_or_insert(t_ms);
                s.last_heartbeat_ms = Some(t_ms);
                let server_ms = Cursor::new(body).u64().unwrap_or(0);
                if server_ms > 0 {
                    s.server_time_ms = Some(server_ms);
                }
                out(Event::Heartbeat { t_ms, server_ms });
            }
            op::MAP_LOAD => map_load(t_ms, body, s, out),
            op::SELF_INFO => self_info(t_ms, body, s, out),
            op::INVENTORY => {
                s.inventories += 1;
                if currencies(t_ms, body, CountSource::Inventory, s, out) == 0 {
                    s.inventories_without_kina += 1;
                }
            }
            op::ITEM_UPDATE => {
                s.item_updates += 1;
                currencies(t_ms, body, CountSource::Update, s, out);
            }
            op::DAILY_KINA | op::DAILY_KINA_GAIN => match two_counts(body).filter(|(n, ms)| *n < MAX_COUNT && plausible_ms(*ms)) {
                Some((counted, server_ms)) => {
                    s.daily_kina_records += 1;
                    let source = if opcode == op::DAILY_KINA { CountSource::Inventory } else { CountSource::Update };
                    out(Event::DailyKina { t_ms, counted, server_ms, source });
                }
                None => s.limit_rejected += 1,
            },
            op::ABYSS_POINTS => match two_counts(body).filter(|(points, cap)| *points < MAX_COUNT && (1..MAX_COUNT).contains(cap)) {
                Some((points, cap)) => {
                    s.abyss_point_records += 1;
                    out(Event::AbyssPoints { t_ms, points, cap });
                }
                None => s.limit_rejected += 1,
            },
            _ => {}
        }
    }
}

/// A 16 byte body of two little endian u64 values.
fn two_counts(body: &[u8]) -> Option<(u64, u64)> {
    if body.len() != 16 {
        return None;
    }
    let mut c = Cursor::new(body);
    Some((c.u64()?, c.u64()?))
}

/// A Unix millisecond time between 2020 and 2096.
fn plausible_ms(ms: u64) -> bool {
    (1_577_836_800_000..4_000_000_000_000).contains(&ms)
}

/// `21 36 <u32 load count> <u32 map id> ...`.
fn map_load(t_ms: u64, body: &[u8], stats: &mut ParserStats, out: &mut dyn FnMut(Event)) {
    let mut c = Cursor::new(body);
    let (Some(_count), Some(map_id)) = (c.u32(), c.u32()) else {
        stats.map_rejected += 1;
        return;
    };
    if !(1..=9_999_999).contains(&map_id) {
        stats.map_rejected += 1;
        return;
    }
    stats.zone_changes += 1;
    out(Event::ZoneChange { t_ms, map_id });
}

/// `33 36 <id varint> <mask1 u32> <mask2 u8> [mask2 & 1: <len u8> <name>] <server u16> <job u32> <u8>
/// <level u32>`, the same reading as the aion2dps meter.
fn self_info(t_ms: u64, body: &[u8], stats: &mut ParserStats, out: &mut dyn FnMut(Event)) {
    let parsed = (|| {
        let mut c = Cursor::new(body);
        let id = c.varint()?;
        if !(1..=MAX_ENTITY_ID).contains(&id) {
            return None;
        }
        let _mask1 = c.u32()?;
        let mask2 = c.u8()?;
        if mask2 & 0x01 == 0 {
            return None;
        }
        let len = usize::from(c.u8()?);
        if !(1..=48).contains(&len) {
            return None;
        }
        let name = valid_name(c.bytes(len)?)?.to_string();
        let server = c.u16()?;
        if !(1000..=2999).contains(&server) {
            return None;
        }
        let mut level = 0;
        if let (Some(_job), Some(_), Some(lv)) = (c.u32(), c.u8(), c.u32()) {
            if (1..=99).contains(&lv) {
                level = lv;
            }
        }
        Some((id, name, server, level))
    })();
    match parsed {
        Some((id, name, server, level)) => {
            stats.self_records += 1;
            out(Event::SelfInfo { t_ms, id, name, server, level });
        }
        None => stats.self_rejected += 1,
    }
}

/// Emits every currency record in an inventory body and returns how many kina records it held.
fn currencies(t_ms: u64, body: &[u8], source: CountSource, stats: &mut ParserStats, out: &mut dyn FnMut(Event)) -> usize {
    let mut kina = 0;
    for (key, count) in currency_records(body) {
        if key == KINA_KEY {
            stats.kina_records += 1;
            kina += 1;
        } else {
            stats.currency_records += 1;
        }
        out(Event::Currency { t_ms, key, count, source });
    }
    kina
}

/// `(key, count)` of every currency record in an inventory body, in body order.
pub fn currency_records(body: &[u8]) -> Vec<(u32, u64)> {
    let mut found = Vec::new();
    let mut k = RECORD_HEAD;
    while k + 12 <= body.len() {
        if body[k + 1..k + 4] == CURRENCY_FAMILY && body[k - 4] == 0 && body[k - 1] == 0 {
            let key = u32::from_le_bytes([body[k], body[k + 1], body[k + 2], body[k + 3]]);
            let mut count_bytes = [0u8; 8];
            count_bytes.copy_from_slice(&body[k + 4..k + 12]);
            let count = u64::from_le_bytes(count_bytes);
            if count < MAX_COUNT {
                found.push((key, count));
                k += 12;
                continue;
            }
        }
        k += 1;
    }
    found
}

/// A character name: 1 to 16 letters or digits, at least one letter.
pub fn valid_name(field: &[u8]) -> Option<&str> {
    let name = std::str::from_utf8(field).ok()?;
    let chars = name.chars().count();
    let ok = (1..=16).contains(&chars) && name.chars().all(char::is_alphanumeric) && name.chars().any(char::is_alphabetic);
    ok.then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    fn decode(opcode: u16, body: &[u8]) -> (Vec<Event>, ParserStats) {
        let mut p = Parser::default();
        let mut events = Vec::new();
        p.packet(1_000, opcode, body, &mut |e| events.push(e));
        (events, p.stats)
    }

    // A single record item update from session4 (2026-10-11): kina rose to 8 348 593.
    const ONE_RECORD: &str = "0101007ec7060000c600003f337037b1637f00000000000100000000000000000000000000001c0000";
    // The head of a three record update from session1: a consumable (count 53) and then kina 6 110 905.
    const TWO_RECORDS: &str = "03010016ce0b0000a80000c17acd1f35000000000000000f1400000000000000000000000000006000010066c41200008d00003f337037b93e5d0000000000010000000000000000000000000000600001";

    #[test]
    fn reads_the_daily_gain_limit_and_the_abyss_points() {
        // The recording of 25 October 2026: 183 413 counted after an open world drop, 192 339 on the next
        // loading screen (the game's window showed 192,339/1,000,000), Abyss Points 138 120 of 2 000 000.
        let (events, stats) = decode(op::DAILY_KINA_GAIN, &hex("75cc0200000000003a528973a1010000"));
        assert_eq!(events, vec![Event::DailyKina { t_ms: 1_000, counted: 183_413, server_ms: 1_792_939_741_754, source: CountSource::Update }]);
        assert_eq!(stats.daily_kina_records, 1);
        let (events, _) = decode(op::DAILY_KINA, &hex("53ef020000000000071a8a73a1010000"));
        assert!(matches!(events[..], [Event::DailyKina { counted: 192_339, source: CountSource::Inventory, .. }]));
        let (events, _) = decode(op::ABYSS_POINTS, &hex("881b02000000000080841e0000000000"));
        assert_eq!(events, vec![Event::AbyssPoints { t_ms: 1_000, points: 138_120, cap: 2_000_000 }]);
        let (events, stats) = decode(op::DAILY_KINA, &hex("53ef0200000000000000"));
        assert!(events.is_empty() && stats.limit_rejected == 1, "a short body is no counter");
    }

    #[test]
    fn reads_kina_from_a_single_record_update() {
        let (events, stats) = decode(op::ITEM_UPDATE, &hex(ONE_RECORD));
        assert_eq!(events, vec![Event::Currency { t_ms: 1_000, key: KINA_KEY, count: 8_348_593, source: CountSource::Update }]);
        assert_eq!(stats.kina_records, 1);
        assert_eq!(stats.item_updates, 1);
    }

    #[test]
    fn skips_other_items_before_the_kina_record() {
        let records = currency_records(&hex(TWO_RECORDS));
        assert_eq!(records, vec![(KINA_KEY, 6_110_905)], "the consumable has no currency key");
    }

    #[test]
    fn inventory_counts_are_checkpoints() {
        let (events, stats) = decode(op::INVENTORY, &hex(ONE_RECORD));
        assert!(matches!(events[0], Event::Currency { source: CountSource::Inventory, count: 8_348_593, .. }));
        assert_eq!(stats.inventories, 1);
        assert_eq!(stats.inventories_without_kina, 0);
        let (events, stats) = decode(op::INVENTORY, &[0u8; 64]);
        assert!(events.is_empty());
        assert_eq!(stats.inventories_without_kina, 1);
    }

    #[test]
    fn other_currencies_are_reported_with_their_key() {
        // Kina and the currency keyed 37 33 70 37 side by side, as in a full inventory.
        let body = hex("00000000aaaa0100000500003f3370374c277e00000000000100bbbb0100000600003733703719d2000000000000");
        let records = currency_records(&body);
        assert_eq!(records, vec![(KINA_KEY, 8_267_596), (0x3770_3337, 53_785)]);
        let (events, stats) = decode(op::INVENTORY, &body);
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].kind(), "currency");
        assert_eq!(stats.currency_records, 1);
    }

    #[test]
    fn rejects_records_that_do_not_validate() {
        // Guard byte before the key is not zero.
        assert!(currency_records(&hex("0101007ec7060000c600013f337037b1637f0000000000000000")).is_empty());
        // Count far beyond any balance.
        assert!(currency_records(&hex("0101007ec7060000c600003f337037b1637f00000000ff0000")).is_empty());
        // Too short to hold the count.
        assert!(currency_records(&hex("0101007ec7060000c600003f337037b163")).is_empty());
        // Key at the very start has no room for a record head.
        assert!(currency_records(&hex("3f337037b1637f00000000000000000000")).is_empty());
    }

    #[test]
    fn reads_the_self_record() {
        // id 14513, masks, "Seraphel", server 2314, job 15, a byte, level 45.
        let mut body = hex("b1715e91c1283708");
        body.extend_from_slice(b"Seraphel");
        body.extend_from_slice(&hex("0a090f00000000"));
        body.extend_from_slice(&45u32.to_le_bytes());
        let (events, stats) = decode(op::SELF_INFO, &body);
        assert_eq!(events, vec![Event::SelfInfo { t_ms: 1_000, id: 14_513, name: "Seraphel".into(), server: 2314, level: 45 }]);
        assert_eq!(stats.self_records, 1);
        let mut bad_server = hex("b1715e91c1283708");
        bad_server.extend_from_slice(b"Seraphel");
        bad_server.extend_from_slice(&hex("0100"));
        let (events, stats) = decode(op::SELF_INFO, &bad_server);
        assert!(events.is_empty());
        assert_eq!(stats.self_rejected, 1);
    }

    #[test]
    fn reads_map_loads_and_heartbeats() {
        let mut body = 1u32.to_le_bytes().to_vec();
        body.extend_from_slice(&600_072u32.to_le_bytes());
        body.extend_from_slice(&[0; 44]);
        let (events, _) = decode(op::MAP_LOAD, &body);
        assert_eq!(events, vec![Event::ZoneChange { t_ms: 1_000, map_id: 600_072 }]);
        let (events, stats) = decode(op::MAP_LOAD, &[1, 0]);
        assert!(events.is_empty());
        assert_eq!(stats.map_rejected, 1);
        let (events, stats) = decode(op::HEARTBEAT, &1_791_722_111_217u64.to_le_bytes());
        assert_eq!(events, vec![Event::Heartbeat { t_ms: 1_000, server_ms: 1_791_722_111_217 }]);
        assert_eq!(stats.server_time_ms, Some(1_791_722_111_217));
    }

    #[test]
    fn names_are_letters_and_digits() {
        assert_eq!(valid_name(b"Ari"), Some("Ari"));
        assert_eq!(valid_name(b"Seraphel2"), Some("Seraphel2"));
        assert_eq!(valid_name(b"1234"), None);
        assert_eq!(valid_name(b"a b"), None);
        assert_eq!(valid_name(b""), None);
    }
}
