# Testing

What there is to run, what each thing proves, and how to run it without
getting in the way of the person whose machine it is. Every script also
documents itself at the top of the file; this page is the index.

## Ground rules

These hold for people and agents alike.

- **Never use the live desktop for a game.** Run titles in a headless
  gamescope (`tools/headless-title.sh`), and GUI tests under
  `xvfb-run` or `tests/run_wayland_gui.py`. A game window on the real
  desktop takes the keyboard focus from whoever is working.
- **Keep windows small.** 1280x720 for games, never fullscreen. Screenshots
  show the test display only, never the desktop.
- **Keep the machine usable.** Run builds and tests under `nice`/`ionice`, or in a
  scope with low `CPUWeight`/`IOWeight`, as `headless-title.sh` does. Prefer a
  light title (Stardew Valley, `9MWR1NC6VQ6L`) unless the test needs a
  particular game.
- **Put scratch space under `/var/tmp`**, not `/tmp`. On many desktops `/tmp`
  is RAM. That includes `CARGO_TARGET_DIR` for side builds. Remove what you
  created when you are done, but never a directory a running test still uses.
- **Keep tests away from the real keyring and accounts.** The GUI tests run
  on a private D-Bus session (`dbus-run-session`) with a fake client and a
  throwaway `HOME`. A test that reaches the user's keyring produces a KWallet
  prompt on their screen.
- **Never ask for or store credentials.** Live checks use the account that is
  already signed in to the launcher, through the launcher.

## 1. Unit tests (Rust)

```sh
cargo test --workspace                    # what CI runs
cargo test -p ferestre-core launch_options   # one module
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

All the decisions the GUI makes live in `crates/ferestre-gui/src/model.rs`
and are unit-tested there. `ferestre-core` tests also read the shipped
recipes in `titles/`, so a recipe change can fail a Rust test.

Recipes have their own checks:

```sh
python3 titles/validate.py                # schema and capability names
python3 titles/test_validate.py
python3 .github/scripts/gen-compatibility.py --check   # docs/COMPATIBILITY.md is current
scripts/check-sources.sh [--remote]       # runtime source pins and the lock agree
```

## 2. Offline script tests (Python)

These run the shipped CLI and scripts against stand-ins: no network, no account, no
game. Each is `python3 tests/<name>.py`; CI's `test` job lists them all.

| Script | What it checks |
|---|---|
| `test_download_cancel.py`, `test_transfer_controls.py` | cancellation and pause/resume reach the downloader |
| `test_media_dependencies.py`, `test_client_dependencies.py` | `ferestre doctor` names a missing library |
| `test_real_path_image_view.py`, `test_caller_ids_image_view.py` | the private real-path view of a licensed image (needs unprivileged user namespaces) |
| `test_proton_wine_shim.py`, `test_launch_gdk.py`, `test_fix_xcurl.py`, `test_install_xodus_proton.py` | the launch scripts in `scripts/` |
| `test_run_already_running.py` | `ferestre run` refuses, with exit code 4, a title already running in its prefix (a fake `FakeGame.exe` in a throwaway prefix under `/var/tmp`), or one another `run` is still starting (it holds `<prefix>/.ferestre-run.lock`); `--force` does not get past it, `--dry-run` does not check; the refusal is appended to the launch log and, outside the window, sent to a fake `gdbus` as a desktop notification |
| `test_game_session.py` | display path, HDR and performance-profile selection |
| `test_no_nvidia_urls.py` | no address of an NVIDIA file anywhere in the repository (also its own `guards` workflow) |
| `test_build_dlssnr.py` | `scripts/build-dlssnr.sh`: the pinned DLSSNR-AMD commit, the packaged layout, and no NVIDIA model in it |
| `test_workflows.py`, `test_source_pins.py`, `test_runtime_staging.py`, `test_release_signing.py`, `test_build_appimage.py` | CI/release invariants, source pins, signing, the AppImage build |
| `test_ci_changes.py` | which Rust CI jobs a pull request runs: every file the `sources` and `gui` jobs name, and every test in the GUI shard manifest, switches that job on (`.github/scripts/ci-changes.sh`) |
| `test_runtime_inputs.py` | `scripts/runtime-inputs.py`: the runtime inputs digest, and every rule for promoting a `main` runtime build to a tag, against API responses recorded in `tests/fixtures/runtime-inputs/` (docs/RELEASING.md, "Runtime promotion") |
| `tools/test_tool_hygiene.py`, `tools/test_xsts_probe.py` | the developer tools below (`python3 -m unittest discover -s tools -p 'test_*.py'`) |

## 3. GUI tests (the real window, through AT-SPI)

Each `tests/gui_*.py` starts the built `ferestre-gui` in a private home, drives
it through the accessibility bus as a person would, and checks what it shows.
No test hooks are compiled into the binary. Build first, then run under a
private X server and D-Bus session, as CI does:

```sh
cargo build -p ferestre-gui -p ferestre-cli
xvfb-run -a --server-args='-screen 0 1280x900x24 -noreset' \
    dbus-run-session -- python3 tests/gui_smoke.py
