//! Turns parser events into kina sessions.
//!
//! Rules (DESIGN.md has the reasoning):
//!
//! - A session starts at the first kina count seen, which becomes its starting balance. That first count
//!   is never income: the tracker does not know what came before it.
//! - Only live inventory changes (`1B 56`) count as income or spending. The full inventory on a loading
//!   screen (`11 56`) is a checkpoint; on all four captures it matched the running balance every time, so
//!   a difference means missed packets or another character, and is logged as a resync without counting.
//! - A character switch sends the new character's inventory a few seconds before its self record. When the
//!   self record names someone else, that checkpoint is handed to the new session and the old one ends.
//! - Changes in the same direction within a few seconds on the same map merge into one log entry.
//! - A run is one visit to a map, from its loading screen to the next; reloads of the same map continue it.
//! - Connected time leaves out gaps of more than a minute without packets; the per hour rates use it.
//! - The session ends by hand (reset), when another character logs in, or after a long silence.

use crate::gamedata::GameData;
use crate::parser::{CountSource, Event, KINA_KEY};
use crate::snapshot::{per_hour, BalancePoint, Character, EndReason, EntryKind, KinaEntry, MapStats, RunStats, SessionSnapshot};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct TrackerConfig {
    /// A session ends when nothing arrived from the game for this long.
    pub idle_end_ms: u64,
    /// Gaps between packets longer than this do not count as connected time.
    pub connected_gap_ms: u64,
    /// Changes of the same direction this close together merge into one entry.
    pub group_window_ms: u64,
    /// A checkpoint this soon before a new character's self record belongs to the new character.
    pub switch_window_ms: u64,
    pub max_entries: usize,
    pub max_timeline: usize,
}

impl Default for TrackerConfig {
    fn default() -> Self {
        Self {
            idle_end_ms: 15 * 60_000,
            connected_gap_ms: 60_000,
            group_window_ms: 3_000,
            switch_window_ms: 120_000,
            max_entries: 2_000,
            max_timeline: 4_000,
        }
    }
}

/// The latest count of a currency other than kina, for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrencySeen {
    pub count: u64,
    pub t_ms: u64,
}

/// Kina that counts toward the daily gain limit per day: what the game's window shows as the second number.
pub const DAILY_KINA_LIMIT: u64 = 1_000_000;

/// The daily gain limit counter as the game last sent it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DailyKina {
    /// Kina counted toward the limit since the last daily reset.
    pub counted: u64,
    /// Server time the counter stood there (Unix ms).
    pub server_ms: u64,
}

/// Abyss Points and their cap as the game last sent them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AbyssPoints {
    pub points: u64,
    pub cap: u64,
}

#[derive(Debug, Clone)]
struct Run {
    map_id: Option<u32>,
    started_ms: u64,
    earned: u64,
    spent: u64,
    changes: u32,
}

/// A checkpoint jump, remembered so a character switch can take it back.
#[derive(Debug, Clone, Copy)]
struct Checkpoint {
    t_ms: u64,
    before: Option<u64>,
    entries_len: usize,
    timeline_len: usize,
}

#[derive(Debug, Clone)]
struct Session {
    id: u64,
    character: Option<Character>,
    started_ms: u64,
    last_ms: u64,
    connected_ms: u64,
    start_balance: Option<u64>,
    balance: Option<u64>,
    earned: u64,
    spent: u64,
    changes: u32,
    resyncs: u32,
    biggest_gain: u64,
    biggest_spend: u64,
    entries: Vec<KinaEntry>,
    runs: Vec<Run>,
    timeline: Vec<BalancePoint>,
    last_checkpoint: Option<Checkpoint>,
}

impl Session {
    fn new(id: u64, character: Option<Character>, t_ms: u64, map_id: Option<u32>, balance: Option<u64>) -> Session {
        let mut s = Session {
            id,
            character,
            started_ms: t_ms,
            last_ms: t_ms,
            connected_ms: 0,
            start_balance: balance,
            balance,
            earned: 0,
            spent: 0,
            changes: 0,
            resyncs: 0,
            biggest_gain: 0,
            biggest_spend: 0,
            entries: Vec::new(),
            runs: vec![Run { map_id, started_ms: t_ms, earned: 0, spent: 0, changes: 0 }],
            timeline: Vec::new(),
            last_checkpoint: None,
        };
        if let Some(b) = balance {
            s.timeline.push(BalancePoint { t_ms, balance: b });
        }
        s
    }

