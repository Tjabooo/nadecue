# Changelog

## v0.2.0 - 2026-11-17

- `import`: lineup packs (JSON array of lineups; `id` updates in place)
- `shoot`: takes every lineup's aim picture in CS2 over the `-netconport` console (setpos/setang)
- Radar pictures: where to stand and where it lands, shown in the card's corner
- `find` asks a language model (any OpenAI-compatible endpoint) with a lineup-picking prompt when word matching isn't sure

## v0.1.0 - 2026-11-16

- Lineup card appears when you pull out a grenade that has lineups on this map and side, and closes when you throw it or switch weapons
- Last-thrown lineup first, strip of the others, `next` / `hide` / `forget`
- `save`: crosshair close-up + whole view + throw style + name + note, from one key press
- `find`: any-order, typo-tolerant lookup by map, side, grenade and name (for voice assistants)
- Arrow, distance and aim offsets when CS2 reports positions (spectating)