```

**`tests/run_gui_ci.py`** runs them the way CI's `gui` job does. Each test in
`tests/gui_ci.json` gets its own Xvfb (a free display picked by Xvfb itself,
so nothing collides) and its own session bus, at the screen size the manifest
names; every test runs even after one fails, and it prints each test's time:

```sh
python3 tests/run_gui_ci.py --list               # the four-way split, by weight
python3 tests/run_gui_ci.py --shard 2/4          # one CI shard
python3 tests/run_gui_ci.py --only smoke,move    # named tests
```

The manifest lists every `tests/gui_*.py` (`test_workflows.py` checks it),
with a weight: seconds measured in CI. A new GUI test goes into the manifest
with its weight, and the split rebalances itself. `gui_smoke.py` alone is
about 150 s, the floor for any number of shards. On a desktop, pass
`--dbus-config` with a session config that activates only the accessibility
bus, so no test can start the real keyring daemon.

Tests that need a real Wayland compositor (focus, window controls) run inside
a private KWin session instead:
`python3 tests/run_wayland_gui.py tests/gui_running.py --binary target/debug/ferestre-gui`.
A test that does not aim the X pointer, such as `gui_already_running.py`,
also runs there where there is no Xvfb; `gui_smoke.py` needs X.

| Script | Covers |
|---|---|
| `gui_smoke.py` | the main window end to end, including the recipe editor and its launch settings |
| `gui_local_games.py` | a local game's row, menu, anti-cheat caution, Add to Steam and forget-only removal |
| `gui_accounts.py`, `gui_accounts_controller.py` | separate Store and Player accounts |
| `gui_presence.py` | "Show me online" while a game runs |
| `gui_launch.py`, `gui_running.py` | the launch spinner, the running state, Resume and Stop |
| `gui_already_running.py` | Play on a title already running in its prefix, started elsewhere: Leave It Running, Close It and Close and Start Again, against a fake game, `wine` and `wineserver`, with a second fake game in another prefix that must be left alone; and the editor's Reset to defaults |
| `gui_notifications.py` | notifications stack: three fired together are all on screen at once in a 1280x900 window, newest at the bottom; Dismiss notification, a failed launch replacing its own "Starting", and F6 then Escape from the keyboard |
| `gui_transfers.py`, `gui_update_existing.py`, `gui_update_resume.py` | the install/update queue |
| `gui_move.py` | moving an installation between drives |
| `gui_controller*.py` | controller navigation, glyphs, focus, the on-screen keyboard |
| `gui_artwork.py`, `gui_library_error.py`, `gui_client_deps.py`, `gui_runtime_selection.py`, `gui_updater.py`, `gui_window_controls.py` | as named |

The fixtures they use are `fake_packagekit.py`, `https_fixture.py`,
`account_roles_fixture.py`, `related_license_fixture.py`,
`store_catalog_fixture.py`, `license_token_fixture.py` and `ws_server.py`. Each
one is a local stand-in and documents itself.

### README screenshots

**`tools/readme-screenshots.py`** regenerates `docs/screenshots/*.png` from
the GUI fixtures above. It is not a test, and CI does not run it. Each picture
runs in its own user, mount, hostname and PID namespace. In it, `/home`,
`/run/user`, `/run/media`, `/media` and `/mnt` are empty, the home folder is
`/home/player`, the hostname is `ferestre-demo` and the account is the
synthetic "Player One". The GUI runs in
a private virtual KWin session with the default renderer, so the dialog blur
appears as it does for users. Artwork is public Store art copied from
`~/.cache/ferestre/catalog`, stripped of everything but product metadata. The
script refuses to start the GUI if the real home folder is reachable.

```sh
CARGO_TARGET_DIR=/var/tmp/ferestre-readme-target nice cargo build --release --offline -p ferestre-gui
nice python3 tools/readme-screenshots.py                       # all of them, into docs/screenshots
python3 tools/readme-screenshots.py --only home --out /var/tmp/shots   # one, for a look first
```

Look at every picture before committing it.

**CI runs GTK 4.14 (Ubuntu 24.04).** AT-SPI naming and pointer behaviour differ
from newer GTK. Expect a GUI test that passes locally to fail in the `gui` job
now and then, and the reverse. To check before pushing, run `run_gui_ci.py`
in an Ubuntu 24.04 container with CI's packages (`libgtk-4-dev
libadwaita-1-dev at-spi2-core gir1.2-atspi-2.0 python3-gi openbox xvfb
dbus-x11`, plus Rust to build). Run the tests as an ordinary user, not root
(`gui_updater.py` fails as root), and give the container `--shm-size=1g` or
more (`gui_move.py` moves 70 MB into `/dev/shm`, and Docker's default is
64 MB).

### What CI runs on a pull request

`.github/workflows/rust.yml` runs every job on a push to `main` or `dev`. On a
pull request it runs only what the change can affect, decided by
`.github/scripts/ci-changes.sh` in the hosted `preflight` job, which also
checks formatting:

| Job | Runner | On a pull request |
|---|---|---|
| `preflight` | hosted | always: rustfmt, and which jobs below the change needs |
| `test` | self-hosted, one build slot (`FERESTRE_RUNNER_LABELS`) | always: clippy, the workspace tests and every offline script test, in a build directory that outlives the job |
| `msrv` | hosted | when `crates/`, `Cargo.toml` or `Cargo.lock` change; `cargo +1.88 check --all-targets` instead of the full test run |
| `gui-build`, then `gui` ×4 | hosted | when the crates, `titles/`, `tests/fixtures/`, the GUI tests, their drivers, `run_gui_ci.py` or `gui_ci.json` change; built once, tested in four shards at once |
| `sources` | hosted | when `sources.lock.json`, `.gitmodules`, `third_party/`, `scripts/check-sources.sh`, `scripts/source-pins.py` or the license fixtures change |
| `ci-ok` | hosted | always, last: fails unless `preflight` and `test` passed and every other job passed or was skipped; the one check to require |

The workflow and `.github/scripts/ci-*.sh` switch all of them on. A newer push
cancels the pull request's older run; runs on `main`, `dev`, tags and manual
runs are never cancelled. `guards.yml` cancels the same way, and also
cancels an older push to a branch other than `main` and `dev`. A test that
hangs is killed with everything it started (`run_gui_ci.py` runs each in its
own session); a shard job stops after 25 minutes. A skipped job is not a pass: when a change needs a
job the filter does not see, add the path to `ci-changes.sh` and to
`tests/test_ci_changes.py`.

Measured locally on four cores (2026-11-22): the `test` job's cargo work
takes about 66 s from an empty build directory and 20-35 s warm, alone or with
a second pull request's `test` running at once; the GUI shards take
117-139 s each in the GTK 4.14 container. Before this layout a pull request
took about 42 build-slot minutes and 20-34 minutes of wall clock with two
pull requests open; it now takes one slot for a few minutes.

The runtime and release workflows run on hosted runners only; how a runtime
tag reuses `main`'s build is in docs/RELEASING.md ("Runtime promotion").
`scripts/build-runtime.sh <work dir> --until clone` clones the locked Proton
graph and checks it against `sources.lock.json` locally in a few minutes,
without Docker.

## 4. Runtime suites (Wine DLLs under Wine)

`tests/run-*.sh` compile a small Windows test program, then run it under the
built Wine against the DLL being tested. They need the Proton build tree
(`XODUS_BUILD_DIR`, see `scripts/xodus-env.sh`) and Docker for the
cross-compiler.

| Script | DLL / area |
|---|---|
| `run-tests.sh` | xgameruntime: saves, Store, task queues, licences |
| `run-account-roles-tests.sh` | xgameruntime: Store/Player account roles, async completion callbacks |
| `run-xnetworking-tests.sh` | XNetworking certificate checks against a loopback HTTPS fixture |
| `run-gameinput-tests.sh` | gameinput (`GAMEINPUT_ARCH=i386` for 32-bit) |
| `run-ntdll-tests.sh`, `run-appmodel-tests.sh`, `run-concrt-tests.sh` | ntdll, kernelbase package identity, concrt140 |
| `check-runtime-modes.sh` | display modes, inside the SDK with its own headless compositor |

Most suites accept `REBUILD=1` to rebuild the DLL first, plus overrides for
testing a DLL from elsewhere: `BUILD_DIR`, `TOOL_DIR` (a runtime to copy
Wine from) and a per-suite `*_TEST_DLL`/`*_TEST_UNIXLIB`. Read the header of
each script for its own.

`tools/in-container.sh` runs the compiler in the Proton SDK image. With a
local Docker it mounts the trees. With a remote one (a `docker context` or
`DOCKER_HOST` that is not a local socket) it streams the sources in and the
binary out, which covers every suite's compile step. It refuses `REBUILD=1`'s
`make`, which needs the build tree mounted: run that build on the host.

`tests/stage-runtime.sh SRC DEST` makes an independent copy of a runtime to
patch for a test, so a test never writes into an installed one.

## 5. Real titles, end to end

These use the installed launcher, the signed-in account and real game files.
They prove that a title launches and runs, which nothing above can.

**`tools/headless-title.sh <product-id> [seconds]`** runs one installed title
through `ferestre run` in a headless gamescope, in its own systemd scope. It
samples which game executables are alive every 30 s and takes a screenshot
each time. Afterwards it stops only what it started and prints a private
output directory containing `summary.txt`, `shot-*.png`, `launch.log` and
`game.stderr`. A title passes if its executable is still listed at the last
sample, the screenshots show the game rather than an error dialog or a
black screen, and `game.stderr` shows no crash before the final stop.
`FERESTRE=` picks a different AppImage, such as a preview build.
`INPUT=<script>` plays a `tools/wine_input_bot.c` script into the game at
`INPUT_AT` seconds, from inside the game's prefix, to get past title screens
and menus. For example, `focus Batman`, then `key 0D` with waits between the
presses. The script header says how to build the bot.

**`tools/wait-for-install.sh`** blocks until the install queue finishes
another title, then prints `installed: <ID>.json`, or `queue empty`. Run it
in the background and follow it with `headless-title.sh`.

```sh
tools/wait-for-install.sh            # background; wakes when a title lands
tools/headless-title.sh 9MWR1NC6VQ6L 120
```

Some titles have drivers that go past the menus without a person. They are
older, and they drive the live desktop's KWin and input, so use them only when
the person has said so:

| Script | Title |
|---|---|
| `tools/drive-fh5.py` | Forza Horizon 5, through its welcome screen |
| `tools/drive-aoe.py` | Age of Empires DE, into a match |
| `tools/auto-join.py` | Minecraft Bedrock server-join freeze reproduction |

They keep their output private through `tools/private_out.py`.

## 6. Diagnostic tools

Use these when a title fails and the log does not say why.

| Tool | Use |
|---|---|
| `tools/pe-inspect.py`, `tools/peek-image.py`, `tools/peek-fault.py` | read a decrypted, memfd-only game image while a launch is in flight (entry point, TLS, bytes at a faulting address) |
| `tools/pe-holes.py` | find elided call sites in a PE image |
| `tools/wgs_dump.py`, `tools/wgs_selftest.sh` | inspect, and exercise the writes of, a Gaming Services save folder |
| `tools/xsts_probe.py`, `tools/msa_probe.py`, `tests/endpoint_suite.py` | which service tokens and endpoints a title can reach. Never prints token material |
| `tools/*.c` probes | small Windows programs for one API each, built with `tools/in-container.sh` |

For a launch problem, start with `ferestre doctor`, then
`~/.local/state/ferestre/launches/<id>.stderr`. After that, look at the
title's `<slug>-launch.log` in the games directory. A known failure has a
`log-match` in its recipe's `[[issues]]`.
