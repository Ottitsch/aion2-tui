//! Demo mode: a generated boss fight that evolves in real time, so the UI can be built and screenshotted
//! without the game. Snapshots are built directly (no tracker involved) and finished fights stay in memory,
//! so the real history on disk is never polluted.
//!
//! One fight lasts about two minutes: five players of different classes burn the boss down, one of them
//! dies and recovers on the way, the boss dies, the result stays on screen for a short pause, and the next
//! fight starts with the next encounter id and another boss.

use meter_core::gamedata::GameData;
use meter_core::snapshot::{
    ActorStats, DeathInfo, EncounterSummary, EndReason, SkillStats, Snapshot, TargetDamage, TargetInfo, TimelinePoint,
};

/// Boss HP is sized so the party needs about this long.
const FIGHT_TARGET_MS: u64 = 120_000;
/// Result stays on screen this long between fights.
const PAUSE_MS: u64 = 8_000;
/// Gaps between an actor's hits longer than this do not count as active time (same rule as the tracker).
const ACTIVE_GAP_MS: u64 = 5_000;
/// Finished fights kept for the history list.
const MAX_FINISHED: usize = 20;
/// Largest step the generator simulates at once (after the machine slept, for example).
const MAX_STEP_MS: u64 = 2_000;
const CRIT_MULTIPLIER: f64 = 1.7;

struct SkillDef {
    id: u32,
    /// Relative pick frequency.
    weight: f64,
    /// Damage per hit relative to the actor's average hit.
    power: f64,
    dot: bool,
    heal: bool,
}

struct PlayerDef {
    name: &'static str,
    class_id: u32,
    level: u32,
    combat_power: u64,
    gear_score: u32,
    /// Target DPS on the boss.
    dps: f64,
    /// Hits per second.
    hit_rate: f64,
    crit_rate: f64,
    back_rate: f64,
    front_rate: f64,
    is_self: bool,
    /// Dies at this point of the fight and is back up ten seconds later.
    dies_at_ms: Option<u64>,
    skills: &'static [SkillDef],
}

const fn skill(id: u32, weight: f64, power: f64) -> SkillDef {
    SkillDef { id, weight, power, dot: false, heal: false }
}

const fn dot(id: u32, weight: f64, power: f64) -> SkillDef {
    SkillDef { id, weight, power, dot: true, heal: false }
}

const fn heal(id: u32, weight: f64, power: f64) -> SkillDef {
    SkillDef { id, weight, power, dot: false, heal: true }
}

const GLADIATOR: &[SkillDef] = &[
    skill(11010000, 3.0, 0.8),
    skill(11020000, 2.0, 0.9),
    skill(11030000, 2.0, 1.2),
    skill(11040000, 1.5, 1.6),
    skill(11050000, 1.0, 2.2),
    skill(11060000, 0.8, 2.6),
    skill(11080000, 1.0, 1.1),
    skill(11090000, 0.6, 3.0),
    skill(11100000, 0.5, 3.5),
];

const SORCERER: &[SkillDef] = &[
    skill(15010000, 3.0, 0.7),
    dot(15020000, 2.0, 0.4),
    skill(15030000, 1.5, 1.4),
    skill(15040000, 1.0, 2.4),
    skill(15050000, 1.5, 1.3),
    skill(15060000, 0.5, 4.0),
    skill(15080000, 1.0, 1.2),
    skill(15120000, 0.7, 2.0),
];

const ASSASSIN: &[SkillDef] = &[
    skill(13010000, 3.0, 0.6),
    skill(13020000, 1.0, 1.0),
    skill(13030000, 2.0, 1.1),
    skill(13040000, 2.0, 0.9),
    skill(13050000, 1.5, 1.4),
    skill(13060000, 0.6, 3.2),
    skill(13070000, 0.8, 2.4),
    skill(13090000, 0.7, 1.8),
    skill(13110000, 0.6, 1.5),
    skill(13120000, 0.5, 2.8),
];

