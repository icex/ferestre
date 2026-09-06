# Recipes: downloading and running your Xbox / Microsoft Store games on Linux

Step-by-step, per game, for **any 64-bit Linux distribution**. Nothing here
bypasses protection: every title is downloaded and decrypted with the licence
of the Microsoft account that owns it, exactly as the Windows Store app would.
What this repo adds is the missing Windows API surface so the game can run under
Wine/Proton, plus the launch plumbing.

All scripts take their locations from `scripts/xodus-env.sh`; nothing is tied
to one machine. Override any of these in your shell profile to relocate things:

| variable | meaning | default |
|---|---|---|
| `XODUS_GAMES_DIR` | decrypted games, Proton prefixes, logs | `~/xbox-games` |
| `XODUS_CLI_DIR` | directory holding `xodus-cli` / `xodus-service` | auto (`PATH`, then `~/src/xodus-cli/target/release`) |
| `XODUS_PROTON_DIR` | the installed Xodus Proton compat tool | auto (Steam's `compatibilitytools.d/xodus`) |
| `XODUS_STEAM_DIR` | a Steam install (Proton wants `STEAM_COMPAT_CLIENT_INSTALL_PATH`) | auto (`~/.steam/steam`, `~/.local/share/Steam`, Flatpak) |
| `XODUS_BUILD_DIR` | the Proton build tree | `~/src/xodus-build` |
| `XGR_XUID` | optional: XUID your Windows saves were made under | unset |

Run `sh -c '. scripts/xodus-env.sh; env | grep ^XODUS_'` to see what resolves
on your machine before starting.

---

## 0. Requirements (once per machine)

- 64-bit Linux, a Vulkan-capable GPU with working drivers (`vulkaninfo` runs),
  and a graphical session (X11 or Wayland). Tested on CachyOS/Arch and Ubuntu.
- **Steam** installed (any distro package or Flatpak). It is only used as the
  home for the compat tool and for `STEAM_COMPAT_CLIENT_INSTALL_PATH`; the
  games are *not* Steam games and are launched by these scripts.
- **Docker or Podman** with the ability to run containers as your user (the
  Proton fork is built inside Valve's SteamRT SDK image, so the build is
  identical on every distro). `newgrp docker` is used in the scripts; on
  Podman set `alias docker=podman`.
- **Rust toolchain** (`rustup`) for `xodus-cli`, **git + git-lfs**, **python3**,
  **rsync**, **x86_64-w64-mingw32-gcc** is *not* needed on the host (it lives in
  the container).
- ~15 GB for the Proton build tree, plus the games (Expedition 33 ≈ 60 GB,
  Minecraft ≈ 2.5 GB, Forza Horizon 5 ≈ 150 GB).

## 1. Build and install the patched Xodus Proton

```bash
# the runtime fork, with submodules (git-lfs bypass: don't fetch gigabytes of
# gstreamer test media, which otherwise leaves wine/ and openfst/ empty)
git clone -b xodus/bleeding-edge https://github.com/xodus-gaming/proton ~/src/xodus-proton
cd ~/src/xodus-proton
git -c filter.lfs.smudge= -c filter.lfs.process= submodule update --init --force --recursive

# this repo, with the patches and scripts
git clone https://github.com/icex/xgdk-launcher ~/src/xgdk-launcher

# apply the local patches (they are plain git diffs against the fork)
cd ~/src/xodus-proton/wine
git apply ~/src/xgdk-launcher/patches/wine/*.patch
git -C dlls/xgameruntime apply ~/src/xgdk-launcher/patches/xgameruntime/*.patch
# the proton script patch is re-applied by the installer after every install

# configure the build tree once (Xodus' own instructions), then build+install:
mkdir -p ~/src/xodus-build && cd ~/src/xodus-build
../xodus-proton/configure.sh --build-name=xodus --container-engine=docker
~/src/xgdk-launcher/scripts/install-xodus-proton.sh
```

`install-xodus-proton.sh` invalidates the stale build stamps (editing the
submodule alone changes nothing otherwise), builds, installs into Steam's
`compatibilitytools.d/xodus`, re-applies the `proton` script patch that keeps
the decrypted-image file descriptors alive, and refuses to finish if the
installed `xgameruntime.dll` is stale.

**Rebuilding one DLL after a change** (seconds instead of a full build):

```bash
cd ~/src/xodus-build
rsync -a --exclude .git ~/src/xodus-proton/wine/dlls/kernelbase/ src-wine/dlls/kernelbase/
echo "WORKDIR=$PWD/obj-wine-x86_64 ~/src/xgdk-launcher/tools/in-container.sh \
  make -j$(nproc) dlls/kernelbase/x86_64-windows/kernelbase.dll" | newgrp docker
# the installed copy is read-only; replace it explicitly
D=$XODUS_PROTON_DIR/files/lib/wine/x86_64-windows/kernelbase.dll
chmod u+w "$D"; cp obj-wine-x86_64/dlls/kernelbase/x86_64-windows/kernelbase.dll "$D"
```

Then `tests/run-tests.sh` and `tests/run-appmodel-tests.sh` prove the build
actually contains what you think it does — they load the *freshly built* DLL,
not the installed one.

## 2. Build xodus-cli and sign in

```bash
git clone https://github.com/xodus-gaming/xodus-cli ~/src/xodus-cli
cd ~/src/xodus-cli && cargo build --release      # ~/src/xodus-cli/target/release/{xodus-cli,xodus-service}
./target/release/xodus-cli login                 # Microsoft sign-in in a window
```

Tokens are kept in your desktop keyring (D-Bus Secret Service: KWallet, GNOME
Keyring, ...). You only log in once per machine.

## 3. Get a game

```bash
scripts/get-game.sh <product id> [destination]
scripts/get-game.sh bedrock                       # alias for 9NBLGGH2JHXJ
```

The product id is the 12-character code in the Store URL
(`https://apps.microsoft.com/detail/9NBLGGH2JHXJ`). `get-game.sh` checks that
`xodus-cli` and free space exist, then runs `xodus-cli streaming`, which streams
the `.msixvc` from Microsoft's CDN, fetches your licence key with your account,
and decrypts + extracts in one pass. It resumes if interrupted.

The main executable stays encrypted on disk (`Minecraft.Windows.exe` starts with
`fb 6f`, not `MZ`); `xodus-cli run` decrypts it into a memfd at launch and hands
it to Wine through `WINE_DLL_FILE_MAP`. That is why every launcher goes through
`scripts/launch-gdk.sh` rather than running the exe directly.

## 4. Per game

### Clair Obscur: Expedition 33 — **working** (saves, video, full game)

```bash
scripts/get-game.sh <its Store product id> "$XODUS_GAMES_DIR/exp33"
scripts/launch-exp33.sh
```

- Prefix: `$XODUS_GAMES_DIR/exp33-proton`. Log: `exp33-launch.log`.
- **Bringing over Windows saves:** the game uses the Windows Gaming Services
  save format. Copy your Windows folder
  `%LOCALAPPDATA%\Packages\KeplerInteractive.Expedition33_ymj30pw7xe604\SystemAppData\wgs`
  to the same path inside the prefix:
  `exp33-proton/pfx/drive_c/users/steamuser/AppData/Local/Packages/KeplerInteractive.Expedition33_ymj30pw7xe604/SystemAppData/wgs`.
  Existing save folders are found by their SCID suffix whatever XUID prefix they
  carry; set `XGR_XUID=<the 16-hex prefix of your Windows save folders>` so new
  saves are named identically and can be copied back.
- Video plays for real (not colour bars) because the launcher prefers the
  system ffmpeg and demotes Proton's placeholder converter; see README.
- Steam shortcut: point a non-Steam game at `scripts/launch-exp33.sh`.

### Minecraft for Windows (Bedrock, Store) — **boots to the main menu**

```bash
scripts/get-game.sh bedrock                       # -> $XODUS_GAMES_DIR/bedrock/game (2.5 GB)
scripts/launch-bedrock.sh
```

- Prefix: `bedrock-proton`. It is a UWP + GDK hybrid, so it needs the
  kernelbase package identity *and* xgameruntime from this repo.
- **Where its data lives** (for save import/backup): worlds and settings are
  under `bedrock-proton/pfx/drive_c/users/steamuser/AppData/Roaming/Minecraft Bedrock/Users/<user id>/games/com.mojang/`
  (`minecraftWorlds/`, `minecraftpe/options.txt`). Copy a Windows
  `%APPDATA%\Minecraft Bedrock\Users\<id>\games\com.mojang` there. The older
  UWP location `Packages/Microsoft.MinecraftUWP_8wekyb3d8bbwe/LocalState` is
  created too but this build keeps its data in Roaming.
- **Networking needs one file swapped.** Microsoft's `XCurl.dll` (its HTTP
  transport) loads under Wine but never gets a request onto the wire: the game
  shows zero TCP connections and retries for ever, so sign-in, marketplace and
  profile all hang. Run once after downloading:

  ```bash
  scripts/fix-xcurl.sh                      # or: scripts/fix-xcurl.sh <game dir>
  ```

  It fetches the official curl build for Windows, checks that it exports
  everything the game actually imports from XCurl (Minecraft imports 16
  symbols, all standard `curl_*`), and only then swaps it, keeping the original
  as `XCurl.dll.microsoft`. Undo with `--restore`. **Re-run this after any
  re-download or update**, which restores Microsoft's build.
