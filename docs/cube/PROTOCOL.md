# Hidden Cubes in the Aion 2 server stream (Global client, October 2026)

Transport, framing, the heartbeat, the map load and the self record are as in aion2dps `docs/PROTOCOL.md`; bytes
are written in wire order and integers are little endian. The object and position records were worked out by
dr89mh/aion2-cube-radar on Global build 25800038 (2026-10-13, `docs/FINDINGS.md` there) from Cube Watch's
reading, and are checked here against its fixtures and against the four aion2dps captures of 2026-10-11.

## Packets

| Wire bytes | Meaning | Layout |
|---|---|---|
| `34 36` | World object comes into view | `id varint, kind u16, template u32, x f32, y f32, z f32, f32, heading f32, u8, u8, state u8, created u64 ms, ...` |
| `35 36` | World object removed | `id varint, reason u8`: 01 left view, 04 opened or destroyed |
| `38 36` | Someone starts opening it | `object varint, actor varint, 01` |
| `3a 36` | Opening finished | `object varint, actor varint, count u32, item u32` |
| `2a 38` | Buff record ending in a position | `..., caster varint, 01 00, x f32, y f32, z f32` (position of the caster) |
| `2b 38` | Buff record ending in a position | `..., caster varint, u8, u32, 02, x f32, y f32, z f32` (position of the caster) |
| `21 36` | Map load | `load count u32, map u32, u32, u32, x f32, y f32, z f32, heading f32, ...` (your position) |
| `23 36` | Teleport | `entity varint (0 = you), 5 bytes, x f32, y f32, z f32, ...` (your position) |
| `33 36` | Self record | your entity id and name, as in aion2kina |

Notes:

- A Hidden Cube is a `34 36` whose template is a cube template. Everything else in that packet (gathering nodes,
  chests, static scenery) uses the same layout with other templates and kinds.
- `created` is the Unix ms time the object spawned. Static objects carry the server start; cubes the moment they
  appeared. Ages are computed with the server clock from the heartbeat, not the PC clock.
- Units are centimetres. While opening a cube the character stands about 200 units (2 m) from it.
- The server sends a cube when you come within about 10 000 units planar (100 m), measured four times by dr89mh
  (9 680 to 10 548), or one second after it spawns inside that range, standing still or not.
- The server never echoes your own movement (`1a 37`, `1b 37`, `1c 37`, `29 37` carry other entities only), and the
  client stream is encrypted. Your position comes only from the records above.
- The caster varint of `2a 38` and `2b 38` is read backwards from the position: a start is only accepted where
  the byte before it has no continuation bit, because the last byte of a long varint is itself a short one.
- Records carrying a small vector instead of a world position (within 2 000 units of the origin) are rejected.
- North is world -Y and east +X, as Cube Watch and dr89mh read the map. Not confirmed in game.
- The state byte read 06 on the first sighting of a cube that had just spawned (1200041 in the dr89mh capture,
  1301199 in the aion2dps one) and 01 on cubes that already existed. Observation only; the tracker does not use it.

## Cube templates

| Template | Zone | Evidence |
|---|---|---|
| 1200039 | Verteron (map 1010) | dr89mh: two cubes opened with the key, positions equal th.gl cube spots exactly; three more seen spawning and opened |
| 1200041 | Verteron (map 1010) | dr89mh: opened with the key, position equals th.gl spot `hidden_cube@99276:70962` |
| 1301199 | Altgard? (map 1110?) | Cube Watch: its test vector sits on a th.gl Altgard cube spot. aion2dps captures of 2026-10-11, see below |

The templates differ per zone and even within one, so the tracker learns more at run time: an object whose
`3a 36` takes item 630900003 (`23 c5 9a 25`, byte identical on every cube opening in the dr89mh and Cube Watch
captures, presumably the consumed key; Questlog's database lists 630900003 as "Key: Hidden Cube (Bound)") has a
cube template. Gathering nodes end in other values. Learned templates are saved in `templates.json`.

### 1301199 in the aion2dps captures

Replaying the four aion2dps captures of 2026-10-11 (Global EU, a character on server 2314) finds one object with
template 1301199, id 64694, at (165 440, -43 972, 9 347) in map 1110:

- first `34 36` at 1791722740386, 0.9 s after its `created` time 1791722739481, state byte 06, 31 m east of the
  character;
- never removed while in view;
- sent again with the same id, position and `created` (state byte 01) after each of three dungeon runs, the last
  one 310 s after it spawned;
- not sent again on later returns to map 1110, so it was gone by then.

That is the lifecycle of a cube: spawned in range, sent within a second, persistent until opened. Server 2314 is
Asmodian (dr89mh notes the Global EU 23xx servers are Asmodian), and Cube Watch puts 1301199 on an Altgard spot,
so map 1110 is probably Altgard. Not confirmed: nobody opened it in the capture.

## Finding them again after a patch

The opcodes or the layouts may move. With a fresh capture of walking up to a cube and opening it:

1. `replay capture.pcap --opcodes` and compare with the table above. `34 36` is a mid sized packet (about 55 bytes)
   arriving in bursts when you move; `2a 38` comes every few seconds.
2. `replay capture.pcap --objects` lists every object template with its spawn count, how often it was opened
   with the cube key, and how close it came to your position. A cube template is opened with the key and sits
   where you stood when you opened it.
3. `replay capture.pcap --dump 3436` shows the spawn bodies; check the template and the position offsets.
4. Update `opcodes.rs`, the offsets in `object.rs` or `position.rs`, and `KNOWN` in `cubes.rs`. The tests hold
   real records; a rising "object records rejected" or "buff records without a position" counter on the Setup
   view is the sign that something moved.

## Unknowns

- The remaining bytes of `34 36` (the f32 before the heading, the two bytes before the state, the tail), and the
  meaning of `kind` (04 for cubes; 01 and 0c seen on other objects).
- The cube templates of other zones (Eltnen, Morheim, ...), and whether event cubes that need no key take another
  item.
- Whether map 1110 is Altgard.
- The leave radius for cubes (`35 36` reason 01).