const TEMPLAR: &[SkillDef] = &[
    skill(12010000, 3.0, 0.8),
    skill(12020000, 2.0, 1.0),
    skill(12030000, 1.0, 1.5),
    skill(12040000, 1.5, 1.1),
    skill(12060000, 1.0, 1.7),
    skill(12090000, 0.6, 2.4),
    skill(12100000, 1.0, 1.3),
];

const CLERIC: &[SkillDef] = &[
    skill(17010000, 2.0, 1.0),
    skill(17020000, 1.5, 1.4),
    skill(17030000, 1.0, 1.2),
    skill(17040000, 0.6, 2.4),
    skill(17050000, 0.5, 2.8),
    skill(17060000, 3.0, 0.7),
    dot(17070000, 1.5, 0.5),
    heal(17100000, 1.2, 2.5),
    heal(17120000, 0.6, 4.0),
];

const PLAYERS: &[PlayerDef] = &[
    PlayerDef {
        name: "Seraphel",
        class_id: 11,
        level: 60,
        combat_power: 48_200,
        gear_score: 1_850,
        dps: 24_000.0,
        hit_rate: 1.6,
        crit_rate: 0.32,
        back_rate: 0.45,
        front_rate: 0.2,
        is_self: true,
        dies_at_ms: None,
        skills: GLADIATOR,
    },
    PlayerDef {
        name: "Nyxara",
        class_id: 15,
        level: 60,
        combat_power: 45_900,
        gear_score: 1_790,
        dps: 18_000.0,
        hit_rate: 1.2,
        crit_rate: 0.28,
        back_rate: 0.1,
        front_rate: 0.6,
        is_self: false,
        dies_at_ms: None,
        skills: SORCERER,
    },
    PlayerDef {
        name: "Kaelith",
        class_id: 13,
        level: 59,
        combat_power: 44_100,
        gear_score: 1_720,
        dps: 16_500.0,
        hit_rate: 2.2,
        crit_rate: 0.41,
        back_rate: 0.7,
        front_rate: 0.1,
        is_self: false,
        dies_at_ms: Some(48_000),
        skills: ASSASSIN,
    },
    PlayerDef {
        name: "Thorne",
        class_id: 12,
        level: 60,
        combat_power: 47_300,
        gear_score: 1_880,
        dps: 10_000.0,
        hit_rate: 1.3,
        crit_rate: 0.22,
        back_rate: 0.05,
        front_rate: 0.85,
        is_self: false,
        dies_at_ms: None,
        skills: TEMPLAR,
    },
    PlayerDef {
        name: "Mirelle",
        class_id: 17,
        level: 58,
        combat_power: 42_800,
        gear_score: 1_690,
        dps: 6_500.0,
        hit_rate: 1.0,
        crit_rate: 0.18,
        back_rate: 0.1,
        front_rate: 0.5,
        is_self: false,
        dies_at_ms: None,
        skills: CLERIC,
    },
];

struct BossDef {
    name: &'static str,
    npc_code: u32,
    map_id: u32,
    map_name: &'static str,
}

const BOSSES: &[BossDef] = &[
    BossDef { name: "Kromede the Corrupt", npc_code: 812_001, map_id: 300_010, map_name: "Kromede's Trial" },
    BossDef { name: "Lord Lannok", npc_code: 812_002, map_id: 300_020, map_name: "Fire Temple" },
    BossDef { name: "Bakarma", npc_code: 812_003, map_id: 300_030, map_name: "Draupnir Cave" },
];

const BOSS_ENTITY_ID: u32 = 90_001;
const DEATH_DOWNTIME_MS: u64 = 10_000;

#[derive(Default)]
struct SkillState {
    id: u32,
    damage: u64,
    casts: u64,
    hits: u64,
    crits: u64,
    max_hit: u64,
    min_hit: u64,
    back: u64,
    front: u64,
    parry: u64,
    perfect: u64,
    double: u64,
    multi: u64,
    dot: bool,
    healing: u64,
}

