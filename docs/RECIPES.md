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
| `XODUS_CLI_DIR` | directory holding `xodus-cli` / `xodus-service` | auto (`PATH`, then `third_party/xodus-cli/target/release`) |
| `XODUS_PROTON_DIR` | the installed Xodus Proton compat tool | auto (Steam's `compatibilitytools.d/xodus`) |
| `XODUS_STEAM_DIR` | a Steam install (Proton wants `STEAM_COMPAT_CLIENT_INSTALL_PATH`) | auto (`~/.steam/steam`, `~/.local/share/Steam`, Flatpak) |
| `XODUS_BUILD_DIR` | the Proton build tree | `~/src/xodus-build` |
| `XODUS_SRC_DIR` | where the `xodus-proton` and `xodus-cli` trees live | `third_party/` in this repo |
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

The runtime fork and the client are submodules of this repository, pinned at
the commits the patch series is written against: `third_party/xodus-proton`
(xodus-gaming/Proton, which pins its own Wine and xgameruntime trees) and
`third_party/xodus-cli` (xodus-gaming/xodus).

```bash
# this repo, with the patches and scripts
git clone https://github.com/icex/ferestre ~/src/ferestre
cd ~/src/ferestre

# the runtime fork, with its submodules (git-lfs bypass: don't fetch gigabytes
# of gstreamer test media, which otherwise leaves wine/ and openfst/ empty)
git -c filter.lfs.smudge= -c filter.lfs.process= \
    submodule update --init --force --recursive third_party/xodus-proton

# apply the local patches (they are plain git diffs against the fork)
cd third_party/xodus-proton/wine
git apply ~/src/ferestre/patches/wine/*.patch
git -C dlls/xgameruntime apply ~/src/ferestre/patches/xgameruntime/*.patch
git -C ../vkd3d-proton apply ~/src/ferestre/patches/vkd3d-proton/*.patch
# the proton script patch is re-applied by the installer after every install

# configure the build tree once (Xodus' own instructions), then build+install:
mkdir -p ~/src/xodus-build && cd ~/src/xodus-build
~/src/ferestre/third_party/xodus-proton/configure.sh --build-name=xodus --container-engine=docker
~/src/ferestre/scripts/install-xodus-proton.sh
```

The build tree stays where it was configured: Wine's generated Makefiles
inside it carry its absolute path hundreds of thousands of times, so moving it
means a full rebuild. Moving the *source* is fine -- only the top-level
`Makefile`'s `SRCDIR` line names it.

`install-xodus-proton.sh` invalidates the stale build stamps (editing the
submodule alone changes nothing otherwise), builds, installs into Steam's
`compatibilitytools.d/xodus`, re-applies the `proton` script patch that keeps
the decrypted-image file descriptors alive, and refuses to finish if the
installed `xgameruntime.dll` is stale.

**Rebuilding one DLL after a change** (seconds instead of a full build):

```bash
cd ~/src/xodus-build
rsync -a --exclude .git ~/src/ferestre/third_party/xodus-proton/wine/dlls/kernelbase/ src-wine/dlls/kernelbase/
echo "WORKDIR=$PWD/obj-wine-x86_64 ~/src/ferestre/tools/in-container.sh \
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
cd ~/src/ferestre
git submodule update --init third_party/xodus-cli
cd third_party/xodus-cli
# the client series, skipping 0001 (an earlier export that 0002 contains)
for p in ../../patches/xodus-cli/*.patch; do
    case $(basename "$p") in 0001-*) continue ;; esac; git apply "$p"; done
cargo build --release                            # target/release/{xodus-cli,xodus-service}
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

### Forza Horizon 5 (Store) — **plays; online services working**

```bash
scripts/get-game.sh <its Store product id> "$XODUS_GAMES_DIR/fh5"   # 150 GB
scripts/launch-fh5.sh
tools/drive-fh5.py                                                   # unattended
```

Signs in and drives with a restored Windows profile, vehicle and livery.
The world, HUD and minimap rendered, and the player confirmed gameplay.
The selected vehicle survived a subsequent launch. The live Festival Playlist
now loads, title-service requests return HTTP 200, and the player confirmed
the online fix. Multiplayer race completion has not been automated.

The online fix is in **client patch 0014**, so updating the Wine runtime alone
is insufficient. It authenticates each title from its own metadata and reads
its published service rules. See [Xbox Live authentication](XBOX_LIVE.md) for
the generic implementation and checks to use with another title.

The runtime needs the capabilities listed in
[`titles/9NNX1VVR3KNQ.toml`](../titles/9NNX1VVR3KNQ.toml). Three findings explain
the previous startup and installation failures:

- The title's code-restoring handler needs the mapped image's real filename,
  and its integrity check needs reads of that file to return the decrypted
  bytes. Wine patches 0010 and 0013 supply those answers; 0011 fixes a separate
  concurrent-vector allocation race.
- libHttpClient can complete a websocket send inside its provider's Begin and
  return `E_PENDING`. Runtime patch 0010 preserved that result, but
  `XAsyncBegin` still returned `E_PENDING` to its caller, which then destroyed
  the context needed by the queued callback. Patches 0014 and 0015 return
  `S_OK` after setup and complete provider failures asynchronously, with cleanup
  after the callback. The missing-title-claim theory did not explain this crash.
- `XPackageGetCurrentProcessPackageIdentifier` must fit the GDK's 33-byte
  buffer. Returning the 35-character package family name made every query fail
  and left the game at `INSTALLING... PLEASE WAIT`. Patch 0016 supplies an
  opaque session identifier shared with enumeration. Package family names used
  by AppModel and saves stay the same.

**Restoring Windows saves:** with the game stopped, copy the existing WGS tree
from `%LOCALAPPDATA%\Packages\<package-family>\SystemAppData\wgs` into
`$FH5_PREFIX/pfx/drive_c/users/steamuser/AppData/Local/Packages/<package-family>/SystemAppData/wgs`.
`FH5_PREFIX` defaults to `$XODUS_GAMES_DIR/fh5-proton`. Preserve the container
layout and the save-folder SCID suffix; patch 0011 resolves Forza's zero
configuration argument to the title's own SCID so that matching folders are
found. The confirmed restoration used a read-only source backup and a local
copy, not an automatic import feature. No account-specific folder name is
needed in this recipe.

### Halo Infinite — **playable; intermittent offline display unresolved**

Use launcher 0.1.5 with runtime 11.0.20261010.1, a licensed Store/Game Pass
installation and the shipped `9PP5G1F0C2B6.toml` recipe. Keep the original
`HaloInfinite.exe` bootstrapper and anti-cheat files. The recipe enables
`FERESTRE_IMAGE_VIEW=real-path` and `XGR_STRICT_MANUAL_QUEUES=1`.

The image view needs Python 3, util-linux and enabled unprivileged user/mount
namespaces. Licensed executable images live in private tmpfs mounts for the
launch and its campaign handoff; the encrypted installation stays intact.
The launch inherits its package manifest into child processes.

Campaign, Academy and multiplayer have live confirmation, as does local saving.
Intermittent offline display is unresolved. Cloud saves and purchases are not
qualified. A Store/Xbox account mismatch dialog can be dismissed to play.
Remove an older personal Halo recipe override, or update it to match the shipped
recipe, since personal overrides take precedence.

### Minecraft Dungeons II — **playable**

Use the shipped `9P5786PJB9RP.toml` recipe and matching updated runtime/service.
Complete the Squid Coast tutorial before evaluating campaign progress after a
restart: the user confirmed saving after tutorial completion. Mid-tutorial
restarts alone did not establish a save failure. Cloud sync and multiplayer
remain unqualified. Accept the bundled Visual C++ installer on a fresh prefix.

### Goat Simulator 3 — **offline playable with saves; online stalls**

Use a runtime providing `xgameruntime.gamesave-interface4-legacy`. The game asks
for `IXGameSaveImpl4` and, without it, plays on but never writes a save: a
relaunch lists every San Angora slot as New Save. With the interface, a clean
offline run loaded a new world in about 30 seconds, autosaved, and after a
relaunch reloaded that save with its quest progress. Offline play is also
player-confirmed.

Answer **No** to online mode. With online features on, the same steps took
about three minutes to load the world, which then advanced only every few
minutes and stopped responding to input; the cause is not established. Quit Game
after playing closes the window but leaves the process running, on this and the
previous runtime, so stop the title from the launcher afterwards. Peer joining
and voice are unqualified. See `titles/9PDS2N82QNXG.toml` for current issues.

### Retro Classics — **playable, player-confirmed**

Use `titles/9MTVJ3HHTQGS.toml` with a runtime providing
`d3d12.recording-allocator-lifetime` and
`xgameruntime.gamesave-interface4-legacy`. Package 3.3.15.0 releases a D3D12
command allocator while a list is still recording from it, which crashed the
older vkd3d-proton during `Close`, and asks for the newer save interface.

The recipe does not set `VKD3D_CONFIG`. `ferestre run` recognises the
Ultralight libraries beside `RetroClassics.exe` and appends
`retain_recording_allocators` to whatever `VKD3D_CONFIG` is already in effect,
when the selected runtime provides the capability. `VKD3D_CONFIG=` (empty)
turns that off. Launching without the launcher needs the variable set by hand.
[Autodetection](AUTODETECTION.md) has the exact order.

The player confirmed it playable; an isolated run also reached the catalogue
and streamed Tennis. Cloud save synchronization, save restoration and broad
game/controller coverage are not separately qualified.

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
