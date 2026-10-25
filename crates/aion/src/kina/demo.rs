//! Demo mode (`--demo`): a generated farming loop for trying the TUI without the game.
//!
//! A script plays town visits (vendor sales, purchases, teleport fees) and dungeon runs (a little loot, the
//! clear reward, a repair) and feeds the events through the real tracker, so the TUI sees exactly what live
//! capture would produce. On start two earlier sessions are played fast forward (one of them on an alt) and
//! the live session is already half an hour in. Finished demo sessions stay in memory and are never
//! written to disk.

use kina_core::parser::{CountSource, Event, KINA_KEY};
use kina_core::snapshot::{Character, EndReason, SessionSnapshot, SessionSummary};
use kina_core::tracker::Tracker;
use std::collections::VecDeque;

const TOWN: u32 = 1110;
/// Vakron Sky Island, Urugugu Canyon, Shattered Arkanis and Deus Research Base.
const DUNGEONS: [u32; 4] = [600_072, 600_012, 600_063, 600_053];
const REWARDS: [u64; 4] = [600_000, 400_000, 600_000, 478_300];
const MAX_FINISHED: usize = 20;
const HEARTBEAT_MS: u64 = 5_000;
const MAIN: &str = "Seraphel";
const ALT: &str = "Nyxara";

/// A small deterministic generator (xorshift), so every demo start looks the same.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform in `lo..=hi`.
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo + 1)
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

struct Script {
    rng: Rng,
    name: &'static str,
    balance: u64,
    queue: VecDeque<Event>,
    /// End of the planned events.
    planned_to: u64,
    cycle: usize,
}

impl Script {
    fn new(seed: u64, name: &'static str, balance: u64, start_ms: u64) -> Script {
        let mut s = Script { rng: Rng(seed), name, balance, queue: VecDeque::new(), planned_to: start_ms, cycle: 0 };
        s.loading_screen(start_ms, TOWN);
        s
    }

    fn currency(t_ms: u64, count: u64, source: CountSource) -> Event {
        Event::Currency { t_ms, key: KINA_KEY, count, source }
    }

    fn change(&mut self, t_ms: u64, delta: i64) {
        self.balance = self.balance.saturating_add_signed(delta);
        self.queue.push_back(Self::currency(t_ms, self.balance, CountSource::Update));
    }

    /// The order the game uses: the inventory, the map load, then the self record.
    fn loading_screen(&mut self, t_ms: u64, map_id: u32) {
        self.queue.push_back(Self::currency(t_ms, self.balance, CountSource::Inventory));
        self.queue.push_back(Event::ZoneChange { t_ms: t_ms + 200, map_id });
        self.queue.push_back(Event::SelfInfo { t_ms: t_ms + 4_000, id: 14_513, name: self.name.into(), server: 2314, level: 45 });
    }

    fn heartbeats(&mut self, from: u64, to: u64) -> Vec<Event> {
        (from / HEARTBEAT_MS + 1..=to / HEARTBEAT_MS).map(|i| Event::Heartbeat { t_ms: i * HEARTBEAT_MS, server_ms: i * HEARTBEAT_MS }).collect()
    }

    /// Plans one town visit and one dungeon run, starting at `planned_to`.
    fn plan_cycle(&mut self) {
        let start = self.planned_to;
        let mut t = start + 6_000;
        // Town: sell loot in quick bursts, buy consumables, sometimes pay a teleport.
        let town_end = start + self.rng.range(120_000, 200_000);
        while t < town_end - 15_000 {
            match self.rng.range(0, 9) {
                0..=4 => {
                    let n = self.rng.range(1, 4);
                    for i in 0..n {
                        let price = self.rng.range(2, 32) * 250;
                        self.change(t + i * 600, price as i64);
                    }
                }
                5..=7 => {
                    let n = self.rng.range(1, 4);
                    for i in 0..n {
                        self.change(t + i * 500, -12_000);
                    }
                }
                _ => self.change(t, -18_000),
            }
            t += self.rng.range(15_000, 35_000);
        }
        let d = self.cycle % DUNGEONS.len();
        self.loading_screen(town_end, DUNGEONS[d]);
        // Dungeon: a little loot on the way, the clear reward at the end, then the repair bill.
        let run_end = town_end + self.rng.range(540_000, 840_000);
        let mut t = town_end + 30_000;
        while t < run_end - 40_000 {
            if self.rng.chance(45) {
                let loot = self.rng.range(12, 60) * 100;
                self.change(t, loot as i64);
            }
            t += self.rng.range(40_000, 90_000);
        }
        self.change(run_end - 20_000, REWARDS[d] as i64);
        let repair = self.rng.range(25, 45) * 1_000;
        self.change(run_end - 14_000, -(repair as i64));
        self.loading_screen(run_end, TOWN);
        self.cycle += 1;

        let mut events: Vec<Event> = self.queue.drain(..).collect();
        events.extend(self.heartbeats(start, run_end));
        events.sort_by_key(Event::t_ms);
        self.queue = events.into();
        self.planned_to = run_end;
    }

    /// Feeds every event before `now_ms` into the tracker.
    fn run_until(&mut self, now_ms: u64, tracker: &mut Tracker) {
        loop {
            if self.queue.is_empty() {
                if self.planned_to >= now_ms {
                    return;
                }
                self.plan_cycle();
            }
            match self.queue.front() {
                Some(e) if e.t_ms() < now_ms => {
                    let e = self.queue.pop_front().expect("front exists");
                    tracker.push(&e);
                }
                _ => return,
            }
        }
    }
}

