# Ferestre 0.1.11

Launcher **0.1.11** pairs with [runtime **11.0.20261017.1**](https://github.com/icex/ferestre/releases/tag/runtime-11.0.20261017.1).

Download the AppImage, make it executable and run it, or extract the portable
`.tar.zst` and run `usr/bin/ferestre`. Both include the Xodus client; the runtime
is downloaded separately. `SHA256SUMS` verifies both downloads.

## Included changes

- Library, Game Pass, Installed and Updates use two-line game rows: icon/title,
  then version, one size phrase and status; action buttons are centered on the
  right. Installed sizes measure allocated bytes and exclude prefixes/downloads.
- Disk usage persists across restarts and refreshes for 24 hours. Install record,
  directory identity/metadata and manifest changes invalidate it; daily scans
  catch deeper changes. Cached totals remain visible during background scans.
- Library/subscriptions, public catalog metadata and game-update checks are
  reused for 24 hours. Navigation does not repeat update queries. The top-right
  Refresh button forces immediate checks; changing accounts invalidates ownership.
  Failed update checks retain known versions and wait for the next day or Refresh.
- Closing during read-only account/list/update refreshes exits immediately.
  Downloads still cancel with cleanup; removals finish safely before closing.
- Steam shortcuts target the persistent AppImage, surviving its temporary mount.
  Re-adding repairs existing targets/names without duplicating the shortcut and
  retains the stored Steam ID, play history, tags and personal Steam settings.
  Store icon, portrait cover, header, hero and wide title logo are added when
  available. Assets are cached and copied into Steam's own grid directory;
  existing custom artwork is retained. Steam must be closed while editing.
- Doctor and launch warnings diagnose missing FFmpeg 4 dependencies and explain
  the Fedora RPM Fusion `compat-ffmpeg4` workaround.
- Runtime GameInput v2 enumerates connected XInput gamepads and returns immutable
  button/stick/trigger readings, stable device identities, status and rumble.

- `ferestre inspect-package PATH` inspects Appx/MSIX package manifests and
  bundle metadata without extracting or launching them. This provides package
  diagnostics; UWP execution remains unsupported.

## Evidence and limits

- 307 Rust workspace tests, formatting, Clippy, isolated GTK smoke and CLI
  cancellation/media integration checks. GTK drives real buttons, both-line
  geometry, one size phrase, cached daily checks across restarts and forced
  Refresh, safe close and persistent Steam artwork/shortcut replacement.
- The real AppImage fetched Minecraft Dungeons II's Store metadata and decoded
  PNG icon, portrait, header and hero. After the GUI exited, its mount disappeared,
  copied artwork remained and the saved persistent target still executed.
  Steam's own Play-button launch/artwork display remains a manual check.
- GameInput's 64-bit and 32-bit runtime suites cover deterministic XInput devices, input updates,
  retained readings, disconnected devices and rumble. The released DLL fails the
  new connected-gamepad enumeration check. A temporary Linux uinput pad delivered
  A, full-left stick and full trigger through Wine into GameInput.
- The actual game reached its title screen with the candidate. The local player
  confirmed controller and video behavior, while noting the controller worked
  before this change. This does not verify the reporter's Fedora/Bluetooth DS4/
  Steam Input setup. Gamepads must be connected when callbacks are registered;
  background hotplug, reading callbacks and v0 gamepad support remain unavailable.
- The installed game's 1080p splash decoded through Wine Media Foundation into
  30 frames with 29 changing checksums. This used locally available FFmpeg 4
  libraries; the runtime does not bundle a replacement codec dependency closure.
- Runtime packaging uses the verified 11.0.20261013.3 release as its base and
  replaces GameInput DLLs built from the pinned Wine source plus patch 0016.
  Runtime provenance lists the base asset and replacement hashes.

Account-sharing [issue #9](https://github.com/icex/ferestre/issues/9) remains separate in [PR #11](https://github.com/icex/ferestre/pull/11); this release
contains no separate Store/player account implementation. Other runtime/game
qualification remains as documented in [0.1.9](https://github.com/icex/ferestre/blob/main/docs/RELEASE-0.1.9.md).