#[derive(Default)]
struct PlayerState {
    index: usize,
    id: u32,
    damage: u64,
    casts: u64,
    hits: u64,
    crits: u64,
    back: u64,
    front: u64,
    parry: u64,
    perfect: u64,
    double: u64,
    multi: u64,
    max_hit: u64,
    healing: u64,
    damage_taken: u64,
    deaths: u32,
    dead_until_ms: Option<u64>,
    active_ms: u64,
    last_hit_ms: Option<u64>,
    /// Fractional hits owed to the actor by the hit rate.
    budget: f64,
    skills: Vec<SkillState>,
    /// Damage per second since the fight started.
    timeline: Vec<u64>,
}

struct Fight {
    id: u64,
    boss: &'static BossDef,
    max_hp: u64,
    started_ms: u64,
    last_event_ms: u64,
    last_step_ms: u64,
    players: Vec<PlayerState>,
    deaths: Vec<DeathInfo>,
    end_reason: Option<EndReason>,
}

enum Phase {
    Fighting,
    Paused { until_ms: u64 },
}

/// xorshift64*, seeded per fight so a demo run always looks the same.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }
}

pub struct Demo {
    rng: Rng,
    fight_count: u64,
    next_id: u64,
    phase: Phase,
    fight: Fight,
    /// Newest first.
    finished: Vec<Snapshot>,
}

impl Demo {
    pub fn new(now_ms: u64) -> Demo {
        let mut demo = Demo {
            rng: Rng::new(1),
            fight_count: 0,
            next_id: 1,
            phase: Phase::Fighting,
            fight: Fight::new(1, &BOSSES[0], now_ms),
            finished: Vec::new(),
        };
        demo.start_fight(now_ms);
        demo
    }

