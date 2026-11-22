# Local games and neural rendering

## Intent

Ferestre should be the one launcher for all of a person's games, not only the
Xbox/Microsoft Store titles it downloads. Windows games already on disk (GOG,
Steam, other stores), many of them already set up as Steam non-Steam
shortcuts with their own Proton prefixes, saves and launch options, come into
Ferestre's library without losing any of that.

On top of that, an experimental per-game "neural rendering" switch runs
NVIDIA's DLSS 5 neural rendering network on AMD RX 9000 (RDNA4) cards through
[DLSSNR-AMD](https://github.com/TheAutomatic/DLSSNR-AMD) (MIT). It is meant for
single-player games; games with anti-cheat are warned about or refused.

Two pull requests, in order: **A. Local games**, then **B. Neural rendering**
(B builds on A; Xbox titles can use B too).

## Goals

A. Local games
- A recipe can describe a game Ferestre did not install: `[title] kind = "local"`,
  an absolute game folder, an absolute or managed prefix, executable arguments,
  wrapper commands (e.g. `gamemoderun`) and environment.
- Import every non-Steam shortcut from Steam (`shortcuts.vdf`, all Steam
  users), keeping: name, executable, start dir, launch options (env before
  `%command%`, wrapper commands, args after it), the shortcut's prefix
  (`STEAM_COMPAT_DATA_PATH` in its launch options, else Steam's
  `steamapps/compatdata/<appid>` when it exists), the shortcut's Proton
  (Steam `config.vdf` `CompatToolMapping`) and its grid artwork.
  Ferestre's own shortcuts are skipped. Import never modifies Steam.
- Add a game by hand: pick an `.exe`; name and folder are derived, the person
  can edit them.
- Local games launch through the same `ferestre run <id>` entry point and the
  existing Proton shim, without any Xbox service: no `xodus-cli`, no licence,
  no xodus-service, presence, WinRT registration, PlayFab or account roles.
- Proton builds Steam knows about (`compatibilitytools.d/*`, Steam's own
  `steamapps/common/Proton*`) are discoverable runtimes, so an imported game
  keeps its GE-Proton/Proton version. Xbox titles keep their current runtime
  rules (an external Proton never satisfies GDK capability requirements).
- Library shows local games (installed = executable exists); Play, Add to
  Steam, edit recipe, and anti-cheat warnings work for them.
- Anti-cheat detection by file signature covering at least: Easy Anti-Cheat,
  BattlEye, Ricochet (`randgrid.sys`), Riot Vanguard (`vgk.sys`, `vgc.exe`),
  EA AntiCheat (`EAAntiCheat.GameServiceLauncher.exe`), nProtect GameGuard
  (`GameGuard/`), ACE (`SGuard64.exe`, `ACE-Base*`), XIGNCODE3 (`x3.xem`),
  mhyprot (`mhyprot*.sys`). Each entry says whether it is a kernel driver.

B. Neural rendering
- A Linux DLSSNR-AMD package built by Ferestre from a pinned upstream commit
  (upstream publishes no Linux release asset), published as a Ferestre
  release asset and downloaded with a pinned SHA-256, the way the anti-cheat
  runtimes are.
- The NVIDIA model is obtained from `nvngx_dlssnr.dll` (accepted only when its
  SHA-256 is 310.9.16 `2bc19bc09a7b89dd44f73e2ac0108fcc586e2154bdd0830f242f4f040e28789f`
  or 310.8.0 `e16bcf15e16e13f527491cdf7845b2fe6521a738d8f7c9c721866a8496e1fc8e`),
  found, in order: an explicit path the person gives; a scan of known game
  locations (Steam libraries, Heroic installs, local-game folders, Ferestre
  install folders); inside an installed Ferestre Xbox package whose layout
  lists it (decrypted with the person's own licence through `xodus-cli`);
  a download URL the person configured (`[dlssnr] dll-url` in
  `~/.config/ferestre/settings.toml`, or `FERESTRE_DLSSNR_DLL_URL`).
  **No URL for the DLL is ever written into this repository.** The model is
  extracted once and cached; the DLL itself is not kept.
- Per game: enable/disable, route (auto-detected: `optiscaler` when the game
  ships DLSS/FSR/XeSS and is 64-bit; else `reshade-feeder` for DX10-12,
  `reshade-feeder-vulkan` for Vulkan, `reshade-feeder-dx9` for DX9), starting
  network. Enabling installs into the executable's folder with upstream's
  `install.sh` run non-interactively; files it would overwrite are backed up
  first and restored on disable; the needed `WINEDLLOVERRIDES` (and int4
  Vulkan layer env) are merged into the launch. The install is re-checked
  before every launch and redone when missing (game updates).
- Anti-cheat gate: a kernel-level anti-cheat refuses enabling; any other
  detected anti-cheat requires an explicit acknowledgement, remembered per
  game and anti-cheat.
- Hardware check: warn (not block) when no RDNA4 GPU / Mesa older than 26.2.

## Non-goals
- Importing from Lutris, Heroic or Epic libraries (later; the recipe format
  allows it).
- Modifying, replacing or deleting Steam shortcuts.
- Bundling or hosting NVIDIA files; supporting RDNA3 or Windows.
- AMDLSS5 (Windows-only, closed 2.x source).
- A SteamGridDB client.

## Anti-goals
- "Remove" on a local game ever deleting the game folder or its prefix.
- A local game launch touching Xbox services, licences, or the person's Xbox
  accounts.
- Breaking any existing Xbox launch, install, update or runtime selection.
- Enabling neural rendering silently on a game with anti-cheat.

## Constraints
- Base `origin/main`; PRs target `main`. Rust 1.88, `cargo test --locked --workspace`.
- No hardcoded machine paths; everything derives from `Paths`.
- Recipes keep `deny_unknown_fields`; new keys are optional so old recipes parse.
- `ferestre-core` never links the Xodus client (runs it as a child).
- Any new `[[setup]]` action is mirrored in `titles/validate.py` and
  `titles/SCHEMA.md` and the setup-action test.

## Approaches considered
A. (picked) Local games as recipes with a `kind`, guarded at the few Xbox-only
   chokepoints. Rejected: a separate local-games store (duplicates editor,
   settings and Steam code); delegating launch to umu-launcher (new
   dependency, settings split across tools).
B. (picked) Ferestre builds/hosts the Linux package and drives upstream's
   `install.sh`. Rejected: reimplementing the installer in Rust (drifts from a
   weekly-moving upstream); building on the user's machine (shader toolchain
   and mingw everywhere).

## Left to the builder
Exact module split, CLI flag spelling, GUI layout of the import and
neural-rendering dialogs (follow the existing dialog patterns).
