# Aion 2 wire protocol, consolidated specification for a Rust DPS meter (Global client, October 2026)

This document merges five independently written protocol specifications of the Aion 2 server to client stream, as decoded by community DPS meters, into one reference for a new Rust implementation targeting the Global (NA/EU) client as of October 2026.

Inputs, in the order of weight given to them when they disagree:

| Tag | Repository | Last pushed | License | Target client | Weight |
|---|---|---|---|---|---|
| A2Tools | `A2Tools-DPS-Meter` (Rust, Tauri) | 11 October 2026 | GPL 3.0 | Global, captures dated 1 to 10 October 2026 | Highest. Most fields decoded, most test vectors, most recent captures. |
| chen-zilei | `chen-zilei/aion2-meter` (Rust) | 11 October 2026 | none (all rights reserved; learn from, do not copy) | Global, post June 2026 client | High. Strict, conservative parser; second hand findings. |
| RATmeter | `Kuroukihime/AIon2-Dps-Meter` (C#) | 8 October 2026 | GPL 3.0 | Global, captures from September 2026 | Medium. Several heuristic scans, known bugs. |
| NOIA2 | `NOIA2` (Rust, TypeScript) | 8 October 2026 | GPL 3.0 | Taiwan client, legacy 4.0 branch | Medium to low. Server ranges hard coded to TW. |
| TK | `TK-open-public/Aion2-Dps-Meter` (Kotlin) | 4 July 2026 | MIT | Korean client, project ended 19 June 2026 | Lowest. Possibly pre June 2026 client. |

Conventions used throughout:

- Bytes are written in wire order, first byte first, two hex digits each: `04 38` means the byte `0x04` followed by the byte `0x38`. Every repo's own notation has been normalised to this (see section 4.1 for how each repo writes opcodes).
- All multi byte integers are little endian (LE) unless stated otherwise. "varint" means the unsigned LEB128 style integer in section 3.1.
- "Consensus" names the layout that the weighted majority of the repos agree on, with the sources it comes from. Where only one repo decodes a field it is marked as single source; that is not a disagreement.
- Disagreements are numbered D1, D2, ... across the whole document so that they can be referenced from the summary and from code comments.
- "Not covered" means no repo decodes the item; such items are listed under Unknowns.

---

## 1. Transport and capture

### 1.1 Consensus

Protocol and ports:

- The game speaks plain TCP. The server to client direction is plaintext apart from LZ4 compressed bundles; the client to server direction uses the same length prefix but its opcode and body are encrypted (A2Tools `ping_tracker.rs:292-294`; chen-zilei `docs/PROTOCOL.md:12-14`).
- Game server port on Global: 13328. Observed in A2Tools captures of 1 October 2026 as server id 1306 (Ariel) at 193.202.112.99:13328 and server id 1502 (Nezekan) at 193.202.112.200:13328; chen-zilei's `DEFAULT_GAME_PORT = 13328`; TK's Korean default is also 13328 on net 206.127.156.0/24. A2Tools, RATmeter and NOIA2 do not hard code the port and discover it at run time; the login/region server is on port 13700 (A2Tools `packet_dump.rs:22`).
- Server connect record (A2Tools diagnostic only): on entering a server the server sends `0F 39 00 00 <server id u16 LE> <n u8> <IPv4 as ASCII, n bytes 7..=15> <port u16 LE>`.

Direction decoded:

- Only server to client bytes are parsed (A2Tools, chen-zilei, RATmeter by `tcp src port`, TK by `src net`). Client to server payloads are used only for ping timing (A2Tools, chen-zilei).

Adapters and link layer:

- Open every adapter including the loopback adapter, because ping reducers and VPNs (NordLynx, WireGuard, wintun, TAP) relay the game through loopback (A2Tools, chen-zilei, RATmeter, NOIA2). A device is "virtual" when it has the `PCAP_IF_LOOPBACK` flag or its name or description contains `loopback`, `tap-windows`, `tap`, `wintun` or `wireguard` (A2Tools, NOIA2). RATmeter skips `Hyper-V`, `VMware`, `VirtualBox`, `TAP-Windows`, `WAN Miniport`.
- Loopback is preferred when both carry the game: A2Tools gives physical adapters a 2500 ms grace period, RATmeter 2000 ms, NOIA2 probes virtual devices first.
- Link types to decode: 1 Ethernet (skip up to two 802.1Q/802.1ad tags), 0 and 108 Null/Npcap loopback (4 byte family header `02 00 00 00`), 12, 14, 101, 228, 229 raw IP (A2Tools: 12, 14, 101, 228; chen-zilei: 12, 101, 228, 229), 113 Linux cooked v1, 276 Linux cooked v2 (union of A2Tools and chen-zilei). IPv4 TCP only in four repos; chen-zilei also accepts IPv6.
- The TCP payload ends at the IP total length, not the frame end: Ethernet padding with non zero bytes once made one player read 77% of a boss (A2Tools `pcap_capturer.rs:518-536`; NOIA2 does the same).
- Timestamps: pcap `ts_sec*1000 + ts_usec/1000`, sanity checked (A2Tools accepts 1e12..2e12 ms, NOIA2 requires > 1e9 s), else wall clock.
- Npcap on Windows: load `System32\Npcap\wpcap.dll` (add the folder to the DLL search path), WinPcap compatible mode is required by RATmeter and TK; Linux `libpcap.so.1` / `.0.8` / `.so` (A2Tools).

Game flow detection (signature based, three of five repos):

- The heartbeat frame `0E 00 36` (see section 3.7) is emitted by the server about 19 to 20 times per second even while idle. A flow is a candidate when its payloads contain that byte string; it is locked when enough hits accumulate: A2Tools 12 hits within a 3000 ms window per flow, RATmeter 5 hits per adapter and source port followed by a 2 s grace period, NOIA2 more than 1 hit within 3 s per directional key. chen-zilei instead creates a flow for any non FIN/RST server segment on a configured port and hunts for the heartbeat inside the stream; TK parses from the first segment with no detection.
- Pre lock, TLS looking payloads are skipped (first byte 0x14..0x17, second 0x03, third 0x00..0x04) and a connection whose first segment is a whole TLS record header is excluded for good, because the game server also talks TLS from the same port on a second connection (A2Tools `capture_dispatcher.rs:351-390`, NOIA2 `dispatcher.rs:505-510`).
- Watchdog: reset the lock when nothing parsed for 120 000 ms (A2Tools), 60 s with no packet from the locked port (RATmeter), 30 s candidate expiry (NOIA2).
- A2Tools additionally requires an `AION2` window to exist before parsing (`window_detector.rs`), re checked every 10 s while idle and 60 s while running.

Cloudflare relay (chen-zilei only): "The address the client connects to is a Cloudflare relay near the player, which acknowledges TCP segments itself, so TCP timing only measures the hop to the relay" (about 2 ms). Consequence: do not derive latency from TCP acknowledgements; use the game's own ping exchange.

Ping measurement:

- Pong frame (server to client): `03 36 00 00 <i64 LE client echo> <i64 LE server Unix ms>`; the full frame is `18 03 36 00 00 <8 bytes> <8 bytes>` (len 0x18 = 24, frame size 21). chen-zilei reads both fields; A2Tools, RATmeter and NOIA2 read only the client echo (A2Tools `ping_tracker.rs:135` reads the i64 at marker + 4 and then advances 12 bytes).
- Heartbeat body (chen-zilei): `00 36 <u64 LE server Unix ms>`, stepping by 50 ms, used as a downlink clock sample.
- Client ping request (client to server, encrypted): a frame of 12 or 13 bytes whose length prefix frames cleanly (A2Tools `PING_RQ_FRAME_BYTES`); the client heartbeat is `0E` + 10 bytes = 11 bytes and is excluded by size. Relay wrapped requests begin `05 23 0C 0F` or `05 23 0D 10`. chen-zilei notes the client sends a heartbeat every 50 ms and a ping every 10 s, and that the sizes change between logins, so it learns the ping frame size from the rarest size whose `t_sent - echo` repeats within 2 ms across two pongs.
- Interpretation of the client echo (three clocks are in use): (a) performance counter milliseconds since boot plus `16_777_216_000` (A2Tools clock (a), RATmeter `CalcCounterPing` with `UnrealOffsetMs = 16_777_216 * 1000`); (b) an arbitrary epoch that must be paired with the request time (A2Tools clock (b), chen-zilei's learned `clock_ms`); (c) .NET DateTime epoch milliseconds, offset `62_135_596_800_000` (A2Tools clock (c), RATmeter `CalcLegacyPing`, NOIA2 `DOTNET_EPOCH_OFFSET_MS`). KR/TW clients echo (c); the Global client as of October 2026 echoes (a) or (b).
- Valid RTT range: 1..=9999 ms (A2Tools), 0..=1000 (RATmeter), 10..=1000 (NOIA2). chen-zilei computes `uplink = server stamp - send` and `estimate = uplink + min(downlink over 3 s)`, clamped to the raw round trip.

### 1.2 Disagreements

| # | Claim | A2Tools | chen-zilei | RATmeter | NOIA2 | TK | Recommendation |
|---|---|---|---|---|---|---|---|
| D1 | Which direction is decoded | server to client only (`src_port == locked`) | server to client only (`src port == game port`) | server to client only (`tcp src port`) | both directions fed; whichever directional key carries the magic twice in 3 s becomes the combat key | server to client only (`src net`) | Parse only the direction whose source port is the locked server port. NOIA2's symmetric approach is harmless only because the client stream is encrypted and never frames; it wastes work and risks phantom frames. |
| D2 | How the game flow is found | heartbeat signature count (12 in 3 s) plus window check | fixed port list (13328) plus in stream heartbeat hunt | heartbeat signature count (5) plus grace | heartbeat signature count (2 in 3 s) | fixed server net and port, no detection | Signature based lock with the port list as an accelerator: candidate flows on 13328 first, but lock on `0E 00 36` hit rate (at least 5 hits inside 3 s) so that a port move after a patch does not blind the meter. |
| D3 | Loopback preference and grace | loopback locks at once, physical waits 2500 ms | opens all, no preference | loopback beats physical even with fewer hits, 2000 ms grace | virtual devices probed first | not covered | Keep a 2000 to 2500 ms grace before locking a physical adapter, and promote to loopback if the same port later appears there (A2Tools `confirm_candidate`). |
| D4 | Kernel BPF filter | none, all filtering in Rust | `tcp port 13328` (both directions, for ping) | `tcp` during detection, `tcp src port N` after lock | none (Npcap backend), `tcp and ip` (WinDivert) | `src net X and port 13328` | Use `tcp` while detecting and narrow to `tcp port N` after lock (both directions, so ping requests can still be timed), never a net filter. |
| D5 | Promiscuous mode | off | off | off | on (Npcap backend) | on | Off. Only this machine's traffic is wanted and promiscuous mode fails on some Wi Fi drivers. |
| D6 | Stale lock watchdog | 120 s without a parsed packet | per flow FIN/RST | 60 s without any packet | 30 s without magic | none | 60 s without a heartbeat frame on the locked flow, then back to detection. |
| D7 | Pong body size | 12 bytes from the start of the marker (`03 36 00 00` plus one i64), only the first i64 read | exactly 18 bytes (`00 00` + i64 + i64) | 13 byte minimum, i64 at frame offset 5 | 12 bytes from the start of the marker (marker plus one i64), only the first i64 read | not covered | Treat the pong as a normal frame with an 18 byte body; read both i64 fields. |
| D8 | Client echo clock | performance counter, then paired request, then .NET epoch | game monotonic clock learned by pairing requests with pongs | performance counter, then .NET epoch | .NET epoch only | not covered | Implement all three in A2Tools' order and accept the first interpretation that yields 1..=9999 ms consistently over two pongs; log which one won so a client change is visible. |
| D9 | What the heartbeat frame is | "per record terminator" seen in the byte stream, counted for the lock | a frame `00 36` with an 8 byte u64 server Unix ms body | an 11 byte keep alive, never dispatched | detection magic only | a 3 byte frame (legacy `06 00 36`) that is dropped | It is an ordinary 11 byte frame (len 14, opcode `00 36`, u64 server ms). Decode it: it gives a server clock, the lock signature and the resync anchor. |
| D10 | Pcap timestamp validation | accept 1e12..2e12 ms else wall clock | `max(1, ts)` | pcap timestamp used as is | accept > 1e9 s else wall clock | wall clock at receive | Accept the pcap timestamp when it is within one day of the wall clock, else use the wall clock; both are needed for replay. |
| D11 | TLS on the game port | skip TLS record headers in the stream and exclude TLS first connections | not covered | not covered | skip TLS looking payloads before the assembler | not covered | Do both A2Tools checks: a connection whose first segment is a TLS record is excluded; a TLS record header met mid stream is skipped whole (`5 + BE len`, len 1..=16640). |

### 1.3 Unknowns

- Cloudflare relay addresses and whether any other relay vendor is used; whether the relay ever coalesces or re segments game frames differently from a direct connection.
- The content of the encrypted client to server frames (only their sizes are known: 11 byte heartbeat, 12 or 13 byte ping).
- Whether the second TLS connection from the game port carries anything combat related (no repo decodes it).
- Whether port 13328 is universal across Global server groups (observed on Ariel 1306 (EU) and Nezekan 1502 (Asia) only).

---

## 2. TCP reassembly

### 2.1 Consensus

Two of five repos implement real sequence number reassembly (chen-zilei `tcp.rs`, NOIA2 `tcp_reassembler.rs`) and they agree on the model; this is the consensus for a new implementation:

- One reassembler per directional flow keyed by (src ip, src port, dst ip, dst port) (chen-zilei keys `(src SocketAddr, dst SocketAddr)`, NOIA2 `TcpFlowKey`).
- State: `next: Option<u32>` expected sequence, `pending` out of order segments (chen-zilei `BTreeMap<u32, Vec<u8>>`; NOIA2 `HashMap<u32, Vec<u8>>` plus a held byte counter), a gap counter.
- SYN sets `next = seq + 1` and clears pending (chen-zilei). Without SYN the first segment seen initialises `next = seq` (both), that is joining mid stream is allowed but the framing layer must then hunt for an alignment point.
- `ahead = seq.wrapping_sub(next) as i32`; wrap around is handled by this signed difference (both; chen-zilei's test `(u32::MAX - 1, b"ab"), (0, b"cd")` yields `b"abcd"`; NOIA2's test `feed(u32::MAX - 1, vec![1, 2])` then `feed(0, vec![3, 4])` emits each as its own chunk).
- `ahead > 0`: store in pending (first copy for a sequence wins in chen-zilei; the longer copy wins in NOIA2). `ahead <= 0`: `overlap = -ahead`; a segment entirely before `next` is a retransmit and is dropped; a partial overlap is trimmed and the fresh tail delivered; then drain pending while contiguous.
- Gap policy: chen-zilei skips the gap when more than 64 segments are pending (`next` jumps to the lowest pending seq, emits `Chunk::Gap`, the frame decoder resets and hunts for a heartbeat); NOIA2 skips when more than 2 MiB are held. Neither uses a timer.
- Empty payloads are ignored. FIN or RST removes the flow (chen-zilei).
- The reassembled byte stream goes into a per flow accumulator; the framing layer consumes complete frames and keeps the incomplete tail (A2Tools `StreamAssembler::process_chunk`, NOIA2 `assembler.rs`, RATmeter `PacketAccumulator`, TK `StreamAssembler`).

### 2.2 Disagreements

| # | Claim | A2Tools | chen-zilei | RATmeter | NOIA2 | TK | Recommendation |
|---|---|---|---|---|---|---|---|
| D12 | Whether to reassemble by sequence number at all | no; payloads appended in capture order, byte by byte framing resync recovers | yes | partial: in order only, gaps appended immediately, no out of order buffering | yes | hold out of order segments, no gap skipping | Reassemble. A2Tools' approach works only because its framing scans every byte offset; it is the reason A2Tools needs an embedded `04 38` scan (D27). |
| D13 | Gap handling | n/a | skip after 64 pending segments, then re hunt heartbeat | append immediately, lose the bytes, framing resyncs | skip after 2 MiB held; otherwise block until the segment arrives or the flow idles 30 s | never; the aligner stalls forever | Skip the gap after 64 pending segments or after 1000 ms, whichever comes first, emit a gap event and reset the frame decoder into heartbeat hunting. A blocked flow in a boss fight is worse than one lost record. |
| D14 | Partial overlap of a retransmit | n/a | trim the already delivered prefix | append the whole payload (duplicates bytes into the stream) | trim | drop the whole segment, losing the fresh tail | Trim the consumed prefix (chen-zilei and NOIA2 agree). |
| D15 | Duplicate pending segments | n/a | first copy wins | n/a | longer copy wins | later copy overwrites | Longer copy wins (NOIA2): a retransmit can carry more data than the original. |
| D16 | Flow identity | (min port, max port) per device | 4 tuple | `Client:<dst port>` per client port | 4 tuple | source IP only | 4 tuple. TK's source IP key merges two connections from the same server and A2Tools recorded that a reconnect from the same server port appended to the old stream corrupted every later record (`evidence_slice.rs:1528-1532`). |
| D17 | Accumulator cap | clear above 2 MiB | 128 KiB initial, frame max 64 KiB | 10 MiB, hard reset above | 50 MiB, clear above | 2 MB, reset above | 2 MiB per flow; on overflow clear and re enter hunting. Legitimate frames never exceed 65 535 bytes. |
| D18 | Behaviour when joining mid stream (no SYN) | scan from byte 0 | start in heartbeat hunting | search `0E 00 36` then parse from it | emit from the first byte, stall resync later | parse from the first byte | Start in hunting mode and align on the first `0E 00 36` (chen-zilei, RATmeter intent). |

### 2.3 Unknowns

- Whether the server ever reorders or coalesces frames across TCP segments in a way that a straddling heartbeat would mis align (A2Tools notes that packets straddle segments routinely).
- Reconnect behaviour: whether the server reuses the same client port after a reconnect (A2Tools saw a reconnect on the same server port).

---

## 3. Framing

### 3.1 Varint

All five repos use the same unsigned little endian base 128 integer: each byte contributes its low 7 bits at shift 0, 7, 14, 21, 28; a byte with bit 0x80 clear ends the number; at most 5 bytes (a sixth byte, shift 35, is invalid). A truncated varint is invalid and must not advance the cursor (chen-zilei `wire.rs`, A2Tools `read_varint`, NOIA2 `utils.rs`, RATmeter `PacketReader.ReadVarInt`, TK `SP:637-668`). Note that A2Tools and TK decode into a signed 32 bit value so a 5 byte varint with the high bit set reads negative and is rejected; NOIA2 decodes into u32 and returns it widened to i64, so it never reads negative; chen-zilei decodes into u32 and accepts `u32::MAX` (`FF FF FF FF 0F`); RATmeter's `PacketReader.ReadVarInt` is uint (up to 5 bytes) while its framing reader `BinaryExtensions.ReadVarInt` is int. Recommendation: decode into u32, reject values that need more than 32 bits, and treat `value < 0` style sentinels as parse failures.

The last byte of a multi byte varint is itself a valid one byte varint. When scanning backwards for an id that ends at a known offset, prefer the candidate that is not preceded by a byte with 0x80 set (A2Tools `varint_ending_at`, tests in section 17).

### 3.2 Frame layout and size formula

```
00                                   padding between frames: skip one byte
<len varint, n bytes>                length prefix
[<ext u8 in F0..=FE>]                optional extension byte of unknown meaning (see D20)
<opcode 2 bytes>                     wire order: first byte, second byte
<body ...>
```

Frame size counted from the first byte of the length varint: `size = len + n - 4`. Equivalently, the number of bytes that follow the varint is `len - 4`, and the server encodes `len = (opcode bytes + body bytes) + 4` (A2Tools `frame_size`, chen-zilei `frame.rs:92` and its encoder, RATmeter `PacketAccumulator.cs:110-111`, TK `StreamAssembler.kt:33`). So:

- 1 byte varint: the frame spans `len - 3` bytes.
- 2 byte varint (payload of 124 bytes or more): the frame spans `len - 2` bytes.
- 3 byte varint: `len - 1` bytes.

Validity (the recommendation, and chen-zilei's rule, `frame.rs:93`): `len >= 6` (the smallest frame is an opcode with no body, size 3); `size <= 65_535`. The other repos are looser: A2Tools requires only `size >= varint width` and `size > 0` (`framing.rs:275-277`), RATmeter and TK require `size > 0`. A2Tools once read the two byte case as `len - 3` and lost 3% of a capture of 4 October 2026 because every packet of 126 bytes or more framed one byte short (`framing.rs:19-23`); NOIA2 still does (D19).

Worked example: the heartbeat `0E 00 36 <8 bytes>`: `len = 14`, `n = 1`, `size = 11` = 1 + 2 + 8.

### 3.3 LZ4 bundle

A frame whose first two payload bytes are `FF FF` is a bundle:

```
<len varint> [ext] FF FF <raw size u32 LE> <LZ4 raw block to the end of the frame>
```

- Raw block format, no LZ4 frame header, no checksum (`lz4_flex::block::decompress(&payload[6..], raw_size)` in A2Tools, chen-zilei and NOIA2; K4os in RATmeter; lz4-java in TK).
- The decompressed buffer is a sequence of frames in the same format, including nested bundles. Inside a bundle: zero bytes are skipped, an invalid or truncated length ends the walk with no resync (the game framed this buffer itself), there are no TLS records (A2Tools `walk_inner`, chen-zilei `walk_inner`).
- Nesting depth is bounded at 4 (A2Tools `MAX_BUNDLE_DEPTH`, chen-zilei `MAX_DEPTH`; test `bundles_nest_four_deep_at_most`).
- Raw size must be at least 1; the upper bound differs (D22).
- A zone load bundle of 17 072 bytes was seen on 9 October 2026, so the "wait for more bytes" threshold for incomplete frames must be higher for bundles than for plain frames (A2Tools: 16 384 for plain frames, 65 535 when a bundle header sits at the payload start).
- Bundles also sit inside other packets where the outer framing never opens them; A2Tools and chen-zilei scan for `FF FF` at any offset `i >= 1`, try a varint of width 3, 2 then 1 ending exactly at `i`, size it with the same formula, and read only identity and roster records from the decompressed data (`scan_embedded_bundles_for_identity`, `scan_embedded_bundles`).

### 3.4 Heartbeat signature and resync

- Current signature: `0E 00 36` (since the June 2026 update); legacy: `06 00 36` (before). The current one is the 11 byte frame `0E 00 36 <u64 server ms>`; the legacy one is a 3 byte frame with an empty body (`len = 6`), which is exactly the "3 byte frame" that TK drops silently (`SP:67`). All five repos know one or both strings.
- Resync after a gap or a bad length: chen-zilei and RATmeter search for the signature and resume at it (chen-zilei caps the hunt at 256 KiB, then falls back to one byte resync); A2Tools resyncs one byte at a time validating each candidate; NOIA2 waits a configurable stall delay before skipping one byte; TK flushes the whole accumulator.
- The signature is also used as a record boundary inside scanned packets (A2Tools nickname patterns B and D, and the generic `<01..07> 00` boundary test in `parse_hit_tail`).

### 3.5 TLS records in the stream

`17 03 03 <u16 BE len>`: a TLS record from the other connection on the same server port. A2Tools skips `5 + len` bytes when the first byte is 0x14..=0x17, second 0x03, third 0x01..=0x04 and len in 1..=16640; if the record runs past the buffer it waits. Without this, TLS records swallowed up to 64 KB and kills read 20 to 35% short (`framing.rs:25-31`).

### 3.6 Size limits (union)

| Limit | A2Tools | chen-zilei | RATmeter | NOIA2 | TK | Recommended |
|---|---|---|---|---|---|---|
| Max frame | 65 535 | 65 535 | 40 960 | 65 535 | none | 65 535 |
| Incomplete plain frame wait | 16 384 | 16 384 | n/a | 8 192 default (2048..16384) | wait always | 16 384 |
| Incomplete bundle wait | 65 535 | 65 535 (bundles are exempt from the 16 384 garbage rule and wait up to `MAX_FRAME`) | n/a | 8 192 | wait always | 65 535 |
| Bundle raw size | 1..=1 000 000 | 1..=8 MiB | 1..=10 000 000 | 1..=5 000 000 | none | 1..=8 MiB, warn above 1 MiB |
| Nesting depth | 4 | 4 | unbounded | unbounded | unbounded | 4 |
| Hunt budget | n/a | 256 KiB | n/a | n/a | n/a | 256 KiB |

### 3.7 Disagreements

| # | Claim | A2Tools | chen-zilei | RATmeter | NOIA2 | TK | Recommendation |
|---|---|---|---|---|---|---|---|
| D19 | Frame size formula | `len + n - 4` | `len + n - 4` | `len + n - 4` | `len - 3` regardless of varint width, plus an ad hoc `+1` for bundle frames | `len + n - 4` | `len + n - 4`. NOIA2's formula is one byte short for every frame with a 2 byte varint; its bundle `+1` is a symptom of that. This is the single most consequential framing disagreement. |
| D20 | Optional `F0..=FE` byte after the varint | not part of framing (never seen in Global captures; the only F0..FF marker known is inside loot records) | skipped when present, included in `size` | skipped only when testing for a bundle inside `TryDecompress`; a top level `[len][F0][FF FF]` would be dispatched as opcode `F0 FF` | skipped (`0xF0..=0xFE`, never `0xFF`) | skipped; two handlers forget to | Skip it when present and count the occurrence. No known opcode begins with F0..FE, so the skip is safe, and the KR/TW lineage (TK, NOIA2) saw it. Keep `FF` out of the range because `FF FF` is the bundle marker. |
| D21 | Zero bytes between frames | skipped (top level and inside bundles) | skipped (both) | skipped inside bundles only; a top level `00` is a desync (size `-3`) | skipped (both) | skipped (both) | Skip at both levels. |
| D22 | Bundle raw size cap | 1 000 000 | 8 MiB | 10 000 000 (`(uint)(size - 1) > 0x98967F` rejects) | 5 000 000 | none | 8 MiB hard cap with a warning above 1 MiB; the largest legitimate bundle seen is 17 KB. |
| D23 | Nesting depth cap | 4 | 4 | none | none (recursive) | none (recursive) | 4. |
| D24 | Resync strategy | byte by byte with validation | hunt `0E 00 36` (or `06 00 36`), 256 KiB budget, then byte by byte | hunt `0E 00 36` (buggy start offset, see RATmeter 3.5) | wait a stall delay (default 1000 ms, configurable 50..2000; 200 ms for the full processor, 10 ms for an unknown opcode) then skip one byte | flush everything | Hunt the heartbeat after a gap or an invalid length; fall back to byte by byte resync when 256 KiB pass without a heartbeat (a patch may change the signature). Validate every candidate frame (opcode known, size sane) before trusting it, as A2Tools does. |
| D25 | Heartbeat dispatch | not dispatched (scanned) | dispatched (u64 read) | explicitly not dispatched | not dispatched | 3 byte frames dropped | Dispatch it; it is cheap and gives the server clock. |
| D26 | Where bundles are looked for | top level frames, nested, and embedded at any offset (identity and roster) | top level, nested, and embedded (identity and roster) | top level and nested | top level and nested | top level and nested | Top level and nested always; embedded scan for identity and roster records only (they are the records known to ride inside other packets). |
| D27 | Embedded `04 38` scanning for damage inside non damage packets | yes; measured 19% of a boss fight's damage arrives that way; dedup on 64 byte hex, trusted shape gate | no (identity and party only) | no | no | no | Do not start with it. A2Tools needs it because it has no TCP reassembly and because records straddle frames in its scan model. Implement reassembly and strict framing first, then measure: if damage records are found inside other opcodes with the trusted shape (actor != target, hit type 1..=3, damage > 0, known skill), add the scan as a gated fallback and log its share. |
| D28 | `06 00 36` second signature | legacy terminator before June 2026 | listed without explanation | comments mention it (stale) | not known | 3 byte frames dropped | Keep as a legacy alternative for hunting only; never required on the current client. |
| D29 | TLS record skipping in the stream | yes | no | no | pre assembler payload skip only | no | Yes (A2Tools' measured loss justifies it). |

### 3.8 Unknowns

- The meaning of the `F0..=FE` extension byte and when the server emits it.
- Why the server multiplexes TLS on the game port and whether the TLS connection can ever share the client port.
- Whether a bundle can legitimately exceed 1 MB.
- Whether the `00 36` opcode with an empty body (`06 00 36`) still occurs on the Global client (A2Tools also saw a `00 36` "container that carried spawns" in one capture, probably a misframe).

---

## 4. Opcode table

### 4.1 Notation in each repo and normalisation

| Repo | How it writes an opcode | Wire order derivation |
|---|---|---|
| A2Tools | byte arrays in wire order, `[0x04, 0x38]`, hex test strings `"0438..."`; some comments use a LE u16 reading (`0x3641` for `41 36`) | the array or the surrounding code gives the wire order |
| chen-zilei | `0xAABB` where AA is the first wire byte (`0x0438`), `opcode = b[o] << 8 | b[o+1]` | big endian u16 of the wire bytes |
| RATmeter | `ushort` constants with the first wire byte in the low byte (`0x3804` for `04 38`), `op1 | (op2 << 8)` | little endian u16 of the wire bytes |
| NOIA2 | tuples `(0x04, 0x38)` in wire order | direct |
| TK | `Opcode(0x04, 0x38)`, key `b1 | (b2 << 8)` | direct |

Every row below is normalised to "first byte, second byte" on the wire. When reading a repo's comment that says `0x3641` or `0x9702` remember that it is the reversed reading.

### 4.2 Merged opcode table

Columns: wire bytes; meaning; repos that agree on the meaning; repos that disagree; repos that decode it (at least partly); region and version note.

| Wire bytes | Meaning | Agree | Disagree | Decoded by | Region / version note |
|---|---|---|---|---|---|
| `00 36` | Heartbeat: u64 LE server Unix ms, 11 byte frame `0E 00 36 ...`, about 19 to 20 per second | chen-zilei (clock), A2Tools, RATmeter, NOIA2 (signature) | none on meaning; D9 on role | chen-zilei | Global since June 2026; legacy frame `06 00 36` (empty body) before, which TK (KR, June) drops as a 3 byte frame |
| `03 36` | Pong: `00 00`, i64 client echo, i64 server Unix ms | A2Tools, chen-zilei, RATmeter, NOIA2 | none | A2Tools, chen-zilei, RATmeter, NOIA2 | All regions; the client echo clock differs by region (D8) |
| `06 38` | Party scope record naming you (hundreds of times) and your party; only the entity id is read | A2Tools | none | A2Tools (id only) | Global captures name only the local player; older KR/TW captures also name party members (A2Tools `data_storage.rs:856-859`) |
| `0F 39` | Server connect record `0F 39 00 00 <server id u16> <n u8> <IPv4 ASCII> <port u16>` | A2Tools | none | A2Tools (diagnostic only) | Global, 1 October 2026 |
| `1B 92` | HP/MP update: `<actor varint> <hp varint> <hp max varint 1..=50 000 000>`; max HP stored | A2Tools | none | A2Tools | Global October 2026 |
| `20 36` | Session id to global character id link: `<2 bytes> <player id varint> <4 bytes> <global id u32 LE>` | RATmeter | none | RATmeter | Global September 2026 |
| `21 36` | Map load: `<u32 LE load count> <u32 LE map id> ...`, 52 bytes | A2Tools, chen-zilei | none | A2Tools, chen-zilei | Global October 2026; opcode did not move in June 2026 |
| `23 36` | Position / teleport: `<entity varint> <x f32> <y f32> <z f32> ...`; entity 0 means the local player was teleported (zone change) | A2Tools | none | A2Tools (entity byte only) | Global; did not move in June 2026 |
| `33 36` | Self identity: id, mask, name, server, class, level, abnormal list | all five | none on meaning; D66 to D74 on fields | all five | All regions, all known versions |
| `40 36` | Mob/summon spawn, pre June 2026 | A2Tools | none | A2Tools (legacy path) | Pre June 2026 only |
| `41 36` | Mob/summon/effect entity spawn since June 2026 (was the death opcode before) | all five | A2Tools notes the pre June reading as death | all five | Global, TW, KR (post June) |
| `42 36` | Death / despawn: `<id varint> <varint 0> <flag varint>`, flags 1 zone init, 3 combat death, 7 gone | A2Tools, chen-zilei | RATmeter uses `04 8D` for player death (D30); TK uses `21 8D` toggle; NOIA2 none | A2Tools, chen-zilei | Global since June 2026 (was `41 36`) |
| `44 36` | Other player spawn, pre June 2026 | A2Tools, TK (both 44 and 45 handled) | NOIA2 lists an open question 44 vs 45 | A2Tools (legacy), TK | Pre June 2026; TK straddles the update |
| `45 36` | Other player spawn / identity since June 2026 | all five | none | all five | Global, TW, KR (post June) |
| `49 36` | All stats sheet: `<2 bytes> <n> (<stat u16 LE> <value i32 LE>)* <8 bytes>` | A2Tools, RATmeter | count u8 (A2Tools) vs varint (RATmeter), D101 | A2Tools, RATmeter | Global |
| `4A 36` | Own stats changed: `<entity varint> <n u8> (<stat u16> <value i32>)* <8 bytes>` | A2Tools | none | A2Tools | Global; three in a row naming another entity move the local id onto it |
| `56 36` | Own combat power: `<u32 LE>` | NOIA2 | none | NOIA2 | TW |
| `02 38` | Cast | chen-zilei (named) | none | none | Global; body not decoded by anyone |
| `04 38` | Damage records (chained), also summon link records and instant self heals | all five | layout details, D37 to D55 | all five | All regions; layout shifted June 2026 (D42) |
| `05 38` | DoT / HoT tick | all five | effect code semantics, D56 to D61 | all five | All regions |
| `2A 38` | Abnormal (buff/debuff) added | A2Tools, RATmeter, NOIA2, TK | none | A2Tools (full), RATmeter, NOIA2, TK (partial) | All regions |
| `2B 38` | Abnormal changed (A2Tools) | A2Tools | RATmeter, NOIA2, TK call it a second "applied" variant with one byte less after the entity id (D97) | A2Tools (full), others partial | All regions |
| `2C 38` | Abnormal removed: `<entity varint> <n u8> (<kind u8> <instance varint> <reason u8> [kind & 7 == 7: <by varint> <skill u32> <effect u32>])*` | A2Tools | none | A2Tools | Global |
| `47 38` | Skill cooldowns: `<count u8> (<skill u32 LE> <ms left varint>)*` | RATmeter | none | RATmeter | Global |
| `00 8D` | Remaining HP: `<entity varint> <varint> <varint> <varint> <hp u64 LE>` | RATmeter, NOIA2, TK, chen-zilei (named); A2Tools reads the same bytes as the sub record `8D <id> 02 01 00 <u32 hp> 00 00 00 00` | HP width (D86) | RATmeter, NOIA2, TK, A2Tools | All regions |
| `04 8D` | Ownership / kill record: `<entity varint> <4 bytes> <owner varint> <server u16 LE> <len u8> <name> [<len u8> <legion>] <u32>`; summon to owner link, loot owner, despawn (all zero) | A2Tools, NOIA2 (ownership) | RATmeter reads it as entity death (D30) | A2Tools, NOIA2, RATmeter | Global, TW |
| `21 8D` | Entity combat state toggle: `<entity varint> <varint> <toggle varint>` (1 enter, 0 leave) | TK | none | TK | KR June 2026; not seen by any Global repo |
| `01 91` | Field boss respawn timer table (map id, entries with u64 timestamps) | NOIA2 | none | NOIA2 | TW; requested when the map UI is opened |
| `02 97` | Party roster | A2Tools, chen-zilei, RATmeter | member tail (D92) | A2Tools, chen-zilei (names), RATmeter | Global |
| `07 97` | Party join request: `<u32 room> <u32 requester> <u32> <u32 class> <u32> <u32> <len varint> <name> <server u16> <4 bytes> <u32 power>` | TK | none | TK | KR June 2026 |
| `25 97`, `0B 97` | Join cancelled / admitted: `<u32 requester>` | TK | none | TK | KR |
| `09 97`, `18 97`, `1D 97` | Join refused, instance start, party exit (no fields read) | TK | none | TK (recognised only) | KR |
| `05 8A` | Unknown list of named entries `EF 03 <u32> <len> <name> 2D 00 00 00 ...` (character list?) | NOIA2 raw sample; A2Tools decodes an unframed character select list `<id u32 LE> <len u8> <name>` | none | none | TW sample; Global character list seen at login |
| `1D 37`, `2A 37` | Records seen in captures, not decoded; `0F 1D 37` anchors a nickname pattern | A2Tools | none | none | Global |
| `E5 A2` | Not a game opcode: A2Tools' synthetic host for lifted records in evidence slices | n/a | n/a | n/a | internal |
| `FF FF` | Not an opcode: LZ4 bundle marker in payload position | all five | none | all five | All regions |

### 4.3 Markers inside records (not leading opcodes)

| Bytes | Meaning | Source |
|---|---|---|
| `01 00` | Separator between chained records in one `04 38` packet | A2Tools, chen-zilei, NOIA2 |
| `03 00 <varint>` | Life steal / heal suffix after a damage record | A2Tools |
| `<01..07> 00` | Generic record boundary accepted by the strict tail parser | A2Tools |
| `0E 00 36` / `06 00 36` | Heartbeat frame start, used as a terminator when scanning | A2Tools, chen-zilei, RATmeter, NOIA2 |
| `08 3B 38 00 00` / `08 3D 38 00 00` | Compact skill context marker; the next `38` byte starts `<actor varint> <uid u8> <skill u32 or 3 byte LE>` | A2Tools |
| `FF FF FF FF FF FF FF FF 80 75 D5 2A BB 03 00 00` | An abnormal list entry with length i64 = -1 and end i64 = 4 102 412 400 000 (never ends); the caster varint follows. NOIA2, TK and RATmeter use the eight `FF` as the "summon boundary"; A2Tools uses the `80 75 ...` half as the buff anchor | A2Tools (meaning), NOIA2, TK, RATmeter (anchor) |
| `07 02 06 <u32 LE>` / `07 02 01 <u32 LE>` | Caster field after a spawn's position; the u32 is the caster/owner entity id | A2Tools (u32), NOIA2, TK, RATmeter (read as u16) |
| `00 (00|40) 02` | Marker following the 3 byte NPC template code in a spawn | all five |
| `01 <cur varint> <max varint>` | HP pair inside a spawn | A2Tools, chen-zilei, RATmeter |
| `F4 CB 1F` | Combat power marker in a `45 36` record (NOIA2) | NOIA2 |
| `11 11` | Server id fallback marker in `45 36` (NOIA2) | NOIA2 |
| `E2 07` / `E0 07` | Server ids 2018 and 2016 in LE, once used as nickname anchors | A2Tools |
| `0F 1D 37`, `04 4C` / `00 4C` | Nickname pattern anchors B and D | A2Tools |
| `<F0..FF> <03|A3> <len> <name>` | Loot actor name field | A2Tools |

### 4.4 Disagreements

| # | Claim | A2Tools | chen-zilei | RATmeter | NOIA2 | TK | Recommendation |
|---|---|---|---|---|---|---|---|
| D30 | Meaning of `04 8D` | summon ownership at the front, loot/kill owner anywhere; an all zero record is a despawning summon | not listed | entity death: `<entityId varint>`, counts a player death when the id is an identified player | summon ownership: `<summon varint >= 100> 00 00 00 00 <owner varint>` | not listed | Decode it as the ownership/kill record per A2Tools (section 8.5). The RATmeter reading is a projection of the same record: it is sent when an entity is removed or a kill is credited. Use it for summon owner links and as a corroborating "boss killed" signal, never as the sole death source. |
| D31 | Other player opcode | `45 36` now, `44 36` before June 2026 | `45 36` | `45 36` | `45 36` with an open question about `44 36` | both `44 36` and `45 36` | `45 36` on the current client; accept `44 36` as a legacy alias. |
| D32 | `41 36` meaning by version | spawn since June 2026, death before | spawn | spawn | spawn | spawn | Spawn. Honour death flag 7 only under `42 36` (A2Tools) so a legacy reading of `41 36` cannot kill a summon. |
| D33 | `2B 38` meaning | abnormal changed | not covered | second applied variant, same layout | second applied variant with one byte less after the target (retry offset) | second applied variant | Changed. The "one byte less" observation is exactly the missing `01` byte in A2Tools' layout. |
| D34 | Death opcode | `42 36` | `42 36` | `04 8D` (player deaths only) | none (`00 8D` with hp 0 in PVP) | none (`21 8D` toggle for fight end) | `42 36` flag 3 is the death event; `00 8D` hp 0 and `04 8D` corroborate. |
| D35 | Opcode constant notation | arrays | BE u16 | LE u16 | tuples | tuples | Store opcodes as `[u8; 2]` in wire order in the new code and never as u16, to avoid the RATmeter versus chen-zilei inversion. |
| D36 | Existence of a cast opcode `02 38` | not named | named, not decoded | not named | not named | not named | Count it in diagnostics; do not depend on it. The no value `04 38` record (D37) is the usable cast signal. |

### 4.5 Unknowns

- Bodies of `02 38`, `06 38` (beyond the id), `4A 36` (beyond the stat list), `1D 37`, `2A 37`, `05 8A`, and the pre June `00 36` spawn container.
- Whether `21 8D` (TK) exists on the Global client; no Global repo lists it.
- The complete opcode families: observed second bytes `36` (entity/identity/world), `37`, `38` (combat), `39` (session), `4C`, `8A`, `8D` (entity state), `91` (world boss), `92` (HP/MP), `97` (party). This grouping is an observation, not stated by any repo.

---

## 5. Damage packet layout (`04 38`)

### 5.1 Consensus record layout (Global client, October 2026)

A `04 38` packet holds one or more records. Records after the first are prefixed with `01 00`; the first may or may not be (A2Tools, chen-zilei, NOIA2). The record, byte by byte:

| # | Field | Encoding | Semantics and validation | Sources |
|---|---|---|---|---|
| 0 | separator | `01 00` | Mandatory before the second and later records; optional before the first. When a record was already parsed and the next bytes are not `01 00`, stop. | A2Tools, chen-zilei, NOIA2 |
| 1 | `target` | varint | Entity id of the target. Plausible range 1..=9 999 999; ids under 100 only when a spawn or identity record announced them (A2Tools), else they are a resync artefact. | all five |
| 2 | `switch` | varint | Low nibble = layout: 4, 5, 6, 7 carry a value; 0 and 2 carry none (miss, resist, or cast). Bit 0x04 = record has a value; bit 0x02 = the plotter (flags, restoration HP, angle) follows the hit type; bit 0x01 = four extra unknown bytes; bit 0x20 = an additional hit list follows the value; bit 0x10 is mirrored by plotter flag 0x80 and is fixed per skill. Any other low nibble aborts the record. | A2Tools (bits), chen-zilei, NOIA2, TK, RATmeter (nibble 4..7) |
| 3 | unused | varint | Unknown ("flag"); read and discarded by all. | all five |
| 4 | `actor` | varint | Entity id of the attacker, same plausibility as the target. | all five |
| 5 | `skill` | u32 LE | Skill id. 1..=299 999 999. Theostone item ids 3 000 000..=3 099 999 are rewritten to `id * 10 + 1`. NPC skills are 7 digit codes 1 000 000..=9 999 999; player skills 10 000 000..=19 999 999; pet skills 100 000..=199 999; 30 000 000..=30 999 999 Theostone skills. | all five |
| 6 | `uid` | u8 | Per cast id; skipped. | A2Tools, chen-zilei, RATmeter, NOIA2, TK |
| 7 | `hit_type` | varint | 1 Miss, 2 ordinary hit, 3 Critical, 6 Resist (A2Tools). Everyone agrees 3 = critical. | all five |
| 8 | no value exit | | If the low nibble is 0 or 2 the record ends here: count a miss (hit type 1) or resist (6) on the skill when actor != target, then stop the chain. | A2Tools; chen-zilei (nibble 0 = cast marker ends the packet) |
| 9 | plotter | `<flags u8> <restoration hp varint> <angle u8>` | Present only when switch bit 0x02 is set (layouts 6 and 7). Flags: 0x01 ShieldBlock, 0x02 Parry, 0x04 Perfect, 0x08 Double (HardHit), 0x10 IronWall, 0x20 Regeneration (restoration), 0x40 PerfectBlock, 0x80 mirrors switch bit 0x10. Restoration HP is a varint (2 bytes from 128 up) that is non zero only with flag 0x20. Angle: 0x00 none, 0x01 Back, 0x02 Front. | A2Tools (full); chen-zilei, NOIA2, RATmeter agree on 0x02/0x04/0x08 and the angle byte at offset 2 of the block |
| 10 | `hit_id` | 8 bytes | Unknown per hit identifier; it looks like a u64 LE counter around 5e9..1e10 in all captures (for example `AF F4 B7 63 01 00 00 00`). Its last byte is always 00, which is why a 10 byte skip in layout 6 "finds a zero pad" (see D38). | A2Tools (8 byte per hit id in `docs/summon-attribution.md:112-123`); block widths of chen-zilei, RATmeter |
| 11 | extra | 4 bytes | Present only when switch bit 0x01 is set (layouts 5 and 7). Meaning and exact position relative to the hit id are unknown; A2Tools skips the block as a whole (12 bytes for layout 5, 14 for 7). | A2Tools, NOIA2, TK (block widths 12 and 14) |
| 12 | `scalar` | varint | The actor's power scalar: damage multiplier in hundredths of a percent (mobs 10000, geared players 16000..22000); a summon inherits its owner's. Recorded when in 1000..=200 000. Pre June 2026 this field was an unknown non zero value. | A2Tools, chen-zilei, NOIA2 (shifted tail) |
| 13 | `damage` | varint | The record's total damage, 0..=99 999 999 (A2Tools, chen-zilei, NOIA2 cap). It already includes the additional hits listed in the tail. | all five by position |
| 14 | layout 4 field | varint 1..=25 | Only in layout 4 records of players; a spirit's layout 4 record omits it. Meaning unknown. | A2Tools (chen-zilei documents "one small varint") |
| 15 | additional hits | `<count varint 1..=25> <hit varint> x count` | Only when switch bit 0x20 is set. The sum must be below the damage value. Entries are often all equal (NOIA2 and TK require it; A2Tools does not). | A2Tools, chen-zilei (documented), NOIA2, TK |
| 16 | heal suffix | `03 00 <varint>` | Life steal amount, 1..=9 999 999, filed as `heal_amount`. | A2Tools |
| 17 | end | `01 00` next record, `04 38` next packet, or end of body | A2Tools accepts `<01..07> 00` as a clean boundary. | A2Tools, chen-zilei |

Block widths implied by the above, measured from the hit type to the scalar:

| Layout (low nibble) | Bits | Bytes before the scalar | How the repos count them |
|---|---|---|---|
| 4 | 0x04 | 8 (hit id) | A2Tools 8, chen-zilei 8, RATmeter 8, NOIA2 8, TK 8 |
| 5 | 0x04+0x01 | 12 (hit id + 4 extra) | A2Tools 12, NOIA2 12, TK 12; chen-zilei and RATmeter 11 (D38) |
| 6 | 0x04+0x02 | 11 (3 plotter + 8 hit id), 12 when the restoration HP varint is 2 bytes | A2Tools 10 + 1 extra for the HP varint and the trailing 00 read as a zero pad; chen-zilei 11; RATmeter 11 (requires 12); NOIA2 10 with the zero pad shift; TK 10 (+2 with Restoration) |
| 7 | 0x04+0x02+0x01 | 15 (3 + 8 + 4), 16 with a 2 byte HP varint | A2Tools 14 + zero pad; NOIA2 14 + zero pad; TK 14; chen-zilei and RATmeter 11 |

Worked example (A2Tools test `hit_flags_are_read_from_player_records`, also decodable with the A2Tools, chen-zilei, RATmeter and NOIA2 models; TK's fixed 10 byte skip from the plotter flags lands on the hit id's last `00`, reads it as the unknown varint and takes the scalar 10340 as the damage, see D42): record `e9de02 06 00 c80b 9147ff00 3f 02 | 02 00 02 | aff4b76301000000 | e450 | 6a | 0100`: target 44905, switch 6, unused 0, actor 1480, skill 0x00ff4791 = 16730001, uid 0x3f, hit type 2, plotter flags 0x02 (Parry) hp 0 angle 2 (Front), hit id `af f4 b7 63 01 00 00 00`, scalar 10340, damage 106, then `01 00` the next separator.

Worked example with additional hits (NOIA2 `docs/...zh-CN.md:616-649`): `04 38 91 C1 02 26 00 A1 14 48 F1 CA 00 20 02 0C 00 01 2C 40 46 4F 01 00 00 00 8C 91 01 B1 D1 14 04 97 36 97 36 97 36 97 36 01 00`: target 41105, switch 0x26 (layout 6, bit 0x20), unused 0, actor 2593, skill 13300040, uid 0x20, hit type 2, plotter `0C 00 01` (Perfect + Double, hp 0, Back), hit id `2C 40 46 4F 01 00 00 00`, scalar 18572, damage 338097, count 4, hits 6935 x 4 (sum 27740 < 338097), `01 00`.

### 5.2 Value selection heuristics (A2Tools and chen-zilei)

- If the first varint after the block is 0, treat it as a pad and read one more varint (first <- second, second <- third). With the 11/15 byte block model this never triggers, but keeping it makes the parser robust to a skip that ends one byte short.
- `first_is_damage` (layout 6 crit without a scalar): when `and_result == 6 && hit_type == 3 && 1000 <= first <= 5 000 000 && second <= 25`, the damage is `first` and the cursor returns to just after it (A2Tools `should_treat_first_value_as_damage`, chen-zilei `parser.rs:168-174`). Both repos carry it; A2Tools verified it live on a Power Burst crit that read the scalar instead of its true 60876.
- `should_use_repeated_hit_damage`: when `switch == 54` (0x36), all additional hits match, `encoded - count*hit <= hit` and `encoded / 10 == hit`, use the repeated hit value (A2Tools only; a legacy quirk).
- Compact skill context (A2Tools only): inside a bundle, a packet holding `08 3B|3D 38 00 00` followed later by `38 <actor varint >= 100> <uid u8> <skill u32 or 3 byte LE>` is remembered; a later `04 38` record with skill 99 745 942 (compact aggregate) from that actor takes the remembered skill.

### 5.3 Classification after parsing

- Spirit link records: skills 16 990 000..=16 999 999 (spirit to owner) and 16 770 000..=16 779 999 (owner to spirit) are neither damage nor heal; they link summon and owner (A2Tools, chen-zilei `SPIRIT_LINKS`). RATmeter lists 16770000 as a healing skill (D63).
- Resource restore skills (MP/SP/DP/OP/FP/AP only, `resource_restore_skills.json`, 61 ids) are neither damage nor heal (A2Tools).
- `actor == target` is an instant self heal: A2Tools requires damage > 1 and a known player actor; chen-zilei accepts any amount >= 1 from any actor. RATmeter and TK drop such records; NOIA2 accepts them as damage.
- A record between two known players is a friendly action (heal) (A2Tools); chen-zilei instead classifies by the heal skill list alone, and its tracker drops player on player hits as PvP.

### 5.4 Validation rules (union, with the repo that applies each)

1. Opcode `04 38` at the computed offset (all).
2. Low nibble of switch in {4,5,6,7} or {0,2} (A2Tools); others reject 0 and 2.
3. Target and actor plausible: 1..=9 999 999 (chen-zilei), >= 100 unless announced (A2Tools), actor != 0 (NOIA2; the target is not checked).
4. Skill 1..=299 999 999 (A2Tools, chen-zilei, NOIA2); not 1 000 000..=9 999 999 for a player damage row (A2Tools breaks the chain, NOIA2 rejects, RATmeter rejects); RATmeter additionally whitelists 3 000 000..=3 099 999, 100 000..199 999, 11 000 000..19 999 999 and the suffix offsets of 16.4.
5. Damage 0..=99 999 999 (A2Tools, chen-zilei, NOIA2); chen-zilei requires >= 1.
6. Tail must end cleanly: empty, `<01..07> 00`, or `04 38` (A2Tools strict tail); chen-zilei skips to the next `01 00`.
7. Additional hit sum below the damage (A2Tools); count 1..=25 (A2Tools, NOIA2); all hits equal and non zero (NOIA2, TK).
8. Embedded (trusted) records only: actor != target, hit type 1..=3, damage > 0, known skill (A2Tools).
9. RATmeter only: switch value <= 255; actor != target; class derivable from the skill prefix (else the record is dropped).
10. TK only: the frame's first byte must not be 0x20 (unexplained).

### 5.5 Disagreements

| # | Claim | A2Tools | chen-zilei | RATmeter | NOIA2 | TK | Recommendation |
|---|---|---|---|---|---|---|---|
| D37 | Low nibble 0 and 2 | no value record: read through the hit type, count Miss (1) or Resist (6), stop the chain | 0 = cast marker, ends the whole packet; 2 rejected | reject (nibble must be 4..7, and 0 explicitly) | reject | reject | Parse 0 and 2 as no value records through the hit type (A2Tools); count misses and resists; stop the chain. Rejecting them loses the cast and miss signal and makes the chain walk desynchronise. |
| D38 | Special block width for layouts 5, 6, 7 | 12 / 10 / 14, plus one byte when the restoration HP varint is 2 bytes, then a "zero pad" varint is skipped | 11 / 11 / 11 | 11 / 11 / 11 (requires 12 remaining) | 12 / 10 / 14 then a zero pad shift | 12 / 10 / 14 (+2 with Restoration) | Model the block structurally: plotter (3 bytes, 4 when HP >= 128) when bit 0x02, then the 8 byte hit id, then 4 bytes when bit 0x01: 8 / 12 / 11 (12) / 15 (16). Keep the "first varint == 0 means pad" fallback. chen-zilei's flat 11 is wrong for layouts 5 and 7 (it reads the scalar one byte early), A2Tools' and NOIA2's 10 and 14 are right only because the hit id ends in 00. |
| D39 | Plotter flag bit assignment | 0x01 ShieldBlock, 0x02 Parry, 0x04 Perfect, 0x08 Double, 0x10 IronWall, 0x20 Regeneration, 0x40 PerfectBlock | 0x02 parry, 0x04 perfect, 0x08 double, 0x20 heavy (documented, only 0x02/0x04/0x08 read) | 0x01 back (overwritten by the angle byte), 0x02 parry, 0x04 perfect, 0x08 double | 0x02 parry, 0x04 perfect, 0x08 double, 0x20 smite (doc says 0x40) | 0x01 BACK, 0x02 UNKNOWN, 0x04 PARRY, 0x08 PERFECT, 0x10 DOUBLE, 0x20 ENDURE, 0x40 Restoration, 0x80 POWER_SHARD | Use A2Tools' table, verified against the game's Damage Analyzer and 4 October 2026 captures. TK's table is A2Tools' shifted left by one bit (parry, perfect, double, iron wall, regeneration all one bit higher) with 0x01 = back; either the KR June client packed the back flag into bit 0 or TK misread, so do not merge the two tables. |
| D40 | Meaning of flag 0x20 | Regeneration (restoration HP follows) | heavy | not read | Smite (code) / 0x40 (doc) | ENDURE (0x20) / Restoration (0x40) | Regeneration. "Smite" and "heavy" are names for the same bit; the restoration HP varint that A2Tools reads after the flags is what TK's "+2 for Restoration" was approximating. |
| D41 | Meaning of flag 0x01 | ShieldBlock | not read | back attack (then overwritten) | not read | BACK | ShieldBlock. Back and Front come from the angle byte (0x01 / 0x02), on which A2Tools, chen-zilei, RATmeter and NOIA2 agree. |
| D42 | Where the damage sits after the block | scalar then damage (post June 2026); pre June the first value was the damage | scalar then damage, with the layout 6 crit heuristic | "unknown varint" then damage | shifted mode: 0, legacy, damage, count; old mode: unknown, damage, count | unknown varint then damage | Scalar then damage. With a 10 byte skip for layout 6 (NOIA2, TK) the "unknown" is the last byte of the hit id and the "damage" is the scalar, which is phantom damage of 10 000 to 22 000 per hit; this is the most consequential layout disagreement. |
| D43 | Does the damage value include the additional hits | yes; A2Tools subtracts them so that `damage + multi = total` | not read | no tail | yes; multi hit sum is informational | per hit values ignored; damage counted alone | The damage varint is the record total. Keep the per hit list as a count and a breakdown; never add it on top. |
| D44 | When to read the additional hit list | only when switch bit 0x20 is set; count 1..=25; sum < value | documented the same, not implemented | never | always attempt; count 1..=25; all equal and non zero | always attempt; count u8; all equal and non zero | Gate on bit 0x20, require count 1..=25 and sum < damage, do not require equality (the game can vary hits). |
| D45 | Layout 4 trailing field | one varint 1..=25 for players, absent for spirits (retry as layout 6) | one small varint | none | none | none | Read it optionally: try with the field, and if the tail does not end cleanly retry without it. |
| D46 | Multiple records per packet | chained with `01 00` | chained with `01 00` | one record per packet | chained with `01 00` | one record per packet | Chain. RATmeter and TK silently lose every record after the first. |
| D47 | Records with actor == target | instant self heal when a known player and damage > 1 | heal | rejected | accepted as damage | rejected | Never count as damage. Credit as a self heal when the actor is a player. |
| D48 | Skill code validation | 1..=299 999 999; 7 digit codes end the chain; Theostone `*10+1` | 1..=299 999 999 only | whitelist of bands plus suffix offsets; drops unknown class | 1..=299 999 999 and not 1 000 000..=9 999 999 | none | At the parser accept 1..=299 999 999; at the tracker route 7 digit skills to damage taken only, player bands to damage dealt. Do not drop a record because its class is unknown (RATmeter). |
| D49 | Theostone transform | 3 000 000..=3 099 999 -> `*10+1` | none | table lookup `fullId / 10` | `*10+1` | none | `*10+1`, then normalise through the skill table as A2Tools does. |
| D50 | Entity id plausibility | >= 100, or 1..99 when announced | 1..=9 999 999 | none | actor != 0 (target unchecked) | none | 1..=9 999 999 and (>= 100 or announced). An Elementalist at id 48 lost 939 hits under a bare >= 100 gate (A2Tools). |
| D51 | Switch value upper bound | none | none | <= 255 | none | none | None; bit 0x100 and above are unknown but harmless. |
| D52 | First byte 0x20 guard (TK) | none | none | none | none | frame rejected when its length byte is 0x20 | Do not reproduce; it drops every 29 byte damage frame. |
| D53 | Hit type values | 1 miss, 2 hit, 3 crit, 6 resist | 3 crit | 3 crit | 3 crit | 3 crit | Use A2Tools' set; log any other value. |
| D54 | Restoration extra bytes | the HP varint's extra byte (1 when HP >= 128) | not covered | not covered | not covered | +2 unconditionally when the flag is set | Read the HP as a varint (A2Tools test `restoration_hp_is_a_varint`). |
| D55 | What ends a record | strict tail: empty, `<01..07> 00`, `04 38`; legacy fallback | skip bytes until `01 00` or end | ignore the rest | multi hit parse then loop | skip 2 bytes after the list | Strict tail with a fallback that resynchronises on `01 00`; count records that fail the strict tail separately so a layout change is visible. |

### 5.6 Unknowns

- The unused varint after the switch; the uid byte; the 8 byte hit id; the 4 bytes added by switch bit 0x01 and their position; switch bit 0x10 (mirrored by flag 0x80); hit types other than 1, 2, 3, 6; the layout 4 trailing field; the second plotter byte when flag 0x20 is clear; the structure after `03 00` beyond one varint; skill 99 745 942 (compact aggregate) and the `08 3B|3D 38 00 00` context record; the exact pre June 2026 layout (NOIA2's "old tail").

---

## 6. DoT / HoT tick layout and the effect type codes (`05 38`)

### 6.1 Consensus

```
<len> 05 38 <target varint> <effect u8> <actor varint> <unknown varint> <effect id u32 LE> <amount varint> [trailing bytes]
```

- One record per packet; no chaining (all five).
- `effect` (exact values, A2Tools and chen-zilei agree): 0x02 and 0x0A damage over time; 0x01 and 0x09 heal; 0x0B heal over time; 0x00 status tick and 0x08 buff tick are ignored. RATmeter, NOIA2 and TK test only bit 0x02 (set = damage tick) and TK's author observed 0x0A, 0x02, 0x03 as accepted and 0x08 as rejected.
- `effect id` is a SkillEffect id: the 8 digit abnormal id times 10 plus a one digit index (A2Tools `abnormals.json` key `12000121` is the Burn abnormal; its test comments say "effect 190000131. The effect is abnormal 19000013" and "effect 120001211 ... abnormal 12000121"). The meters derive `skill = effect_id / 100` (A2Tools, chen-zilei, RATmeter), which is abnormal / 10, usually a 7 digit code that owns no skill but has a name: 190000131 -> abnormal 19000013 -> 1900001 "Restore HP"; 120001211 -> abnormal 12000121 -> 1200012 "Burn". The result must be 1..=299 999 999.
- `amount`: 1..=99 999 999.
- A damage tick with `actor == target` is rejected; a heal tick on self is legitimate (A2Tools, chen-zilei).
- Damage ticks are accepted only if the derived skill is in `dot_skill_ids.json` (A2Tools, RATmeter); chen-zilei, NOIA2 and TK accept any.
- Heals go to the healer keyed by (skill, is_hot = effect == 0x0B) (A2Tools).
- The unknown varint between actor and effect id is not decoded by anyone.

### 6.2 Disagreements

| # | Claim | A2Tools | chen-zilei | RATmeter | NOIA2 | TK | Recommendation |
|---|---|---|---|---|---|---|---|
| D56 | Effect code test | exact: {02, 0A} damage, {01, 09} heal, {0B} HoT, others ignored | same set | bit 0x02 set = damage, else ignored | bit 0x02 set = damage, else ignored | bit 0x02 set = tick, else ignored | Exact set. A bit test turns a HoT tick (0x0B has bit 0x02 set) into damage against a party member. NOIA2 has no effective guard (its heal list is matched against `raw - raw % 10000`, which never equals an 8 digit list key for a 9 digit effect id), so 0x0B ticks are counted as damage; RATmeter is shielded mainly by `IsReasonableSkillCode` rejecting the 7 digit derived codes and only secondarily by its heal list; TK does not avoid it at all. |
| D57 | Effect 0x03 | not listed (ignored) | not listed (dropped) | accepted (bit 0x02) | accepted | accepted (observed "normal") | Treat 0x03 as a damage tick but count it separately until a Global capture confirms it; A2Tools' Global captures never mention it. |
| D58 | Skill id derivation | `/ 100` | `/ 100` | `/ 100` | `raw - raw % 10000` (keeps 10 digit codes as their own key) | `/ 10` if that id is in the skill table, else `/ 100` | `/ 100`. NOIA2's normalisation does not apply to SkillEffect ids. |
| D59 | DoT allowlist | required (`dot_skill_ids.json`, 2641 ids) | none | required (1540 ids, exact match) | none | none | Optional and off by default for correctness, on for upload grade data: the allowlist protects against misread ticks but goes stale with every content patch. Log ticks rejected by the list. |
| D60 | HoT handling | heal rows `<skill> - HoT` | `Event::Heal` | dropped | counted as damage on the healed target (the heal list is keyed on 8 digit codes and never matches a tick) | would be stored as damage against the player target | Credit heals; never count a 0x01/0x09/0x0B tick as damage. |
| D61 | Target and actor gates | no id plausibility gate on damage ticks (varint validity and actor != target only); heal ticks drop an implausible healer in `append_heal` | both 1..=9 999 999 | none | actor >= 1 | actor != target | Same plausibility as damage records (the 1..=9 999 999 range is chen-zilei's alone). |

### 6.3 Unknowns

- The varint between actor and effect id: 182 (`b6 01`) and 272 (`90 02`) in A2Tools' two vectors (V77, V78), so it is not a small flag; meaning unknown.
- Trailing bytes after the amount.
- Effect codes 0x00 and 0x08 content (status and buff ticks per A2Tools' naming), and any code above 0x0B.
- Whether a DoT tick can be a critical hit (no flag exists in the record).

---

## 7. Healing

### 7.1 Consensus

Heals are recognised by four signals, in this order of reliability:

1. `05 38` effect codes 0x01, 0x09 (heal) and 0x0B (heal over time) (A2Tools, chen-zilei). Skill = effect id / 100, amount = the varint, healer = actor, target = target.
2. A `04 38` record whose actor equals its target: instant self heal, not a HoT (A2Tools requires damage > 1 and a known player actor; chen-zilei accepts any amount >= 1 from any actor).
3. A2Tools: a `04 38` record between two known players (summons resolved to owners) is a friendly action whose value is a heal on the target (`party_heal`). chen-zilei: a record whose skill base is in `HEAL_SKILLS` (18 120 000, 18 170 000, 16 190 000, 17 120 000, 17 800 000, 17 100 000, 17 410 000) is a heal regardless of who the actor and target are; player on player records with other skills are dropped as PvP by its tracker, not credited as heals.
4. The `03 00 <varint>` suffix on a damage record: life steal, credited to the actor as `heal_amount` / regen (A2Tools).

Exclusions:

- Spirit link skills 16 990 000..=16 999 999 and 16 770 000..=16 779 999 are neither damage nor heal (A2Tools, chen-zilei). The Wind Spirit's 1.5% HP restore (16990003) is a link record (A2Tools test `mp_restores_are_not_healing`).
- Resource restore skills (61 ids, A2Tools `resource_restore_skills.json`) restore MP/SP/DP/OP/FP/AP and are excluded from both damage and healing.
- A mob is never a healer (A2Tools filters heal rows whose actor is a mob).
- Shields and absorbs: not decoded by any repo; "the game may not send absorbed amounts at all" (chen-zilei). The only shield related data are the defensive hit flags ShieldBlock, IronWall, PerfectBlock on hit records (A2Tools). NOIA2's 29 entry "healing" list mixes heals and shields (11720000, 12720000, 15160000) and is used only to exclude them from damage.
- Overhealing cannot be told apart (chen-zilei).
- Heals never start or extend an encounter (chen-zilei); A2Tools counts only ticks inside the fight window.

Heal skill lists in circulation (for reference, all credited to the same community source):

| Repo | Ids |
|---|---|
| RATmeter `healing_skill_ids.json` | 18120000 Recuperation, 18170000 Healing Touch, 16770000 Spirit Communion, 16190000 Enhance: Spirit's Benediction, 17120000 Radiant Recovery, 17800000 Radiant Benediction, 17100000 Healing Light, 17410000 Light of Protection |
| chen-zilei `HEAL_SKILLS` | the same minus 16770000 (which it treats as a spirit link) |
| NOIA2 `healing_skill_code.json` | 11120000, 11380000, 11720000, 12230000, 12250000, 12720000, 13710000, 13790000, 14710000, 14790000, 15160000, 15790000, 16790000, 17090000, 17100000, 17120000, 17160000, 17270000, 17290000, 17320000, 17390000, 17410000, 17720000, 17800000, 18120000, 18160000, 18170000, 18420000, 18720000 |

### 7.2 Disagreements

| # | Claim | A2Tools | chen-zilei | RATmeter | NOIA2 | TK | Recommendation |
|---|---|---|---|---|---|---|---|
| D62 | Primary heal signal | effect code and self target; no heal skill allowlist | self target, heal skill base list, effect codes | skill list only (heals dropped) | skill list only (heals dropped) | none (self target records dropped) | Effect codes and self target first; the skill list only to classify player to player `04 38` records. Lists go stale; effect codes do not. |
| D63 | Is 16770000 a heal | spirit link band 16 770 000..=16 779 999, not a heal | spirit link | heal | not listed | not covered | Spirit link (two most recent repos agree). |
| D64 | What to do with a recognised heal | credit HPS per healer and skill | credit HPS | drop | drop | drop | Credit it; a party meter needs healer rows. |
| D65 | Heals and the encounter clock | counted only inside the fight window | never start or extend a fight | n/a | n/a | n/a | Never start or extend; attribute to the open encounter only. |

### 7.3 Unknowns

- Shield and absorb amounts, if the server sends them at all.
- Whether heal ticks carry a crit flag.
- Overheal.
- The semantics of effect 0x01 versus 0x09 (both heal; the difference is not decoded).

---

## 8. Identity / player info packets

### 8.1 Consensus layout (`33 36` self, `45 36` other player, `44 36` legacy other player)

```
<opcode 2 bytes> <entity id varint 1..=9 999 999> <mask1 u32 LE> <mask2 u8> [mask2 & 0x01: <len u8> <utf8 name>] ...
```

- Four repos read this front: A2Tools (`mask1 u32`, `mask2 u8`, bit 0 gates the name), chen-zilei ("4 bytes u32 mask, flags u8 bit 0 = has name, len u8"), RATmeter ("4 bytes skipped, flags with bit 0x01, nameLen, name"), NOIA2 for `33 36` only ("five metadata bytes, name_len one byte", `parse_main_fixed`). NOIA2's `45 36` reader (`parse_other`) and TK's `44 36` / `45 36` reader (`searchOtherNickname`) instead read `<id varint> <varint> <varint> <1 byte>` and then probe five consecutive offsets for a varint name length 1..=71 (NOIA2 keeps the longest sanitised candidate, TK the first valid one), read the job byte directly after the name and find the server by scanning; this coincides with the mask layout only because the mask bytes happen to decode as two varints. TK's older approach searches for a `07` byte within 10 bytes of the id and reads a varint length after it: `07` is simply the `mask2` value with bit 0 set on that client, and A2Tools records that it once matched the literals `0B 37` (self) and `07` (players) before reading the bit (`stream_processor.rs:761-769`).
- Name length byte 1..=48 (A2Tools `NAME_FIELD_BYTES`); name is UTF 8.
- Self record only, after the name: `<server u16 LE> <class u32 LE> <u8> <level u32 LE 1..=99>` (A2Tools `self_profile`; chen-zilei documents "server u16, class u32"; RATmeter and NOIA2 read `server u16` then `job u8`, which is the low byte of the same u32; TK reads `server u16` then a class byte). A `33 36` without a valid server is not a self record (A2Tools).
- Reference self record (A2Tools): `33 36 ed 74 5e 91 c1 28 37 06 4e 61 69 63 68 61 18 05 1e 00 00 00 01 1c 00 00 00 ...` -> entity 14957, mask1 `5e 91 c1 28`, mask2 0x37 (bit 0 set), name "Naicha", server 1304 (`18 05`, Kaisinel, Europe), class 30 (Cleric), level 28.
- Other player record (`45 36`): same front; the bytes after the name are not fixed width. NOIA2 scans for a u16 server id followed by a legion name (`<len varint <= 24> <utf8>`), and finds combat power after the marker `F4 CB 1F` as a u32 LE in 1..=10 000 000 followed by a u32 0; TK scans for a server id followed by a 2..24 byte legion name; RATmeter scans for any known server id. A2Tools reads only the front (`player_spawn_at`) and the abnormal list that follows (section 12).
- The self record arrives on zone loads and every few minutes, often inside bundles embedded in larger packets; other player records arrive when a player comes into view or on a loading screen (A2Tools, chen-zilei). Both also ride mid body inside other packets, so both scan for `33 36` / `45 36` mid body and parse an identity record there, with strict name validation as the false positive filter: chen-zilei over every unlisted packet body of at least 16 bytes (and embedded bundles); A2Tools over every frame region and every bundle regardless of opcode, with records of at least 9 bytes (`masked_records`).
- Identity ids are never folded (chen-zilei, A2Tools).

### 8.2 Class id (roster and self record)

u32 LE with four values per class; the low byte is what RATmeter, NOIA2 and TK read as a "job byte":

| Value | Class | Skill prefix (first two digits of 8 digit skill ids) | Sources |
|---|---|---|---|
| 5..=8 | Gladiator | 11 | A2Tools, NOIA2, TK (5 estimated) |
| 9..=12 | Templar | 12 | A2Tools, NOIA2, TK |
| 13..=16 | Ranger | 14 | A2Tools, NOIA2, TK |
| 17..=20 | Assassin | 13 | A2Tools, NOIA2, TK (17, 19 estimated) |
| 21..=24 | Elementalist | 16 | A2Tools, NOIA2, TK (21 estimated) |
| 25..=28 | Sorcerer | 15 | A2Tools, NOIA2, TK |
| 29..=32 | Cleric | 17 | A2Tools, NOIA2, TK (31 estimated) |
| 33..=36 | Chanter | 18 | A2Tools, NOIA2, TK (33, 35 estimated) |
| 45..=48 | Fighter (Brawler) | 19 | NOIA2 only; A2Tools has no roster value for Fighter |
| 37..=44, other | unknown | | |

Observed values in A2Tools captures: Gladiator 5..8, Templar 9/12, Ranger 14, Assassin 17, Elementalist 22/24, Sorcerer 28, Cleric 32, Chanter 36.

### 8.3 Name validation (union)

| Rule | A2Tools | chen-zilei | RATmeter | NOIA2 | TK |
|---|---|---|---|---|---|
| Byte length | 1..=48 field, exact name 1..=12 characters | 1..=36 bytes, <= 16 characters | 1..72 | 1..=36 (self), 1..=71 (other) | 1..71 (other) |
| Characters | alphanumeric in any script, at least one letter | alphanumeric any script, at least one alphabetic | letters or digits, not all digits | alphanumeric or Han, not all digits, single character only if Han or Hangul | Hangul, ASCII letters, digits; at least one letter |
| Tutorial placeholder | `$` + at least 4 ASCII alphanumerics binds the id with no name | `$x9a2` style rejected | n/a | n/a | n/a |
| Control bytes | rejected | rejected | `b < 32` is a repeat count (quirk) | stripped | rejected |

### 8.4 Server id

- u16 LE immediately after the self name. Valid range on Global: 1000..=2999 (A2Tools, which once matched only `E0 07` / `E2 07` = 2016, 2018 and skipped every other server). Observed Global ids: 1304 Kaisinel (EU), 1305, 1306 Ariel, 1502 Nezekan; A2Tools' `servers.json` lists 616 servers across regions AS, EU, JP, LA, NAE, NAW.
- KR/TW: 1001..=1021 (Elyos) and 2001..=2021 (Asmodian) (TK, NOIA2, RATmeter `ServerMap`).

### 8.5 Other identity sources

- Character select list at login (A2Tools): unframed entries `<entity id u32 LE 100..=9 999 999> <len u8 1..=48> <utf8 name>`; only the entry matching the configured character name binds the local id, and only until a self record arrives.
- `20 36` (RATmeter): `<2 bytes> <session id varint> <4 bytes> <global id u32 LE>` links a session entity id to the global character id (the party roster's dbid low 32 bits).
- `56 36` (NOIA2): own combat power u32 LE.
- `06 38` party scope and `4A 36` own stats (A2Tools): local id fallbacks (section 4).
- `04 8D` loot owner (A2Tools): votes for the local id among entities in party scope.
- `07 97` join request (TK): name, server, class, combat power of an applicant without an entity id.

### 8.6 Ownership and loot record (`04 8D`), A2Tools reading

Front of packet (`ownership_at_front`): `<len> 04 8D <summon varint >= 100> <4 bytes> <owner varint >= 100> <varint> <len u8 1..=36> <name>`; owner != summon; no server validation. The varint after the owner is where the server u16 sits, so for a server whose low byte is below 0x80 (1304 = `18 05`, 1305 = `19 05`, 1306 = `1A 05`) that varint is one byte and the parser takes the high server byte `05` as the name length. The front parser never reads the legion name or the trailing u32; `[<len u8> <legion>] <u32>` is observed in the test vector only. Anywhere in a buffer (`ownership_records`): after `04 8D <summon varint 100..=9 999 999> <4 bytes>`, the byte after the fixed field must be non zero (a despawning summon's record is all zeros), then within 128 bytes find a `<server u16 LE 1000..=2999>` preceded by an `<owner varint 100..=99 999>` ending exactly there and followed by `<len u8 1..=48> <exact name>`. Test vector: `04 8d ec de 02 | 72 28 e9 00 | ae 0b | 19 05 | 05 ApexZ | 06 Ventus | 01 00 00 00` -> entity 44908, owner 1454, server 1305, name ApexZ, legion Ventus. NOIA2's reading `04 8D <summon >= 100> 00 00 00 00 <owner varint>` requires the 4 bytes to be zero, which this vector contradicts (D30).

### 8.7 Disagreements

| # | Claim | A2Tools | chen-zilei | RATmeter | NOIA2 | TK | Recommendation |
|---|---|---|---|---|---|---|---|
| D66 | Valid server id range | 1000..=2999 | documented, not validated | 1001..1021, 2001..2021 (`ServerMap`) | 1001..=1021, 2001..=2021 | 1001..1021, 2001..2021 | 1000..=2999 for Global. The KR/TW ranges reject every EU/NA server (1304, 1306, 1502 all fail), so an implementation copying them identifies nobody. Keep the known list only for naming. |
| D67 | Name length field | u8 | u8 | varint | u8 (`33 36`), varint (`45 36`) | varint | u8 (identical for lengths under 128, which every valid name is). |
| D68 | Name validation | <= 12 characters, any script | <= 16 characters, <= 36 bytes | <= 72 bytes | sanitiser; single ASCII letter rejected | Hangul/ASCII/digits only | Field 1..=48 bytes, name 1..=16 characters, alphanumeric in any script with at least one letter, reject `$` placeholders. TK's Hangul only rule would reject accented Latin names on Global; A2Tools accepts one character names ("A", "あ") and NOIA2 does not; accept them. |
| D69 | Class field width | u32 LE | u32 (documented) | u8 | u8 | u8 | u32 LE; the three high bytes are zero so the u8 readers agree in practice, but reading u32 keeps the level offset right (`after + 7..+11`). |
| D70 | Tail of the other player record | front only, then the abnormal list | front only | server id by scan, combat power by backward u64 pair scan (self) | server by scan with legion check, combat power after `F4 CB 1F` | server by scan with legion check | Read the front; take server, class and level from the roster when available; scan for the server id with the legion name check only as a diagnostic. |
| D71 | What establishes the local player | the self record and the `4A 36` streak override each other, latest wins (the streak clears `local_identity_from_game`; the next self record clears `local_from_stats`); below them a configured name already matched to an entity (also via the character list) blocks a loot vote with another name, an applied loot vote blocks the character list and overrides a scope derived id, and the `06 38` scope leader applies only while nothing else has set the local id (or scope itself did) | self record (restored from disk for 45 min) | self record (`isUser`) | self record | self record (`isExecutor`) | Self record first; keep A2Tools' fallbacks for a meter opened mid session (its test `a_meter_opened_mid_session_finds_you_in_party_scope_records`). |
| D72 | Does a self record reset the meter | no | no (a changed self id resets `self_in_combat`) | no | yes (main actor callback resets the meter) | no | No. Reset on map load instead. |
| D73 | Fighter class ids | none | none | 19 from skills only | 45..=48 | none | Accept 45..=48 as Fighter; verify on a Global roster. |
| D74 | Five metadata bytes | u32 mask + u8 flags | u32 mask + u8 flags | 4 bytes + flags u8 | 5 bytes (`33 36`); two varints plus one byte, then a varint length probed at five offsets (`45 36`) | search `07` within 10 bytes | u32 mask + u8 flags with bit 0 = name present. |
| D75 | Identity persistence across restarts | not persisted (fuzzy names forgotten on reset) | saved to disk, ignored after 45 minutes | never cleared | metadata survives reset | until hard reset | Persist names for 45 minutes keyed by server and name, drop them when a fresh self record disagrees (chen-zilei). |

### 8.8 Unknowns

- The bits of `mask1` and the other bits of `mask2`.
- The byte between class and level in the self record.
- The full structure of the `45 36` tail (legion id, legion name, combat power, gear) beyond the scans above.
- The framing of the character select list and the `05 8A` list.
- The 4 bytes after the entity id in `04 8D`.

---

## 9. Spawn / NPC packets (`41 36`, legacy `40 36`)

### 9.1 Consensus

```
41 36 <entity id varint> <mask u32 LE> <subtree ...> ... <npc code u24 LE> 00 (00|40) 02 <x f32> <y f32> <z f32> ... 01 <cur hp varint> <max hp varint> ... [abnormal list] ... 07 02 06 <caster u32 LE> ...
```

- Entity id: varint; ids above 1 000 000 fold to `(id & 0x3FFF) | 0x4000` (A2Tools, chen-zilei, NOIA2; no repo explains the fold).
- Mask: u32 LE (A2Tools; it was u16 before a patch, both subtree positions `offset + 4` and `offset + 2` are tried). Low byte = kind: 0x0C / 0x0D NPC, 0x5F summon or pet, 0x1C short lived skill effect entity parented to the skill's target, 0x1F / 0x1D / 0x5D other players' spirits and Sorcerer ground spells (A2Tools; chen-zilei lists `5F 1C 1F 1D 5D` as summon kinds). Bit 0x0010 of the mask means a parent key block is present (A2Tools).
- Inline name: the subtree byte's bit 0 gates `<len u8 1..=48> <exact name>`; for a summon the name is the owner's, for an effect entity the caster's (A2Tools).
- NPC template code: the 3 bytes immediately before the marker `00 (00|40) 02`, as a u24 LE, found within 60 bytes after the mask (A2Tools, chen-zilei, RATmeter); NOIA2 and TK search the whole frame for `00 40 02` first then `00 00 02`. The code registers the mob, its boss flag and dummy flag from the NPC table.
- HP: within 64 bytes after the marker, the first `01 <cur varint > 0> <max varint >= cur>` gives max HP (A2Tools, chen-zilei). RATmeter reads one varint after the first `01` as the max (and the same bytes as the current, a bug).
- Position: x, y, z as f32 after the marker; the caster field follows the position (A2Tools).
- Owner link for summons and pets, in A2Tools' priority order: (1) kind 0x5F only (`is_summon && mask & 0x0010 != 0`): parent key block preceded by the byte 0x06: `<parent u32 LE 1..=9 999 999, != self> <legion id u32> <u16 0> <server u16 1..=9999> <len u8 <= 40> <legion name>` (`parse_spawn_owner_block`; a summoner with no legion has an empty legion name, len 0, while the parent id, the zero pad and the server id must still validate); (2) the inline name resolved through the nickname table; (3) the abnormal list anchor, on the wire `FF FF FF FF FF FF FF FF 80 75 D5 2A BB 03 00 00` (a permanent abnormal: length -1, end 2100-01-01): A2Tools matches only the last 8 bytes `80 75 D5 2A BB 03 00 00` (anchor start within 120 bytes of the mask start; its unit test vector carries no `FF` bytes) and reads `<caster varint 1..=9 999 999>` right after them, for kinds 0x5F/0x1F/0x1D/0x5D; (4) the caster field `07 02 06 <u32 LE>` or `07 02 01 <u32 LE>` within 240 bytes, for kinds 0x1F/0x5F, never the spell itself or a known mob. NOIA2, TK and RATmeter implement (3)+(4) as one path: find the eight `FF`, then `07 02 06` after them, then read the id at `07 02 06` + 3 as u16 LE; the caster u32 at the same offset read by A2Tools is the same field.
- A spawn whose NPC code is in the table but not in `player_summon_npcs.json` (186 codes, RelationshipEntity PC_Summon) is no player's summon whatever name it carries (A2Tools).
- Every spawn marks the id as a non player and forgets the id's previous owner links; ids are reused once an entity is gone (A2Tools, chen-zilei).
- Summons are attributed to their owner at aggregation: A2Tools resolves up to 16 parent hops and also uses `04 8D`, link records and the power scalar; NOIA2 up to 8 hops; TK and RATmeter one hop; chen-zilei does not attribute (summon damage is credited to nobody).

### 9.2 Disagreements

| # | Claim | A2Tools | chen-zilei | RATmeter | NOIA2 | TK | Recommendation |
|---|---|---|---|---|---|---|---|
| D76 | Mask width | u32 (was u16), kind in the low byte | u32, kind in the first byte | not read | not read | not read | u32 LE; read the kind from the low byte and try both subtree offsets. |
| D77 | NPC code marker search range | within 60 bytes of the mask, at least 3 bytes after it | `mask_start + 3 .. mask_start + 60` | `P .. P + 60` (with a double offset bug) | whole payload | whole frame including the length header | Bounded: from 3 bytes after the mask to 60 bytes after it. Whole frame searches can match inside the length header or a name. |
| D78 | Marker variants | `00 <40|00> 02` either | either | either (`& 0xBF`) | `00 40 02` first, then `00 00 02` | `00 40 02` first, then `00 00 02` | Either, first occurrence in the bounded window. |
| D79 | Owner id width in the caster field | u32 LE | not read | u16 LE (truncated to ushort) | u16 LE | u16 LE | u32 LE. Owners above 65 535 exist (raid player ids run past 99 999). |
| D80 | Owner link paths | parent key block, inline name, abnormal anchor + varint, caster field | not implemented | anchor + caster field | anchor + caster field | anchor + caster field | Implement all four of A2Tools' paths in its order; the anchor + caster path alone misses summons with no abnormal list and ground spells name themselves after the anchor. |
| D81 | Summon kinds | 0x5F summon/pet, 0x1C effect, 0x1F/0x1D/0x5D spirits and ground spells, 0x0C/0x0D NPC | `5F 1C 1F 1D 5D` all summon like, skipped as NPCs | no kinds; NPC vs summon by whether the owner anchors are found | no kinds; NPC = has a mob code | no kinds; NPC = has a mob code | A2Tools' kinds. |
| D82 | HP from the spawn | `01 <cur> <max>` within 64 bytes after the marker | same | one varint after the first `01` (cur = max) | not read (`00 8D` only) | not read | `01 <cur> <max>`, max >= cur > 0. |
| D83 | Id folding | spawn ids > 1 000 000 | spawn and death ids > 1 000 000 | none | spawn ids > 1 000 000 | none | Fold spawn and death ids above 1 000 000 but keep the raw id too and log every fold; the fold is unexplained and may be a misread of a wider field. |
| D84 | Summon owner map reset | links kept across zone changes; dropped only on replay of another session | n/a | cleared on reset | cleared on every reset (including the self record reset) | until hard reset | Keep links across zone changes; drop them when the id is respawned as something else. |

### 9.3 Unknowns

- Mask bits other than 0x0010; the subtree bytes; the variable fields between the mask and the NPC code; the meaning and origin of the id fold; whether the parent key block exists without a legion; the `1D 37` / `2A 37` records that accompany spawns; what A2Tools' "friendly effect target attribution" (not implemented) would read.

---

## 10. Death, remaining HP, map load / zone change packets

### 10.1 Death / despawn (`42 36`, pre June `41 36`)

```
<len> 42 36 <entity varint> <varint, always 0> <flag varint>
```

Flags: 1 zone init (loaded dead), 3 combat death, 7 gone from the world (A2Tools). chen-zilei: 1 or 3 = dead. Flag 7 is honoured only under `42 36` because under the old opcode a summon's spawn mask can read as 7 (A2Tools). A despawned summon's later DoT ticks are not counted until it acts directly again (A2Tools). The game can report one death more than once; chen-zilei ignores duplicates within 5 s. A boss death finishes the encounter (chen-zilei, on flag 1 or 3; the flag 3 only rule is A2Tools').

### 10.2 Remaining HP (`00 8D`)

```
<len> 00 8D <entity varint> <varint a> <varint b> <varint c> <hp u64 LE>
```

RATmeter reads the u64; NOIA2 reads a u32 (<= 1 000 000 000); TK reads a u32; A2Tools reads the same bytes as the anywhere scan `8D <id varint 100..=9 999 999> 02 01 00 <u32 LE current HP <= 100 000 000> 00 00 00 00` for NPCs, with the four trailing zero bytes required; it documents `01 01 01` instead of `02 01 00` as the local player's record and deliberately skips it (`scan_for_entity_hp` accepts only the `02` discriminator). So the three varints are observed as (2, 1, 0) for NPCs and (1, 1, 1) for the local player, and the u64 reading is the u32 plus four zero bytes. Uses: boss HP bar and time to kill (A2Tools, NOIA2, TK), HP reset dummy detection (current HP 1 then any higher value without a death, A2Tools), new attempt detection (HP above the last known value more than 5 s after the last hit, RATmeter), possible boss when HP > 10 000 000 (NOIA2), player death in PvP when HP is 0 (NOIA2).

### 10.3 HP/MP update (`1B 92`, A2Tools only)

`<len> 1B 92 <actor varint 100..=9 999 999> <hp varint> <hp max varint 1..=50 000 000>`; stores max HP.

### 10.4 Map load (`21 36`)

`<len> 21 36 <u32 LE load count> <u32 LE map id> ...`, 52 bytes on every load (A2Tools, chen-zilei). Map ids 600 000..699 999 are dungeons (instances) and equal the roster dungeon id; open world maps are listed in `open_world_maps.json` (287 ids, A2Tools). A second load of the same map is an in map teleport (chen-zilei). A load clears party scope counts (A2Tools) and ends every abnormal except the local player's (A2Tools timeline); a different map id finishes the encounter and clears NPC and boss sets because ids are reissued (chen-zilei).

### 10.5 Zone change (`23 36`)

`<len> 23 36 <entity varint> <x f32> <y f32> <z f32> ...`; only entity 0 (the local player teleported) counts as a zone change; the reset it drives is gated by a 1500 ms damage lull and a 4000 ms debounce and clears only combat aggregates (A2Tools).

### 10.6 Other signals

- `21 8D` battle toggle (TK, KR): `<entity varint> <varint> <toggle varint>`; 1 = the boss entered combat, 0 = left. Not observed by any Global repo.
- `04 8D` with all zero fields: a despawning summon (A2Tools); RATmeter counts a `04 8D` naming an identified player as a player death.

### 10.7 Disagreements

| # | Claim | A2Tools | chen-zilei | RATmeter | NOIA2 | TK | Recommendation |
|---|---|---|---|---|---|---|---|
| D85 | Death flag semantics | 1 loaded dead (not an event), 3 death, 7 gone | 1 or 3 = dead | n/a | n/a | n/a | Death event only on 3; mark gone on 7; ignore 1 except to mark the entity dead at zone init. |
| D86 | Remaining HP width | u32 followed by four required zero bytes | not decoded | u64 | u32 <= 1e9 | u32 | u64 LE; validate <= 100 000 000 for a boss bar and treat the four high bytes being non zero as a misparse. |
| D87 | Who gets an HP record | NPCs only (`02 01 00`); the local player's `01 01 01` record is recognised but skipped | n/a | NPCs; unknown entities created only when hp >= 1 000 000 | mobs, and players in PvP mode | bosses only (table lookup) | Store for every entity; use the (a, b, c) triple as a diagnostic for player versus NPC. |
| D88 | Zone change signal | `23 36` entity 0 with lull and debounce; `21 36` for the dungeon id | `21 36` different map id | none | `33 36` (self record) resets | none | `21 36` with a different map id ends the encounter and marks ids as reissued; `23 36` entity 0 is a secondary teleport signal. |
| D89 | Max HP source | spawn HP pair, `1B 92`, raised by live HP | spawn pair (ignored by the tracker) | spawn (first varint after `01`) and `00 8D` when it exceeds total + 10 000 000 | `00 8D` max observed | never populated | Spawn pair first, `1B 92` second, raise to any observed current HP. |
| D90 | Header offset for `00 8D` | scanned anywhere | n/a | hard coded 3 (1 byte varint assumed) | parsed after the prefix | parsed after the prefix | Parse after the real prefix; never assume a 1 byte length. |

### 10.8 Unknowns

- The three varints before the HP (values 2,1,0 and 1,1,1 observed) and the zero second varint of `42 36`.
- The remaining 44 bytes of the map load record (likely position and instance data; `f7 e6 46 46 ...` look like floats).
- Whether `1B 92` carries MP as well as HP.
- Whether `21 8D` exists on Global.

---

## 11. Party roster packet (`02 97`)

### 11.1 Consensus layout

Header (A2Tools, chen-zilei, RATmeter agree byte for byte):

```
02 97
party_key     u32 LE
party_name    <len u8 1..=40> <utf8>
party_size    u8 (1..=12)
dungeon_id    u32 LE   (the instance the party is queued for or inside, including difficulty tier: Ferocious Horn Den is 600091/600092/600093 for Exploration / Conquest Normal / Conquest Hard)
unnamed       u8, u8
leader_dbid   u64 LE   (low u32 = global character id, bits 48..63 = world/server id)
unnamed       u8, u8, u8   (RATmeter: one "bit" byte plus two bytes)
member_count  varint (1..=12)
```

Member record, repeated `member_count` times (A2Tools layout, RATmeter mask bits, chen-zilei names only):

```
presence_mask u8      (0 = vacant slot)
slot          u8      (1 based)
dbid          u64 LE  (low u32 = global character id; high u16 = world id)
nickname      <len u8 1..=40> <utf8>   (len 0 = vacant slot)
class         u32 LE  (section 8.2)     RATmeter: "unnamed u32"
level         u32 LE  (1..=200)
[mask & 0x01: conqueror_level u32 LE]  (RATmeter)
gear_score    u32 LE  (0..=1 000 000)
[mask & 0x02: ready 1 bit]             (RATmeter `ReadBit`)
login         1 bit, always present     (RATmeter `ReadBit`; its bit reader shares the buffer opened by the header's `ReadBit`, so the first seven bit reads consume no byte, which is why the byte layout of the A2Tools capture still lines up)
[mask & 0x04: born_server u16 LE]      (RATmeter; A2Tools anchors on this: it repeats the dbid's world id)
[mask & 0x08: current_server u16 LE]   (RATmeter; A2Tools: "a second u16")
party_role    u8                        (A2Tools: "one tag byte")
combat_power  u64 LE  (<= 100 000 000)
tail          variable (see D92)
```

- A vacant slot is `00 <slot> 00 x9 ...` (A2Tools): mask 0, slot, zero dbid, zero name length, and the fixed fields zero; in the 15 August 2026 capture a vacant record is 36 bytes with the byte at offset 23 equal to 0x04 (the role byte, which confirms that the two server u16s are absent when mask bits 0x04 and 0x08 are clear).
- The stretch between gear score and combat power is not fixed width; A2Tools finds the member's world id (top u16 of the dbid) within 10 bytes after the gear score and reads combat power at `anchor + 5`. The record tail is variable too; the next member is reacquired by its header within 32 bytes: `<mask> <slot = expected> <dbid whose high u16 is 1..=9999> <len 1..=40> <utf8>`; members after vacant slots are found within `48 * gap` bytes. chen-zilei likewise searches the next member within 64 bytes using the server id and a valid name.
- `complete` when a vacant slot ends the list or the last slot equals the count; a complete roster replaces the previous one (A2Tools). Going from 2+ members to 1, or a roster that no longer lists you, ends the party and resets combat (A2Tools).
- Members are matched to entity ids by NAME only; the dbid is an account level id unrelated to session entity ids (A2Tools, chen-zilei). RATmeter can also link through `20 36` (global id = dbid low u32). Unnamed members can be matched by class when exactly one roster member of a class is unnamed and exactly one unnamed fighting player has that class (A2Tools).
- The roster's dungeon id is the party's queue, not the player's location; the current dungeon comes from the last map load (A2Tools).
- The roster also rides inside other packets and bundles; A2Tools and chen-zilei scan for `02 97` at any offset of a buffer of at least 32 (A2Tools) or 40 (chen-zilei) bytes and accept the first offset where the header validates.

### 11.2 Disagreements

| # | Claim | A2Tools | chen-zilei | RATmeter | NOIA2 | TK | Recommendation |
|---|---|---|---|---|---|---|---|
| D91 | The u32 after the nickname | class (four values per class) | class | unnamed, always present | not covered | not covered (join request carries a class u32) | Class. The A2Tools capture shows 6 (Gladiator) for a member and 0 for vacant slots. |
| D92 | Member tail after combat power | variable; reacquire the next member by header | variable; search within 64 bytes | ticket count varint + count x (u8, u32), [mask & 0x10: u64], mentoring u8, latency u8 | not covered | not covered | Do not trust a fixed tail. RATmeter's ticket list does not fit the A2Tools capture of 15 August 2026, where every filled member ends `01 01` right after combat power and a vacant member ends with four zero bytes. Reacquire by header as A2Tools does, and record the tail bytes for later decoding. |
| D93 | Header bytes between leader dbid and member count | 3 unnamed bytes | 3 bytes | one bit (consumes a byte) + 2 bytes | n/a | n/a | 3 bytes; RATmeter's bit reader never aligns, so its first `ReadBit` is the first of those 3 bytes. |
| D94 | Party name length cap | 1..=40 | <= 64 | u8 string | n/a | n/a | 1..=40. |
| D95 | How the roster is used | party membership, party end, placeholders, class matching | `is_ours` name set | global player records with level, gear, combat power | not covered | join request panel only | Party membership by name, plus level, class, gear and combat power for the member rows. |
| D96 | dbid split | high u16 = world id | top u16 = server | low 32 = global id, bits 48..63 = server, bits 32..47 unused | n/a | n/a | Both: low u32 global id, high u16 world id. |

### 11.3 Unknowns

- Presence mask bits 0x20 and 0x40 (declared by RATmeter, never tested; it tests 0x01, 0x02, 0x04 and 0x08 in the member record and 0x10 in the tail), the role byte values, the "network latency state", the tickets, the two bytes after the dungeon id and the three after the leader dbid.
- Whether `00 36` scope records (`06 38`) and the roster agree on party membership on Global.

---

## 12. Buffs / abnormals and casts

### 12.1 Consensus layouts (A2Tools `abnormal.rs`, with the partial readings of RATmeter, NOIA2 and TK mapped onto them)

```
<len> 2A 38 <entity varint> 01 <flags u8> <instance varint> <abnormal u32 LE> <length i64 LE> <end i64 LE>
            <caster varint> <level u8> [flags & 2: <skill u32 LE>] <u8> <x f32> <y f32> <z f32>        added
<len> 2B 38 <entity varint> <flags u8> <instance varint> ... the same from <abnormal> on                changed
<len> 2C 38 <entity varint> <n u8> (<kind u8> <instance varint> <reason u8>
            [kind & 7 == 7: <by entity varint> <skill u32 LE> <skill effect u32 LE>])*                  removed
<len> 4A 36 <entity varint> <n u8> (<stat u16 LE> <value i32 LE>)* <8 bytes>                           stats changed
<len> 49 36 <2 bytes> <n u8> (<stat u16 LE> <value i32 LE>)* <8 bytes>                                 all stats
<len> 41 36 | 45 36 | 33 36 <entity varint> ... <n u8> (<entry>)* 07 02 | 0F ...                       list at spawn
  entry: <flags u8 in {0x10,0x11,0x13}> <instance varint> <abnormal u32> <length i64> <end i64>
         [flags & 1: <caster varint>] <level u8> [flags & 2: <skill u32>] [<u8>] <x f32> <y f32> <z f32>
```

- `abnormal` is a SkillAbnormal id (1..1e9); `skill` the skill that applied it; `level` that skill's level; `instance` is numbered per entity, one per stack.
- `length` is the duration in ms (NOIA2 and TK read its low u32 and skip the high 4 bytes, which is the same i64); `end` is the server clock in ms since the epoch (NOIA2 and TK read it as `server_time u64`). A permanent abnormal has `length == -1` (`FF FF FF FF FF FF FF FF`) and `end == 4 102 412 400 000` (`80 75 D5 2A BB 03 00 00`, 2100-01-01 in Korea), which NOIA2 and TK see as `duration == 0xFFFFFFFF` and ignore. This pair is the "summon boundary" anchor of section 9.
- The byte before the position is 2 when a change reapplied the abnormal (`restarted`). Add and change records end with exactly 12 bytes after that byte; removals end exactly; stat records end with 8 unnamed bytes.
- `2B 38` lacks the `01` byte after the entity id; NOIA2 discovered this empirically as "retry with 1 byte skipped instead of 2".
- Removal reasons: 1 timer ran out; kind 7 with reason 11: a skill took it off; 5: a passive replaced by its new level at login.
- The spawn list is found by trying every start offset and both entry lengths (with and without the optional byte) ; the search is abandoned as soon as zero or more than four readings survive an entry, and the list is kept only when exactly one reading remains at the end with every entry well formed and ending at the expected bytes; timed entries need `0 <= length <= 400 days`, `2024-01-01 <= end < NEVER` and `end - length >= 2024-01-01` (the start must be after EARLIEST).
- Stats are `EStat` ids; percent stats in hundredths. Known ids (RATmeter `StatIds.cs`): AttackGear 317, AttackBase 19, AttackMax 31, AttackMin 33, AttackIncrease 425, AttackPve 56, AttackBoss 50, DamageBoost 28, WeaponDamageBoost 32, PveDamageBoost 379, BossDamageBoost 520, CriticalDamageBoost 44, Perfect 442, Smite 443, FrontDamageBoost 589, BackDamageBoost 102, CongniDamageBoost 483, FeraDamageBoost 485, NatureDamageBoost 487, VarienDamageBoost 489, CombatSpeed 282. A2Tools example: Spirit's Benediction sets 65, 76, 379, 380.
- Skill cooldowns `47 38` (RATmeter): `<count u8> (<skill u32 LE> <ms left varint>)*`.
- Casts: no repo decodes a cast packet. chen-zilei names `02 38` as the cast opcode; the usable signals are the no value `04 38` record (section 5, D37) and A2Tools' compact skill context marker. Hit timestamps per skill serve as cast lanes (A2Tools).

### 12.2 Timeline logic (A2Tools, the only full implementation)

- Live instances keyed by (entity, instance). A map load ends everything except the local player's; a `42 36` with flag 3 or 7 ends that entity's.
- Added: a reused instance number ends the old instance; `end - length` versus capture time gives `lag_ms` clamped 0..1000; stack limits from `abnormals.json` push out the oldest stacks.
- Changed: a different caster ends the instance as Recast and restarts it under the new caster; an instance never seen is dated `end - length + lag`.
- Listed (spawn or self record): instances absent from the list end as Unseen at their server end plus lag, or NotListed.
- Removed: Expired (reason 1), TakenOff (kind 7 with skill), Removed(reason).
- Instances 2 s past their server end with no record close as Unseen. Retention 30 min.
- Simpler models: RATmeter and TK store `(applied_at, duration)` intervals and merge overlaps at report time; NOIA2 extends an interval when a new application starts within 100 ms of the previous end.

### 12.3 Disagreements

| # | Claim | A2Tools | chen-zilei | RATmeter | NOIA2 | TK | Recommendation |
|---|---|---|---|---|---|---|---|
| D97 | `2B 38` meaning | changed | not covered | applied, same layout as `2A 38` | applied, one byte shorter after the target | applied | Changed; it carries no `01` byte, which is what the others observed as "shorter". |
| D98 | Fields between the abnormal id and the caster | `length i64`, `end i64` | not covered | `duration u32`, 4 bytes, `timestamp u32`, then caster (4 bytes short) | `duration u32`, 4 skipped, `server_time u64`, then actor | same as NOIA2 | i64 length and i64 end. RATmeter's caster is read 4 bytes early and is wrong. |
| D99 | Abnormal id to skill | `abnormals.json` and `i18n/abnormals` tables | not covered | `/ 10` then looked up in `skills.json` (must be >= 10 000 000 with a class) | range filter 110 000 000..=200 000 000 or 20 000 000..<30 000 000 | range filter 110 000 000..190 000 000 or 20 000 000..<30 000 000 | Keep the abnormal id as the key and name it from an abnormal table; the applying skill is a separate field (`flags & 2`). |
| D100 | Permanent abnormals | length -1, end NEVER, tracked as passives | n/a | duration must be 100..=3 600 000 ms | ignored (`0xFFFFFFFF`) | ignored | Track them (they identify class passives and summon ownership) but exclude them from uptime. |
| D101 | `49 36` count field | u8 | n/a | varint | n/a | n/a | u8 (identical below 128). |
| D102 | Buff removal | `2C 38` decoded with reasons | n/a | expiry by duration only | expiry by duration | expiry by duration | Decode `2C 38`; durations alone overstate uptime when a buff is dispelled. |
| D103 | Cast signal | no value `04 38` record and compact context | layout 0 cast marker in `04 38`; `02 38` named | none | none | none | Use the no value `04 38` record; investigate `02 38` in captures. |

### 12.4 Unknowns

- Flags bits other than 1 and 2; the `01` byte of `2A 38`; the optional entry byte in spawn lists; removal reasons other than 1, 5 and 11; the two leading bytes of `49 36` and the 8 byte stat tails; the body of `02 38`; whether RATmeter's `type` byte equals A2Tools' flags byte.

---

## 13. Entity model and id spaces

### 13.1 Consensus

- Every entity id on the wire is a session scoped varint, usually 100..=9 999 999; ids below 100 exist (an Elementalist at id 48, A2Tools) and raid player ids run past 99 999. Ids are reissued on every zone change and reused once an entity is gone (entity 65746 was one player's pet and then another's spirit 14 minutes later, A2Tools; chen-zilei: "ids are reissued on a new map").
- Kinds and how they are established:
  - Player: named by a `45 36` / `44 36` spawn, a `33 36` self record, the character list, or the roster (authoritative), or an actor that used a player band skill (A2Tools: 11 000 000..=19 999 999, 3 000 000..=3 999 999, 100 000..=199 999; chen-zilei: 10 000 000..=19 999 999 with `(code / 10 000) % 100 != 0`; RATmeter: class and player status from the first two decimal digits of the skill code looked up in `classes.json` (10 Elementalist spirit, 11..19 classes) for any code length, with Theostone 3 000 000..=3 099 999 inheriting the actor's known class; its ranges [11e6, 20e6), [1e6, 10e6), [100 000, 200 000) and [29e6, 30e6) only gate `NormalizeSkillCode`) without having spawned as a mob or summon. NPCs use class skills too (solo instance companions, summons), so a spawned NPC never counts as a player (chen-zilei).
  - NPC / mob: an id that spawned via `41 36` with an NPC code (A2Tools, chen-zilei, NOIA2, TK). NPC skills are 7 digit codes 1 000 000..=9 999 999; players also fire some (item and godstone procs), so they only mark an actor not yet known as a player (chen-zilei). A mob using one on a player counts as damage received (A2Tools).
  - Summon / pet / spirit: an id with an owner, confirmed by the spawn parent key, inline name, abnormal anchor, caster field, `04 8D`, or a spirit link record (A2Tools); summons are never players (A2Tools, NOIA2: an id is either a player or a summon).
  - Effect entity (kind 0x1C): parented to the target, linked only by its inline caster name (A2Tools).
  - Boss: a spawned NPC whose template has `isBoss` (all five via their tables).
- Multiple ids per player: a player has a stable id, a combat id and a new one after a dungeon re bind (chen-zilei); the local player and others change entity id several times per fight (A2Tools). Name eviction with `same_character` moves damage and summon links to the new id; the canonical row for a name is the id with the most damage, pinned to the local id when present (A2Tools).
- Id folding: spawn (and death, chen-zilei) ids above 1 000 000 fold to `(id & 0x3FFF) | 0x4000`, giving 0x4000..=0x7FFF (16 384..32 767). Damage, HP, buff and identity ids are never folded.
- Party rows for members that have not fought are synthetic (A2Tools uses 90 000 000 + slot).
- Local identity priority (A2Tools `data_storage.rs`: `note_self_stats`, `note_loot_owner`, `note_party_scope`, `set_game_identity`): the self record and the own stats streak (`4A 36` x3) override each other, latest wins (the streak clears `local_identity_from_game` and the name; the next self record clears `local_from_stats`). Below them: a configured name already matched to an entity (including via the character list) blocks a loot owner vote (`04 8D`) for another name; an applied loot vote blocks the character list (which binds only while `local_identity_from_game` is false) and overrides a scope derived id, since `note_loot_owner` never checks `local_from_scope`; the party scope leader (`06 38`) applies only while nothing else has set the local id, or scope itself did.
- Across zones: keep nicknames, known players, summons and classes; clear only per segment aggregates (A2Tools); clear NPC and boss sets, keep players and names (chen-zilei); persist identities to disk for 45 minutes (chen-zilei).

### 13.2 Disagreements

| # | Claim | A2Tools | chen-zilei | RATmeter | NOIA2 | TK | Recommendation |
|---|---|---|---|---|---|---|---|
| D104 | Player skill bands | 11e6..=19 999 999, 3e6..=3 999 999, 100 000..=199 999 | 10e6..=19 999 999 with a non zero class digit pair | first two decimal digits of the code looked up in `classes.json` (10..19), any code length; Theostone 3e6..=3 099 999 inherits the actor's class; the four normalisation ranges [11e6,20e6), [1e6,10e6), [100 000,200 000), [29e6,30e6) only gate code rewriting | first two digits 11..19 | first two digits 11..18 | 10 000 000..=19 999 999 with prefix 11..19, plus 3 000 000..=3 099 999 (Theostone items) and 100 000..=199 999 (pet skills); prefix 10 is the Elementalist spirit (RATmeter) and 6 digit ids 100 510..=103 500 and 109 300..=109 362 are Elementalist (A2Tools). |
| D105 | Summon resolution depth | up to 16 parent hops | none (summon damage dropped) | 1 hop (late registration re stamps old hits) | 8 hops at snapshot, 1 at ingestion | 1 hop | At least 8 hops with cycle protection; re attribute hits recorded before the link arrived (RATmeter `TransferSummonDamage`, A2Tools name eviction). |
| D106 | What a bare summon counts as | summon, merged into the owner; orphan merge by class and power scalar at display time | NPC; its hits on monsters are dropped | nothing (dropped if the owner is unknown) | its own row until linked | its own numeric row | Keep the summon's damage under its own id until linked, then merge; never drop it. Implement A2Tools' orphan merge by class only as a display option. |
| D107 | Persistence of identities | until a manual flush; fuzzy names forgotten on reset | 45 minutes on disk | never cleared; unidentified session players purged after 10 min | metadata survives every reset | never (hard reset unreachable) | Keep for the process lifetime plus a 45 minute disk cache; drop an id's name when the game respawns the id as something else. |
| D108 | Fold application | spawn only | spawn and death | none | spawn only | none | Spawn and death (they come from the same family); never damage. |
| D109 | Classification of an id seen using a class skill before its spawn | unclassified and its damage removed when the spawn arrives | NPC wins over skill inference | class from the skill prefix, unknown class drops the record | player = has nickname and no mob code | user auto created with the id as its name | Spawn beats inference; when a spawn arrives for an id that was treated as a player by skill inference, reclassify and move its damage out of the player rows. |

### 13.3 Unknowns

- The origin of the fold and whether the folded id is really the same entity (no repo explains it).
- Whether an effect entity (0x1C) can be attributed without its inline name.
- How the server allocates the "stable" versus "combat" ids per player.

---

## 14. Encounter logic

### 14.1 Consensus (majority rules, with the variants)

- Fight start: the first counted damage record on a target opens that target's fight with `first_damage_time` = the packet's timestamp (A2Tools, chen-zilei, RATmeter, NOIA2). TK instead waits for the `21 8D` toggle from a boss that is in `mobs.json` and not a dummy.
- Grouping: per target entity (A2Tools `TargetCombatData`, RATmeter `TargetEntry`, NOIA2 per target maps, TK `currentTarget`); chen-zilei keeps one encounter across targets.
- Fight end:
  - Idle timeout on the target: 30 000 ms since the target's last damage starts a new segment (A2Tools); 45 s (RATmeter, 10 s for dummies); 8 s normal, raised to 180 s while an instance boss that the encounter damaged is alive (chen-zilei); 5 s after the last hit for dummies only (TK); none (NOIA2).
  - Boss death: chen-zilei finishes the encounter when a living boss it damaged gets a `42 36` with flag 1 or 3 (its parser treats both as dead); A2Tools marks the entity dead on flag 3 only and the segment then ends by idle; TK ends on toggle 0; RATmeter and NOIA2 do not react.
  - Zone change: `21 36` to a different map (chen-zilei), `23 36` entity 0 after a 1500 ms lull with a 4000 ms debounce (A2Tools).
  - Party end (roster shrinks to one or no longer lists you) resets combat (A2Tools).
  - Manual reset (A2Tools, chen-zilei hotkey, NOIA2 overlay).
  - Saving: fights under 5 000 ms or without damage are skipped; a fight is saved once "ended" (10 000 ms since its last hit) and, while still running, as periodic snapshots once at least 15 000 ms long (A2Tools); boss sessions persisted on completion (RATmeter); targets above 1 000 000 damage kept (NOIA2).
- Boss detection: `isBoss` from the NPC table at spawn (A2Tools, chen-zilei, RATmeter, TK; NOIA2 `boss_ids.json`). RATmeter also treats `isDummy` as boss; NOIA2 can promote a mob whose HP exceeded 10 000 000. The first hit on a boss while the segment holds only trash clears the trash (A2Tools).
- "Ours" / only my fights: any fight in an instance; otherwise a fight where the local player, a summon of theirs, or a named party member hit the target (A2Tools). chen-zilei: once the self id appears in a hit, the current encounter is discarded and from then on a hit counts if the target is ours, or the target is engaged, or the actor is engaged and the target is a player (a mob in your fight hitting another player pulls that player in). TK: on a training dummy only, the live report is not refreshed unless the local player is among the contributors; bosses are never filtered.
- Wipe / reset: no explicit packet. Signals used: the 180 s instance boss idle rule (chen-zilei), boss HP above the last known value more than 5 s after the last hit (RATmeter `IsNewTry`), player HP 0 by `00 8D` (NOIA2 PvP only), the zone change rules. No repo detects a wipe from player deaths: chen-zilei only records death recaps (`DeathRecap`) on a `42 36`, so the "every party member dead" rule in D114 and 19.3 is a new recommendation.
- Dummy handling: dummies from `isDummy`, the names "Training Scarecrow" / "Punching Bag", a code list, or the HP 1 then up behaviour (A2Tools); DoT ticks after the last direct hit are held so the fight time ends at the last direct hit (A2Tools); 60 s cap option (RATmeter); 5 s idle (TK); dummy fights never uploaded (A2Tools, NOIA2 requires 60 s).
- Dungeon vs open world: map ids 600 000..699 999 are instances (A2Tools, chen-zilei); `dungeon_of_map(roster_dungeon, map)`: no map seen -> roster's id; open world map -> 0; map in the dungeon band or equal to the roster id -> that map (A2Tools). Outside instances a boss gets the normal idle timeout (chen-zilei).
- Grouping into a dungeon run: no repo links the bosses of one clear; each boss is its own record with `dungeon_id`, `server_id`, `mob_code` (A2Tools), or a flat history (chen-zilei, RATmeter, NOIA2, TK).
- Active target for display: the most recently hit boss that is ours; in a dungeon with no boss yet nothing; in the open world the mob with most damage that you or your party hit (A2Tools); the target the user hit most recently within 5 s, else the most hit (RATmeter); the main actor's last target (NOIA2).

### 14.2 Disagreements

| # | Claim | A2Tools | chen-zilei | RATmeter | NOIA2 | TK | Recommendation |
|---|---|---|---|---|---|---|---|
| D110 | Fight start trigger | first damage on the target | first counted damage event | first damage on the target | first accepted damage | `21 8D` toggle 1 from a boss | First counted damage. The toggle is KR only and unverified on Global. |
| D111 | Idle timeout | 30 s per target | 8 s, 180 s for a live instance boss | 45 s, 10 s dummies | none | 5 s dummies only | 30 s per target for trash and open world, 180 s for a live boss inside an instance (phase changes and invulnerability stretches are longer than 30 s), 10 s for dummies. |
| D112 | Boss death ends the fight | marks dead on flag 3 only; segment ends by idle; dead summon ticks held | yes, immediately, on flag 1 or 3 | no | no | toggle 0 | Yes: `42 36` flag 3 on the boss closes the encounter at that timestamp; late DoT ticks after death are dropped. |
| D113 | Zone change handling | `23 36` entity 0 with lull and debounce; map load clears scope | `21 36` different map finishes; same map ignored | none | `33 36` resets | none | `21 36` with a changed map id finishes every open encounter and marks ids as reissued; a same map load is a teleport and does nothing. |
| D114 | Wipe detection | none explicit | 180 s idle in an instance | boss HP rising after 5 s quiet | none | none | Open a new attempt when the boss's `00 8D` HP rises back to its max after at least 5 s without hits, or when every party member is dead (`42 36` flag 3 for all known party ids), or on the 180 s idle rule. |
| D115 | Dummies are bosses | no (separate dummy flag) | no | yes (`IsBoss || IsDummy`) | separate dummy codes | separate flag | No; dummy fights are training sessions with their own capping and are never uploaded. |
| D116 | Trash clearing on the first boss hit | yes | no (one encounter) | no (per target) | no | no | Yes, when the display is per boss. |
| D117 | Only my fights | "ours" rule by local id, summon or party member; any fight in an instance | option (default on): discard what was counted before you joined | none | `my_muzhuang_only` for dummies | on a dummy only: the report is not refreshed unless the executor (local player) is among the contributors; no such filter for bosses | A2Tools' "ours" rule: in an instance everything counts; in the open world only targets hit by you, your summons or a party member. |
| D118 | Heals and encounter time | inside the window only | never extend | n/a | n/a | n/a | Never extend. |

### 14.3 Unknowns

- No repo found an explicit encounter start, end or wipe packet; `21 8D` (TK) is the closest and is unverified on Global.
- Whether the server sends anything on a boss reset (HP refill) besides `00 8D`.
- How to link the bosses of one dungeon clear (no repo does it; the map load count u32 in `21 36` may enumerate loads within a session and could serve as a run key).

---

## 15. Metrics

### 15.1 Consensus

- Battle time: `last_damage_time - first_damage_time` on the target or encounter, floored at 1000 ms (A2Tools `max(battle_time, 1000)`, chen-zilei `max(1.0, duration_s)`); DPS per row = `damage / battle_time`. No repo computes an "active time" DPS that excludes gaps.
- Damage share: `row damage / total damage over the selected targets * 100` (all five).
- Party DPS: the summed damage of the counted player rows divided by the encounter duration (chen-zilei `party_dps` = damage of every row with `is_player`, strangers counted into the encounter included, / `duration_s`; RATmeter `GetPartyDps` = sum of the active target session's `PlayerStats` / that session's combat duration; NOIA2 overlay footer); A2Tools exposes `total_damage` and `target_total_damage` and leaves the division to the UI.
- Per skill: hit count (hits with damage), total, min, max, crit count (hit type 3), back and front counts (angle byte), shield block, parry, perfect, double, iron wall, regeneration, perfect block counts (plotter flags), miss and resist counts (no value records), multi hit records, additional hits and their damage, life steal (A2Tools, the superset). Rates are `count / hit_count` computed in the UI. DoT rows are keyed `(skill, dot)` and shown as separate rows (`<skill> - DOT`, A2Tools; `<name> - 지속`, TK).
- Casts are not counted separately by anyone; "times" is the hit count.
- Per actor: total damage, party heal, regen, damage received, hits received, last damage time, class (A2Tools); healing and HPS (A2Tools, chen-zilei); deaths (RATmeter, chen-zilei recaps).
- Boss HP bar and time to kill: `target_max_hp` / `target_current_hp` with a 20 s rate window (A2Tools).
- Berserk timer: not covered by any repo (NOIA2 remaps Fighter berserk skill ids to their base skills, `FIGHTER_SKILLID_MAP`).
- Summon damage is merged into the owner at display time (A2Tools, NOIA2, RATmeter `GroupSummonDamage`).

### 15.2 Disagreements

| # | Claim | A2Tools | chen-zilei | RATmeter | NOIA2 | TK | Recommendation |
|---|---|---|---|---|---|---|---|
| D119 | Duration base for a player's DPS | target's first to last hit (any actor) | encounter's first to last counted hit | the player's own first to last hit (`max(…, 0.1 s)`) | player's first hit on the target to the last hit by anyone | battle start to end (toggle or last record) | Encounter duration (first to last hit on the boss by anyone) for every member, so that the shares and the party DPS are consistent; show the per player active span as a secondary number. |
| D120 | Minimum duration | 1000 ms | 1 s | 0.1 s | 1 s | none | 1 s. |
| D121 | DoT hits in the hit count | separate rows | included | excluded from `HitCount`, included in totals | counted as records | separate rows | Separate rows keyed `(skill, dot)`; the actor total includes them. |
| D122 | Multi hit accounting | `multi_hit_count` records, `multi_hit_hits`, `multi_hit_damage` | not read | not covered | `MULTIHIT`, `MULTIHIT{n}`, `MULTIHITDMG` | `multiHitTimes` only | A2Tools' three counters. |
| D123 | Crit rate base | crits / hits with damage | crits / hits | crits / non DoT hits | crits / records | crits / non DoT times | Crits divided by direct hits with a value (exclude DoT rows and misses). |
| D124 | Summon rows | merged into the owner; orphans merged by class or power scalar | dropped | merged when the skill also occurs for the owner, else grouped | merged with skill specs folded | merged one hop | Merge into the owner with a child breakdown; keep orphans visible under their own id. |

### 15.3 Unknowns

- An active time DPS definition (gap excluded) that the community agrees on.
- Berserk / enrage timers (not present in any packet decoded so far).

---

## 16. Game data tables

### 16.1 Consensus sources and shapes

The tables originate from one community extraction (credited to taengu / A2Tools and Karim) and are consumed by A2Tools, chen-zilei (downloaded from A2Tools commit `82e53c1008ac4c2974446cc703703f473bbbed81`) and RATmeter. Recommended shapes (A2Tools `src/data/`):

| File | Shape | Size (A2Tools) | Used for |
|---|---|---|---|
| `i18n/skills/<lang>.json` | `{"<skill code>": "<name>"}` | 9268 entries in `en`; de, en, es, fr, ja, ko, pt, ru, zh-Hans, zh-Hant | Skill names; lookup maps 3 000 000..=3 099 999 to `code*10+1`; fallback to the base code `code - code % 10000` (chen-zilei, A2Tools) |
| `i18n/npcs/<lang>.json` | `{"<npc code>": {"name", "isBoss": bool, "isDummy"?: bool, "dungeonId"?: int, "category"?, "tier"?}}` or a bare string | 9780 entries; 1412 `isBoss`, 164 `isDummy`, 563 with `dungeonId` | Boss and dummy flags, dungeon of a boss; RATmeter's copy has 9869 entries, 1340 `isBoss` |
| `i18n/dungeons/<lang>.json` | `{"<dungeon id>": {"name", "difficulty"?: normal / hard / exploration}}` | 60 | Dungeon names |
| `i18n/abnormals/<lang>.json` | `{"<abnormal id>": "<name>"}` | 3574 | Abnormal names |
| `abnormals.json` | `{"source", "abnormals": {"<id>": {"icon"?, "stacks"?: n}}}` | 3839 | Stack limits for the buff timeline |
| `dot_skill_ids.json` | flat array of ints | 2641 (A2Tools), 1540 (RATmeter) | DoT allowlist; ids come in runs of ten consecutive values (12090020..12090029, 100500..100509, 20..29) |
| `open_world_maps.json` | `{"source", "maps": [int]}` | 287 | Open world versus instance |
| `player_summon_npcs.json` | `{"source", "npcs": [int]}` | 186 | NPC codes that are player summons |
| `resource_restore_skills.json` | `{"source", "skills": [int]}` | 61 | Skills that restore only non HP resources |
| `servers.json` | `{"regions": {lang: {code: name}}, "servers": {"<id>": {lang: name}}}` | 616 servers, regions AS, EU, JP, LA, NAE, NAW | Server names |
| `skill_icons.json` | `{"<skill code>": "<icon file>"}` | 898 | Icons |
| `classes.json` (RATmeter) | `{"classes": [{"id", "name", "icon"}]}` | 10 Elementalist spirit, 11 Gladiator, 12 Templar, 13 Assassin, 14 Ranger, 15 Sorcerer, 16 Elementalist, 17 Cleric, 18 Chanter, 19 Brawler | Class by skill prefix |
| `healing_skill_ids.json` (RATmeter), `HEAL_SKILLS` (chen-zilei), `healing_skill_code.json` (NOIA2) | arrays / maps of skill ids | 8 / 7 / 29 | Heal classification (section 7) |

Hard coded tables worth carrying over:

- Training dummy codes (A2Tools `TRAINING_DUMMY_CODES`): 2300229, 2300919, 2310229, 2310919, 2320229, 2320919, 2400032, 2400035, 2400392, 2500075, 2500076, 2701376, 2090773, 2702605. NOIA2's `mobs.json` flags 2090773 (also boss), 2300229, 2300919, 2310229, 2310919, 2320229, 2320919, 2400032, 2400392, 2500075, 2500076, 2701376, 2702605; its Rust constants are 2400032 and 2400035. RATmeter: 2000000 "Punching Bag", 2090773 "Training Scarecrow (Specimen)" (boss and dummy), 2300229 "Training Scarecrow".
- Class by skill prefix (first two digits of 8 digit ids): 11 Gladiator (검성), 12 Templar (수호성), 13 Assassin (살성), 14 Ranger (궁성), 15 Sorcerer (마도성), 16 Elementalist (정령성), 17 Cleric (치유성), 18 Chanter (호법성), 19 Fighter / Brawler (권성) (A2Tools, RATmeter, NOIA2; TK predates the class and maps only 11..18). A2Tools' `convert_from_skill` adds: 6 digit ids 100 510..=103 500 and 109 300..=109 362 are Elementalist; `sub = (id/10000) % 100 == 0` (generic skills) yields no class unless prefix 16 and `(id/100) % 100` in 11..=13; prefix 16 is restricted to a sub list. NOIA2 ignores codes starting with 1134 and 1740 for class inference (a cleric skill and a gladiator blood drain misled it).
- Skill id normalisation (A2Tools `normalize_skill_id`): `base = id - id % 10000`; use `base` when the table names `base` and either does not name `id` or names it the same; 30 000 000..=30 999 999 untouched. Specialisation flags: `suffix = (id % 10000) / 10`, each decimal digit 1..=5 sets slot `digit - 1`; any other digit clears all (A2Tools, RATmeter). Player skill codes are 8 digits: class x 1 000 000 + skill x 10 000 + level / specialisation (chen-zilei).
- Spirit link bands 16 990 000..=16 999 999 and 16 770 000..=16 779 999; compact aggregate skill 99 745 942; Divine Aura 17 153 450 (A2Tools).
- Known NPC codes used in tests: 2000002 "Draconute Ranger", 2000000 "Punching Bag" (dummy), 2400101 "Field Boss" (chen-zilei); 2920063 Blazing Totem, 2920149 Wind Spirit, 2920548 (A2Tools); 2300001 "Deep Sea Pydeon" (boss, Transcendence, Stage 2, dungeonId 610011, RATmeter).
- Map ids seen: 600021 Fire Temple, 1010 World_L_A (A2Tools); NOIA2 field boss maps 1010, 1110, 1011, 1111, 20, 22.

### 16.2 Disagreements

| # | Claim | A2Tools | chen-zilei | RATmeter | NOIA2 | TK | Recommendation |
|---|---|---|---|---|---|---|---|
| D125 | NPC table shape | object keyed by code with `isBoss` / `isDummy` | same (downloaded) | same | array of `{code, name, boss, isDummy?}` plus `boss_ids.json` | array of `{code, name, boss}` with an ambiguous `isDummy` | A2Tools' object shape; accept a bare string value and a missing `isDummy`. |
| D126 | Fighter / Brawler naming and prefix 19 | Fighter, prefix 19 | n/a | Brawler, id 19 | FIGHTER, prefix 19 | not present (pre Fighter) | Prefix 19 = Fighter; the English client calls the class Fighter in A2Tools' tables. |
| D127 | Elementalist spirit skills | 6 digit ids 100 510..=103 500 and 109 300..=109 362 | n/a | class 10 "Elementalist spirit" for prefix 10 | `100_010..=100_059` folded to `(code / 10) * 10` | n/a | Treat prefix 10 and the 6 digit bands as Elementalist spirit skills that attribute to the Elementalist owner. |
| D128 | Theostone id handling | packet 3 000 000..=3 099 999 -> `*10 + 1` | n/a | table key `30000xxx / 10` | `*10 + 1` | n/a | `*10 + 1` then look up; keep 30 000 000..=30 999 999 unnormalised. |
| D129 | DoT allowlist size | 2641 | none | 1540 | none | none | Ship the larger list, mark it as advisory (section 6, D59). |
| D130 | Dummy code list | 14 codes plus `isDummy` plus names plus HP behaviour | `isDummy` parsed, unused | `isDummy` (164) | 13 flagged, 2 constants | `isDummy` ambiguous | `isDummy` from the table plus the 14 code union plus the HP 1 then up behaviour. |

### 16.3 Unknowns

- An official list of which skills are heals, shields or resource restores (every list is hand maintained).
- The `category` and `tier` vocabularies of the NPC table beyond the observed values (categories in `i18n/npcs/en.json`: Nightmare 300, Dungeon 128, Transcendence 64, Ascension Trial 44, Raid 27; tiers, spelled exactly as in the file: `Conquest [Normal]` 43, `Conquest [Hard]` 40, Exploration 43, Level 1..10 (30 each), Stage 1..4 (16 each), Easy 10, Normal 15, Hard 15, Extreme 7, Insane 7, Hell 7, Ordeal 2, Doom 2).

---

## 17. Test vectors

Union of every hard coded byte vector in the five repositories, deduplicated, grouped by what a Rust unit test would exercise. Each vector is tagged with its source repo and the original test name. Hex strings are wire order. "Derived" marks bytes expanded by the source document from a test helper rather than copied from a literal. A2Tools' own numbering (A2Tools #n) is kept so the two documents can be cross read. RATmeter ships no tests (only the literal patterns listed under 17.13) and TK ships none (only the synthetic expectations under 17.14).

### 17.1 Varint

| Id | Source | Bytes | Expected |
|---|---|---|---|
| V1 | chen-zilei `reads_varints` | `0E AC 02 FF FF FF FF 0F` | three varints 14, 300, `u32::MAX`; offset ends at 8 |
| V2 | chen-zilei `truncated_varint_does_not_advance` | `80 80` | `None`, offset stays 0 |
| V3 | A2Tools #21 `an_id_is_read_whole_not_from_its_last_byte` | `varint_ending_at(&[0x01, 0x9A, 0x6D, 0xE2, 0x07], end 3, min 0, range 100..=99_999)` | 13978 |
| V4 | A2Tools #21 | `(&[0x01, 0xED, 0x74, 0x18, 0x05], 3, ...)` | 14957 |
| V5 | A2Tools #21 | `(&[0x01, 0x6D, 0xE2, 0x07], 2, ...)` | 109 |
| V6 | A2Tools #21 | `(&[0x01, 0xBD, 0x44, 0xE2, 0x07], 3, ...)` | 8765 |
| V7 | TK synthetic (`SP:651`) | `E8 07` | value 1000, length 2 |

### 17.2 Framing, bundles, padding, TLS

| Id | Source | Bytes | Expected |
|---|---|---|---|
| V8 | chen-zilei `heartbeat_encodes_like_the_wire` (derived) | `0e 00 36 00 00 00 00 00 00 00 00` | 11 byte frame, opcode `00 36`, 8 byte body |
| V9 | chen-zilei `splits_frames_across_segments` (derived) | `0e 00 36 00 00 00 00 00 00 00 00 | 0a 04 38 01 02 03 04 | 00 | 0e 00 36 00 00 00 00 00 00 00 00`, fed as `[..7]` then `[7..]` to an aligned decoder | opcodes `[00 36, 04 38, 00 36]`, second body `01 02 03 04` |
| V10 | chen-zilei `unpacks_lz4_bundles` (derived) | inner `09 04 38 09 09 09 07 05 38 07` (10 bytes) compressed as an LZ4 block, body = `0a 00 00 00` + block, wrapped as a frame whose opcode position is `FF FF` | opcodes `[04 38, 05 38]`, `bundles == 1` |
| V11 | chen-zilei `resyncs_on_heartbeat_after_joining_mid_stream` (derived) | `83 44 12 99 | 0e 00 36 00 00 00 00 00 00 00 00 | 07 04 38 05` with an unaligned decoder | opcodes `[00 36, 04 38]`, `dropped_bytes == 4` |
| V12 | A2Tools #27 `a_garbage_length_past_16_kb_without_a_bundle_header_still_resyncs` | `varint(40_000) + [12 34 00 00 00 00] + packet([23 36 07])` | `consumed > 0` |
| V13 | A2Tools #27 | `varint(40_000) + [FF FF 00 00 01 00]` | `consumed == 0` (waited for as a bundle) |
| V14 | A2Tools #27 | `varint(40_000) + [FF FF FF FF FF 7F]` | not a bundle header |
| V15 | A2Tools #28 `a_tls_record_between_packets_is_skipped_whole` | `packet([23 36 01]) + [17 03 03 05 7a] + [88 71] + 1400 filler bytes + packet(body(0x04, 200))` | both packets framed; cut at 604 bytes -> 1 frame, consumed 4 |
| V16 | A2Tools #29 `skips_zero_padding` | `[00 00] + packet([AA BB])` | one frame, payload `AA BB` |
| V17 | A2Tools #30 `stops_on_an_incomplete_trailing_packet` | `packet([01 02 03]) + [40]` | 1 frame; the `40` (claims 61 bytes) is left for the next read |
| V18 | A2Tools #31 `the_length_counts_the_payload_plus_four` | `varint(length_value(200)).len() == 2`, `varint(length_value(20_000)).len() == 3`; payload sizes 3, 122, 123, 124, 200, 1000, 16379, 16380, 20000 | all frame correctly with `size = len + n - 4` |
| V19 | A2Tools #7 `bundles_nest_four_deep_at_most` | `04 38` + `b1ea011600f30a40c0f40063028000010b199b5f01000000ac52d007`, framed, wrapped in `FF FF <u32 len> lz4` bundles 1, 4, 5 and 40 deep | read at depths 1 and 4, not at 5 and 40 |
| V20 | A2Tools #35 `a_tls_record_header_is_told_from_game_bytes` | `is_tls_record` true: `[17 03 03 05 7a 88]`, `[16 03 03 00 5d]`; false: `[34 21 36 01 00 00]` (map load), `[24 04 38 fe 9e 02]` (damage), `[17 03 03 05]`, `[17 03 03 ff ff]` | as listed |
| V21 | A2Tools #36 `a_tls_connection_from_the_game_port_is_left_out_whole` | from server 13328: client 50349 `[24 04 38 fe 9e 02]` admitted; client 50350 `[17 03 03 05 7a 88 71]` rejected; 50349 `[34 21 36 01]` admitted; 50350 `[88 71 04 38]` rejected; 50349 `[17 03 03 00 10]` admitted | as listed |
| V22 | A2Tools #37 `each_connection_has_its_own_stream_name` | `stream_key(13328, 50349)` | `"Client:50349:13328"` |
| V23 | A2Tools #39 `requests_are_only_found_in_cleanly_framed_payloads` | `[10] + [AB; 12]` is a ping request; also after the heartbeat `[0E] + [CD; 10]`; the heartbeat alone is not; 4096 bytes of `7F` with `[0] = FF` and `[100] = 10` is not | as listed |
| V24 | A2Tools #40 `relay_wrapped_request_is_recognised` | `[01 33 00 00 05 22 02 02 2B] + [05 23 0C 0F] + [CD; 11] + [05 25 01 01]` | a ping request |
| V25 | chen-zilei `splits_client_frames_by_their_length_prefix` | `hb = [0E 1 2 3 4 5 6 7 8 9 10]`, `ping = [0F 1 2 3 4 5 6 7 8 9 10 11]`; stream `hb ++ ping ++ hb` split at 15 | sizes `[11, 12, 11]`; after `reset()`, `ping` appends 12 |
| V26 | A2Tools #76 `slice_structure` | lifted `[09 E5 A2 04 38 01 02]`, bundle `[0A FF FF 00 00 00 00 00]` twice, damage `[08 04 38 01 02 03]` | (4 records, 1 lifted, 2 bundles) |
| V27 | A2Tools #77 `a_slice_with_no_damage_derives_nothing` | payload `[23 36 00]` framed as `[(len+1+3) as u8] + payload` | derives nothing |
| V28 | TK synthetic | `0B 04 38 ...` | L = 11, n = 1, frame 8 bytes, opcode at index 1 |
| V29 | TK synthetic | `0C F2 04 38 ...` | L = 12, frame 9 bytes, extension byte `F2`, opcode at index 2 |

### 17.3 Link layer and TCP

| Id | Source | Bytes | Expected |
|---|---|---|---|
| V30 | A2Tools #32 `ipv4_tcp()` | IP `[45 0 0 42 0 0 0 0 64 6 0 0 10 0 0 2 193 202 112 99]`, TCP `[C7 38 34 10 0 0 0 1 0 0 0 0 50 18 0 0 0 0 0 0]`, payload `b"hi"`; link headers: Ethernet `[0;12] + [08 00]` (type 1), VLAN `[0;12] + [81 00 00 05 08 00]` (1), `[2 0 0 0]` (0), none (12, 101), `[0;14] + [08 00]` (113), `[08 00] + [0;18]` (276); also type 999 with Ethernet and type 1 with raw IP via the guess | src 51000, dst 13328, dst ip 193.202.112.99, data `hi` |
| V31 | A2Tools #33 `ethernet_padding_is_not_payload` | a 40 byte ACK (`ack[3] = 40`) padded with `[be de 00 03 66 fd]` -> None; a 41 byte packet (`short[3] = 41`) padded with `[be de 00 03 66]` -> data `h`; total length 0 -> data `hi` | as listed |
| V32 | A2Tools #34 `non_ipv4_frames_are_ignored` | ARP `[0;12] + [08 06] + [0;28]` | None |
| V33 | NOIA2 `parses_ipv4_tcp_payload` | 44 byte IPv4/TCP: `[0] = 45`, total length 44 BE, `[9] = 6`, src 10.0.0.1, dst 10.0.0.2, src port 50000, dst port 10007, seq 12345 BE, `[32] = 50`, payload `33 36 AA BB` | src 10.0.0.1:50000, dst 10.0.0.2:10007, seq 12345, data `33 36 AA BB`; `normalized_connection(50000, 10007) == (10007, 50000)` |
| V34 | chen-zilei `reorders_and_trims_retransmits` | segments `(100,"abc"), (106,"ghi"), (103,"def"), (103,"def"), (104,"efgh")` | `"abcdefghi"`, 0 gaps |
| V35 | chen-zilei `handles_sequence_wraparound` | `(u32::MAX - 1, "ab"), (0, "cd")` | `"abcd"` |
| V36 | NOIA2 `holds_out_of_order_data_until_gap_arrives` | `feed(100, [1,2]) -> [[1,2]]`; `feed(104, [5,6]) -> []`; `feed(102, [3,4]) -> [[3,4],[5,6]]` | as listed |
| V37 | NOIA2 `drops_retransmits_and_trims_partial_overlap` | `feed(100, [1,2,3,4]) -> [[1,2,3,4]]`; `feed(100, [1,2,3,4]) -> []`; `feed(102, [3,4,5,6]) -> [[5,6]]` | as listed |
| V38 | NOIA2 `handles_sequence_wrap` | `feed(u32::MAX - 1, [1,2]) -> [[1,2]]`; `feed(0, [3,4]) -> [[3,4]]` | as listed |
| V39 | NOIA2 `trims_overlap_from_a_previously_held_segment` | `feed(100, [1,2])`; `feed(104, [5,6,7,8]) -> []`; `feed(102, [3,4,5,6]) -> [[3,4,5,6],[7,8]]` | as listed |
| V40 | NOIA2 `removes_a_held_segment_fully_covered_by_later_data` | `feed(100,[1,2])`; `feed(106,[7,8]) -> []`; `feed(104,[5,6]) -> []`; `feed(102,[3,4,5,6,7,8]) -> [[3,4,5,6,7,8]]`; `feed(108,[9])` yields a part equal to `[9]` | as listed |
| V41 | NOIA2 `keeps_candidate_with_recent_magic` / `removes_candidate_without_magic_for_more_than_thirty_seconds` | candidate `(10007, 50000)` observed at t; `remove_stale(t + 30 s)` keeps it; `remove_stale(t + 31 s)` removes it | as listed |
| V42 | chen-zilei `decodes_damage_end_to_end_from_ethernet_frames` (derived) | stream = heartbeat frame + `encode_frame(04 38, a8 46 06 00 07 e0 26 a8 00 01 01 00×11 b9 60 c4 13)` + `encode_frame(04 38, a8 46 06 00 07 e0 26 a8 00 01 03 00×11 b9 60 cc 3a)`, split at byte 20, two TCP segments (seq 1000, 1020) without SYN, with an unrelated port 443 packet `b"not the game"` between | `total_damage == 10_000`, `actors[0].crits == 1`, `heartbeats == 1` |

### 17.4 Damage records (`04 38`)

A2Tools' helper `feed(p, record)` builds `[len, 04, 38] + hex(record)` with `len = total length`. Results are `(total_damage, hit_count, multi_hit_count, multi_hit_hits, multi_hit_damage)` unless stated.

| Id | Source | Record bytes after `04 38` | Expected |
|---|---|---|---|
| V43 | A2Tools #6 `additional_hits_are_read_from_the_record_tail` | `b1ea013600f30a40c0f4007a038000010b199b5f01000000ac52a40d021818` | target 30001, actor 1395; (1700, 1, 1, 2, 48); layout 6, switch 0x36 |
| V44 | A2Tools #6 | `b1ea013400f30ae0b7f800cd028bd3276101000000ac52d330010159` | (6227, 1, 1, 1, 89); layout 4, switch 0x34 |
| V45 | A2Tools #6 | `fe9e0224009e9b01c3f8f5000302792b156002000000ac52e10301060200` | (481, 1, 1, 1, 6); spirit, layout 4 without the field |
| V46 | A2Tools #6 | `fe9e0224009e9b01c18601001702a7a2980001000000ac52e401030303030100` | (228, 1, 1, 3, 9) |
| V47 | A2Tools #6 | `fe9e0204009e9b01c3f8f5000302792b156003000000ac52b7030300` | (439, 1, 0, 0, 0) |
| V48 | A2Tools #6 (also V19) | `b1ea011600f30a40c0f40063028000010b199b5f01000000ac52d007` | (976, 1, 0, 0, 0); layout 6, switch 0x16 |
| V49 | A2Tools #11 `hit_flags_are_read_from_player_records` | `e9de020600c80b9147ff003f02020002aff4b76301000000e4506a0100` | target 44905, actor 1480, skill 16730001; (damage 106, crit 0, parry 1, perfect 0, double 0, frontal 1) |
| V50 | A2Tools #11 | `fcc2010600bf6bc759d1003203020002c711c75101000000904e660100` | target 24956, actor 13759, skill 13720007; (102, 1, 1, 0, 0, 1) |
| V51 | A2Tools #11 | `cac20406009e389147ff00e102040000aff4b76301000000a4587f0100` | target 74058, actor 7198, skill 16730001; (127, 0, 0, 1, 0, 0) |
| V52 | A2Tools #11 | `bfe6020600d23310ffd6006f0208000055a2fb5302000000aa56d20e0200` | target 45887, actor 6610, skill 14090000; (1874, 0, 0, 0, 1, 0) |
| V53 | A2Tools #12 `hit_flags_of_received_hits` (`read_plotter(record, 13)` -> flags, hp, angle; monsters hitting players) | `ca6f0600ca9804b8c112000102010002ebab530701000000904e4a0100` | ([ShieldBlock], 0, Some(2)) |
| V54 | A2Tools #12 | `fe2a0600c68d03b8c112000102100002ebab530701000000904e740100` | ([IronWall], 0, Some(2)) |
| V55 | A2Tools #12 | `df7e0600a3b103b0b512000302200d02cbf84e0701000000904e440100` | ([Regeneration], 13, Some(2)) |
| V56 | A2Tools #12 | `e9230600ee9e03e4cc120001024100021b09580701000000904e010100` | ([ShieldBlock, PerfectBlock], 0, Some(2)) |
| V57 | A2Tools #12 | `e06d0600aeb1024cba12000202420002bbc5500701000000904e010100` | ([Parry, PerfectBlock], 0, Some(2)) |
| V58 | A2Tools #13 `restoration_hp_is_a_varint` | `ed080600f0ef04bab51200010220b10102b3fc4e0701000000904ef9060100` | plotter at 13: flags 0x20, hp 177, hp_len 2, angle Some(2); varints at `13 + 1 + 2 + 1 + 8` read 10 000 then 889 |
| V59 | A2Tools #13 | `ee0d0600849d03bab51200010220cc0101b3fc4e0701000000904efc070100` | hp 204, angle Some(1); at `13 + 1 + hp_len + 1 + 8 + 2` reads 1020 |
| V60 | A2Tools #14 `misses_and_resists_count_without_damage` | `a8a3041400be77802a04015d02109aa06501000000dc51b80f010100` (feed true) then `a8a3040001be77802a04015d06139aa06501000000dc5101c9e1f5050100` (feed false) | target 70056, actor 15294, skill 17050240: (damage 1976, hits 1, resist 1, miss 0); target total 1976 |
| V61 | A2Tools #14 | miss `e981020000899803172df9000101079d556101000000904e0100` alone | creates no target; after a 500 damage hit by 52233 on 33001 it gives skill 16330007 (0, 0, miss 1, resist 0) |
| V62 | A2Tools #10 `a_meter_opened_mid_session_finds_you_in_party_scope_records` | damage `240438efc9010600b115c859d100fc0300000001c711c75101000000904eca050100` followed by 23 copies of scope `0e0638b115171dd200f700` | consumes the whole stream, `local_player_id == None`; one more scope record -> `Some(2737)`, `local_id_from_scope()` |
| V63 | A2Tools #3 `mp_restores_are_not_healing` (summons 61001 and 49482 linked to 10137; prior damage from 10137 and 2787 on 16720) | `994f0400c9dc03333f03010602f7af446501000000ac52670100` (Wind Spirit 16990003, 103 HP of 6871) | feed true, no heal, no combat for 10137 or 2787 |
| V64 | A2Tools #3 | `994f0400ca8203323f0301060293af446501000000ac52140100` (Water Spirit 16990002, 20 MP) | same |
| V65 | A2Tools #3 | `e3150400e315877af0005302c7dcef5d01000000865d1e0100` (self cast 15760007, 30 MP) | same |
| V66 | A2Tools #19 `a_pets_spawn_names_its_owner` | `a7b0010600c80b9147ff003f02020002aff4b76301000000e4506a0100` | feed true, actor 1480 unnamed |
| V67 | A2Tools #9 link record (`a_spirit_s_ticks_stop_counting_when_it_leaves`) | `ccc8010400fc7ad1e3ff000102affdf46301000000e65bf4010100` | links 25676 -> 15740 (link record, not damage) |
| V68 | chen-zilei `decodes_a_damage_record` (derived from `encode_damage(5001, 42, 11_020_000, 31_337, true)`) | `89 27 06 00 2a e0 26 a8 00 01 03 00 00 00 00 00 00 00 00 00 00 00 b9 60 e9 f4 01` | one `Damage` (actor 42, target 5001, skill 11 020 000, amount 31 337), crit |
| V69 | chen-zilei `follows_chained_records` (derived) | `89 27 06 00 2a e0 26 a8 00 01 01 00×11 b9 60 64 | 01 00 | 8a 27 06 00 2a e0 26 a8 00 01 01 00×11 b9 60 c8 01` | 2 events (100 on 5001, 200 on 5002) |
| V70 | chen-zilei `rejects_garbage` | `FF FF FF FF FF 00` as a `04 38` body | no events |
| V71 | chen-zilei `heal_skills_in_damage_packets_are_heals` (derived) | `4d 06 00 2a 43 7d 14 01 01 01 00×11 b9 60 a0 1f` | `Heal { actor 42, target 77, skill 18_120_003, amount 4_000 }` |
| V72 | chen-zilei (derived) | `4d 06 00 2a 31 3f 03 01 01 01 00×11 b9 60 a0 1f` | no events (skill 16 990 001 is a spirit link) |
| V73 | NOIA2 documented sample (`docs/...zh-CN.md:616-649`) | `04 38 91 C1 02 26 00 A1 14 48 F1 CA 00 20 02 0C 00 01 2C 40 46 4F 01 00 00 00 8C 91 01 B1 D1 14 04 97 36 97 36 97 36 97 36 01 00` | target 41105, actor 2593, skill 13300040 (base 13300000), damage 338097, hit count 4, hits 6935 x 4 (sum 27740), flags Perfect + Double, Back; scalar 18572 |
| V74 | NOIA2 documented tail (`:550-568`) | tail bytes `00 8C 91 01 CD FC 09 01 00` | v0 = 0, v1 = 18572, v2 = 163405, v3 = 1; damage 163405, hit count 1 |
| V75 | NOIA2 documented old tail (values only) | `v0 = 14150, v1 = 7312, v2 = 2, v3 = 149, v4 = 149, v5 = 1, v6 = 0` | pre June layout: damage 7312, hit count 2, per hits [149, 149] |
| V76 | A2Tools #63, #65, #66 blinding (bytes left alone) | `[04 38 fe 9e 02 04 00 9e 9b 01 02 d3 86 01 00]` (skill 100051); `[04 38 fe 9e 02 04 00 95 fa 02 50 77 f6 00]` (actor 48405, skill 16152400); `[04 38 fe 9e 02 04 00 9e 9b 01 02 51 28 f4 00]` (Water Bomb 16001105) | the skill bytes are not names |

### 17.5 DoT / HoT (`05 38`)

| Id | Source | Bytes (whole frame) | Expected |
|---|---|---|---|
| V77 | A2Tools #4 `a_heal_from_no_skill_has_a_name` | `130538b12701b127b601032c530bb739` | heal by 5041, key (1 900 001, hot false), total 7351, 1 tick, name "Restore HP" |
| V78 | A2Tools #5 `a_damage_tick_from_no_skill_has_a_name` (dot ids {1 200 012}) | `170538812a0abdd1019002bb1227073332931200` | target 5377, actor 26813, skill (1 200 012, dot), total 51, 1 hit, name "Burn" |
| V79 | A2Tools #9 (dot ids {16 001 109}) | tick `19053888fd020accc801881241c15f5ff7025828f400` twice, death `0b4236ccc8010007`, tick twice | actor 25676 on 48776 skill (16 001 109, dot): hits 2, damage 750; after the link (V67) again and one tick: hits 3 |
| V80 | chen-zilei `unpacks_lz4_bundles` inner | `07 05 38 07` | a `05 38` frame with body `07` (framing only) |

### 17.6 Identity (`33 36`, `45 36`, `44 36`), names

| Id | Source | Bytes | Expected |
|---|---|---|---|
| V81 | A2Tools #22 `the_self_record_says_your_server_class_and_level` | `3336ed745e91c12837064e616963686118051e000000011c0000007f0100007f0100001c000000d002040000000000` | name "Naicha", local id 14957, server 1304, class Cleric, level 28 |
| V82 | A2Tools #24 `a_self_record_needs_your_server_and_class` | `101d37f5282703ab13e776e776111d37fc792f031557ff33363336101d37a3132703c6b679be97bc111d37ab262f032250d8722f722f` | local id None, nickname(16) None |
| V83 | A2Tools #25 `own_stats_records_name_the_local_entity` (local 5100 "Mine") | `114A36B8300A0000020050165636` twice keeps 5100; a third time | local id 6200, `local_id_from_scope()`, nickname(6200) None |
| V84 | A2Tools #70 `records_with_names` self | `3336ed745e91c1283702587918051e000000011c000000` | name bytes 11..13 "Xy" |
| V85 | A2Tools #70 player spawn | `4536d74800000000070141000000000000000000` | name 11..12 "A", entity 9303 |
| V86 | A2Tools #62 | `[45 36 05 00 02 'M' '7' 00]` | name "M7" at offset 6 |
| V87 | A2Tools #64 | `[44 36 9e 9b 01 03 e3 81 82]` | the one letter name "あ" |
| V88 | A2Tools #67 (capture 4 October 2026 01:45) | `[45 36 3c 03 00 00 de 02 0c 64 5c 64 64 64 64 3a 5e 64 64 5a 6c 44 b2 64 6a 64 64 58 64 48 7c 8a]` with known name "8D" | only the 12 byte run after `de 02 0c` is a name field |
| V89 | A2Tools #72 `spawn_named` | `[45 36 b5 02 00 00] + [len] + name + [00; 8]`, name "Grandine" | one name field |
| V90 | A2Tools #73 | `[45 36 07] + b"Velkora"` | one token per name |
| V91 | chen-zilei `decodes_identity` (derived) | body `2a 00 00 00 00 01 05 5a 69 6c 65 69` under `33 36` | `Identity { id 42, name "Zilei", is_self true }` |
| V92 | chen-zilei `finds_identities_inside_other_packets` (derived) | `07×9 | 45 36 | 4d 00 00 00 00 01 06 46 61 65 6c 69 73 e8 03 18 00 00 00 | 00×8` under opcode `12 34` | `Identity { id 77, name "Faelis", is_self false }` |
| V93 | chen-zilei `finds_identities_inside_embedded_bundles` (derived) | inner `01×5 | 33 36 | 2a 00 00 00 00 01 05 5a 69 6c 65 69 e8 03 18 00 00 00` LZ4 compressed; bundle `FF FF <u32 inner len> <block>`; body `09×6 | varint(bundle.len() + 4) | bundle | 00×4` | `Identity { id 42, name "Zilei", is_self true }` from `scan_embedded_bundles(0, &body, out)` called directly, not from `Parser::packet` (source comment: LZ4 keeps short inputs as literals, so the bundle path is scanned on its own to prove it decompresses) |
| V94 | chen-zilei `rejects_placeholder_and_junk_names` (derived, under `45 36`) | `4d 00 00 00 00 01 05 24 78 39 61 32 e8 03 18 00 00 00` (`$x9a2`); `4d 00 00 00 00 01 03 61 20 62 ...` (`a b`); `4d 00 00 00 00 01 04 31 32 33 34 ...` (`1234`) | nothing |
| V95 | chen-zilei (derived) | `4d 00 00 00 00 01 0c ec 97 98 eb a6 ac ec 8b 9c ec 98 a8 e8 03 18 00 00 00` (`엘리시온`) | 1 identity |
| V96 | A2Tools #20 `names_are_one_to_twelve_letters_or_digits_in_any_script` | accepts "A", "é", "あ", "ApexZ", "Amber1", "Zoë", "Ñandú", "さくら", "桜子", "전사", "Abcdefghijkl"; rejects `b"Abcdefghijklm"`, `b"12345"`, `b"Apex Z"`, `b"ApexZ\x06"`, `b"\x05ApexZ"`, `b""`, `[0xC3]` | as listed |
| V97 | NOIA2 `sanitize_nickname_keeps_single_cjk_extension_character` | "𠮷", "𫠜" | kept |
| V98 | NOIA2 `sanitize_nickname_still_rejects_single_ascii_letter` | "A" | rejected (conflicts with V96; see D68) |
| V99 | A2Tools #74 `token_for(&[7; 32], name, 0x03f6_0000_0001_4b85, len)` | "Misti", "丨Mamepoko丨", "九州依然在", "a", "Grandine" | token keeps the byte length |

### 17.7 Spawn (`41 36`, `40 36`) and ownership (`04 8D`)

| Id | Source | Bytes | Expected |
|---|---|---|---|
| V100 | A2Tools #16 `another_players_spirit_is_linked_at_spawn_by_its_caster` | `[41 36 dc f1 02 1f 10 00 c6]` + 24 bytes `22` + `[80 75 d5 2a bb 03 00 00 bc 31 0c 02]`, `parse_summon_spawn_at(2)` | true, summon 47324 -> owner 6332; with byte 2 = `dd` and byte 5 = `0c` (a mob) -> false, 47325 not a summon |
| V101 | A2Tools #17 `a_monsters_summon_is_not_the_player_its_spawn_names` (NPC table {2920063 Blazing Totem, 2920149 Wind Spirit}, player 3640 "Abcd") | `4136` + id + `1c00010441626364` + code + `0000020e6ccdc607af014800089b46a0411d43d46f0103030000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000006011101819698` + `00ffffffffffffffff8075d52abb030000c3910301020e6ccdc607af014800089b460676700000001e00000000` | id `c39103` code `7f8e2c` (2920063) -> false, 51395 not a summon; id `c49103` code `d58e2c` (2920149) -> true, 51396 -> 3640 |
| V102 | A2Tools #18 `a_sorcerers_ground_spell_is_linked_to_its_caster` | `4136` + id + `1F00004B8E2C004002000CF0C7CFA9D0C70090C04672498542642F01A925A9258E0800008E08000000000000000000000000000010E9010064000000F04902000100000000000000A08601000000000090D00300010101110181969800FFFFFFFFFFFFFFFF8075D52ABB030000` + id + `0102000CF0C7CFA9D0C70090C046070206` + caster + `02CD002800` | (`C98E01`, `3F370000`) -> 18249 -> 14143; (`CA8E01`, `4A470000`) false (names itself); (`CB8E01`, `30750000`) false after `append_mob(30000, 1)`; (`CC8E01`, `BF000000`) with `07 02 06` changed to `07 02 01` -> 18252 -> 191 |
| V103 | A2Tools #19 `a_pets_spawn_names_its_owner` mechanic | `4136a582011f000108` + "Hawkeye1" + `968f2c0040024fbd66c6c86b804600a05345600d7d43f3b301c0ee6dc0ee6d640000006400000000` + `00000000000000000000000000000090650000000000000100000000000000000000000000000000` + `000000010602110181969800ffffffffffffffff8075d52abb030000a5820101044fbd66c6c86b80` + `4600a05345110284969800ffffffffffffffff8075d52abb030000a58201014fbd66c6c86b804600` + `a05345070206` + `27580000` + `002500000000` | no name for 22567 or 1480 |
| V104 | A2Tools #19 pet | `4136{id}5f000108` + owner name + `ac902c00000200c46ac600043f460030524524e90b437e6301f73ef73efc090000fc090000000000` + `00000000000000000018f0010064000000f04902000100000000000000a08601000000000000e204` + `000101110181969800ffffffffffffffff8075d52abb030000b4c601010200c46ac600043f460030` + `5245070206` + `c8050000` + `ad01000000000f0905` + "Guild" + `02000000000000000000000000000000000002cd0096000000d000340100002600000000` | (`b4c601`, "Hawkeye1") -> 25396 -> 1480 and nickname(1480) = "Hawkeye1"; after `append_nickname_authoritative(1480, "Bowmaster")`, (`b5c601`, "Hawkeye1") keeps "Bowmaster" |
| V105 | A2Tools #23 `a_record_ends_with_its_frame` / #69 `spawn_after_8f_01` | `1b2a37e880011d034a065afad1bff9d53a4436337c421740` + `8f014136efb4031c000064902c0000026b519247339aa5c700540a4600d78b3fc7000107076400000064` + `000000000000000000000000000000000000006400000064000000010000000000000000000000000000` + `00000000000601110181969800ffffffffffffffff8075d52abb030000efb40301026b519247339aa5c7` + `00540a46063ed40000002900000000`, as is and inside a bundle (two byte length `(len as u8) | 0x80, (len >> 7) as u8`) | nickname(51) None; mob 55919 code 2 920 548, no name field |
| V106 | A2Tools #58 `spawn_packet(entity_id, mask_bytes, owner_name)` = `[(body.len() + 1 + 3) as u8] + [41 36 entity_id] + mask_bytes + [01] + [name len] + name + [00; 24]`, entity 120, owner 4099 "Misti" named first | mask `[5F 00 00 00]` links 120 -> 4099; mask `[5F 00]` links too; name "Stranger" links nothing |
| V107 | A2Tools #60 `host_with_spawn` | `[99 36 11 11] + [8d 08 41 36 b6 f5 01]` padded to 1039 then 40 more | the embedded spawn is lifted whole, starting `41 36 b6 f5 01` |
| V108 | A2Tools #61 `blinding_leaves_packet_headers_alone` | `[40 36 c9 8f 07 0c]` resized to 304; also inside host `[99 36 00 b3 02 40 36 c9 8f 07 0c]` | keeps `40 36 c9 8f 07` |
| V109 | A2Tools #70 spawn naming its caster | `4136c391031c0001014264902c0000026b5192473300` | name 10..11 "B" |
| V110 | A2Tools #71 | `[41 36 9e 9b 01 10 'J' 'o' 22 00 00]` keeps "Jo" (chance bytes); `[41 36 9e 9b 01 02 'J' 'o' 22 00 00]` is a name; `name_in`: `[10 'J' 'o' 22 01]` no; `[10 02 'J' 'o' 22]` yes; `[00 'M' 'i' 's' 't' 'i' 00]` yes; `[00 'M' 'i' 's' 't' 00]` no | as listed |
| V111 | chen-zilei `decodes_npc_spawn` (derived from `spawn_body(5001, 0x0C, 2_000_002)`) | `89 27 0c 00 00 00 00 07 00 82 84 1e 00 40 02 00×12 05 01 80 f7 02 d0 86 03` | `NpcSpawn { id 5001, npc_code 2_000_002, max_hp Some(50_000) }` |
| V112 | chen-zilei `folds_large_spawn_ids_and_skips_summons` (derived) | `bb 8e b7 01 0c 00 00 00 00 07 00 82 84 1e 00 40 02 00×12 05 01 80 f7 02 d0 86 03` | `NpcSpawn` with id `(3_000_123 & 0x3FFF) | 0x4000` = 18235 |
| V113 | chen-zilei (derived) | as V111 with byte 2 = `5f` | `Summon { id 5001 }` |
| V114 | A2Tools #26 `kill_record_names_its_owner_on_any_server` | `[04 8d ec de 02 72 28 e9 00 ae 0b 19 05 05] + b"ApexZ" + [06] + b"Ventus" + [01 00 00 00]` | `scan_for_embedded_04_8d` true (owner 1454, server 1305); with byte 13 set to `07` -> false |
| V115 | A2Tools #70 summon owner | `048dece6027228e900ae0b19050243640001000000` | name 15..17 "Cd" |

### 17.8 Party roster (`02 97`)

| Id | Source | Bytes | Expected |
|---|---|---|---|
| V116 | A2Tools #1 `roster_reads_members_past_vacant_slots` (capture 15 August 2026, `parse_party_roster_at(&data, 2)`) | `02 97 a5 f9 08 00 0b 38 35 30 4b 20 e8 bf 9e e5 88 b7 05 63 28 09 00 00 03 85 4b 01 00 00 00 f6 03 1f 02 00 05` / `0c 01 85 4b 01 00 00 00 f6 03 0f e4 b9 9d e5 b7 9e e4 be 9d e7 84 b6 e5 9c a8 06 00 00 00 32 00 00 00 07 17 00 00 f6 03 f6 03 04 38 39 0d 00 00 00 00 00 00 01 01` / `00 02 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 04 00 00 00 00 00 00 00 00 00 00 00 00` / `00 03 ...` (same 36 bytes with slot 03) / `00 04 ...` (slot 04) / `0e 05 62 64 01 00 00 00 f6 03 02 4d 37 0e 00 00 00 32 00 00 00 5e 16 00 00 f6 03 f6 03 04 6a 03 0d 00 00 00 00 00 00 01 01` / `02 0e 00 36` | members `[("九州依然在", slot 1), ("M7", slot 5)]`, `complete == true` |
| V117 | A2Tools #2 | same header; `0c 01 ...` member 1; `0e 02 62 64 01 00 00 00 f6 03 02 4d 37 0e 00 00 00 32 00 00 00 5e 16 00 00 f6 03 f6 03 04 6a 03 0d 00 00 00 00 00 00 01 01`; vacant slots `00 03 ...`, `00 04 ...`, `00 05 ...` (36 bytes each) | 2 members, `complete == true` |
| V118 | A2Tools #70 roster | `029701000000012e0600000000000000000000000000000000000101010100000000001905` + `0245661e0000002d0000006400000019050000000102030000000000000000` | name 39..41 "Ef", dbid `0x0519000000000001` |
| V119 | chen-zilei `decodes_party_list` (derived from `party_body(&["Zilei", "Faelis", "Brannoc"])`) | `01 02 03 04 03 47 72 70 04 00×17 03 00 00 01 02 03 04 05 06 e9 03 05 5a 69 6c 65 69 18 00 00 00 2d 00 00 00 7b×5 00 01 01 02 03 04 05 06 e9 03 06 46 61 65 6c 69 73 18 00 00 00 2d 00 00 00 7b×6 00 02 01 02 03 04 05 06 e9 03 07 42 72 61 6e 6e 6f 63 18 00 00 00 2d 00 00 00 7b×7` | `Party { members: ["Zilei", "Faelis", "Brannoc"] }`; the same body preceded by `07×6 | 02 97` under opcode `12 34` yields the same |

### 17.9 Abnormals and stats (`2A 38`, `2B 38`, `2C 38`, `4A 36`, lists) (A2Tools, capture 6 October 2026, local player 6759, spirits 71747 and 28113)

| Id | Source | Bytes | Expected |
|---|---|---|---|
| V120 | A2Tools #41 `BENEDICTION_ON` | `342a38e73401138202e165a6091027000000000000bc5cba14a1010000e73403300af7000047e44c47b46a22c700e85a46` | `Added { entity 6759, instance 258, abnormal 161900001, length 10000, end 1791349120188, caster 6759, level 3, skill 16190000, restarted false }` |
| V121 | A2Tools #42 `BENEDICTION_OFF` | `0d2c38e7340100820201` | `Removed { entity 6759, [instance 258, reason 1, by None] }` |
| V122 | A2Tools #43 `STATS_ON` | `294a36e7340441006c0700004c00e80300007b01041000007c01d00700000000000000000000` | `Stats { entity Some(6759), [(65, 1900), (76, 1000), (379, 4100), (380, 2000)] }` |
| V123 | A2Tools #43 `STATS_OFF` | `294a36e734044100840300004c00000000007b01340800007c01000000000000000000000000` | `[(65, 900), (76, 0), (379, 2100), (380, 0)]` |
| V124 | A2Tools #44 | `2f2a38a15801110661bef509ffffffffffffffff8075d52abb030000a15801005e71f0c7196dd6c700dcbf46` | entity 11297, instance 6, abnormal 167100001, length None, end None, skill None (a passive) |
| V125 | A2Tools #45 | `332b38e73413fe01cb6c000ace31000000000000f663ba14a1010000e73402e10a00010047e44c47b46a22c700e85a46` | `Changed`: instance 254, abnormal 167800011, length 12750, level 2, restarted false |
| V126 | A2Tools #46 | `342b38e73413f601a0c2bb09b80b0000000000003241ba14a1010000fcca0201f8c5f50002b2364a47e3ae1ec709e15b46` | `Changed`: instance 246, abnormal 163300000, length 3000, caster 42364, restarted true |
| V127 | A2Tools #47 | `412c38e734040791020be734e2b7f8005dd427610780020be734e2b7f8005dd4276107f9010be734e2b7f8005dd4276107f5010be734e2b7f8005dd42761` | `Removed` entity 6759: instances 273, 256, 249, 245, each reason 11, by (6759, 16300002, 1630000221) |
| V128 | A2Tools #48 | `BENEDICTION_ON` with a byte appended and length byte +1; `BENEDICTION_OFF` with the last byte removed and length -1 | both `None` |
| V129 | A2Tools #49 | `342a38d1db01011303e165a6091027000000000000bc5cba14a1010000e73403300af7000047e44c47b46a22c700e85a46` (spirit 28113 buff) and `0b4236d1db010007` (its flag 7) | timeline ends: (6759, 1000, Some(11042), Expired), (28113, 1000, Some(2052), Gone) |
| V130 | A2Tools #50 Curse on Kernon of the West 48776 | `352a3888fd020113a502c5c49e09581b000000000000b2a79214a101000090090aea46f6000037cbb3c7f0172dc700e44646` at 10974, `342b3888fd0213a502c5c49e09581b000000000000cefe9214a1010000fc7a0d6c47f6000242f9b6c7d97b2dc700c43d46` at 33279, `342b3888fd0213a502c5c49e09581b000000000000dc049314a1010000817e0d5847f600028a09b5c7229527c700284746` at 34816 | instance 293 by 1168 (10974..33279, Recast), by 15740 (33279..34816, Recast), by 16129 (open); the second alone at 1 791 346 533 279 -> start 1 791 346 533 238, server end 1 791 346 540 238 |
| V131 | A2Tools #51 map load ends abnormals | `34213603000000f20300008bc8da000000000049e14a47ad204dc7004c60465253bd420200000000000000000000000000` (map 1010, load 3) | ends every abnormal but the local player's |
| V132 | A2Tools #52 | `342a38a158011372c32eb7093075000000000000f813140ea1010000919e0101f8c5f5000080f1f1c78035d6c700ccc046` (an Element on 11297, 30 s) added at 1 791 237 529 395 | instance 114 closes Unseen at 1 791 237 559 395 |
| V133 | A2Tools #53 six Elements then Elemental Fusion | `342a38a158011372c32eb7093075000000000000f813140ea1010000919e0101f8c5f5000080f1f1c78035d6c700ccc046` @1791237529395, `342a38a158011376c32eb70930750000000000004229140ea1010000919e0101f8c5f5000080f1f1c78035d6c700ccc046` @1791237534795, `352a38a15801139001c32eb70930750000000000003f53140ea1010000e3bd0301f8c5f5000080f1f1c78035d6c700ccc046` @1791237545595, `352a38a15801139401c32eb70930750000000000005b5f140ea1010000e3bd0301f8c5f5000080f1f1c78035d6c700ccc046` @1791237548695, `352a38a15801139a01c32eb7093075000000000000e57a140ea101000097b60201f8c5f500008029f1c780ead5c70068c046` @1791237555695, `352a38a15801139f01c32eb7093075000000000000057e140ea1010000ffa50101f8c5f500008074f1c78035d6c70098c046` @1791237556495, `412c38a15804079f010ba158e2b7f8005dd427610790010ba158e2b7f8005dd42761079a010ba158e2b7f8005dd427610794010ba158e2b7f8005dd42761` @1791237556595 | with limit {163000003: 4}: 114 PushedOut @555695, 118 PushedOut @556495, 144/148/154/159 TakenOff by 16300002 @556595; without limits 114 Unseen @559346, 118 Unseen @564796 |
| V134 | A2Tools #54 Spirit Strike levels at login | `2f2a38a15801110661bef509ffffffffffffffff8075d52abb030000a15801005e71f0c7196dd6c700dcbf46` @695, `2f2a38a15801111061bef509ffffffffffffffff8075d52abb030000a15804005e71f0c7196dd6c700dcbf46` @795, `152c38a15804000f05000a05000805000605` @795, `2f2a38a15801111961bef509ffffffffffffffff8075d52abb030000a15805005e71f0c7196dd6c700dcbf46` @895, `182c38a15805000d05000c05000905001105001005` @895 | (6, level 1, Removed(5) @795), (16, level 4, Removed(5) @895), (25, level 5, open) |
| V135 | A2Tools #55 `SPIRIT_SPAWN` | `c9014136d1db015f1000b18e2c00400200394b47000f20c700c05b465c5a3943ce8301985598554f0a00004f0a000000` + `0000000000000000000000f837020064000000f04902000100000000000000a08601000000000000e204000101011101` + `40b39809ffffffffffffffff8075d52abb030000e7340d02705c4c47ab1020c79cf25a46070206671a00006c00000000` + `00b1040b787878787878787878787801000200000000000000000000000000000002cd008c000000d000500100002d00` + `0000dd1d030000` | `Listed { entity 28113, own false, [Applied { instance 1, abnormal 161002304, length None, end None, caster 6759, level 13, skill None }] }` |
| V136 | A2Tools #56 player record | `b3024536f47c08110581db8f0affffffffffffffff8075d52abb030000f47c01008b73d6c7ab544ac7004c0a46110861` + `6f940affffffffffffffff8075d52abb030000f47c018b73d6c7ab544ac7004c0a461009273c5101e093040000000000` + `ae239614a101000001ffff7f7fffff7f7fffff7f7f100a3b3c5101e0930400000000008ecf9614a101000001ffff7f7f` + `ffff7f7fffff7f7f110be1548e0affffffffffffffff8075d52abb030000f47c068b73d6c7ab544ac7004c0a46110c21` + `62910affffffffffffffff8075d52abb030000f47c038b73d6c7ab544ac7004c0a46110dc1e8920affffffffffffffff` + `8075d52abb030000f47c038b73d6c7ab544ac7004c0a46131021ba2f0a307500000000000014df9414a1010000f47c04` + `d0c5040195f3c0c73ad236c71ba628460f` | entity 15988, list (instance, abnormal, length, caster, level, skill): (5, 177200001, None, 15988, 1, None), (8, 177500001, None, 15988, 1, None), (9, 22101031, Some(300000), 0, 1, None), (10, 22101051, Some(300000), 0, 1, None), (11, 177100001, None, 15988, 6, None), (12, 177300001, None, 15988, 3, None), (13, 177400001, None, 15988, 3, None), (16, 170900001, Some(30000), 15988, 4, Some(17090000)) |
| V137 | A2Tools #57 `stack_limits_read_from_the_table` | `{"abnormals":{"103":{"icon":"x","stacks":3},"100":{"icon":"y"},"163000003":{"stacks":4}}}` | {103: 3, 163000003: 4}; the shipped table gives Element 163000003 -> 4 |

### 17.10 Map load, HP, death, server connect, ping

| Id | Source | Bytes | Expected |
|---|---|---|---|
| V138 | A2Tools #15 `only_a_map_load_into_the_open_world_ends_the_dungeon` | `34213601000000d52709003b1a350000000000f7e646460d7fb0c60080b045409da54200000000000000000000004f0000` (map 600021, Fire Temple); `34213602000000d5270900d74c390000000000a8f805c610861245008036453ccd24c204000000000000000000004f0000` (600021, teleport inside); `34213601000000f2030000dd7f3c00000000006868d047d0c62c470098da46fa63284300000000000000000000004f0000` (map 1010, World_L_A) | after `storage.set_current_dungeon(600021)` (the roster path): loads 1 and 2 keep 600021; load 3 (map 1010) -> 0; `set_current_dungeon(600021)` again stays 0; load 1 again -> 600021 |
| V139 | chen-zilei `decodes_map_load` (derived) | body `03 00 00 00 3b 28 09 00 00 00 00 00 00 00 00 00` | `ZoneChange { map_id 600_123 }` |
| V140 | A2Tools #8 `a_dummy_whose_spawn_was_missed_is_known_by_its_hp` | `consume_stream(hex("1400 8DEFC901 020100 <hp> 00000000"))` with hp `55E50000` and `DF120000` | consumes 17 bytes each; `get_mob_current_hp(25839) == Some(0x12DF)`; after `01000000` still no dummy; after `8F380100` `is_hp_reset_dummy(25839)` |
| V141 | A2Tools #9 death | `0b4236ccc8010007` | entity 25676 gone (flag 7) |
| V142 | A2Tools #49 death | `0b4236d1db010007` | entity 28113 gone (flag 7), ends its abnormals |
| V143 | A2Tools #75 `parse_server_connect` | `1A 05 0E 31 39 33 2E 32 30 32 2E 31 31 32 2E 39 39 10 34 17 03` | (1306, 193.202.112.99, 13328) |
| V144 | A2Tools #75 | `DE 05 0F 31 39 33 2E 32 30 32 2E 31 31 32 2E 32 30 30 10 34 9A 02` | (1502, 193.202.112.200, 13328) |
| V145 | A2Tools #75 | `1A 05 0E 31 39 33 2E 32 30 32 2E 31 31 32 2E 39 39 00 00`; `1A 05 0E 41 42 43` | None; None |
| V146 | A2Tools #38 ping (request `[10] + [AB; 12]`, response `[18 03 36 00 00] + client_sent LE + at LE`) | `wall_clock_timestamp_needs_no_request`: response at `now + 80` with `now + 62135596800000` -> 80; `arbitrary_epoch_is_timed_against_the_request`: clock 17 552 452 660, request at `now`, response at `now + 64` -> 64; next pair -> 90; unpaired response at `+20045` with `clock + 20000` -> 45; `a_pc_clock_jump_does_not_skew_ping`: a 2320 ms jump keeps 70 then relearns 45; `perf_counter_timestamp_needs_no_request`: sent `990_000 + 16_777_216_000`, counter 1 000 000, wall 1 790 000 000 000, arrival 1 789 999 990 064 -> 64; `perf_counter_matches_the_captured_request`: boot `1_790_798_496_918 - 787_574_334`, response at 1 790 798 497 088 with 17 564 790 334 -> 170 | as listed |
| V147 | chen-zilei latency (`pong_body(echo, stamp) = [0, 0] ++ u64le(echo) ++ u64le(stamp)`, `T0 = 1_791_515_750_000`, ping frame 12, heartbeat 11) | `ignores_garbage_pongs`: heartbeat `T0 + 850`, pong bodies `[0, 0, 1]` and `pong_body(5, T0 + 850)` -> `ping_ms(T0) == None`; `recognises_pongs_by_shape_under_any_opcode`: a pong under opcode `03 37` with `pong_body(ping - 1_000, ping + 910)` after heartbeat `ping + 900` is matched on the second round; an opcode `2B 38` that first carried `[1; 30]` is never taken for a pong; `learns_the_games_clock_from_two_pings` (clock 1_774_633_750_910): rtt 117, 118, 116 give `None`, `Some(118)`, `Some(116)`, `ping_frame_size == Some(12)`; `adds_uplink_and_lowest_downlink_cancelling_the_clock_offset`: raw 160, heartbeats held 90/40/0/70 ms, server 850 ms ahead, 60 ms each way -> `Some(120)`, `None` after `UPLINK_VALID_MS + 1`; `never_reads_above_the_raw_round_trip`: `Some(118)`; `falls_back_to_the_raw_round_trip_when_the_estimate_collapses`: `Some(118)` after a 1313 ms backward step | as listed |
| V148 | chen-zilei `measures_ping_from_the_games_ping_and_heartbeats` (`T = 1_791_515_750_000`, `GAME_CLOCK = 1_774_633_752_223`) | client packets 192.168.1.2:50000 -> 10.0.0.1:13328 at `ping` (`[0E; 11]` + `[0F; 12]`) and `ping + 50`; server packets at `ping + 210`: heartbeat frame with `ping + 1_000` then `03 36` frame with `00 00 + u64(ping - GAME_CLOCK) + u64(ping + 910)`; two rounds 10 s apart | `ping_ms == Some(120)` after the second round, `heartbeats == 2` |

### 17.11 Evidence slice and lifted record shapes (A2Tools only)

| Id | Source | Bytes | Expected |
|---|---|---|---|
| V149 | A2Tools #59 `host_with` | `[99 36]` resized with `11` and `04 38` at the given offsets; two records 40 bytes apart | lift as one span; 280 apart as two bounded by `3 + 2 + 160` |
| V150 | A2Tools #68 | `frame_packet([04 38 01 02 03])` not allowed in `Keep::State` but in `Keep::All`; `frame_packet([41 36 01 02 03])` allowed in `Keep::State` | as listed |

### 17.12 Table loading (chen-zilei, JSON)

| Id | Source | Input | Expected |
|---|---|---|---|
| V151 | chen-zilei `names.rs:127-135` | skills `{"11020000": "Keen Strike", "11020010": "Keen Strike II", "1000": "Rest", "bad": "x"}`, npcs `{"2000000": {"name": "Punching Bag", "isBoss": false, "isDummy": true}, "2400101": {"name": "Field Boss", "isBoss": true}}` | `skill(11_020_010) == "Keen Strike II"`, `skill(11_020_047) == "Keen Strike"`, `skill(1000) == "Rest"`, `skill_or_code(11_030_000) == "Skill 11030000"`, 3 skills loaded; layering `{"2400101": {"name": "Feldboss"}, "2000000": "Sandsack"}` keeps `is_boss` and `is_dummy` |

### 17.13 Literal byte patterns without tests (RATmeter)

`HeartbeatPattern = { 0x0E, 0x00, 0x36 }`; heartbeat frame test `packet.Length == 11 && packet[0] == 0x0E && packet[1] == 0x00 && packet[2] == 0x36`; `SummonBoundaryMarker = FF FF FF FF FF FF FF FF`, `SummonActorHeader = 07 02 06`, `SummonActorHeader2 = 07 02 01`, `SummonActorHeaderBackup = 07 02`.

### 17.14 Synthetic expectations without tests (TK)

DoT effect byte: `0A`, `02`, `03` accepted, `08` rejected (bit `0x02`). Opcode key for `04 38` is `0x3804` in TK's LE reading. Bundle shapes `L F2 FF FF <u32 originLength> <lz4 block>` and `L FF FF <u32 originLength> <lz4 block>`.

### 17.15 Raw sample without a parser (NOIA2 `1.txt`)

`05 8A 00 00 3B 00 AE 25 00 00 00 00 00 00 8C 01 00 00 00 00 EF 03 22 00 00 00 06 E6 BA AB E6 9F 94 2D 00 00 00 02 00 79 40 CF 64 9B 01 ...` (3 667 raw bytes; the `1.txt` file is 11 009 bytes of space separated hex text behind a `full_hex=` prefix); entries `EF 03 <u32 LE> <name len u8> <utf8 name> 2D 00 00 00 <01|02> ...`.

Totals: 151 numbered vectors by the Source column: A2Tools 107, chen-zilei 29, NOIA2 12, TK 3, RATmeter 0 (the per repo document item counts, A2Tools 77 items, chen-zilei 25, NOIA2 17 including documented samples, TK 3 synthetic, number the source documents' entries and do not partition the 151), after merging A2Tools #7 with #6's record, #23 with #69, and the heartbeat frame shared by chen-zilei and A2Tools.

---

## 18. Pitfalls and patch notes

### 18.1 Client versions and opcode moves (chronological)

- Pre June 2026 (KR client, TK era): mob/summon spawn `40 36`, player spawn `44 36`, death `41 36`, record terminator `06 00 36`, damage tail "old" (`[unknown][damage][hit count][per hit]`), spawn mask u16, self record name gate literal `0B 37` and player spawn `07` (A2Tools, NOIA2, TK).
- June 2026 update (the client Global launched on): the `36` spawn/death family shifted by +1 (`41 36` spawn, `45 36` player, `42 36` death; `21 36` and `23 36` did not move); the terminator became `0E 00 36` (an 11 byte heartbeat with a u64 server clock), which silently broke single signature port detection; the damage record gained the zero padded power scalar ahead of the value (NOIA2 dates it to July samples; NOIA2 changelog 3.3.5/3.4.0 "fix DPS too low after the game update"); the spawn mask widened to u32 (A2Tools, chen-zilei, NOIA2).
- September to October 2026 (Global): client echo in the pong moved from a wall clock to a performance counter or arbitrary epoch, the ping request frame grew from 12 to 13 bytes (A2Tools); `06 38` records name only the local player on Global (older KR/TW captures name the party too); Subjugation content is not on Global yet (chen-zilei, 11 October 2026).
- Expected to move again: every opcode is a literal byte pair; chen-zilei's `docs/REVERSING.md` says "If the heartbeat count is zero, framing or the port changed. If it's fine but Damage is missing, the damage opcode moved: look for a new high count opcode during combat", and recognises the pong by shape rather than opcode for that reason.

### 18.2 Region differences

- Server ids: Global 1000..=2999 (EU 13xx, NA 15xx observed); KR/TW 1001..=1021 and 2001..=2021. Server names: A2Tools `servers.json` (616, regions AS, EU, JP, LA, NAE, NAW), TK's 42 Korean names, NOIA2's TW table, RATmeter's 42 abbreviations.
- Ping echo clock: .NET epoch on KR/TW, performance counter or arbitrary epoch on Global.
- `06 38` party scope: names only you on Global.
- Name scripts: Hangul only validation (TK) is wrong for Global.
- The Global client runs on UTC+9 in every region for schedules; Korea on KST, Taiwan on UTC+8 (chen-zilei UI).
- Default server net 206.127.156.0/24 (TK) is KR only.

### 18.3 Known bugs in the sources (do not reproduce)

- NOIA2: frame size `L - 3` (D19) and the bundle `+1`; `find_server_id_after_marker` reads a u16 overlapping the `0x02` byte it checked; the self record resets the meter; the SMITE bit documented as 0x40 but coded as 0x20.
- RATmeter: sync search starts at the valid length (scans stale bytes); no partial retransmit trimming and no 32 bit wrap; header offsets hard coded to a 1 byte length; the `sv != 4` branch demands 12 bytes and consumes 11; `damageFlagByte & 0x01` overwritten by the direction byte; buff caster read 4 bytes early; mob code marker double offset; current HP read at the max HP offset; `petId == int.MaxValue` guard never fires; fallback header search returns index 0 when nothing is found; owner id truncated to ushort; never aligned bit reader in the roster; the name filter contains a literal `'?'..'?'` range; `IsBoss` includes dummies.
- TK: `searchOwnNickname` ignores the extension byte; the varint byte count is compared instead of the value for the 71 cap; owner id read as u16; marker search from frame byte 0 without a lower bound check; aligner drops overlapping retransmits wholesale and never times out gaps; hard reset unreachable; `UseBuffRepository` never flushed; `switch & 0x0F` outside 4..7 silently drops new layouts; the `packet[0] == 0x20` guard; class byte discarded.
- A2Tools (historical, fixed): two byte length read as `len - 3` (3% loss); TLS records swallowed up to 64 KB (kills 20 to 35% short); a bundle over 16 KB resynced through (lost the self record); Ethernet padding corrupted the stream (77% of a boss); a reconnect on the same server port appended to the old stream; records read across a frame end invented a player named "A" at entity 51; a `1d 37` record ending `33 36 33 36` read as a self record for entity 16; the `04 8D` scan ran into damage records after an all zero despawn record and named a Divine Aura as entity 10210 "M"; the last byte of a varint read as a small id; the self record's name gate was a literal.
- chen-zilei: the hunt budget is reset only when hunting is re entered, so after 256 KiB without a heartbeat a flow keeps byte resyncing until a TCP gap resets it; summon damage is dropped entirely; `02 97` tails beyond names are not read.

### 18.4 Unknown fields recorded by the sources (union)

Damage: the unused varint after the switch, the uid byte, the 8 byte hit id, the 4 bytes of switch bit 0x01, switch bit 0x10, hit types other than 1/2/3/6, the layout 4 field, the `03 00` suffix beyond one varint. DoT: the varint between actor and effect id, trailing bytes, effect codes 0x00/0x08. Identity: `mask1`, other `mask2` bits, the byte between class and level, the `45 36` tail. Spawn: mask bits other than 0x0010, the subtree bytes, the fields between mask and code, the id fold, `1D 37` / `2A 37`. HP: the three varints of `00 8D`, the semantics of `1B 92`. Death: the zero varint. Map: 44 bytes of `21 36`. Roster: the unnamed bytes, mask bits 0x20/0x40, tickets, role, latency, the member tail. Abnormals: flags bits beyond 1 and 2, the `01` byte, the optional entry byte, removal reasons beyond 1/5/11, `49 36` leading bytes and stat tails. Ownership: the 4 bytes of `04 8D`. Casts: `02 38`. Transport: the `F0..=FE` byte, the client frames.

### 18.5 Operational pitfalls

- Entity ids are reused and reissued on every zone change; a name bound to an id must be dropped when that id respawns as something else.
- Summons spawned out of render range before the fight produce unattributed numeric rows (RATmeter, TK, A2Tools all note it). Keep their damage and merge when a link arrives.
- Identity records arrive only on loading screens or when a player comes into view; a meter started mid zone shows numeric ids until then (TK FAQ: start at character select; chen-zilei: restore names for 45 minutes; A2Tools: party scope and loot owner fallbacks).
- Heals, shields and resource restores ride in `04 38` / `05 38` shapes; without the effect code and self target rules they become phantom damage on party members.
- Any skip that lands one byte short of the hit id reads the power scalar (10 000 to 22 000) as damage; any read of a 10 digit SkillEffect id as a skill id produces unknown skills; any bit test on the DoT effect byte counts HoTs as damage.
- Opcode notation inversion between repos (`0x3804` vs `0x0438`) has caused confusion; keep wire order byte arrays.

---

## 19. Recommended decoding plan for the new implementation

### 19.1 Pipeline

1. Capture: pcap on every adapter including loopback, promiscuous off, snaplen 65 535, kernel filter `tcp`; decode link types 1, 0, 108, 12, 101, 228, 229, 113, 276; IPv4 (and IPv6 if cheap) TCP; payload bounded by the IP total length; pcap timestamp validated against the wall clock.
2. Flow detection: count `0E 00 36` (and `06 00 36`) occurrences per server flow; lock the first flow with at least 5 hits inside 3 s, after a 2000 ms grace that prefers a loopback adapter; narrow the kernel filter to `tcp port N`; watchdog 60 s without a heartbeat. Exclude connections whose first segment is a TLS record.
3. TCP reassembly per 4 tuple (section 2): trim retransmits, hold out of order segments, skip a gap after 64 pending segments or 1 s, reset the frame decoder on a gap.
4. Framing (section 3): skip `00` padding and TLS records, `size = len + n - 4`, `len >= 6`, `size <= 65 535`, optional `F0..=FE` byte, `FF FF` bundles decompressed with raw LZ4 (raw size 1..=8 MiB, depth <= 4), heartbeat hunting after a gap with byte by byte fallback, incomplete frame wait 16 384 (65 535 for bundles).
5. Dispatch on the two opcode bytes in wire order. Count every opcode (chen-zilei `opcode_counts`); a change in the top combat opcode after a patch is then visible without a debugger.
6. Scan the bodies of unlisted opcodes (>= 16 bytes) and the contents of embedded bundles for `33 36`, `45 36` and `02 97` records only, with strict name validation.
7. Record every frame to a replay log (`timestamp|stream key|hex`, RATmeter and A2Tools style) so captures can become test vectors.

### 19.2 Packets to decode, in order, with the validation rules that avoid phantom damage

1. `00 36` heartbeat: body 8 bytes, u64 server ms; feed the lock counter, the clock and the resync anchor. No state change.
2. `33 36` self identity: id 1..=9 999 999, mask1 u32, mask2 u8 with bit 0, name len 1..=48 and a valid name (1..=16 alphanumeric characters with a letter, no `$` placeholder), server u16 in 1000..=2999, class u32 with a known value, level u32 1..=99. All of server, class and level must validate or the record is not a self record. Sets the local id and name; never resets combat.
3. `45 36` (and `44 36`) other player: same front; name validation as above; mark the id as a player; do not read the tail blindly.
4. `02 97` party roster: header exactly as section 11, count 1..=12, member names validated, class u32 known, level 1..=200, gear 0..=1 000 000, combat power <= 100 000 000 found through the world id anchor, next member reacquired by header. A complete roster replaces the party; members map to entity ids by name only.
5. `41 36` spawn: id 1..=9 999 999 (fold above 1 000 000 and keep the raw id), mask u32, kind = low byte, NPC code = u24 LE before `00 (00|40) 02` within 3..=60 bytes after the mask and non zero, max HP from the first `01 <cur> <max>` with max >= cur > 0 within 64 bytes after the marker; boss and dummy flags from the NPC table; for kinds 0x5F/0x1F/0x1D/0x5D/0x1C try the owner paths in A2Tools' order (parent key block, inline name, abnormal anchor + caster varint, `07 02 06|01` + u32) and reject an owner that equals the spawn, is a known mob, or is outside 1..=9 999 999. A spawn always reclassifies the id as a non player and clears its old links.
6. `04 38` damage (section 5): walk the chain with `01 00`; per record: target and actor plausible (1..=9 999 999, >= 100 unless announced); switch low nibble in {0,2,4,5,6,7}; skill 1..=299 999 999 with the Theostone rewrite; hit type read; for nibble 0/2 count miss/resist and stop; block = plotter (when bit 0x02) + 8 byte hit id + 4 bytes (when bit 0x01); read scalar then damage; if the first varint is 0 shift by one; apply the layout 6 crit heuristic; damage 0..=99 999 999; optional layout 4 field 1..=25; additional hits only with bit 0x20, count 1..=25, sum < damage; optional `03 00 <varint 1..=9 999 999>` life steal; the tail must end at `01 00`, `04 38` or the body end, else drop the record and count a tail failure. Then classify: spirit link bands and resource restore skills are neither damage nor heal; actor == target is a self heal when the actor is a player; player to player with a heal skill is a heal; 7 digit skills by an NPC on a player are damage taken; otherwise damage dealt. Never credit a mob with a heal; never count a record whose scalar reads as the damage (10 000..=22 000 with a tiny second value) without the crit heuristic conditions.
7. `05 38` DoT/HoT: target and actor plausible; effect in {02, 0A} damage, {01, 09} heal, 0B HoT, 03 damage but counted separately, others ignored; skill = effect id / 100 in 1..=299 999 999; amount 1..=99 999 999; damage with actor == target rejected; optional DoT allowlist as a diagnostic. Ticks from a despawned summon are held until it acts directly again.
8. `42 36` death: id, zero varint, flag; flag 3 = death (boss death closes the encounter; player death counts for the recap and wipe detection); flag 7 = gone (ends the entity's abnormals and summon ticks); flag 1 ignored except to mark loaded dead; duplicates within 5 s ignored.
9. `21 36` map load: load count u32, map id u32 1..=9 999 999; a changed map id ends every encounter, marks all ids as reissued (keep names for 45 minutes), sets the dungeon id when the map is 600 000..699 999 or equals the roster's dungeon id; a same map load is a teleport. `23 36` with entity 0 is a secondary zone change signal gated by a 1500 ms lull.
10. `00 8D` remaining HP: id, three varints, hp u64 <= 100 000 000 for display; drives the boss HP bar, the time to kill estimate, the HP reset dummy rule (1 then higher without a death) and the new attempt rule (HP back to max after 5 s without hits).
11. `1B 92` HP/MP update: actor, hp, max 1..=50 000 000; raises max HP.
12. `04 8D` ownership / kill record: entity >= 100, 4 bytes, owner varint >= 100 and != entity, server u16 1000..=2999, name; link summon to owner; a known boss named here corroborates a kill; an all zero record is a despawn. Never treat it alone as a player death.
13. `2A 38`, `2B 38`, `2C 38` abnormals and `4A 36` / `49 36` stats: section 12 layouts with the exact end checks (12 bytes after the restarted byte, removals ending exactly, stats ending with 8 bytes); optional for a DPS meter, required for buff uptime.
14. `03 36` pong: `00 00` + i64 + i64; ping via the three clock interpretations; optional.
15. `06 38` / `4A 36` local id fallbacks: optional, for a meter opened mid session.

Global phantom damage guards, applied after parsing: a damage row needs an actor that is a player or a linked summon of a player, a target that is an NPC (or a player, for damage taken), a skill in a player band or a known table entry, and a timestamp inside an open encounter or opening one; rows for actors that later spawn as mobs are removed; records found by an embedded scan (if ever enabled) need the trusted shape (actor != target, hit type 1..=3, damage > 0, known skill).

### 19.3 Encounter state machine (majority of the sources)

States: `Idle`, `Active { target, first_hit, last_hit, attempt }`, with one state per target (A2Tools, RATmeter, NOIA2, TK) and a boss focus rule for the display.

- `Idle -> Active`: the first counted damage record on a target (actor is the local player, a party member or one of their summons; in an instance any player). `first_hit` = that packet's timestamp. A heal never opens or extends a fight.
- `Active`, on each counted damage record: `last_hit` = timestamp; if the target is a boss and the segment held only trash, clear the trash (A2Tools).
- Boss detection: `isBoss` from the NPC table at spawn; `isDummy` is a separate flag (dummies are never bosses). Unknown codes are never bosses; a mob with HP above 10 000 000 may be promoted as a "possible boss" behind an option (NOIA2).
- `Active -> Ended` (in priority order): (1) `42 36` flag 3 for the target: end at the death timestamp; (2) `21 36` with a different map id: end at the load; (3) idle timeout since `last_hit`: 30 s for trash and open world targets, 180 s for a living boss inside an instance (600 000..699 999), 10 s for dummies; (4) the party ends (roster shrinks to one or drops you); (5) manual reset.
- Wipe / new attempt: while `Active` on a boss, if `00 8D` reports the boss's HP at or above the last known value after at least 5 s without hits, or `42 36` flag 3 has been seen for every known party member, end the attempt and open a new one on the next hit (`attempt += 1`).
- Saving: skip fights under 5 s or without damage; save a fight once 10 s have passed since its last hit, and additionally save in progress snapshots once it is at least 15 s long (A2Tools `dps_calculator.rs:944-957`); dummy fights are kept as training and never uploaded.
- Display focus: the most recently hit boss that is ours; in a dungeon without a boss yet nothing; in the open world the mob with the most damage that you or your party hit.
- Reset of identities: never on a self record; names kept across zone changes and for 45 minutes on disk; dropped when an id respawns as something else or a fresh self record disagrees.

### 19.4 Metrics to emit per encounter

Per member (local player, roster members, their summons merged): damage, DPS over the encounter duration (first to last hit on the boss by anyone, floor 1 s), share, hits, crits, back/front, parry/perfect/double/shield block/iron wall/regeneration/perfect block counts, misses and resists, multi hit counters, per skill rows keyed `(skill base, dot)`, healing and HPS, life steal, damage taken, deaths. Per encounter: boss code and name, dungeon id, map id, server id, attempt number, duration, party DPS, boss max and current HP, time to kill.

### 19.5 Patch readiness

Keep every opcode and marker in one table with its version note; count unknown opcodes; alert when the heartbeat rate drops to zero (framing or port changed) or when `04 38` disappears during combat (damage opcode moved); verify against a fresh capture after every game update, starting with the test vectors in section 17.

---

## 20. Verification log

Each finding below was checked against the cited primary source under `ref/repos` before the text was changed. All 40 were confirmed and applied; none was rejected. Line numbers in the evidence that did not match the file as it is now are noted where they were off; the substance held in every case.

| # | Section | Status | What was verified |
|---|---|---|---|
| 1 | 1.1 Ping measurement | applied | A2Tools `combat/ping_tracker.rs:135` reads only `read_i64_le(data, i + 4)` and advances `i += 12`; no other `03 36` parser exists in src or tests. chen-zilei `latency.rs:113` reads `u64_at(body, 2)` and `u64_at(body, 10)`. |
| 2 | 1.3 Port 13328 | applied | `tests/identity_replay.rs:6,10` label Ariel as EU and Nezekan as Asia; `diagnostics/packet_dump.rs:397-400` carries the two server connect records. |
| 3 | 1.1 Link types | applied | A2Tools `pcap_capturer.rs:444` accepts `12 | 14 | 101 | 228` as raw IP and `:448` 276; chen-zilei `net.rs:21-24` accepts `12 | 101 | 228 | 229` and has no 276. |
| 4 | D7 Pong body size | applied | A2Tools `MIN_PING_RS_BYTES = 12` checked as `i + 12 <= len` with `i` at the marker; NOIA2 `ping_tracker.rs:7,42` the same with `idx` at the marker, bytes idx+4..idx+11 read. |
| 5 | 2.1 TCP reassembly | applied | chen-zilei `tcp.rs:7,20` BTreeMap and `:105-107` the `ab`/`cd` wrap test; NOIA2 `tcp_reassembler.rs:1,22` HashMap plus `held_bytes` and `:220-225` `handles_sequence_wrap` emitting two chunks. |
| 6 | 3.1 Varint | applied | NOIA2 `utils.rs:6,21` (`u32`, returned as `i64::from`); A2Tools `stream_processor.rs:1907` `i32`; TK `StreamProcessor.kt:638` `var value = 0` (Int); RATmeter `PacketReader.cs:82-98` uint, `BinaryExtensions.cs:9-34` int. |
| 7 | 3.6 and D22 RATmeter raw size | applied | `PacketProcessor.cs:160` `if ((uint)(size - 1) > 0x98967F) return false;` accepts size 10 000 000. |
| 8 | 3.6 chen-zilei bundle wait | applied | `frame.rs:13-14` and `:98-103`: a frame over `MAX_UNBUNDLED_WAIT` that `is_bundle` is waited for up to `MAX_FRAME`. |
| 9 | D24 NOIA2 stall delay | applied | `config.rs:8-10` defaults 1000, 200, 10 and `:111-115` clamp 50..2000, min 2000, min 500; `processor.rs:236-245` picks by mode. |
| 10 | D26 A2Tools embedded bundles | applied | `stream_processor.rs:205-208` calls both `scan_masked_identity` and `scan_party_roster` on the decompressed data. |
| 11 | 3.2 Validity rule | applied | chen-zilei `frame.rs:93` `len < 6`; A2Tools `framing.rs:275-277` (not 289-292) `size >= varint_bytes && size > 0`; RATmeter `PacketCapture/PacketAccumulator.cs` (not under PacketProcessing) `packetSize <= 0`; TK `StreamAssembler.kt:33-34` `realLength <= 0`. |
| 12 | 6.3 Unknown varint | applied | Vectors at `stream_processor.rs:3316` and `:3341` decoded by hand against `parse_dot_packet` (:428-490): the varint is `b6 01` = 182 and `90 02` = 272. |
| 13 | 6.1 Effect id | applied | `src/data/abnormals.json` has key `12000121` (Burn icon) and no `1200012`; test comments at `:3302-3306` and `:3326-3330` say abnormal 19000013 and 12000121; `120001209` appears in no repository. |
| 14 | 5.1 Worked example | applied | TK `StreamProcessor.kt:548-575`: `start` is the plotter flags byte, layout 6 skips 10, then reads `unknownInfo` and `damageInfo` with no zero check, so the damage read is `e4 50` = 10340. NOIA2 `damage.rs:172-185` shifted mode and A2Tools `:1507-1515` realign on a zero first value. |
| 15 | D61 A2Tools column | applied | `parse_dot_packet` (:428-526) checks varint validity and `is_damage && actor == target` only; `is_plausible_entity_id` is called at `:1378`, `:1401` and `data_storage.rs:1690` (`append_heal`); `append_damage` (:1112 on) has no plausibility check. |
| 16 | 5.3 and 7.1 heals | applied | chen-zilei `parser.rs:174-177` (amount 1..=99 999 999) and `:187-191` (`actor == target || HEAL_SKILLS.contains(&base)` with no player set); `combat.rs:338-343` credits healing without a player check, `:306-308` drops PvP. A2Tools `stream_processor.rs:1732` and `data_storage.rs:2340-2344`. |
| 17 | D56 and D60 | applied | NOIA2 `damage.rs:327-331` `normalize_skill_id` = `raw - raw % 10_000`, used at `:293`; `data_storage.rs:421` compares against keys parsed from `healing_skill_code.json` (all 8 digits, `loaders.rs:32-43`). RATmeter `DotDamagePacketProcessor.cs:47-48` rejects via `IsReasonableSkillCode` before the heal list at `:64`; `DataValidationHelper.cs:24-27` returns false for 1 000 000..=9 999 999. |
| 18 | 5.4 rule 3 and D50 | applied | NOIA2 `damage.rs:61-64` reads `target_id` with no value test; `:85-88` breaks on `actor_id == 0`. |
| 19 | D71 and 13.1 identity priority | applied | `data_storage.rs:860-894` (`note_party_scope` gates on `local_identity_from_game`, `!undecided`, `local_from_stats`), `:792-855` (`note_loot_owner`: blocked by the self record unless already applied, by a stats streak naming another entity, and by a configured name found in the world; no `local_from_scope` check), `:915-949` (`note_self_stats` clears `local_identity_from_game` and the name), `:2080-2087` (`set_game_identity`), `:773-774` (the self record resets `applied`), `stream_processor.rs:703-712` (character list returns if `local_identity_from_game`). |
| 20 | 8.6 Ownership front | applied | `stream_processor.rs:2439-2490` `ownership_at_front`: after the owner one varint (`meta_info`) then a u8 name length 1..=36; no server range, no legion, no u32. `:2500-2575` `ownership_records`: u16 server in 1000..=2999, `NAME_FIELD_BYTES`, `exact_name`. |
| 21 | 8.1, D67, D74 other player readers | applied | NOIA2 `nickname.rs:39-66` `parse_main_fixed` (u8 length at +5, 1..=36) versus `:103-193` `parse_other` (two varints, `offset += 1`, `for relative in 0..5`, length 1..=71, longest sanitised name, job byte after the name). TK `StreamProcessor.kt:182-230` (not 238-283) `searchOtherNickname` with the same two varints, `offset += 1`, `for (i in 0 until 5)`, length 1..71, first valid wins. |
| 22 | 8.1 Embedded identity scan | applied | chen-zilei `parser.rs:81-85` (`body.len() >= 16` on the unlisted arm); A2Tools `stream_processor.rs:112-134`, `:142-180` (every region, every packet) and `:2656-2660` (`data.len() < 9`). |
| 23 | 9.1 path (1) | applied | `stream_processor.rs:997-1001` `let is_summon = kind == 0x5F; if is_summon && mask & 0x0010 != 0 ...`; `:2264-2287` `parse_spawn_owner_block` requires a non zero parent, zero u16 pad, server 1..=9999 and allows only an empty legion name. |
| 24 | 9.1 path (3) | applied | `stream_processor.rs:1139-1152` anchors on the 8 bytes `80 75 D5 2A BB 03 00 00` within `start_offset + 120`; test at `:3646-3663` has no `FF` bytes before the anchor. NOIA2 `summon.rs:375-402`, TK `:436-446`, RATmeter `MobPacketProcessor.cs:11,105-121` anchor on `FF` x8. |
| 25 | 10.2 and D87 | applied | `stream_processor.rs:591-642` comment and condition: only `02 01 00` plus four zero bytes is stored; `01 01 01` is documented as the local player's record and excluded. |
| 26 | 11.1 and 11.3 RATmeter bits | applied | `PartyProcessor.cs:106-110` `ReadBit()` for ready under mask 0x02 and an unconditional login bit; `:81` header `ReadBit()`; `PacketReader.cs:120-132` loads a byte only when `_bitsRemainingInBuffer == 0`. |
| 27 | 13.1 and D104 RATmeter bands | applied | `SkillRepository.cs:208-224` four `SkillRanges` used by `IsSkillCodeInRange` inside `NormalizeSkillCode` (:150-184); `:87-94` `IsTheostone`; `ClassRepository.cs:50-60` `s[..2]`; `classes.json` ids 10..19; `DamagePacketProcessor.cs:46-66` unknown class returns null, Theostone takes `player.CharacterClass`. |
| 28 | 14.1, D112, 10.1 boss death | applied | chen-zilei `parser.rs:358-367` `Some(1 | 3)` emits `Event::Death`; `combat.rs:369-376` finishes on the death of a fought boss. A2Tools `stream_processor.rs:415-418` marks dead on flag 3 only. |
| 29 | 14.1 Wipe signals | applied | chen-zilei `combat.rs:369-376` and `:397-430`: a death only pushes a `DeathRecap`; `:172-174` and `:472-478` the 180 s idle rule is the only wipe handling. |
| 30 | 15.1 Party DPS | applied | chen-zilei `combat.rs:552` sums rows with `is_player`, `:560` divides by `duration_s`; test `:587-593` counts the stranger's 30. RATmeter `CombatSessionManager.cs:171-176` sums `PlayerStats` over `GetCombatDuration()` of the active target session. |
| 31 | D117 TK and 14.1 | applied | TK `DpsCalculator.kt:142-146` applies the contributor test only inside `if (DataManager.isCurrentTargetDummy())`. |
| 32 | 19.3 and 14.1 Saving | applied | A2Tools `dps_calculator.rs:944-957`: skip under 5 000 ms or without damage; save when `idle_time >= 10_000` or `battle_time >= 15_000`. |
| 33 | 16.3 NPC tiers | applied | `i18n/npcs/en.json` counted with Python: tiers include Extreme 7, Insane 7, Hell 7, Ordeal 2, Doom 2, spelled `Conquest [Normal]` and `Conquest [Hard]`; categories Nightmare 300, Dungeon 128, Transcendence 64, Ascension Trial 44, Raid 27. |
| 34 | 12.1 Spawn list | applied | `abnormal.rs:294` `end - length >= EARLIEST`; `:273-275` `if next.is_empty() || next.len() > 4 { return None; }`; `:278` `readings.len() == 1`. |
| 35 | 14.1 Only my fights | applied | chen-zilei `combat.rs:467-468` `target_ours || engaged.contains(&target) || (engaged.contains(&actor) && is_player(target))`. |
| 36 | 16.1 Class prefix 19 | applied | TK `JobClass.kt:8-15` eight values and `:18-43` ranges 11..18 only; A2Tools `job_class.rs:74` `19 => Fighter`; NOIA2 `data_storage.rs:1054` `"19" => FIGHTER`; RATmeter `classes.json` id 19. |
| 37 | 17.15 Sample size | applied | `NOIA2/1.txt` is 11 009 bytes, one line starting `full_hex=`, 3 667 hex pairs (counted with Python). |
| 38 | 17 Totals | applied | The Source column of the V1..V151 rows counted with Python: A2Tools 107, chen-zilei 29, NOIA2 12, TK 3, total 151. |
| 39 | 17.6 V93 | applied | chen-zilei `parser.rs:527-530`: the test calls `scan_embedded_bundles(0, &body, ...)` directly with the comment about LZ4 keeping short inputs as literals. |
| 40 | 17.10 V138 | applied | `stream_processor.rs:3621` `storage.set_current_dungeon(600021);` precedes the first load; `:3630-3631` set it again and assert 0; `:3633-3634` reload the first map and assert 600021. |

## 21. Corrections observed on the Global client (captures of 2026-10-11)

The parser was validated against four 30 minute captures of the Global EU client (about 140 000 damage records).
These observations override the community consensus above where they differ; the code in
`crates/meter-core/src/parse/` follows them and the fixture tests pin them.

1. `01 00` is not a chain separator. Every `04 38` packet holds exactly one record, and the record ends with a
   trailer `<N u8> 00` where N was observed in 1..=20 (10 271 times `01 00`, 2 631 times `02 00`, 526 times
   `03 00` in session1). A parser that treats `01 00` as "next record follows" simply stops, which is harmless,
   but one that treats `02 00` or `03 00` as a failed tail drops the record.
2. The "unused" varint after the switch is a flag set. Bit 0x04 appends one varint after the trailer (1 209
   records in session1; A2Tools reads it as the next record's target and silently stops). Bit 0x01 puts
   `<count> <u32 effect id>*` after the scalar of a no value record.
3. `03 00 <varint>` is not a life steal suffix. All 86 occurrences in session1 are the trailer N = 3 on a bit 0x04
   record, and the value exceeds the hit. No heal is derived from it.
4. Switch bit 0x40 adds `<count> <u32>*` after the value (35, 62 and 196 records per session). Switch bit 0x08
   adds 4 bytes after the damage (3 records, the u32 equals damage / 5).
5. Plotter flag bit 0x10 on a record without a plotter (switch 0x44) leaves exactly one `01` byte after the
   trailer; with a plotter (switch 0x46) it never does. Verified on all 84 such records.
6. `00 8D` is `<entity> <mask varint> [mask & 1: n x (kind u8, u32)] [mask & 2: m x (kind u8, u64)]`; kind 0 of
   the u64 list is HP. The "(1,1,1) + u64" player form in section 10 is mask 1 with a 4 byte MP entry; player HP
   arrives with mask 3. The layout ends exactly at the body end on all 11 414 packets of session1.
7. `45 36` carries `<job u32>` directly after the name (no server there); the server sits later, before the
   legion name. `41 36` is `<id> <kind> <flags> <subtree> [name] <code u24> 00 xx 02` with xx also 0x08; kinds
   0x04, 0x05, 0x07 and 0x85 appear for NPCs, and 0x1C for named specimen NPCs with 9.86M HP, so kind 0x1C is an
   NPC add unless the spawn carries an owner.
8. Hit type 0 (no value) is a skill start that pairs with a hit type 2 marker; hit type 3 no value records are
   crit cast markers. `06 38` names mobs too (by 7 digit skill), so a self id hint from it must filter on player
   band skills.
9. Players never receive `42 36` flag 3 in these captures (0 of 74 player deaths). Player deaths arrive as
   `00 8D` or `1B 92` reports with HP 0, and resurrections as positive HP reports.
10. The zero pad shift and the layout 6 crit heuristic of section 5.2 never fired on the Global client (counters
    `pad_shifts` and `crit_heuristics` stay at 0 and the fixture asserts it); the crit heuristic is only tried when
    the structural tail fails.
11. Observed scalars are all in 10 000..=20 999 (10 000 for mobs). The hit id's last byte is always 00.
12. Hand decoded expectations are error prone: a two byte varint such as `A9 39` (7 337) is easy to misread as
    three bytes. Always confirm a vector with the parser and the trailer position before pinning it.