    pub fn self_name(&self) -> &'static str {
        PLAYERS.iter().find(|p| p.is_self).map(|p| p.name).unwrap_or("Seraphel")
    }

    /// Advances the simulation to `now_ms`.
    pub fn step(&mut self, now_ms: u64) {
        match self.phase {
            Phase::Paused { until_ms } => {
                if now_ms >= until_ms {
                    self.start_fight(now_ms);
                }
            }
            Phase::Fighting => {
                if now_ms <= self.fight.last_step_ms {
                    return;
                }
                let from = self.fight.last_step_ms.max(now_ms.saturating_sub(MAX_STEP_MS));
                self.simulate(from, now_ms);
                self.fight.last_step_ms = now_ms;
                if self.fight.damage_total() >= self.fight.max_hp {
                    self.finish(EndReason::BossDied, now_ms);
                }
            }
        }
    }

    /// Ends the running fight by hand, like the reset hotkey does for a real one.
    pub fn reset(&mut self, now_ms: u64) {
        if matches!(self.phase, Phase::Fighting) {
            self.finish(EndReason::Manual, now_ms);
        }
    }

    /// The live fight, or the last finished one during the pause.
    pub fn current(&self) -> Option<Snapshot> {
        match self.phase {
            Phase::Fighting => Some(self.fight.snapshot(true)),
            Phase::Paused { .. } => self.finished.first().cloned(),
        }
    }

    pub fn summaries(&self) -> Vec<EncounterSummary> {
        self.finished.iter().map(Snapshot::summary).collect()
    }

    pub fn get(&self, id: u64) -> Option<Snapshot> {
        if matches!(self.phase, Phase::Fighting) && self.fight.id == id {
            return Some(self.fight.snapshot(true));
        }
        self.finished.iter().find(|s| s.id == id).cloned()
    }

    pub fn delete(&mut self, id: u64) -> bool {
        let before = self.finished.len();
        self.finished.retain(|s| s.id != id);
        self.finished.len() != before
    }

    fn start_fight(&mut self, now_ms: u64) {
        let boss = &BOSSES[(self.fight_count as usize) % BOSSES.len()];
        self.fight_count += 1;
        self.rng = Rng::new(self.fight_count);
        let id = self.next_id;
        self.next_id += 1;
        self.fight = Fight::new(id, boss, now_ms);
        self.phase = Phase::Fighting;
    }

    fn finish(&mut self, reason: EndReason, now_ms: u64) {
        self.fight.end_reason = Some(reason);
        let snapshot = self.fight.snapshot(false);
        self.finished.insert(0, snapshot);
        self.finished.truncate(MAX_FINISHED);
        self.phase = Phase::Paused { until_ms: now_ms + PAUSE_MS };
    }

    fn simulate(&mut self, from_ms: u64, to_ms: u64) {
        let dt_ms = to_ms - from_ms;
        let fight_t = from_ms - self.fight.started_ms;
        let player_count = self.fight.players.len();
        for (index, def) in PLAYERS.iter().enumerate().take(player_count) {
            if let Some(dies_at) = def.dies_at_ms {
                let state = &mut self.fight.players[index];
                if state.deaths == 0 && fight_t >= dies_at {
                    state.deaths = 1;
                    state.dead_until_ms = Some(from_ms + DEATH_DOWNTIME_MS);
                    state.damage_taken += 180_000;
                    self.fight.deaths.push(DeathInfo { t_ms: from_ms, id: state.id, name: def.name.into(), is_player: true });
                }
            }
            let dead = self.fight.players[index].dead_until_ms.map(|until| from_ms < until).unwrap_or(false);
            if dead {
                continue;
            }
            self.fight.players[index].budget += def.hit_rate * dt_ms as f64 / 1000.0;
            while self.fight.players[index].budget >= 1.0 {
                self.fight.players[index].budget -= 1.0;
                let t_hit = from_ms + (self.rng.unit() * dt_ms as f64) as u64;
                self.hit(index, t_hit);
            }
        }
    }

    fn hit(&mut self, index: usize, t_ms: u64) {
        let def = &PLAYERS[index];
        let total_weight: f64 = def.skills.iter().map(|s| s.weight).sum();
        let mut pick = self.rng.unit() * total_weight;
        let mut chosen = &def.skills[0];
        for s in def.skills {
            if pick < s.weight {
                chosen = s;
                break;
            }
            pick -= s.weight;
        }
        let average_hit = def.dps / def.hit_rate / damage_scale(def);
        let variance = 0.75 + self.rng.unit() * 0.5;
        let crit = self.rng.chance(def.crit_rate);
        let back = self.rng.chance(def.back_rate);
        let front = !back && self.rng.chance(def.front_rate);
        let parry = self.rng.chance(0.03);
        let perfect = self.rng.chance(0.05);
        let double = self.rng.chance(0.08);
        let multi = self.rng.chance(0.06);
        let counts_as_cast = !chosen.dot || self.rng.chance(0.3);
        let mut amount = (average_hit * chosen.power * variance) as u64;
        if crit {
            amount = (amount as f64 * CRIT_MULTIPLIER) as u64;
        }
        let amount = amount.max(1);

        let fight = &mut self.fight;
        let state = &mut fight.players[index];
        let skill_state = match state.skills.iter().position(|s| s.id == chosen.id) {
            Some(i) => &mut state.skills[i],
            None => {
                state.skills.push(SkillState { id: chosen.id, dot: chosen.dot, ..Default::default() });
                state.skills.last_mut().expect("just pushed")
            }
        };
        skill_state.casts += u64::from(counts_as_cast);
        if chosen.heal {
            let healing = amount * 2;
            skill_state.hits += 1;
            skill_state.healing += healing;
            state.healing += healing;
            state.casts += u64::from(counts_as_cast);
            return;
        }
        skill_state.hits += 1;
        skill_state.damage += amount;
        skill_state.crits += u64::from(crit);
        skill_state.back += u64::from(back);
        skill_state.front += u64::from(front);
        skill_state.parry += u64::from(parry);
        skill_state.perfect += u64::from(perfect);
        skill_state.double += u64::from(double);
        skill_state.multi += u64::from(multi);
        skill_state.max_hit = skill_state.max_hit.max(amount);
        skill_state.min_hit = if skill_state.min_hit == 0 { amount } else { skill_state.min_hit.min(amount) };

        state.damage += amount;
        state.hits += 1;
        state.casts += u64::from(counts_as_cast);
        state.crits += u64::from(crit);
        state.back += u64::from(back);
        state.front += u64::from(front);
        state.parry += u64::from(parry);
        state.perfect += u64::from(perfect);
        state.double += u64::from(double);
        state.multi += u64::from(multi);
        state.max_hit = state.max_hit.max(amount);
        if let Some(last) = state.last_hit_ms {
            let gap = t_ms.saturating_sub(last);
            if gap <= ACTIVE_GAP_MS {
                state.active_ms += gap;
            }
        }
        state.last_hit_ms = Some(t_ms);
        let bucket = ((t_ms.saturating_sub(fight.started_ms)) / 1000) as usize;
        if state.timeline.len() <= bucket {
            state.timeline.resize(bucket + 1, 0);
        }
        state.timeline[bucket] += amount;
        // The tank takes most of the boss's attention.
        state.damage_taken += if def.class_id == 12 { amount / 2 } else { amount / 12 };
        fight.last_event_ms = fight.last_event_ms.max(t_ms);
    }
}

