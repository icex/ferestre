# Release 0.1.8 qualification

Launcher **0.1.8** uses runtime **11.0.20261010.2**, unchanged from 0.1.6.

## Included fixes

- **Install directory from the install record.** A recipe that names no
  `[install] dir` now uses the directory the launcher recorded when it installed
  the title, while that directory exists, and the slug otherwise. A title
  downloaded before it had a recipe went to `<product id>`; a recipe shipped
  later defaults to its slug, so the only way to find the game had been a copy
  of the recipe in `~/.config/ferestre/titles/`. Such a copy replaces the
  shipped recipe as a whole, so it also froze the title's state, summary,
  requirements and environment at the time it was written. Retro Classics kept
  showing **Issues**, and setting `VKD3D_CONFIG` outright, after the shipped
  recipe had moved on. A recipe's own `dir` and `install.dir-env` still win.
- **Goat Simulator 3 is playable.** The player confirmed online play on 0.1.7,
  after offline play earlier. Peer joining and voice remain untested.

## Upgrading

A personal recipe that only existed to hold a product-id install directory can
be deleted after upgrading; the title is still found, and the shipped recipe
applies again. Keep a personal recipe that changes anything you want kept, such
as a different prefix, and compare it with the shipped one.

## Live evidence and limits

| Title | Observed result | Remaining scope |
|---|---|---|
| Retro Classics, Hollow Knight: Silksong | With the personal copies removed, 0.1.8 resolves the installs under `9mtvj3hhtqgs` and `9n116v0599hb` from their records, with the same prefixes as before; 0.1.7 resolved the slug directories, which do not exist | — |
| Goat Simulator 3, Minecraft Dungeons II, Halo Infinite | Install directory and prefix resolve as in 0.1.7 | — |
| Goat Simulator 3 | Player-confirmed offline and online on 0.1.7 | Peer joining and voice |

## Verification

- `paths::tests::a_recorded_install_is_found_without_a_recipe_naming_its_directory`:
  a recorded directory is used, a recipe's own `dir` still wins, and a record
  whose directory has gone is not followed. It fails without the change.
- Rust workspace tests, `cargo fmt` and Clippy with `-D warnings`.
- `gen-compatibility --check`.