pub struct Demo {
    tracker: Tracker,
    script: Script,
    /// Newest last.
    finished: Vec<SessionSnapshot>,
}

impl Demo {
    pub fn new(now_ms: u64) -> Demo {
        let mut tracker = Tracker::default();
        let mut finished = Vec::new();
        // Each script logs its character in first, so the tracker never sees a switch mid session.
        let login = |tracker: &mut Tracker, t_ms: u64, name: &str| {
            tracker.push(&Event::SelfInfo { t_ms, id: 14_513, name: name.into(), server: 2314, level: 45 });
        };
        // Earlier today: the main for about an hour, then an alt for 40 minutes.
        let early_ms = now_ms - 4 * 3_600_000;
        login(&mut tracker, early_ms, MAIN);
        let mut early = Script::new(7, MAIN, 6_110_905, early_ms);
        early.run_until(early_ms + 65 * 60_000, &mut tracker);
        tracker.finish(EndReason::Idle);
        let alt_ms = now_ms - 2 * 3_600_000;
        login(&mut tracker, alt_ms, ALT);
        let mut alt = Script::new(11, ALT, 1_240_550, alt_ms);
        alt.run_until(alt_ms + 40 * 60_000, &mut tracker);
        tracker.finish(EndReason::Idle);
        finished.extend(tracker.drain_finished().into_iter().filter(|s| s.changes > 0));
        // The live session, already half an hour in.
        let live_ms = now_ms - 30 * 60_000;
        login(&mut tracker, live_ms, MAIN);
        let mut script = Script::new(23, MAIN, early.balance, live_ms);
        // The daily counter and the Abyss Points as the game sends them on the first loading screen.
        tracker.push(&Event::DailyKina { t_ms: live_ms, counted: 182_677, server_ms: live_ms, source: CountSource::Inventory });
        tracker.push(&Event::AbyssPoints { t_ms: live_ms, points: 138_120, cap: 2_000_000 });
        script.run_until(now_ms, &mut tracker);
        Demo { tracker, script, finished }
    }

    pub fn step(&mut self, now_ms: u64) {
        self.script.run_until(now_ms, &mut self.tracker);
        self.tracker.tick(now_ms);
        self.collect();
    }

    fn collect(&mut self) {
        self.finished.extend(self.tracker.drain_finished().into_iter().filter(|s| s.changes > 0));
        let excess = self.finished.len().saturating_sub(MAX_FINISHED);
        self.finished.drain(..excess);
    }

    pub fn reset(&mut self, now_ms: u64) {
        self.tracker.reset(now_ms);
        self.collect();
    }

    pub fn current(&self) -> Option<SessionSnapshot> {
        self.tracker.snapshot()
    }

    pub fn character(&self) -> Option<Character> {
        self.tracker.character().cloned()
    }

    pub fn balance(&self) -> Option<u64> {
        self.tracker.balance()
    }

    pub fn tracker(&self) -> &Tracker {
        &self.tracker
    }

    /// Newest first.
    pub fn summaries(&self) -> Vec<SessionSummary> {
        self.finished.iter().rev().map(SessionSnapshot::summary).collect()
    }

    pub fn get(&self, id: u64) -> Option<SessionSnapshot> {
        self.finished.iter().find(|s| s.id == id).cloned().or_else(|| self.current().filter(|s| s.id == id))
    }

    pub fn delete(&mut self, id: u64) {
        self.finished.retain(|s| s.id != id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_791_722_000_000;

    #[test]
    fn starts_with_history_and_a_live_session() {
        let demo = Demo::new(NOW);
        let history = demo.summaries();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].character_name.as_deref(), Some(ALT));
        assert_eq!(history[1].character_name.as_deref(), Some(MAIN));
        assert!(history.iter().all(|s| s.net > 0 && s.changes > 0));
        let live = demo.current().unwrap();
        assert!(live.active);
        assert_eq!(live.character.as_ref().unwrap().name, MAIN);
        assert!(live.changes > 5, "{} changes", live.changes);
        assert!(live.duration_ms > 25 * 60_000, "{} ms", live.duration_ms);
        assert_eq!(live.resyncs, 0, "every checkpoint agrees with the script");
        assert!(live.runs.iter().any(|r| r.map.as_ref().map(|m| m.is_instance).unwrap_or(false)));
        assert_eq!(live.balance, demo.balance());
    }

    #[test]
    fn steps_forward_and_resets() {
        let mut demo = Demo::new(NOW);
        let before = demo.current().unwrap();
        demo.step(NOW + 20 * 60_000);
        let after = demo.current().unwrap();
        assert_eq!(after.id, before.id);
        assert!(after.changes > before.changes);
        demo.reset(NOW + 20 * 60_000);
        assert_eq!(demo.summaries().len(), 3);
        let fresh = demo.current().unwrap();
        assert_eq!(fresh.net, 0);
        assert_eq!(fresh.start_balance, after.balance);
        let id = demo.summaries()[0].id;
        assert!(demo.get(id).is_some());
        demo.delete(id);
        assert_eq!(demo.summaries().len(), 2);
    }
}