impl Fight {
    fn new(id: u64, boss: &'static BossDef, now_ms: u64) -> Fight {
        let party_dps: f64 = PLAYERS.iter().map(|p| p.dps).sum();
        let max_hp = (party_dps * FIGHT_TARGET_MS as f64 / 1000.0) as u64;
        let players = PLAYERS
            .iter()
            .enumerate()
            .map(|(index, _)| PlayerState { index, id: 1_001 + index as u32, ..Default::default() })
            .collect();
        Fight { id, boss, max_hp, started_ms: now_ms, last_event_ms: now_ms, last_step_ms: now_ms, players, deaths: Vec::new(), end_reason: None }
    }

    fn damage_total(&self) -> u64 {
        self.players.iter().map(|p| p.damage).sum()
    }

    fn snapshot(&self, active: bool) -> Snapshot {
        let data = GameData::get();
        let total_damage = self.damage_total().min(self.max_hp);
        let duration_ms = self.last_event_ms.saturating_sub(self.started_ms);
        let duration_s = (duration_ms as f64 / 1000.0).max(1.0);
        let cur_hp = self.max_hp.saturating_sub(total_damage);
        let dead = self.end_reason == Some(EndReason::BossDied);
        let boss = TargetInfo {
            id: BOSS_ENTITY_ID,
            npc_code: Some(self.boss.npc_code),
            name: self.boss.name.into(),
            is_boss: true,
            is_dummy: false,
            cur_hp: Some(if dead { 0 } else { cur_hp }),
            max_hp: Some(self.max_hp),
            hp_pct: Some(if dead { 0.0 } else { cur_hp as f64 / self.max_hp as f64 }),
            damage_taken: total_damage,
            dead,
        };
        let party_total: u64 = self.players.iter().map(|p| p.damage).sum();
        let mut actors: Vec<ActorStats> = self
            .players
            .iter()
            .map(|p| {
                let def = &PLAYERS[p.index];
                let mut skills: Vec<SkillStats> = p
                    .skills
                    .iter()
                    .map(|s| SkillStats {
                        skill_id: s.id,
                        name: data.skill_name(s.id),
                        icon: data.skill_icon(s.id),
                        damage: s.damage,
                        share: ratio(s.damage, p.damage),
                        casts: s.casts,
                        hits: s.hits,
                        crits: s.crits,
                        crit_rate: ratio(s.crits, s.hits),
                        max_hit: s.max_hit,
                        min_hit: s.min_hit,
                        avg_hit: if s.hits > 0 { s.damage as f64 / s.hits as f64 } else { 0.0 },
                        back: s.back,
                        front: s.front,
                        parry: s.parry,
                        perfect: s.perfect,
                        double: s.double,
                        multi: s.multi,
                        dot: s.dot,
                        healing: s.healing,
                    })
                    .collect();
                skills.sort_by(|a, b| b.damage.cmp(&a.damage).then(b.healing.cmp(&a.healing)));
                let active_s = (p.active_ms as f64 / 1000.0).max(1.0);
                ActorStats {
                    id: p.id,
                    name: def.name.into(),
                    class_id: Some(def.class_id),
                    class_name: data.class_name(def.class_id).map(str::to_owned),
                    level: Some(def.level),
                    is_self: def.is_self,
                    is_party: true,
                    is_player: true,
                    combat_power: Some(def.combat_power),
                    gear_score: Some(def.gear_score),
                    damage: p.damage,
                    damage_all_targets: p.damage,
                    dps: p.damage as f64 / duration_s,
                    active_ms: p.active_ms,
                    active_dps: p.damage as f64 / active_s,
                    share: ratio(p.damage, party_total),
                    casts: p.casts,
                    hits: p.hits,
                    crits: p.crits,
                    crit_rate: ratio(p.crits, p.hits),
                    back_rate: ratio(p.back, p.hits),
                    front_rate: ratio(p.front, p.hits),
                    parry_rate: ratio(p.parry, p.hits),
                    perfect_rate: ratio(p.perfect, p.hits),
                    double_rate: ratio(p.double, p.hits),
                    multi_rate: ratio(p.multi, p.hits),
                    max_hit: p.max_hit,
                    healing: p.healing,
                    hps: p.healing as f64 / duration_s,
                    damage_taken: p.damage_taken,
                    deaths: p.deaths,
                    skills,
                    targets: if p.damage > 0 {
                        vec![TargetDamage { id: BOSS_ENTITY_ID, name: self.boss.name.into(), damage: p.damage, share: 1.0 }]
                    } else {
                        Vec::new()
                    },
                    timeline: p.timeline.iter().enumerate().map(|(i, d)| TimelinePoint { t_s: i as u32, damage: *d }).collect(),
                }
            })
            .collect();
        actors.sort_by_key(|a| std::cmp::Reverse(a.damage));
        Snapshot {
            id: self.id,
            active,
            end_reason: self.end_reason,
            started_ms: self.started_ms,
            last_event_ms: self.last_event_ms,
            duration_ms,
            map_id: Some(self.boss.map_id),
            map_name: Some(self.boss.map_name.into()),
            is_instance: true,
            boss: Some(boss.clone()),
            targets: vec![boss],
            total_damage: party_total,
            party_dps: party_total as f64 / duration_s,
            actors,
            deaths: self.deaths.clone(),
            self_id: self.players.iter().find(|p| PLAYERS[p.index].is_self).map(|p| p.id),
        }
    }
}

