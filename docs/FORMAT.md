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

Parsed in full by `crates/lib/src/header.rs`.

| field | type | notes |
|---|---|---|
| magic | `"tf**"` | |
| version | u32 | 604 |
| ? | u32 ×3 | |
| date | u32 | date shown in the game, as a Julian day number (2415021 is 1 January 1900) |
| money | i64 | copy of the journal balance, shown in the load dialog |
| experience | u32 | copy of the company's experience points; grows with play, not a year |
| info | lua table | usually empty |
| mods | vec<{str ×5, u32}> | id, source, path, display name, extra, flags |
| ? | u8 | 0 or 1; not a "has preview" flag, as saves with 0 still carry a full preview |
| width, height | u32, u32 | 640×360 |
| preview | str | raw RGB8, rows bottom-up (as in BMP or OpenGL) |
| stats | i64 ×6 | index 3 is a copy of the money |
| stats | i32 ×11, vec<(u32,u32)>, u32 ×2, i64 ×9, u32 ×2 | |
| labels | vec<{str, u32}> | e.g. `"%d Point(s) for Company Value"` |
| stats | u32, i64 ×2, u32 | |
| config.mods | vec<str> | |
| config.resources | vec<(str, str)> | `climate`, `economy`, `nameList` |
| config.params | vec<(str, lua table)> | the empty key holds the base settings: `advancedOptions.*`, `townConfig.*`, `map.size`, … |
| config.mission, kind | str, str | |
| config flag, value | u8, u32 | |
| config.id | str | |

The game configuration is stored only here, so editing it really changes
the game's settings.

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
followed by a stored balance. The player's money is the sum of the amounts.

```
u32 count
count × { i64 time_ms, i64 amount, u8 type, u8 construction, u8 maintenance, u8 other, u8 carrier }
i64 balance        // the header's money; sum(amount) in most saves
```

The stored balance and the sum differ in some saves (67 of the 198 catalog
saves that load). In one of them the header holds 0 and the sum is
-18,153,221; the game shows -18,153,221 (**Observed**, one save).

Category enums:

* type: LOAN 0, INTEREST 1, CONSTRUCTION 2, ACQUISITION 3, MAINTENANCE 4,
  INCOME 5, OTHER 6, SUBSIDY 7
* construction: STREET 0, TRACK 1, SIGNAL 2, STATION 3, DEPOT 4,
  BULLDOZER 5, OTHER 6, WAREHOUSE 7
* maintenance: VEHICLE 0, INFRASTRUCTURE 1, OTHER 2, VEHICLE_MAINTENANCE 3
* other: OTHER 0
* carrier: ROAD 0, RAIL 1, TRAM 2, OTHER 3, AIR 4, WATER 5

The starting capital is booked as `(6, 6, 2, 0, 3)`, that is OTHER for every
field. To change money, the editor appends an OTHER booking so that the sum
becomes the new value, stores the new value as the balance and updates both
header copies. A save edited this way loads in the game with the new balance.

### Where the journal is

The player's journal sits at a fixed place in the entity data, near the start
of the stream (between offsets 973,548 and 2,001,292 in the 198 catalog saves
that load). It follows a table of the "edge_addons/*.edge" resources:

```
count × { str mod, str path, u32 index }   // base game: path ends "barrier_track_b.edge", index 5
u32 ?                                      // 18, 21 or 24
vec<u32> ids
u32 ?
u32 1
u8 0
journal
```

The `u32` before the ids is 24 in 415 of 766 saves checked, 18 in 337 and 21
in 14 (all early games, experience 2,046 or less). It is 18 in 292 of the 315
map-editor saves, 24 in 22 and 21 in 1. It does not follow the format version,
the climate, the number of mods, the map size or the start year.

Mods add their own entries after the base game's. The stored balance is not
used to find the journal, so it is found when the header money is 0 or does
not match. Map-editor saves have a journal at the same place. In 275 of the 316 saves in
the map folders the header money and the stored balance are 0 while the
amounts sum to a non-zero value. In one catalog save the journal starts 12 bytes after the usual
place. The editor accepts that only if the stored balance equals the header's
money.

When the table is not found, the editor falls back to searching the stream
for a journal whose stored balance equals the header's money and the sum of
its amounts, and uses it only if exactly one matches.

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