    fn current_map(&self) -> Option<u32> {
        self.runs.last().and_then(|r| r.map_id)
    }

    fn enter_map(&mut self, map_id: u32, t_ms: u64) {
        match self.runs.last_mut() {
            Some(run) if run.map_id == Some(map_id) => {}
            // The session began before any loading screen and nothing happened yet: that was this map.
            Some(run) if run.map_id.is_none() && run.changes == 0 => run.map_id = Some(map_id),
            _ => self.runs.push(Run { map_id: Some(map_id), started_ms: t_ms, earned: 0, spent: 0, changes: 0 }),
        }
    }

    fn push_point(&mut self, t_ms: u64, balance: u64, max: usize) {
        self.timeline.push(BalancePoint { t_ms, balance });
        if self.timeline.len() > max {
            // Halve the resolution, keeping the first and the last point.
            let last = *self.timeline.last().expect("not empty");
            let mut kept: Vec<BalancePoint> = self.timeline.iter().step_by(2).copied().collect();
            if kept.last() != Some(&last) {
                kept.push(last);
            }
            self.timeline = kept;
            self.last_checkpoint = None;
        }
    }

    fn push_entry(&mut self, entry: KinaEntry, max: usize) {
        self.entries.push(entry);
        if self.entries.len() > max {
            self.entries.remove(0);
            self.last_checkpoint = None;
        }
    }

    fn change(&mut self, t_ms: u64, delta: i64, balance: u64, config: &TrackerConfig) {
        let kind = if delta > 0 { EntryKind::Gain } else { EntryKind::Spend };
        let amount = delta.unsigned_abs();
        if delta > 0 {
            self.earned += amount;
            self.biggest_gain = self.biggest_gain.max(amount);
        } else {
            self.spent += amount;
            self.biggest_spend = self.biggest_spend.max(amount);
        }
        self.changes += 1;
        self.balance = Some(balance);
        let map_id = self.current_map();
        if let Some(run) = self.runs.last_mut() {
            if delta > 0 {
                run.earned += amount;
            } else {
                run.spent += amount;
            }
            run.changes += 1;
        }
        let merged = match self.entries.last_mut() {
            Some(last) if last.kind == kind && last.map_id == map_id && t_ms.saturating_sub(last.last_ms) <= config.group_window_ms => {
                last.delta += delta;
                last.count += 1;
                last.last_ms = t_ms;
                last.balance = balance;
                true
            }
            _ => false,
        };
        if !merged {
            let map_name = map_id.map(|m| GameData::get().map_label(m));
            self.push_entry(KinaEntry { t_ms, last_ms: t_ms, delta, count: 1, balance, kind, map_id, map_name }, config.max_entries);
        }
        self.push_point(t_ms, balance, config.max_timeline);
        self.last_checkpoint = None;
    }

    fn resync(&mut self, t_ms: u64, delta: i64, balance: u64, config: &TrackerConfig) {
        let checkpoint = Checkpoint { t_ms, before: self.balance, entries_len: self.entries.len(), timeline_len: self.timeline.len() };
        self.resyncs += 1;
        self.balance = Some(balance);
        let map_id = self.current_map();
        let map_name = map_id.map(|m| GameData::get().map_label(m));
        self.push_entry(KinaEntry { t_ms, last_ms: t_ms, delta, count: 1, balance, kind: EntryKind::Resync, map_id, map_name }, config.max_entries);
        self.push_point(t_ms, balance, config.max_timeline);
        if self.entries.len() == checkpoint.entries_len + 1 {
            self.last_checkpoint = Some(checkpoint);
        }
    }

    /// Takes back the last checkpoint jump if it is recent and nothing happened after it. Returns its time.
    fn take_back_checkpoint(&mut self, now_ms: u64, window_ms: u64) -> Option<u64> {
        let cp = self.last_checkpoint.take()?;
        if now_ms.saturating_sub(cp.t_ms) > window_ms || self.entries.len() != cp.entries_len + 1 {
            return None;
        }
        self.entries.truncate(cp.entries_len);
        self.timeline.truncate(cp.timeline_len);
        self.resyncs = self.resyncs.saturating_sub(1);
        self.balance = cp.before;
        self.last_ms = self.last_ms.min(cp.t_ms).max(self.started_ms);
        Some(cp.t_ms)
    }