/// Expected damage of one hit relative to `dps / hit_rate`: the weighted skill power (heals deal none)
/// times the average crit bonus. Dividing by it makes the actor land on its target DPS.
fn damage_scale(def: &PlayerDef) -> f64 {
    let total_weight: f64 = def.skills.iter().map(|s| s.weight).sum();
    let mean_power: f64 = def.skills.iter().filter(|s| !s.heal).map(|s| s.weight * s.power).sum::<f64>() / total_weight;
    let crit_bonus = 1.0 + def.crit_rate * (CRIT_MULTIPLIER - 1.0);
    (mean_power * crit_bonus).max(0.01)
}

fn ratio(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 / whole as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(demo: &mut Demo, from_ms: u64, to_ms: u64) {
        let mut t = from_ms;
        while t < to_ms {
            t += 250;
            demo.step(t);
        }
    }

    #[test]
    fn a_fight_runs_for_about_two_minutes_and_ends_with_the_boss_dead() {
        let start = 1_700_000_000_000;
        let mut demo = Demo::new(start);
        let live = demo.current().unwrap();
        assert!(live.active);
        assert_eq!(live.id, 1);
        assert_eq!(live.actors.len(), 5);
        run(&mut demo, start, start + 30_000);
        let mid = demo.current().unwrap();
        assert!(mid.active);
        let boss = mid.boss.as_ref().unwrap();
        assert!(boss.is_boss);
        assert!(boss.hp_pct.unwrap() < 1.0 && boss.hp_pct.unwrap() > 0.5);
        assert!(mid.total_damage > 0);
        assert!(mid.party_dps > 50_000.0 && mid.party_dps < 100_000.0, "party dps {}", mid.party_dps);
        let shares: f64 = mid.actors.iter().map(|a| a.share).sum();
        assert!((shares - 1.0).abs() < 1e-6);
        assert!(mid.actors.windows(2).all(|w| w[0].damage >= w[1].damage));
        for actor in &mid.actors {
            assert!(actor.is_player && actor.is_party);
            assert!(actor.class_name.is_some());
            assert!(!actor.skills.is_empty());
            assert!(actor.skills.iter().all(|s| s.icon.is_some() && !s.name.starts_with('#')), "{:?}", actor.skills);
            assert!(!actor.timeline.is_empty());
        }
        assert_eq!(mid.actors.iter().filter(|a| a.is_self).count(), 1);
        assert_eq!(demo.self_name(), "Seraphel");

        run(&mut demo, start + 30_000, start + 180_000);
        assert_eq!(demo.summaries().len(), 1, "the first fight finished");
        let done = demo.get(1).unwrap();
        assert!(!done.active);
        assert_eq!(done.end_reason, Some(EndReason::BossDied));
        assert!(done.boss.as_ref().unwrap().dead);
        assert_eq!(done.boss.as_ref().unwrap().cur_hp, Some(0));
        assert!(done.duration_ms > 90_000 && done.duration_ms < 150_000, "duration {}", done.duration_ms);
        assert_eq!(done.deaths.len(), 1);
        assert_eq!(done.deaths[0].name, "Kaelith");
        assert_eq!(done.actors.iter().find(|a| a.name == "Kaelith").unwrap().deaths, 1);
        assert!(done.actors.iter().find(|a| a.name == "Mirelle").unwrap().healing > 0);
        for actor in &done.actors {
            assert!(actor.skills.len() >= 6 && actor.skills.len() <= 10, "{} has {} skills", actor.name, actor.skills.len());
        }
        let next = demo.current().unwrap();
        assert!(next.active && next.id == 2, "the second fight is running");
        assert_eq!(next.boss.as_ref().unwrap().name, "Lord Lannok");
    }

    #[test]
    fn reset_ends_the_fight_by_hand_and_delete_forgets_it() {
        let start = 1_000;
        let mut demo = Demo::new(start);
        run(&mut demo, start, start + 10_000);
        demo.reset(start + 10_000);
        let current = demo.current().unwrap();
        assert!(!current.active);
        assert_eq!(current.end_reason, Some(EndReason::Manual));
        assert!(!current.boss.as_ref().unwrap().dead);
        assert_eq!(demo.summaries().len(), 1);
        assert!(demo.delete(1));
        assert!(!demo.delete(1));
        assert!(demo.summaries().is_empty());
        assert!(demo.current().is_none());
        run(&mut demo, start + 10_000, start + 20_000);
        assert_eq!(demo.current().unwrap().id, 2);
    }

    #[test]
    fn large_gaps_are_capped() {
        let start = 1_000;
        let mut demo = Demo::new(start);
        demo.step(start + 60_000);
        let snap = demo.current().unwrap();
        assert!(snap.total_damage < 400_000, "a single step simulates at most two seconds");
    }
}
