# Transport Fever 3 save format notes

Describes save format version **604**. All integers are little-endian.

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
| ? | u32 ×4 | |
| money | i64 | copy of the journal balance, shown in the load dialog |
| year | u32 | in-game year |
| info | lua table | usually empty |
| mods | vec<{str ×5, u32}> | id, source, path, display name, extra, flags |
| has preview | u8 | |
| width, height | u32, u32 | 640×360 |
| preview | str | raw RGB8, top-down |
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

Game time is in milliseconds. A default year lasts 730 500 ms; for example,
a 4-year loan has a duration of 2 922 000.
