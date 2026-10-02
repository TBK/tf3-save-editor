# TF3 Save Editor

A save game editor for **Transport Fever 3**, for Linux, Windows and macOS.

* `tf3se-gui`: desktop GUI built with [gpui-base](https://crates.io/crates/gpui-base)
* `tf3se-cli`: command-line tool with the same capabilities
* `tf3save`: Rust library that reads and writes the save format

> ⚠️ This is an unofficial tool. Close the game before editing, and keep
> copies of saves you care about. Overwriting a save in place creates a
> `.sav.bak` backup first.

## What can be edited

| | |
|---|---|
| **Money** | Set the balance. The difference is booked as an "Other" entry in the finance journal. |
| **Game settings** | The options chosen when the game was created: `advancedOptions.*` (costs, income, inflation, maintenance), town sensitivity, industry density, … |
| **Script state** | Every value in the game's Lua script states: loans, company progression, towns (level, experience), subsidies, mission and achievement progress, … |
| Read-only | Title, year, mods, preview image, journal history |

Values keep their type: numbers stay numbers, booleans stay booleans.
Adding or removing table entries isn't supported.

## GUI

```
tf3se-gui [path/to/save.sav]
```

Click **Open…** and pick a `.sav` file. Use the tabs on the left to move
between sections. In *Game settings* and *Script state*, click a value, type
the new value and press Enter or **Apply**. Click **Save** to overwrite the
file (a backup is made first), or **Save as…** to write a copy.

## CLI

```sh
tf3se-cli info  game.sav                           # summary
tf3se-cli money game.sav                           # print balance
tf3se-cli money game.sav --set 50000000            # set balance (writes game.sav.bak)
tf3se-cli money game.sav --set 5000000 -o new.sav  # write to another file
tf3se-cli journal game.sav --last 20               # recent bookings
tf3se-cli settings game.sav                        # list settings
tf3se-cli settings game.sav --set advancedOptions.inflationFactor=1
tf3se-cli scripts game.sav                         # list script states
tf3se-cli scripts game.sav loan                    # dump one (substring match)
tf3se-cli set game.sav loan availableLoans.1.percentage 0.01
tf3se-cli thumbnail game.sav preview.png
```

The save can also be given with `--save` / `-s`, before or after the
command. This is handy for running several commands on the same file:

```sh
tf3se-cli --save game.sav money --set 50000000
tf3se-cli -s game.sav set loan availableLoans.1.percentage 0.01
```

## Downloads and verification

Each release has an archive per platform. On Linux there are two kinds:

* `…-x86_64-unknown-linux-gnu` / `…-aarch64-unknown-linux-gnu`: GUI and CLI
  for regular glibc distributions.
* `…-x86_64-unknown-linux-musl` / `…-aarch64-unknown-linux-musl`: a fully
  static CLI that runs on any Linux, including Alpine and minimal
  containers. The GUI can't be built statically, because it loads the
  graphics libraries at runtime.

Releases are built by GitHub Actions and carry
[build provenance attestations](https://docs.github.com/actions/security-for-github-actions/using-artifact-attestations).
To check a download:

```sh
gh attestation verify tf3-save-editor-<version>-<target>.tar.gz --repo tbk/tf3-save-editor
```

macOS builds aren't notarised. After extracting, run
`xattr -d com.apple.quarantine tf3se-gui tf3se-cli` once.

## Releasing

Bump `version` in the root `Cargo.toml`, commit, and push a matching
semantic-version tag **without** a `v` prefix (for example `1.2.0`, or
`1.3.0-rc.1` for a pre-release). The release workflow refuses a tag that
doesn't match the Cargo version.

## Building

Requires Rust 1.99 or newer (edition 2024).

```sh
cargo build --release                      # library + CLI
cargo build --release -p tf3se-gui         # GUI
```

Static CLI (needs `musl-tools` on Debian/Ubuntu, `musl-gcc` on Fedora):

```sh
rustup target add x86_64-unknown-linux-musl
CC_x86_64_unknown_linux_musl=musl-gcc \
  cargo build --release -p tf3se-cli --target x86_64-unknown-linux-musl
```

Both binaries use [mimalloc](https://github.com/microsoft/mimalloc) as
their allocator. It loads large saves about 25% faster than the system
allocators.

GUI build dependencies on Linux (Debian/Ubuntu):

```sh
sudo apt-get install build-essential pkg-config libfontconfig-dev libfreetype-dev \
  libxcb1-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libvulkan-dev
```

On Fedora: `gcc-c++ fontconfig-devel libxcb-devel libxkbcommon-x11-devel wayland-devel vulkan-loader-devel`.
Running the GUI needs a Vulkan driver.

Tests that need a real save run only when `TF3_TEST_SAVE` points to one of
your own saves. Saves are never checked in, as they may contain material
that cannot be redistributed.

```sh
TF3_TEST_SAVE=~/game.sav cargo test -p tf3save
```

## How it works

See [docs/FORMAT.md](docs/FORMAT.md) for the save format. In
short: the save is a zstd-compressed sequential stream. The header is parsed
in full. The money journal and the script states are found by validated
search, re-serialised after editing, and spliced back between the untouched
bytes. Loading a save and writing it back unchanged reproduces the game's
uncompressed data byte for byte, which shows every parsed field is written
back exactly. The `.sav` file itself can still differ in size, because the
editor's zstd compression settings aren't identical to the game's.

## License

Licensed under either of

* Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
* MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in this work, as defined in the Apache-2.0 license,
shall be dual licensed as above, without any additional terms or conditions.

Transport Fever 3 is a trademark of its respective owners. This project is
not affiliated with or endorsed by the game's developers or publisher.
