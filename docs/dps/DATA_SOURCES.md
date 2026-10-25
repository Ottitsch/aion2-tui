# Game data sources

The tables under `crates/meter-core/data` are compiled into the binary. They are the community's extractions
of NCSOFT's Aion 2 client data, redistributed under GPL-3.0 by the repositories below. This project is
GPL-3.0-or-later for that reason. Rebuild them with `scratchpad/tools/build_tables.py` (kept outside the repo)
or by hand from the sources.

| File | Source | Notes |
|---|---|---|
| `npcs.json` | taengu/A2Tools-DPS-Meter `src/data/i18n/npcs/en.json`, plus keys only present in Kuroukihime/AIon2-Dps-Meter `mobs.json` | `isDummy` is the flag, or a hard coded dummy code list, or a name starting with Punching Bag, Training Scarecrow or Balance Test |
| `skills.json` | A2Tools `src/data/i18n/skills/en.json` | skill id to English name |
| `skill_icons.json` | A2Tools `src/data/skill_icons.json` | first four digits of a class skill code to icon stem |
| `dot_skill_ids.json` | A2Tools `src/data/dot_skill_ids.json` | ids accepted from DoT tick packets (after dividing the wire value by 100) |
| `heal_skill_ids.json` | AIon2-Dps-Meter `healing_skill_ids.json` and NOIA2 `healing_skill_code.json`, normalised to base codes | damage records with these skills are heals |
| `resource_skill_ids.json` | A2Tools `src/data/resource_restore_skills.json` | MP/SP/DP restores that must not count as healing |
| `open_world_maps.json` | A2Tools `src/data/open_world_maps.json` | map ids that end a dungeon run |
| `player_summon_npcs.json` | A2Tools `src/data/player_summon_npcs.json` | NPC templates a player can summon |
| `dungeons.json` | A2Tools `src/data/i18n/dungeons/en.json`, extended with dungeon ids referenced from the NPC table | instance map id to name |
| `classes.json` | merged from AIon2-Dps-Meter `classes.json`, A2Tools `entity/job_class.rs`, NOIA2 `nickname.rs` | skill prefix id, names, icon code, job byte block |

Licenses: taengu/A2Tools-DPS-Meter GPL-3.0, Kuroukihime/AIon2-Dps-Meter GPL-3.0, ZDYoung0519/NOIA2 GPL-3.0.
The A2Tools tables were extracted from the Steam client builds named in that repository's `source` fields.

## Fonts

| File | Source | License |
|---|---|---|
| `public/fonts/PressStart2P-Regular.ttf` | google/fonts `ofl/pressstart2p` (Press Start 2P by codeman38) | SIL Open Font License 1.1, copy in `public/fonts/PressStart2P-OFL.txt` |

The terminal themes use Cascadia Mono, which ships with Windows Terminal and Windows 11, with Consolas as the
fallback; neither is bundled.
