//! Offline decoder: runs one or more pcap or pcapng captures, in order, through the pipeline and the kina
//! tracker, the way the app would have seen them live.
//!
//! ```text
//! replay <capture>... [--port 13328] [--opcodes] [--dump 1b56] [--find 182677,183200] [--limit N] [--events [type]] [--json]
//! ```
//! Without a mode flag it prints every session: balances, totals, rates, the runs and the kina log.
//! `--find` lists every packet that holds one of the numbers (as u32 or u64 little endian, u32 big endian or a
//! varint), with where it sits, for finding a value the game shows in the traffic.
//! Event types for `--events`: kina, currency, daily, abyss, zone, self, heartbeat.

use anyhow::{anyhow, Context};
use kina_core::capture_file::read_packets;
use kina_core::gamedata::GameData;
use kina_core::net::LinkType;
use kina_core::parser::Event;
use kina_core::pipeline::{Pipeline, DEFAULT_PORT};
use kina_core::snapshot::{EntryKind, SessionSnapshot};
use kina_core::tracker::Tracker;
use kina_core::opcodes;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

const EVENT_TYPES: &[&str] = &["kina", "currency", "daily", "abyss", "zone", "self", "heartbeat"];

struct Options {
    paths: Vec<String>,
    ports: Vec<u16>,
    show_opcodes: bool,
    dump: Option<u16>,
    find: Vec<u64>,
    limit: usize,
    events: bool,
    event_type: Option<String>,
    json: bool,
}

fn usage() -> String {
    "usage: replay <capture.pcap|pcapng>... [--port 13328] [--opcodes] [--dump 1b56] [--find 182677,183200] [--limit N] [--events [kina|currency|daily|abyss|zone|self|heartbeat]] [--json]".into()
}

fn parse_args() -> anyhow::Result<Options> {
    let mut o = Options { paths: Vec::new(), ports: vec![DEFAULT_PORT], show_opcodes: false, dump: None, find: Vec::new(), limit: 50, events: false, event_type: None, json: false };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "--port" => {
                i += 1;
                o.ports = vec![args.get(i).ok_or_else(|| anyhow!(usage()))?.parse()?];
            }
            "--opcodes" => o.show_opcodes = true,
            "--dump" => {
                i += 1;
                let s = args.get(i).ok_or_else(|| anyhow!(usage()))?;
                o.dump = Some(opcodes::parse(s).ok_or_else(|| anyhow!("bad opcode {s}"))?);
            }
            "--find" => {
                i += 1;
                let s = args.get(i).ok_or_else(|| anyhow!(usage()))?;
                for v in s.split([',', ' ']).filter(|v| !v.is_empty()) {
                    o.find.push(v.replace(['.', '_'], "").parse().map_err(|_| anyhow!("--find takes numbers like 182677,183200, not {v:?}"))?);
                }
            }
            "--limit" => {
                i += 1;
                o.limit = args.get(i).ok_or_else(|| anyhow!(usage()))?.parse()?;
            }
            "--events" => {
                o.events = true;
                if let Some(next) = args.get(i + 1) {
                    if EVENT_TYPES.contains(&next.as_str()) {
                        o.event_type = Some(next.clone());
                        i += 1;
                    }
                }
            }
            "--json" => o.json = true,
            "-h" | "--help" => return Err(anyhow!(usage())),
            _ if a.starts_with("--") => return Err(anyhow!("{}\nunknown argument {a}", usage())),
            _ => o.paths.push(a.to_string()),
        }
        i += 1;
    }
    if o.paths.is_empty() {
        return Err(anyhow!(usage()));
    }
    Ok(o)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

fn varint(mut v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return out;
        }
        out.push(byte | 0x80);
    }
}

