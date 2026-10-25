//! kina-core: pure Rust decoding of Aion 2 game traffic into kina balance changes, plus the session tracker.
//!
//! Pipeline: link layer (`net`) to TCP byte stream (`tcp`) to game frames (`frame`) to packets (`parser`)
//! to sessions (`tracker`). Nothing here touches the network or the game; the terminal app feeds captured
//! bytes in and reads snapshots out, and `bin/replay.rs` does the same from a pcap file.
//!
//! The transport layers (`net`, `tcp`, `frame`, `wire`, `capture_file`) live in `aion-net`, shared with the meter
//! and the cube radar.

pub mod gamedata;
pub mod opcodes;
pub mod parser;
pub mod pipeline;
pub mod snapshot;
pub mod tracker;

/// The shared transport layers, under the paths this crate always used.
pub use aion_net::{capture_file, frame, net, tcp, wire};
