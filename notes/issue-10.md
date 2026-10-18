# Issue 10: video dependencies, controllers and Steam shortcuts

The report uses Fedora 44, runtime `11.0.20261010.2`, and a Bluetooth
DualShock 4, both directly and through Steam Input's virtual Xbox pad. There
is no physical DualShock 4 or Fedora install here. The pad was emulated
through `/dev/uhid`, over USB and over Bluetooth (report `0x11` with its CRC,
the calibration and firmware feature reports), and Steam was the real client
on CachyOS. With that setup the reporter's log was reproduced, and every
fix below was checked against it by automated runs in a 1280x720 window.

## Media

The bundled GStreamer libav plugin needs FFmpeg 4 sonames. Newer FFmpeg packages
cannot satisfy them. `ferestre doctor`, the CLI launch path and a GUI launch
warning now name unresolved dependencies and explain the Fedora RPM Fusion
`compat-ffmpeg4` workaround. See `docs/DEPENDENCIES.md`.

The check searches libraries in Proton's order: Steam's
`ubuntu12_64/video` and `ubuntu12_32/video` (Steam ships the FFmpeg 4 sonames
there, with H.264 and AAC), then the runtime's lib dirs, then the
`LD_LIBRARY_PATH` that launch would actually pass on. The default launch
drops the inherited one; `--steam` and recipes keep theirs. A machine with
Steam installed no longer gets a warning it does not need.

Bundling the build tree's FFmpeg is insufficient: its configure disables all
codecs, including H.264 and AAC. A future self-contained runtime needs a
decoder-enabled build and its dependency closure.

Validation: an actual ELF plugin with an unavailable shared library produces
the actionable warning, and moving that library onto Proton's search path
clears it. The installed game's 1080p splash movie decoded through Wine Media
Foundation into 30 frames, 29 with changing checksums.

## Controller

Four separate faults kept a pad from a GDK title. Each was reproduced
before it was fixed.

1. **GameInput had no gamepads.** The v2 implementation enumerated keyboard
   and mouse only. Wine patch 0016 bridges Wine's XInput slots to GameInput
   v2: enumeration, stable per-slot identity, immutable readings, next-reading
   polling, status and two-motor rumble. A reading for "any pad" comes from
   the pad whose input changed last, and its timestamp is when the packet
   changed, not when it was polled. Both views of a pad report 045e:028e.
   Patch 0017 answers `QueryInterface(IID_IGameInputDevice_v0)` on those pads.
   Minecraft Dungeons II makes that query and logs `E_NOINTERFACE` without it,
   but its input comes through XInput either way. `IGameInput_v0` itself still
   enumerates no virtual devices.

2. **A DualShock 4 or DualSense never became an XInput pad.** Wine's winebus
   prefers hidraw for Sony pads and then drops the SDL copy, and only the SDL
   copy becomes an XInput gamepad. The Proton shim therefore sets
   `PROTON_DISABLE_HIDRAW` for the five Sony IDs, unless the user, a recipe
   or Proton's `user_settings.py` already chose either `PROTON_DISABLE_HIDRAW`
   or `PROTON_ENABLE_HIDRAW`. Winebus loads the host's `libSDL2-2.0.so.0` for
   this. The AUR runtime now depends on `sdl2-compat`, `ferestre doctor`
   checks for it, and the shim warns when it is missing.
   - Emulated USB DS4: Minecraft Dungeons II went from no pad to the main menu
     driven by the pad.
   - Emulated Bluetooth DS4: before the fix, the log matched the report
     (`failed to create device for L"SDL"`) and 0 gamepad reports arrived;
     after it, 48 arrived.

3. **Steam Input's virtual pad stayed silent.** Steam applies a shortcut's
   controller layout only to processes carrying `SteamGameId`/`SteamAppId`.
   The launch stripped them with the rest of Steam's environment. They are now
   kept by default (`STEAM_IDENTITY`). `--steam` is only needed for the overlay
   and Remote Play. Steam's `SDL_GAMECONTROLLER_IGNORE_DEVICES` still reaches
   Wine, which skips the physical pads Steam wraps, so there is no double
   input. Through real Steam, the virtual pad went from 0 to 10-14 input events
   per run, and only the 28DE XInput device was present.

4. **Steam thought the game never exited.** The Xbox service started by a
   launch kept the AppImage mount and Steam's reaper alive. The service now
   runs from a copy in the cache, with its own process group. It is shared by
   concurrent launches through a lock in its runtime directory, and the last
   launch to finish stops it, including when the launcher is signalled.
   `launch-gdk.sh` takes the same lock. After the fix, Steam saw the game exit
   within a second.

After registering WinRT input classes, the launch waits for the Wine session
that `wine reg` started to end. Otherwise the title joins that session, whose
input bus was started without the shim's hidraw setting. The wait is bounded,
so a session that is already running cannot hang the launch.

Validation: the GameInput suite (gated, enabled and gamepad) passes against
the built DLL. The gamepad cases cover two connected slots, input updates,
immutable readings, disconnects, rumble, any-pad selection, idle timestamps
and vendor IDs. The DLL compiles with `-Werror` in the SDK container.