/// An encoding's name and the bytes it gives.
type Patterns = Vec<(&'static str, Vec<u8>)>;

/// The byte patterns `value` takes on the wire: the longest first, so a u64 hit is not also listed as its u32.
fn patterns(value: u64) -> Patterns {
    let mut p = vec![("u64 LE", value.to_le_bytes().to_vec())];
    if let Ok(v) = u32::try_from(value) {
        p.push(("u32 LE", v.to_le_bytes().to_vec()));
        p.push(("u32 BE", v.to_be_bytes().to_vec()));
    }
    p.push(("varint", varint(value)));
    p
}

/// 8 308 193 as "8,308,193".
fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn signed(n: i64) -> String {
    if n < 0 {
        format!("-{}", group(n.unsigned_abs()))
    } else {
        format!("+{}", group(n as u64))
    }
}

fn clock(t_ms: u64, origin_ms: u64) -> String {
    let s = t_ms.saturating_sub(origin_ms) / 1000;
    format!("{:>2}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

fn print_session(s: &SessionSnapshot, origin_ms: u64) {
    let who = s.character.as_ref().map(|c| format!("{} (server {}, level {})", c.name, c.server, c.level)).unwrap_or_else(|| "unidentified character".into());
    let state = match s.end_reason {
        Some(r) => format!("ended: {r:?}"),
        None => "live".into(),
    };
    println!("\nsession {} | {who} | {state}", s.id);
    println!(
        "  {} to {}, connected {} min",
        clock(s.started_ms, origin_ms),
        clock(s.last_ms, origin_ms),
        s.duration_ms / 60_000
    );
    let balance = |b: Option<u64>| b.map(group).unwrap_or_else(|| "unknown".into());
    println!("  balance {} -> {}", balance(s.start_balance), balance(s.balance));
    println!(
        "  earned {}  spent {}  net {}  ({}/h net, {}/h earned)  changes {}  resyncs {}",
        group(s.earned),
        group(s.spent),
        signed(s.net),
        signed(s.net_per_hour.round() as i64),
        group(s.earned_per_hour.round() as u64),
        s.changes,
        s.resyncs
    );
    println!("  runs with kina:");
    for r in s.runs.iter().filter(|r| r.changes > 0) {
        let name = r.map.as_ref().map(|m| GameData::get().map_label(m.id)).unwrap_or_else(|| "before the first loading screen".into());
        println!("    #{:<3} {} {:>4} min  {:>12}  {name}", r.index, clock(r.started_ms, origin_ms), r.duration_ms / 60_000, signed(r.net));
    }
    println!("  log:");
    for e in &s.entries {
        let kind = match e.kind {
            EntryKind::Gain => "gain",
            EntryKind::Spend => "spend",
            EntryKind::Resync => "RESYNC",
        };
        let times = if e.count > 1 { format!(" x{}", e.count) } else { String::new() };
        println!(
            "    {} {:<6} {:>12}{times:<5} -> {:>12}  {}",
            clock(e.t_ms, origin_ms),
            kind,
            signed(e.delta),
            group(e.balance),
            e.map_name.as_deref().unwrap_or("")
        );
    }
}

fn main() -> anyhow::Result<()> {
    let opts = parse_args()?;
    let mut pipe = Pipeline::new(opts.ports.clone());
    let dumped = Arc::new(Mutex::new(0usize));
    if let Some(want) = opts.dump {
        let limit = opts.limit;
        let dumped = dumped.clone();
        pipe.tap = Some(Box::new(move |t_ms, op, body| {
            if op == want {
                let mut n = dumped.lock().unwrap();
                if *n < limit {
                    *n += 1;
                    println!("{t_ms} {} len {}: {}", opcodes::hex(op), body.len(), hex(body));
                }
            }
        }));
    }

    if !opts.find.is_empty() {
        let wanted: Vec<(u64, Patterns)> = opts.find.iter().map(|v| (*v, patterns(*v))).collect();
        let limit = opts.limit;
        let dumped = dumped.clone();
        pipe.tap = Some(Box::new(move |t_ms, op, body| {
            for (value, pats) in &wanted {
                let mut taken: Vec<std::ops::Range<usize>> = Vec::new();
                for (enc, pat) in pats {
                    for at in (0..body.len().saturating_sub(pat.len() - 1)).filter(|&i| body[i..].starts_with(pat)) {
                        if taken.iter().any(|r| r.contains(&at)) {
                            continue;
                        }
                        taken.push(at..at + pat.len());
                        let mut n = dumped.lock().unwrap();
                        if *n >= limit {
                            return;
                        }
                        *n += 1;
                        let (from, to) = (at.saturating_sub(24), (at + pat.len() + 24).min(body.len()));
                        println!(
                            "{t_ms} {} len {:>5} @{at:<5} {enc:<6} {}: {} [{}] {}",
                            opcodes::hex(op),
                            body.len(),
                            group(*value),
                            hex(&body[from..at]),
                            hex(&body[at..at + pat.len()]),
                            hex(&body[at + pat.len()..to])
                        );
                    }
                }
            }
        }));
    }

    let mut tracker = Tracker::default();
    let mut finished: Vec<SessionSnapshot> = Vec::new();
    let mut printed_events = 0usize;
    let (mut first_ms, mut last_ms) = (u64::MAX, 0u64);
    let mut last_tick = 0u64;
    let mut unknown_links: BTreeMap<u32, u64> = BTreeMap::new();
    let mut pending: Vec<Event> = Vec::new();

    for path in &opts.paths {
        let bytes = std::fs::read(path).with_context(|| format!("reading {path}"))?;
        read_packets(&bytes, |link_code, t_ms, data| {
            first_ms = first_ms.min(t_ms);
            last_ms = last_ms.max(t_ms);
            let Some(link) = LinkType::from_pcap(link_code) else {
                *unknown_links.entry(link_code).or_default() += 1;
                return;
            };
            pipe.push(link, t_ms, data, &mut |ev| pending.push(ev));
            // A tick every 250 ms of capture time, as the terminal app does.
            if t_ms.saturating_sub(last_tick) >= 250 {
                tracker.tick(t_ms);
                last_tick = t_ms;
                finished.extend(tracker.drain_finished());
            }
            for ev in pending.drain(..) {
                if opts.events && printed_events < opts.limit {
                    let wanted = match &opts.event_type {
                        Some(kind) => ev.kind() == kind,
                        None => !matches!(ev, Event::Heartbeat { .. }),
                    };
                    if wanted {
                        printed_events += 1;
                        println!("{}", serde_json::to_string(&ev).unwrap_or_default());
                    }
                }
                tracker.push(&ev);
            }
        })?;
    }
    finished.extend(tracker.drain_finished());

    if first_ms == u64::MAX {
        return Err(anyhow!("no packets in {}", opts.paths.join(", ")));
    }
    let ps = &pipe.parser.stats;
    let st = &pipe.stats;
    eprintln!("capture: {} packets over {:.1}s from {} file(s)", st.packets_seen, (last_ms - first_ms) as f64 / 1000.0, opts.paths.len());
    eprintln!(
        "frames: {} ({} bundles, {} bad), resyncs {}, tcp gaps {}; packets {} ({} heartbeats)",
        st.frames.frames, st.frames.bundles, st.frames.bad_bundles, st.frames.resyncs, st.tcp_gaps, ps.packets, ps.heartbeats
    );
    eprintln!(
        "inventories {} ({} without kina), item updates {}, kina records {}, other currency records {}, self records {}, zone changes {}",
        ps.inventories, ps.inventories_without_kina, ps.item_updates, ps.kina_records, ps.currency_records, ps.self_records, ps.zone_changes
    );
    if !unknown_links.is_empty() {
        eprintln!("unsupported link types: {unknown_links:?}");
    }

    if opts.show_opcodes {
        let mut counts: Vec<(u16, u64)> = ps.opcode_counts.iter().map(|(k, v)| (*k, *v)).collect();
        counts.sort_by_key(|c| std::cmp::Reverse(c.1));
        println!("\nopcode   name          count      bytes");
        for (op, n) in counts.iter().take(opts.limit.max(20)) {
            println!("{}    {:<12} {:>7} {:>10}", opcodes::hex(*op), opcodes::name(*op), n, ps.opcode_bytes.get(op).copied().unwrap_or(0));
        }
        return Ok(());
    }
    if opts.dump.is_some() || opts.events || !opts.find.is_empty() {
        return Ok(());
    }

    let mut sessions = finished;
    if let Some(live) = tracker.snapshot().filter(|s| s.active) {
        sessions.push(live);
    }
    if opts.json {
        println!("{}", serde_json::to_string_pretty(&sessions)?);
        return Ok(());
    }
    if sessions.is_empty() {
        println!("no kina seen: the capture holds no inventory packet ({} and {})", opcodes::hex(opcodes::INVENTORY), opcodes::hex(opcodes::ITEM_UPDATE));
    }
    for s in &sessions {
        print_session(s, first_ms);
    }
    if let Some(d) = tracker.daily_kina() {
        println!("\ndaily gain limit: {} of {} counted (as of server time {})", group(d.counted), group(kina_core::tracker::DAILY_KINA_LIMIT), d.server_ms);
    }
    if let Some(a) = tracker.abyss_points() {
        println!("abyss points: {} of {}", group(a.points), group(a.cap));
    }
    if !tracker.currencies().is_empty() {
        println!("\nother currencies (item key: latest count):");
        for (key, seen) in tracker.currencies() {
            println!("  0x{key:08X}: {}", group(seen.count));
        }
    }
    Ok(())
}
