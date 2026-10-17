# Issue 10: video dependencies, GameInput and AppImage shortcuts

The report uses Fedora 44, runtime `11.0.20261010.2`, and a Bluetooth
DualShock 4, including Steam Input's virtual Xbox pad. That exact combination
has not been reproduced here. The changes address three independently
observable deficiencies; they do not certify every controller/backend.

## Media

The bundled GStreamer libav plugin needs FFmpeg 4 sonames. Newer FFmpeg packages
cannot satisfy them. `ferestre doctor`, the CLI launch path and a GUI launch
warning now name unresolved dependencies and explain the Fedora RPM Fusion
`compat-ffmpeg4` workaround. See `docs/DEPENDENCIES.md`.

Bundling the build tree's FFmpeg is insufficient: its configure disables all
codecs, including H.264 and AAC. A future self-contained runtime needs a
decoder-enabled build and its dependency closure. This change documents and
diagnoses the compatibility-library requirement rather than shipping that
incomplete build.

Validation: an actual ELF plugin with an unavailable shared library produces
the actionable warning. Moving that library onto Proton's search path clears
the warning. The installed game's actual 1080p splash movie decoded through
Wine Media Foundation into 30 frames, with 29 changing frame checksums.
That playback check used the local host's available compatibility libraries.

## Controller

The previous v2 implementation enumerated keyboard and mouse only and returned
no gamepad state. Wine patch 0016 bridges already-connected Wine XInput pads to
GameInput v2: enumeration, stable per-slot device identity, immutable readings,
button/trigger/stick conversion, next-reading polling, status and two-motor
rumble. It uses Wine's existing HID/SDL backend, including virtual Xbox pads,
instead of exposing duplicate devices through DevQuery.

Validation: the actual DLL passes the existing gated/keyboard/mouse suite and
a test-only native XInput backend covering two connected slots, input updates,
immutable retained readings, disconnects and rumble. The released DLL fails the
new enumeration test. A temporary Linux uinput Xbox pad also delivered its A
button, full-left stick and full trigger through Wine XInput into GameInput.
The temporary device was removed after that test.

The candidate launched Minecraft Dungeons II to its title screen. The local
user reported the controller worked in that window, and clarified it had
already worked on this machine before the change. This is regression evidence,
not confirmation of the Fedora reporter's path.

Limitations: gamepads must be connected when the game registers its callback.
This does not add background hotplug notifications, reading callbacks or
GameInput v0 gamepad support. Raw Bluetooth DS4 mapping and Steam Input still
depend on the underlying Wine input backend; they need reporter verification.

## Steam shortcut

Saved Steam targets use the persistent `APPIMAGE` path, while immediate child
processes continue using the CLI inside the mount. Native installations keep
their CLI target. The GUI test clicks Add to Steam in an isolated Steam tree,
checks quoted Exe/StartDir and title options, closes the GUI, removes the
mount-local CLI and launches the saved target. Unrelated Steam entries remain
covered by the existing round-trip/upsert tests.

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

Final local checks: 307 Rust tests, workspace formatting and Clippy with warnings
as errors; complete isolated GTK smoke including all three size-label sections,
cache reuse across restart, account-change invalidation, immediate close during
a held list refresh, download cleanup and removal completion; CLI media and
cancellation integration tests; all four patch series; GameInput compilation
with `-Werror`; launch-helper/authentication probe tests and recipe validation.
The Fedora/Bluetooth/Steam Input combination remains unverified.

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