    fn snapshot(&self, active: bool, end_reason: Option<EndReason>, current_map: Option<u32>) -> SessionSnapshot {
        let gd = GameData::get();
        let net = self.earned as i64 - self.spent as i64;
        let duration_ms = self.connected_ms;
        let runs: Vec<RunStats> = self
            .runs
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let ended_ms = self.runs.get(i + 1).map(|n| n.started_ms).unwrap_or(self.last_ms).max(r.started_ms);
                RunStats {
                    index: i as u32 + 1,
                    map: r.map_id.map(|m| gd.map(m)),
                    started_ms: r.started_ms,
                    ended_ms,
                    duration_ms: ended_ms - r.started_ms,
                    earned: r.earned,
                    spent: r.spent,
                    net: r.earned as i64 - r.spent as i64,
                    changes: r.changes,
                    current: active && i + 1 == self.runs.len(),
                }
            })
            .collect();
        let mut by_map: BTreeMap<Option<u32>, MapStats> = BTreeMap::new();
        for r in &runs {
            let m = by_map.entry(r.map.as_ref().map(|m| m.id)).or_insert_with(|| MapStats {
                map: r.map.clone(),
                visits: 0,
                time_ms: 0,
                earned: 0,
                spent: 0,
                net: 0,
                changes: 0,
            });
            m.visits += 1;
            m.time_ms += r.duration_ms;
            m.earned += r.earned;
            m.spent += r.spent;
            m.net += r.net;
            m.changes += r.changes;
        }
        let mut maps: Vec<MapStats> = by_map.into_values().collect();
        maps.sort_by(|a, b| b.net.cmp(&a.net).then(b.time_ms.cmp(&a.time_ms)));
        SessionSnapshot {
            id: self.id,
            active,
            character: self.character.clone(),
            started_ms: self.started_ms,
            last_ms: self.last_ms,
            duration_ms,
            end_reason,
            start_balance: self.start_balance,
            balance: self.balance,
            earned: self.earned,
            spent: self.spent,
            net,
            net_per_hour: per_hour(net as f64, duration_ms),
            earned_per_hour: per_hour(self.earned as f64, duration_ms),
            changes: self.changes,
            resyncs: self.resyncs,
            biggest_gain: self.biggest_gain,
            biggest_spend: self.biggest_spend,
            current_map: current_map.or(self.current_map()).map(|m| gd.map(m)),
            entries: self.entries.clone(),
            runs,
            maps,
            timeline: self.timeline.clone(),
        }
    }
}

pub struct Tracker {
    config: TrackerConfig,
    next_id: u64,
    character: Option<Character>,
    map_id: Option<u32>,
    /// Latest kina count seen, whichever session it belongs to.
    balance: Option<u64>,
    session: Option<Session>,
    /// The session that ended last, shown until a new one starts.
    last_finished: Option<SessionSnapshot>,
    finished: Vec<SessionSnapshot>,
    last_seen_ms: u64,
    currencies: BTreeMap<u32, CurrencySeen>,
    daily: Option<DailyKina>,
    abyss: Option<AbyssPoints>,
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new(TrackerConfig::default())
    }
}

impl Tracker {
    pub fn new(config: TrackerConfig) -> Self {
        Self {
            config,
            next_id: 1,
            character: None,
            map_id: None,
            balance: None,
            session: None,
            last_finished: None,
            finished: Vec::new(),
            last_seen_ms: 0,
            currencies: BTreeMap::new(),
            daily: None,
            abyss: None,
        }
    }

    pub fn config(&self) -> &TrackerConfig {
        &self.config
    }

    pub fn set_config(&mut self, config: TrackerConfig) {
        self.config = config;
    }

    /// Session ids continue after the highest one in the history.
    pub fn set_next_session_id(&mut self, id: u64) {
        self.next_id = self.next_id.max(id);
    }

    pub fn character(&self) -> Option<&Character> {
        self.character.as_ref()
    }

    pub fn balance(&self) -> Option<u64> {
        self.balance
    }

    pub fn last_seen_ms(&self) -> u64 {
        self.last_seen_ms
    }

    /// Currencies other than kina by item key, with the latest count seen.
    pub fn currencies(&self) -> &BTreeMap<u32, CurrencySeen> {
        &self.currencies
    }

