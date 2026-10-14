# Transport Fever 3 save format notes

Describes save format version **604**. All integers are little-endian.

The reader accepts versions 599 and newer. 599 and 601 have the same header
as 604. Versions 568 and 585 match it up to and including the preview and
then differ, so they are rejected with "unsupported format version" (see
`MIN_VERSION`).

## Container

A `.sav` file is zstd. The game writes one frame with the data, then an
empty frame (`28 b5 2f fd 20 00 01 00 00`). Neither frame has a checksum.
The decompressed data is one sequential stream, about 190 MB for a mid-size
map. **It has no offset tables and no section sizes**: every component and
system writes its fields in turn. A section can therefore be re-serialised
at a different length and spliced back between the surrounding bytes.

Notation: `str` is a `u32` length followed by that many bytes. `vec<T>` is a
`u32` count followed by that many `T`.

## Header

Parsed in full by `crates/lib/src/header.rs`. The field column uses the
Rust field names, and a dotted name in backticks is a field of a field.
Strings stored in the save, such as the keys of a Lua table, are in double
quotes.

| field | type | notes |
|---|---|---|
| magic | `"tf**"` | |
| `version` | u32 | 604 |
| `start_year` | u32 | year the game was started |
| `map_width`, `map_height` | u32, u32 | map size in metres, see [Map size](#map-size) |
| `date` | u32 | date shown in the game, as a Julian day number (2415021 is 1 January 1900) |
| `money` | i64 | copy of the journal balance, shown in the load dialog |
| `experience` | u32 | copy of the company's experience points; grows with play, not a year |
| `info_table` | lua table | key "company.level" (1 to 15) in most saves |
| `mods` | vec<{str ×5, u32}> | per mod: `id`, `source`, `path`, `name`, `extra`, `flags`; `extra` is the mod's web page URL when it has one, `flags` is 0, 1 or 2 |
| `achievements_earnable` | u8 | 1 if achievements can be earned, 0 if not; the game writes it at every save, from the mod list |
| `thumbnail.width`, `thumbnail.height` | u32, u32 | 640×360 |
| `thumbnail.rgb` | str | raw RGB8, rows bottom-up (as in BMP or OpenGL) |
| `stats_i64` | i64 ×6 | index 3 is a copy of the money |
| `stats_i32` | i32 ×11 | |
| `stats_pairs` | vec<(u32,u32)> | |
| `stats_u32` | u32 ×2 | |
| `stats2_i64` | i64 ×9 | |
| `stats2_u32` | u32 ×2 | |
| `labels` | vec<{str, u32}> | e.g. "%d Point(s) for Company Value" |
| `stats3_u32` | u32 | |
| `stats3_i64` | i64 ×2 | |
| `unknown_d` | u32 | meaning not known |
| `config.mod_ids` | vec<str> | |
| `config.resources` | vec<(str, str)> | keys "climate", "economy", "nameList" |
| `config.params` | vec<(str, lua table)> | the empty key "" holds the base settings: "advancedOptions.*", "townConfig.*", "map.size", … |
| `config.mission`, `config.kind` | str, str | |
| `config.achievements_earnable` | u8 | same value as `achievements_earnable` |
| `config.value` | u32 | meaning not known |
| `config.id` | str | |

The game configuration is stored only here, so editing it really changes
the game's settings.

### Map size

The header's width and height give the size of the map in metres. The
settings hold it as two options:

* "map.size" (Map Size) and "map.format" (Map Format) are 1-based positions
  in the New Game option lists: Map Size 1 is Tiny and 8 is Gigantomaniac,
  Map Format 1 is 1 : 1 and 5 is 1 : 5. Map Format is unrelated to the save
  format version.
* Each Map Size is a set area and the Map Format only changes its shape, so
  a 1 : 2 map of a size has about the same area as its 1 : 1 map, as a
  longer and narrower rectangle.
* A save can store a "map.size" or "map.format" that does not match its
  width and height.

| "map.size" | 1 : 1 map, m | km² |
|---|---|---|
| 1 Tiny | not seen | – |
| 2 Small | 8192 × 8192 | 67.11 |
| 3 Medium | 11264 × 11264 | 126.88 |
| 4 Large | 14336 × 14336 | 205.52 |
| 5 Very Large | 16384 × 16384 | 268.44 |
| 6 Huge | 20480 × 20480 | 419.43 |
| 7 Megalomaniac | 24576 × 24576 | 603.98 |
| 8 Gigantomaniac | 28672 × 28672 | 822.08 |

The Load Game dialog shows its "experimental" notice for a stored
"map.size" of 1 or 6-8 and a "map.format" of 4 or 5.

## Lua values

These serialise a `std::variant` and are used by script states and config:
a `u32` variant index followed by its payload.

| index | type | payload |
|---|---|---|
| 0 | nil | – |
| 1 | boolean | u8 |
| 2 | number | f64 |
| 3 | string | str |
| 4 | table | u8 non-null, then a table if non-null |

A table is a `u32` pair count followed by key/value pairs, in sorted (btree)
order.

## Player journal (money)

The engine keeps a journal per entity: a time-ordered list of bookings
followed by the cached balance. The player's money **is** that balance.

```
u32 count
count × { i64 time_ms, i64 amount, u8 type, u8 construction, u8 maintenance, u8 other, u8 carrier }
i64 balance        // == sum(amount)
```

Category enums:

* type: LOAN 0, INTEREST 1, CONSTRUCTION 2, ACQUISITION 3, MAINTENANCE 4,
  INCOME 5, OTHER 6, SUBSIDY 7
* construction: STREET 0, TRACK 1, SIGNAL 2, STATION 3, DEPOT 4,
  BULLDOZER 5, OTHER 6, WAREHOUSE 7
* maintenance: VEHICLE 0, INFRASTRUCTURE 1, OTHER 2, VEHICLE_MAINTENANCE 3
* other: OTHER 0
* carrier: ROAD 0, RAIL 1, TRAM 2, OTHER 3, AIR 4, WATER 5

The starting capital is booked as `(6, 6, 2, 0, 3)`, that is OTHER for every
field. To find the journal, the editor scans for a stream that validates as
a journal and whose balance equals the header's money. To change money, it
appends an OTHER booking for the difference and updates both header copies.
A save edited this way loads in the game with the new balance.

## Game script states

These are the persistent states of the `*.gs` Lua game scripts: company,
loans, towns, mission, achievements and so on.

```
u32 ?              // 0
u32 count
count × {
  str  mod         // "" for the base game
  str  name        // e.g. "game_mechanics/finance/loan.gs"
  u8   flag
  lua table state
  u8   has_subscriptions
  [vec<str> events]
}
```

The editor finds this block through the first `.gs` name and validates it
with a full parse.

## Time

Game time is in milliseconds. At calendar speed 1.00x a day lasts 4000 ms
and a year 1 461 000 ms; at other speeds the date shown moves faster or
slower against the same clock. A table after the game speed component maps
each tick to a date (below).

## Calendar

The game speed component sits in the entity data and nothing names it, so
the editor finds it by its bytes (`crates/lib/src/calendar.rs`) and requires
exactly one match. It is read only.

```
u64 9, u32 1
u32 millisPerDay      4000 at 1.00x, 0 when the calendar is stopped
u32 playSpeed         0 in a save written with the game paused
u64 0
u32 1, u64 8, u32 1
u64 t1                the game clock when saved
u8 1
u64 t2
u32 n
n x { u64 tick, u32 day }
```

`day` is a Julian day number and `tick` the clock value at which that day
started. The date of a clock value is the last entry at or before it, which
gives a booking its date at whatever speed the game ran. The header's `date`
is the last entry's day. The table has one entry per day while the game runs,
so the gap between entries is the day's length at the speed then; a stopped
calendar makes one long day. Not every table is one entry per day: dates can
be set, which leaves jumps.

Found exactly once, with the last day equal to the header `date` and ticks
never decreasing, in all 198 saves of format 599 to 604 in the mod.io catalog
(2026-10). See tf3-save-format's
[calendar.md](https://github.com/WuphonsReach/tf3-save-format/blob/main/docs/calendar.md).
