# Release 0.1.6 qualification

Launcher **0.1.6** and runtime **11.0.20261010.2** should be updated together.
The runtime is 11.0.20261010.1 with three binaries rebuilt: `xgameruntime.dll`
(GDK patches 0001–0027) and the x86_64 vkd3d-proton `d3d12core.dll`/`d3d12.dll`
(vkd3d-proton patch 0001). Every other runtime file is byte-identical to
11.0.20261010.1; no upstream upgrade is claimed.

## Included fixes

- **IXGameSaveImpl4.** The complete 50-slot save interface is exposed. Legacy
  local saves work through it; the 17 PlayFab cloud-save operations fail
  explicitly with `E_NOTIMPL` and nothing claims cloud synchronization.
- **Recording allocator lifetime.** `VKD3D_CONFIG=retain_recording_allocators`
  keeps a command allocator a title releases mid-recording alive until the list
  resets or is destroyed, without changing public COM reference counts. Without
  the setting, the lost recording is invalidated and `Close` fails with an error
  logged once, instead of ending a null Vulkan command buffer. Recording calls
  made before `Close` remain unprotected.
- **Engine defaults.** `ferestre run` recognises a complete Ultralight/WebCore
  library set beside the executable and appends `retain_recording_allocators` to
  whatever `VKD3D_CONFIG` is in effect, only when the selected runtime provides
  the capability. An empty `VKD3D_CONFIG` turns it off. See
  [autodetection](AUTODETECTION.md).
- **Generated recipes** require only what a runtime probe can confirm;
  package identity is wanted rather than required.
- **Library scroll.** Starting a title, and its exit, no longer scroll the
  library back to the top after a mouse click on its button.

## Live evidence and limits

| Title | Observed result | Remaining scope |
|---|---|---|
| Retro Classics | Playable, confirmed by the player. An isolated run reached the catalogue and streamed Atari Tennis | Cloud saves, save restoration and broad game/controller coverage are unqualified |
| Goat Simulator 3 | Clean-prefix automated runs: an offline new save reached the world in about 30 s, played (WASD tutorial), autosaved, and after a relaunch the save was listed and reloaded at the Goat Tower with its quest progress. Offline play is also player-confirmed. On runtime 11.0.20261010.1 the same save state listed no save and a new world wrote no save at all | With online features enabled the world loaded in about 3 minutes and then advanced only every few minutes, without responding to input; cause not established. Quit Game after playing leaves the process running, on this and the previous runtime. Peer joining and voice are unqualified |

Goat's earlier progress cannot be recovered: before this runtime the game wrote
no save files.

## Verification

- 278 Rust workspace tests, `cargo fmt` and Clippy with `-D warnings`.
- 259 GDK checks and 46 strict manual-queue checks against the release runtime,
  including the interface4 slot positions (3, 29–33, 49), QueryInterface
  identity and a container round trip through `IXGameSaveImpl4`.
- The allocator lifetime test on a real GPU: 14 checks with retention, including
  executing the retained list and waiting on a fence; 9 checks without it, with
  `Close` failing safely and the new error logged.
- The GTK accessibility smoke test, with a real pointer click: the library
  scroll moved 200 → 24 before the fix and stayed at 200 after launch and exit.
- Both patch series apply to their pinned bases; the release binaries were built
  from them, and the capability manifest verified 40 of 40 capabilities.