- **Status: signs in and plays.** Boots to the main menu, signs in with your own
  Microsoft account (real gamertag and XUID), and single-player works. Requires
  `xodus-service` running for tokens -- the launcher starts it. See README
  "Minecraft Bedrock" for the full chain of blockers that had to be closed.

### Forza Horizon 5 (Store) — **downloads, does not run**

```bash
scripts/get-game.sh <its Store product id> "$XODUS_GAMES_DIR/fh5"   # 150 GB
scripts/launch-fh5.sh                                                # fails
```

The Store build crashes inside its own in-binary code protection in a C++
static initialiser, before any runtime API is reached. That is not a Wine gap
that can be filled and is documented in the README; the recipe is here so the
launch path is ready if that ever changes.

## 5. Troubleshooting (every error we actually hit, and its fix)

| symptom | cause | fix |
|---|---|---|
| `Selection failed` from `xodus-cli download` | it needs an interactive terminal for its file picker | use `xodus-cli streaming` (what `get-game.sh` does) — it picks the `.msixvc` itself |
| `not entitled to this content: Device group is full` | the Microsoft account's device limit | remove an old device at account.microsoft.com/devices, retry |
| streaming fails immediately | not signed in | `xodus-cli login` |
| game exits at once, `APPMODEL_ERROR_NO_PACKAGE` | packaged app with no package identity | fixed by `patches/wine/0002-kernelbase-current-package-identity.patch`; the manifest must sit beside the exe (or set `WINE_PACKAGE_MANIFEST`) |
| `Critical Failure: GameInput Runtime could not be loaded` | Wine's `GameInputCreate` is allow-listed | `WINE_GAMEINPUT=1` (set by `launch-gdk.sh`) + `patches/wine/0003-...` |
| `Failed to Register TaskQueue Monitor for GameInput tasks` | `XTaskQueueRegisterWaiter` was a stub | xgameruntime patch (RegisterWaiter/RegisterMonitor) |
| `RoGetActivationFactory Failed to find library for Windows.Internal.System.Profile.RegionPolicyEvaluator` then `frame is not in stack limits` | undocumented region-policy class missing; the title throws on a fiber | hosted in `windows.system.profile.systemid.dll`; a fresh prefix registers it, an existing prefix needs `wine reg add` of its `ActivatableClassId` (see README) |
| `cp: Permission denied` installing a DLL into the tool | Proton installs files read-only | `chmod u+w` the file first |
| `newgrp docker` prompts / hangs | user not in the docker group | `sudo usermod -aG docker $USER`, log out and in |
| game runs but no picture / colour bars in videos | Steam's ffmpeg has no H.264 outside Steam | launcher sets `PROTON_PREFER_SYSTEM_FFMPEG=1`; needs a system ffmpeg with H.264 |
| several instances make everything slow | stray processes from testing | `scripts/kill-gdk.sh` |

## 6. Debugging a launch

Set `WINEDEBUG` before the launcher; `launch-gdk.sh` passes it through and
tees everything to `$XODUS_GAMES_DIR/<game>-launch.log`:

```bash
WINEDEBUG=+debugstr scripts/launch-bedrock.sh    # the game's own OutputDebugString log
WINEDEBUG=+ginput,+debugstr scripts/launch-bedrock.sh   # + GameInput calls
WINEDEBUG=+appmodel,+seh scripts/launch-bedrock.sh      # package identity + exceptions
```

`+debugstr` is the most useful channel by far for GDK titles: their own
`[INFO]/[ERROR]` lines say exactly which API failed, which is how every blocker
above was found.
