# Data sources

The tables under `crates/kina-core/data` are compiled into the binary. They are copied unchanged from the
aion2dps meter, which took them from the community's extractions of NCSOFT's Aion 2 client data, redistributed
under GPL-3.0. This project is GPL-3.0-or-later for that reason.

| File | Source | Notes |
|---|---|---|
| `dungeons.json` | taengu/A2Tools-DPS-Meter `src/data/i18n/dungeons/en.json`, extended in aion2dps with dungeon ids referenced from its NPC table | instance map id to name, tier and category |
| `open_world_maps.json` | A2Tools `src/data/open_world_maps.json` | map ids that are open world, not instances |

The transport layers (`net.rs`, `tcp.rs`, `frame.rs`, `wire.rs`) are copied unchanged from aion2dps
`crates/meter-core`, whose protocol knowledge is credited in its own `docs/PROTOCOL.md`.

Licenses: taengu/A2Tools-DPS-Meter GPL-3.0.
