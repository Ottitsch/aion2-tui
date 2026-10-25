# Kina in the Aion 2 server stream (Global client, October 2026)

None of the community meters (A2Tools, chen-zilei, RATmeter, NOIA2, TK) decode the inventory, so this was worked
out for this tracker from the four aion2dps captures of 11 October 2026 (Global EU, character on server 2314,
two hours of dungeon runs). Transport, framing, the heartbeat, the map load and the self record are as in
aion2dps `docs/PROTOCOL.md`; bytes are written in wire order.

## Packets

| Wire bytes | Meaning | When |
|---|---|---|
| `11 56` | The whole inventory, about 6 to 7 KB | On every loading screen, right before the map load `21 36`; the self record `33 36` follows about 5 s later |
| `1B 56` | Changed inventory entries | Whenever an item or currency count changes (sale, reward, purchase, repair, potion used) |
| `A5 FF` | Daily gain limit counter, `<u64 counted> <u64 server ms>` | On every loading screen, about 230 ms before the map load |
| `A6 FF` | The same counter after a gain that counts toward it | Right after each kina drop from an open world mob |
| `A9 FF` | Abyss Points, `<u64 points> <u64 cap>` | On loading screens and whenever they change |

## Daily gain limit and Abyss Points

The game window (Kina, Daily Gain Limit) shows `counted / 1,000,000` and "Resets every 08:00"; below it Abyss
Points `points / 2,000,000`. Found on a recording of 25 October 2026 (`aion --record`, then `replay --find`):

- Twenty `A6 FF` packets climbed from 183,413 to 192,339, the window showed 192,339/1,000,000 afterwards, and
  the 8,926 between them is exactly the kina the open world kills of that stretch earned.
- Kina earned in the Abyss (map 20) right after, +2,409, left the counter at 192,339; the Abyss Points moved
  from 138,120 to 138,211 in `A9 FF`, the cap 2,000,000 beside them.
- The second u64 of `A5 FF` and `A6 FF` is a Unix millisecond time, 0.2 to 0.6 s before the packet arrived:
  the server time of the count, which the tracker uses to tell whether the 08:00 reset has passed since.
- The captures of 11 October (dungeon runs) carry `A5 FF` with 23,886 on every loading screen and no `A6 FF`
  at all: dungeon rewards do not count either.
- The limit (1,000,000) is not in any packet seen so far; it is a constant in the tracker.

## Currency record

Both packets carry currencies as the same fixed record, found anywhere in the body:

```text
<u32 uid> 00 <u16 slot> 00 <u32 key> <u64 count LE> ...
```

- Kina is key `0x3770333F`, wire bytes `3F 33 70 37`.
- The uid and slot change on every loading screen (the full inventory hands out new ones); the key does not.
- A record is accepted when both zero guard bytes are zero and the count is below 2^40. Every one of the 77
  kina records in the captures passes; random body bytes do not.
- Each `11 56` holds exactly one kina record; a `1B 56` holds at most one.

Other keys of the same family seen in the captures (`?? 33 70 37`), not identified:

| Key | Count at the end of the captures |
|---|---|
| `0x37703321` | 3,000 |
| `0x37703323` | 117,706 |
| `0x3770332B` | 1,245 |
| `0x3770332D` | 5 |
| `0x37703331` | 1,080 |
| `0x37703333` | 15,260 |
| `0x37703337` | 53,913 |
| `0x37703338` | 18,000 |

The Setup view lists them with their latest count, which makes naming them a matter of comparing with the game.

## Evidence

- The kina count chains across all four captures without a gap: the last `1B 56` before every one of the 32 full
  inventories equals the count in that inventory, and each capture starts where the last one ended.
- The jumps look like the game: dungeon clear rewards of +600,000, +400,000, +478,300 and +268,200, a 35,000 to
  45,000 spend seconds after most clears (a repair, probably), fees of 16,000 and 18,000 in town, and a burst of
  ten escalating purchases
  (8,750, 12,500, 22,800, 35,000, ... 139,000 within six seconds).
- Balance 6,796,955 at the first record, 8,308,193 at the last; 2026-10-23 live traffic showed 7,728,402.

## Finding it again after a patch

The record layout or the opcodes may move. With a fresh capture:

0. Record one: `aion --record captures\new.pcapng` saves the game traffic while the app runs as usual. With a
   number the game shows (a balance, a counter), `replay new.pcapng --find 192339` lists every packet that
   holds it, as u64 or u32 little endian, u32 big endian or a varint, with the bytes around it.
1. `replay capture.pcap --opcodes` and look for a large packet that appears once per loading screen (the
   inventory) and a frequent mid sized one (the updates).
2. Buy or sell something with a known price while capturing, then search the capture for the before and after
   balances as u64 LE (`replay capture.pcap --dump <opcode>`).
3. Update `opcodes.rs` and, if the key moved, `KINA_KEY` in `parser.rs`. The parser tests hold real records.

## Unknowns

- The record kind byte(s) before each record in `1B 56` and the meaning of the slot field.
- Whether warehouse or legion kina would appear as a second kina record (none in the captures).
- What the other `?? 33 70 37` currencies are.
