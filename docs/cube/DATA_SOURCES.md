# Data sources

The tables under `crates/cube-core/data` are compiled into the binary. They are copied unchanged from aion2kina,
which copied them from the aion2dps meter, which took them from the community's extractions of NCSOFT's Aion 2
client data, redistributed under GPL-3.0. This project is GPL-3.0-or-later for that reason.

| File | Source | Notes |
|---|---|---|
| `dungeons.json` | taengu/A2Tools-DPS-Meter `src/data/i18n/dungeons/en.json`, extended in aion2dps | instance map id to name and tier |
| `open_world_maps.json` | A2Tools `src/data/open_world_maps.json` | map ids that are open world, not instances |

Code taken from the sibling projects:

- `net.rs`, `tcp.rs`, `frame.rs`, `wire.rs`, `pipeline.rs` and `capture_file.rs` are copied unchanged from aion2kina
  `crates/kina-core`, which has them from aion2dps `crates/meter-core`. Their protocol knowledge is credited in the
  aion2dps `docs/PROTOCOL.md`.
- `crates/cube-tui/src/capture.rs` and `logging.rs` are copied from aion2kina `crates/kina-tui`, with only the
  crate name changed. `fmt.rs`, `main.rs`, `app.rs`, `feed.rs` and `ui.rs` follow the aion2kina ones.

Protocol facts and test vectors:

- dr89mh/aion2-cube-radar (GPL-3.0, https://github.com/dr89mh/aion2-cube-radar): the `34 36`, `35 36`, `38 36`,
  `3a 36` layouts, the position records `2a 38`, `2b 38`, `21 36`, `23 36`, the cube templates 1200039 and
  1200041, the key item, the send radius and the north axis (`docs/FINDINGS.md`, `config/*.json`). The hex bodies
  in the cube-core tests are taken from its `fixtures/cubes/*.json` and `fixtures/selfpos/*.json`. No C# code was
  copied; the Rust readers are written for this project.
- Cube Watch (ayi-afk/aion2cubedetector, no license stated): only the facts that dr89mh credits to it (the object
  opcodes and template 1301199) are used, through the dr89mh documentation. Nothing was copied from its code.
- The aion2dps captures of 2026-10-11 (the user's own traffic, not distributed): the 1301199 sighting in map 1110
  described in `docs/PROTOCOL.md`. The one spawn body in the tracker tests holds no names.