    /// The daily gain limit counter, once a loading screen or a counted gain sent it. It is not reset here: the
    /// game resets it at a fixed time of day, which the app applies.
    pub fn daily_kina(&self) -> Option<DailyKina> {
        self.daily
    }

    pub fn abyss_points(&self) -> Option<AbyssPoints> {
        self.abyss
    }

    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn push(&mut self, event: &Event) {
        let t_ms = event.t_ms();
        self.touch(t_ms);
        match event {
            Event::Heartbeat { .. } => {}
            Event::ZoneChange { map_id, .. } => {
                self.map_id = Some(*map_id);
                if let Some(s) = self.session.as_mut() {
                    s.enter_map(*map_id, t_ms);
                }
            }
            Event::SelfInfo { name, server, level, .. } => {
                self.on_self(t_ms, Character { name: name.clone(), server: *server, level: *level });
            }
            Event::Currency { key, count, source, .. } => {
                if *key == KINA_KEY {
                    self.on_kina(t_ms, *count, *source);
                } else {
                    self.currencies.insert(*key, CurrencySeen { count: *count, t_ms });
                }
            }
            Event::DailyKina { counted, server_ms, .. } => {
                // Packets of one batch can arrive out of order; the counter's own clock decides.
                if self.daily.is_none_or(|d| *server_ms >= d.server_ms) {
                    self.daily = Some(DailyKina { counted: *counted, server_ms: *server_ms });
                }
            }
            Event::AbyssPoints { points, cap, .. } => self.abyss = Some(AbyssPoints { points: *points, cap: *cap }),
        }
    }

    /// Connected time and the last seen clock.
    fn touch(&mut self, t_ms: u64) {
        self.last_seen_ms = self.last_seen_ms.max(t_ms);
        if let Some(s) = self.session.as_mut() {
            if t_ms > s.last_ms {
                let gap = t_ms - s.last_ms;
                if gap <= self.config.connected_gap_ms {
                    s.connected_ms += gap;
                }
                s.last_ms = t_ms;
            }
        }
    }

    fn on_self(&mut self, t_ms: u64, who: Character) {
        self.character = Some(who.clone());
        let Some(s) = self.session.as_mut() else { return };
        match &s.character {
            None => s.character = Some(who),
            Some(c) if c.name == who.name && c.server == who.server => s.character = Some(who),
            Some(_) => {
                // The new character's inventory came a few seconds before this record and was booked as a
                // checkpoint jump of the old session: give it back, end the old session at the switch and
                // start the new one from that inventory.
                let switched_at = s.take_back_checkpoint(t_ms, self.config.switch_window_ms);
                let balance = if switched_at.is_some() { self.balance } else { None };
                let start = switched_at.unwrap_or(t_ms);
                self.finish(EndReason::CharacterChange);
                let id = self.take_id();
                let mut next = Session::new(id, Some(who), start, self.map_id, balance);
                next.last_ms = t_ms.max(start);
                next.connected_ms = t_ms.saturating_sub(start).min(self.config.connected_gap_ms);
                self.session = Some(next);
            }
        }
    }

    fn on_kina(&mut self, t_ms: u64, count: u64, source: CountSource) {
        self.balance = Some(count);
        if self.session.is_none() {
            let id = self.take_id();
            self.session = Some(Session::new(id, self.character.clone(), t_ms, self.map_id, None));
            self.last_finished = None;
        }
        let config = self.config.clone();
        let s = self.session.as_mut().expect("just ensured");
        match s.balance {
            None => {
                s.start_balance = Some(count);
                s.balance = Some(count);
                s.push_point(t_ms, count, config.max_timeline);
            }
            Some(b) if b == count => {}
            Some(b) => {
                let delta = count as i64 - b as i64;
                match source {
                    CountSource::Update => s.change(t_ms, delta, count, &config),
                    CountSource::Inventory => s.resync(t_ms, delta, count, &config),
                }
            }
        }
    }

    /// Ends the session after a long silence. `now_ms` is the capture clock while packets flow, the wall
    /// clock otherwise.
    pub fn tick(&mut self, now_ms: u64) {
        let idle = self.session.as_ref().map(|s| now_ms.saturating_sub(s.last_ms) > self.config.idle_end_ms).unwrap_or(false);
        if idle {
            self.finish(EndReason::Idle);
        }
    }

