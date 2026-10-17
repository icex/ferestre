# Release 0.1.7 qualification

Launcher **0.1.7** uses runtime **11.0.20261010.2**, unchanged from 0.1.6.

## Included fix

- **Gaming Services wait.** Before loading `gameplatformservices.dll`, PlayFab
  Party waits for the Microsoft Gaming Services watchdog for up to
  `HKLM\Microsoft\GamingServices\ServiceRetryTimeout`, 60 seconds when unset.
  Wine has no Gaming Services, so every wait ran out. The Proton shim now sets
  the timeout to 0, once per prefix, through Proton so that a new prefix is
  created first. A value already in the prefix is left alone. The decrypted
  game images are not passed to that `reg` process.

## How it was found

Quit Game in Goat Simulator 3 left the process running in every mode, including
straight from the title screen. A read-only stack dump of the stuck process
showed the main thread inside the game's `Party.dll`, in `WaitForMultipleObjects`
on the watchdog's two events. Disassembling the loader around that wait showed
the `ServiceRetryTimeout` read and the `gameplatformservices.dll` load attempt.
The same 60-second wait explains the online-mode stall: online play goes
through Party constantly.

## Live evidence and limits

| Title | Observed result | Remaining scope |
|---|---|---|
| Goat Simulator 3 | Clean prefixes, automated, released runtime. Offline: a new save reached the world in about 40 s, played, autosaved, returned to the title and reloaded after a relaunch at the Goat Tower. Online enabled, with the value set by hand: a new save reached the world in under 20 s, ran at 60 fps (DXVK counter), took movement and menu input, saved, and the save reloaded online after a relaunch. On a fresh prefix the new shim wrote the value before the first start, and online mode reached the menu. Quit Game exited in 2–4 s with status 0 in every run, offline and online; before the fix it had not exited after 2–4 minutes in any of seven runs | Peer joining and voice are untested. The intro, offline and online alike, runs about three minutes before control is handed over |

A `gameplatformservices.dll` forwarding to the runtime's `xgameruntime.dll` was
also tried alongside the value. Online play behaved the same with and without
it, so it is not shipped.

## Verification

- `tests/test_proton_wine_shim.py`: a new prefix gets the value before the game
  starts, without the image map; a prefix with any value is not touched; the
  same value name under another key does not count. The new tests fail against
  the 0.1.6 shim.
- Rust workspace tests, `cargo fmt` and Clippy with `-D warnings`.
- `gen-compatibility --check`.