Not verified here: Fedora itself, a physical DualShock 4 or DualSense, and
hotplug after a title has registered its device callback.

## Steam shortcut

Saved Steam targets use the persistent `APPIMAGE` path, while immediate child
processes keep using the CLI inside the mount. `APPIMAGE` is used only when the
CLI really runs from that AppImage's `APPDIR`, so an AppImage launcher that
started a native install does not become its shortcut target. Native
installations keep their CLI target. The GUI test clicks Add to Steam in an
isolated Steam tree, checks quoted Exe/StartDir and title options, closes the
GUI, removes the mount-local CLI and launches the saved target. Unrelated Steam
entries remain covered by the existing round-trip/upsert tests.

## Related requested GUI changes

Library, Game Pass, Installed and Updates share a two-line layout: icon and
name, details (version, one size phrase, compatibility/status), and centered
right-hand actions. Size is no longer repeated in the catalog subtitle. Installed
sizes are allocated bytes, measured off the GTK thread; sparse files and
hardlinks are handled, symlinks are not traversed within a game tree, and a
reload discards obsolete in-flight results. Disk usage persists in an atomic
per-game cache for 24 hours, reused across app loads and list refreshes. Root
identity/metadata, manifest metadata and install-record changes invalidate it;
deep file changes are picked up at expiry. Stale totals remain visible while
background remeasurement runs. Catalog rows identify download sizes.

List/account/update refreshes may be closed immediately. Download cancellation
and removal completion retain their existing close protocols. Cache snapshots
publish by atomic rename; warm startup reuses ownership, game-update checks, the public Game Pass list
and product details for 24 hours. Product metadata
uses stale-while-revalidate: cached rows draw before network requests, stale
entries refresh in the background, and explicit refresh includes named and
off-page entries. Market/language changes also invalidate metadata freshness.
Failed revalidation retains both the product and its original freshness time;
a partial list/tier refresh never replaces a complete Game Pass snapshot. Account changes and detected subscription
changes invalidate the corresponding data; explicit refresh bypasses freshness.
No Microsoft change notification is available here: changes made elsewhere are
detected by the periodic ownership check or an explicit refresh.

When the client reports that nobody is signed in, for example after
`xodus-cli logout` or a revoked token, the cached account and its library
are forgotten instead of shown. Sign-out removes them before the client runs.
A Refresh pressed while another task runs is kept and runs when that task
ends. A reload during a disk-usage scan keeps the scan instead of starting
another walk of the same folders.

Final local checks: 307 Rust tests, workspace formatting and Clippy with warnings
as errors; complete isolated GTK smoke including all three size-label sections,
cache reuse across restart, account-change invalidation, immediate close during
a held list refresh, download cleanup and removal completion; CLI media and
cancellation integration tests; all four patch series; GameInput compilation
with `-Werror`; launch-helper/authentication probe tests and recipe validation.

Catalogue revalidation follow-up: a local HTTP fixture checks cold load, a warm
load with no request, changed stale metadata replacing the cached row, forced
refresh of fresh data, locale changes and failure preserving data/freshness.
The real GTK smoke also counts proxy requests across restarts and checks forced
and automatic stale revalidation with cached names visible on failure.

Disk usage follow-up: a filesystem test covers persistent reuse, expiry, deep
content changes, changed manifests/install records, directory changes/moves and
failure preserving the cache. The GTK restart/refresh checks assert the stored
measurement contents and timestamps remain unchanged for fresh installs.

Steam artwork/update follow-up: re-adding matches launch options even when the
old display name or AppImage mount differs. Existing app IDs, play history,
overlay settings and tags survive. Store icons/covers/heroes/title logos are
downloaded concurrently, cached by source URL and copied into each account's
grid directory; unavailable assets do not block the shortcut. Personal artwork
is retained. Naming follows [SGDBoop's implementation](https://github.com/SteamGridDB/SGDBoop/blob/main/sgdboop.c),
with asset roles described by [Steamworks](https://partner.steamgames.com/doc/store/assets/libraryassets).
Steam's non-Steam shortcut format has no supported description field, so this
adds the artwork rather than an ignored description key.

Validation: local HTTP artwork downloads and offline cache reuse; all grid
slots and preservation of personal art; metadata purpose selection; updating a
renamed/moved shortcut while preserving its stored ID/history. The actual GTK
test checks both text lines, a single size phrase, icon and vertically centered
actions in Library/Installed/Game Pass, then downloads artwork, saves and
repairs a deliberately stale shortcut without duplicating it.

Live artwork qualification: the real mounted AppImage used old cached metadata
for Minecraft Dungeons II, refreshed that one title, and downloaded Store PNGs:
256x256 icon, 600x900 portrait, 920x518 header and 1920x1080 hero. All decoded
with ImageMagick. The GUI closed, the mount disappeared, the copied icon stayed
on disk and the saved persistent target still executed. Steam's own Play-button
launch and visual rendering of its library artwork remain untested.

Daily-check policy: game-update results persist per installed record for 24
hours and are restored on startup. Navigation reuses them. Failed checks retain
the last known version and are also throttled until tomorrow; the top-right
Refresh button forces library/subscription, catalogue and game-update checks.
Changed installs invalidate their own cached update result.