    /// Ends the session by hand and starts a fresh one from the current balance.
    pub fn reset(&mut self, now_ms: u64) {
        self.finish(EndReason::Manual);
        if self.balance.is_some() {
            let id = self.take_id();
            self.session = Some(Session::new(id, self.character.clone(), now_ms.max(self.last_seen_ms), self.map_id, self.balance));
            self.last_finished = None;
        }
    }

    /// Ends the live session, if any, with `reason`.
    pub fn finish(&mut self, reason: EndReason) {
        if let Some(s) = self.session.take() {
            let snap = s.snapshot(false, Some(reason), None);
            self.last_finished = Some(snap.clone());
            self.finished.push(snap);
        }
    }

    /// Sessions that ended since the last call, oldest first.
    pub fn drain_finished(&mut self) -> Vec<SessionSnapshot> {
        std::mem::take(&mut self.finished)
    }

    pub fn is_active(&self) -> bool {
        self.session.is_some()
    }

    /// The live session, or the last finished one while no session runs.
    pub fn snapshot(&self) -> Option<SessionSnapshot> {
        match &self.session {
            Some(s) => Some(s.snapshot(true, None, self.map_id)),
            None => self.last_finished.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VAKRON: u32 = 600_072;
    const TOWN: u32 = 1110;

    fn kina(t_ms: u64, count: u64) -> Event {
        Event::Currency { t_ms, key: KINA_KEY, count, source: CountSource::Update }
    }

    fn inventory(t_ms: u64, count: u64) -> Event {
        Event::Currency { t_ms, key: KINA_KEY, count, source: CountSource::Inventory }
    }

    fn zone(t_ms: u64, map_id: u32) -> Event {
        Event::ZoneChange { t_ms, map_id }
    }

    fn me(t_ms: u64, name: &str) -> Event {
        Event::SelfInfo { t_ms, id: 1, name: name.into(), server: 2314, level: 45 }
    }

    fn beat(t_ms: u64) -> Event {
        Event::Heartbeat { t_ms, server_ms: t_ms }
    }

    fn run(events: &[Event]) -> Tracker {
        let mut t = Tracker::default();
        for e in events {
            t.push(e);
        }
        t
    }

    #[test]
    fn first_count_is_the_baseline_not_income() {
        let t = run(&[kina(1_000, 6_796_955)]);
        let s = t.snapshot().unwrap();
        assert!(s.active);
        assert_eq!(s.start_balance, Some(6_796_955));
        assert_eq!(s.balance, Some(6_796_955));
        assert_eq!((s.earned, s.spent, s.net, s.changes), (0, 0, 0, 0));
        assert_eq!(s.timeline.len(), 1);
    }

    #[test]
    fn live_changes_count_and_checkpoints_that_agree_do_nothing() {
        // The start of session1: a purchase run, a checkpoint that matches, a dungeon reward.
        let t = run(&[
            inventory(1_000, 6_110_905),
            zone(1_100, TOWN),
            me(1_200, "Ari"),
            zone(10_000, VAKRON),
            kina(20_000, 6_710_905),
            kina(30_000, 6_675_905),
            inventory(40_000, 6_675_905),
            zone(40_100, TOWN),
        ]);
        let s = t.snapshot().unwrap();
        assert_eq!(s.earned, 600_000);
        assert_eq!(s.spent, 35_000);
        assert_eq!(s.net, 565_000);
        assert_eq!(s.balance, Some(6_675_905));
        assert_eq!(s.resyncs, 0);
        assert_eq!(s.biggest_gain, 600_000);
        assert_eq!(s.character.as_ref().map(|c| c.name.as_str()), Some("Ari"));
        assert_eq!(s.entries.len(), 2);
        assert_eq!(s.entries[0].kind, EntryKind::Gain);
        assert_eq!(s.entries[0].map_name.as_deref(), Some("Vakron Sky Island (Conquest [Normal])"));
        assert_eq!(s.runs.len(), 3, "town, Vakron, town");
        assert_eq!(s.runs[1].net, 565_000);
        assert_eq!(s.runs[1].duration_ms, 30_100);
        assert!(s.runs[2].current);
        assert_eq!(s.maps[0].map.as_ref().map(|m| m.id), Some(VAKRON));
        assert_eq!(s.maps[0].net, 565_000);
        assert_eq!(s.current_map.as_ref().map(|m| m.id), Some(TOWN));
    }

    #[test]
    fn quick_changes_in_one_direction_merge() {
        // Ten escalating purchases within five seconds, then a sale.
        let mut events = vec![inventory(0, 6_736_205), zone(1, TOWN)];
        let steps = [6_723_705, 6_700_905, 6_665_905, 6_616_905, 6_551_905, 6_469_905, 6_370_905, 6_251_905, 6_112_905];
        for (i, b) in steps.iter().enumerate() {
            events.push(kina(1_000 + i as u64 * 500, *b));
        }
        events.push(kina(6_000, 6_120_905));
        events.push(kina(20_000, 6_110_905));
        let s = run(&events).snapshot().unwrap();
        assert_eq!(s.entries.len(), 3);
        assert_eq!(s.entries[0].count, 9);
        assert_eq!(s.entries[0].delta, 6_112_905 - 6_736_205);
        assert_eq!(s.entries[0].balance, 6_112_905);
        assert_eq!(s.entries[1].kind, EntryKind::Gain);
        assert_eq!(s.entries[2].count, 1, "fourteen seconds later is a new entry");
        assert_eq!(s.changes, 11);
        assert_eq!(s.net, 6_110_905 - 6_736_205);
    }

    #[test]
    fn a_checkpoint_that_disagrees_is_a_resync_not_income() {
        let s = run(&[inventory(0, 1_000_000), kina(10_000, 1_200_000), inventory(20_000, 1_500_000)]).snapshot().unwrap();
        assert_eq!(s.earned, 200_000);
        assert_eq!(s.net, 200_000);
        assert_eq!(s.resyncs, 1);
        assert_eq!(s.balance, Some(1_500_000));
        assert_eq!(s.entries.last().unwrap().kind, EntryKind::Resync);
        assert_eq!(s.entries.last().unwrap().delta, 300_000);
    }

    #[test]
    fn switching_characters_hands_the_inventory_to_the_new_session() {
        let mut t = run(&[
            inventory(0, 6_000_000),
            me(5_000, "Ari"),
            kina(60_000, 6_600_000),
            // Character select, then another character: its inventory, map load, self record.
            inventory(200_000, 52_000),
            zone(200_100, TOWN),
            me(205_000, "Alt"),
            kina(300_000, 60_000),
        ]);
        let finished = t.drain_finished();
        assert_eq!(finished.len(), 1);
        let old = &finished[0];
        assert_eq!(old.end_reason, Some(EndReason::CharacterChange));
        assert_eq!(old.character.as_ref().unwrap().name, "Ari");
        assert_eq!(old.net, 600_000);
        assert_eq!(old.resyncs, 0, "the alt's inventory was taken back");
        assert_eq!(old.balance, Some(6_600_000));
        assert!(old.entries.iter().all(|e| e.kind != EntryKind::Resync));
        let new = t.snapshot().unwrap();
        assert_eq!(new.character.as_ref().unwrap().name, "Alt");
        assert_eq!(new.start_balance, Some(52_000));
        assert_eq!(new.net, 8_000);
        assert_eq!(new.started_ms, 200_000);
        assert!(new.id > old.id);
    }

    #[test]
    fn same_character_again_keeps_the_session() {
        let mut t = run(&[inventory(0, 100_000), me(1_000, "Ari"), inventory(50_000, 100_000), me(51_000, "Ari")]);
        assert!(t.drain_finished().is_empty());
        assert!(t.is_active());
    }

    #[test]
    fn connected_time_skips_long_gaps_and_drives_the_rates() {
        let mut events = vec![inventory(0, 1_000_000)];
        for i in 1..=60 {
            events.push(beat(i * 30_000)); // 30 minutes of heartbeats every 30 s
        }
        events.push(kina(1_800_000, 1_600_000));
        events.push(beat(1_800_000 + 3_600_000)); // an hour of silence: not connected time
        let s = run(&events).snapshot().unwrap();
        assert_eq!(s.duration_ms, 1_800_000);
        assert_eq!(s.net_per_hour, 1_200_000.0);
        assert_eq!(s.earned_per_hour, 1_200_000.0);
    }

    #[test]
    fn idle_ends_the_session_and_the_next_count_starts_a_new_one() {
        let mut t = run(&[inventory(0, 500), kina(1_000, 700)]);
        t.tick(1_000 + TrackerConfig::default().idle_end_ms);
        assert!(t.is_active(), "not idle yet");
        t.tick(2_000 + TrackerConfig::default().idle_end_ms);
        assert!(!t.is_active());
        let done = t.drain_finished();
        assert_eq!(done[0].end_reason, Some(EndReason::Idle));
        assert_eq!(done[0].net, 200);
        assert_eq!(t.snapshot().unwrap().id, done[0].id, "the last session stays visible");
        t.push(&inventory(10_000_000, 900));
        let s = t.snapshot().unwrap();
        assert!(s.active);
        assert_eq!(s.start_balance, Some(900));
        assert_eq!(s.net, 0);
    }

    #[test]
    fn reset_starts_from_the_current_balance() {
        let mut t = run(&[inventory(0, 1_000), kina(1_000, 3_000), zone(1_500, VAKRON)]);
        t.reset(2_000);
        let done = t.drain_finished();
        assert_eq!(done[0].end_reason, Some(EndReason::Manual));
        assert_eq!(done[0].net, 2_000);
        let s = t.snapshot().unwrap();
        assert!(s.active);
        assert_eq!(s.start_balance, Some(3_000));
        assert_eq!(s.net, 0);
        assert_eq!(s.runs[0].map.as_ref().map(|m| m.id), Some(VAKRON));
        t.push(&kina(3_000, 2_500));
        assert_eq!(t.snapshot().unwrap().net, -500);
    }

    #[test]
    fn reset_without_a_balance_waits_for_one() {
        let mut t = run(&[me(0, "Ari")]);
        t.reset(1_000);
        assert!(t.snapshot().is_none());
        t.push(&kina(2_000, 10));
        assert_eq!(t.snapshot().unwrap().character.unwrap().name, "Ari");
    }

    #[test]
    fn the_daily_counter_and_the_abyss_points_keep_the_latest() {
        let daily = |t_ms, counted, server_ms, source| Event::DailyKina { t_ms, counted, server_ms, source };
        let t = run(&[
            daily(10, 183_413, 1_000, CountSource::Update),
            daily(20, 192_339, 9_000, CountSource::Update),
            daily(21, 191_263, 8_000, CountSource::Update),
            Event::AbyssPoints { t_ms: 30, points: 138_120, cap: 2_000_000 },
        ]);
        assert_eq!(t.daily_kina(), Some(DailyKina { counted: 192_339, server_ms: 9_000 }), "an older counter arriving late is ignored");
        assert_eq!(t.abyss_points(), Some(AbyssPoints { points: 138_120, cap: 2_000_000 }));
        assert!(t.snapshot().is_none(), "neither starts a kina session");
    }

    #[test]
    fn other_currencies_are_kept_apart() {
        let t = run(&[Event::Currency { t_ms: 5, key: 0x3770_3337, count: 53_913, source: CountSource::Inventory }]);
        assert!(t.snapshot().is_none());
        assert_eq!(t.currencies().get(&0x3770_3337), Some(&CurrencySeen { count: 53_913, t_ms: 5 }));
    }

    #[test]
    fn timeline_is_bounded() {
        let config = TrackerConfig { max_timeline: 10, group_window_ms: 0, ..Default::default() };
        let mut t = Tracker::new(config);
        t.push(&inventory(0, 0));
        for i in 1..=50u64 {
            t.push(&kina(i * 10_000, i * 100));
        }
        let s = t.snapshot().unwrap();
        assert!(s.timeline.len() <= 11, "{}", s.timeline.len());
        assert_eq!(s.timeline.first().unwrap().balance, 0);
        assert_eq!(s.timeline.last().unwrap().balance, 5_000);
        assert_eq!(s.net, 5_000);
    }

    #[test]
    fn summary_names_the_best_map() {
        let s = run(&[inventory(0, 0), zone(1, VAKRON), kina(2, 600_000), zone(3, TOWN), kina(4, 550_000)]).snapshot().unwrap();
        let sum = s.summary();
        assert_eq!(sum.top_map.as_deref(), Some("Vakron Sky Island"));
        assert_eq!(sum.runs, 2);
        assert_eq!(sum.net, 550_000);
        assert_eq!(sum.end_balance, Some(550_000));
    }
}
