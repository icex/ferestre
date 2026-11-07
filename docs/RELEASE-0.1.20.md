# Ferestre 0.1.20

Games can use native Wayland HDR on supported KDE and GNOME desktops. Update both Ferestre and the signed runtime to **11.0.20261107.1** for the new display support. Existing per-game runtime overrides remain in effect; choose the new runtime in game options if a title is pinned to an older version.

## Changes

- Detect the active desktop mode, HDR state and scale when a game starts. Enable native Wayland and HDR when the runtime and desktop support them, while respecting explicit environment and Proton settings. Installing Gamescope no longer wraps games automatically; it remains an experimental opt-in.
- Build complete 64-bit and 32-bit Wayland drivers and prefix templates. Correct fractional scaling so game surfaces are not enlarged or cropped on scaled desktops.
- Expose 3840×2160, 3440×1440, 3840×1080 and 5120×1440 virtual modes when they fit the display, preserving the native mode and refresh rate. Games on large ultrawide displays can select a practical rendering resolution.
- Correct the clamped XWayland refresh-rate report that could show about 158 Hz for a matching 7680×2160 desktop running at 240 Hz. GPU memory budgets remain driver-managed.
- Hold the installed CachyOS game-performance profile during play, or use GameMode on other distributions when available. Performance helpers preserve the licensed game's inherited descriptors and Stop channel.
- Follow the desktop's window-button layout automatically, including left-side controls. Keep controls at the visible edges on narrow windows and hide them in fullscreen.
- Confirm sign-out in the app, with Cancel selected by default and controller navigation supported.
- Isolate native GUI test services from the active desktop. Runtime builds check driver completeness, measured capabilities and the public Windows display-mode API before packaging.

## Verification and scope

Hosted Rust tests, MSRV, source-pin checks and GUI suites pass. Local policy and isolation tests cover desktop detection, explicit overrides, performance helpers and preservation of inherited descriptors. A Windows display API probe rejects the preceding runtime's missing modes and passes with all four new resolutions, including in isolated headless runtime CI. All 51 measured runtime capabilities pass.

The [application release build](https://github.com/icex/ferestre/actions/runs/42404027813) and [runtime release build](https://github.com/icex/ferestre/actions/runs/42404027678) passed. Published signatures, signed file/version comments and SHA256SUMS were verified for both archives and the AppImage. The AppImage runs normally and with `--appimage-extract-and-run`; its binaries match the portable archive. The downloaded GUI also passed isolated native Wayland window-controls, Home/Resume/Stop/Cancel, account-confirmation and controller installation suites.

Retail Forza Horizon 6 reaches gameplay through the installed AppImage, applies both 3840×2160 and 5120×1440, accepts keyboard navigation and exits normally through its own menu. Native output was traced as 10-bit BT.2020 with ST.2084 PQ, accepted and presented by KWin at desktop scale 1.5. The player confirmed Forza works correctly in HDR with the final candidate. This qualifies the tested KDE/Mesa configuration, not every compositor, GPU or game's HDR path.

Windowed/fullscreen switching and simulated Xbox, DualShock 4 and DualSense Home/Resume checks pass; controller cancellation preserves the running game in the Stop dialog. Packaged GUI tests cover installation, folder selection, queues and confirmation dialogs. Simulated-device results do not establish physical-controller compatibility on every device.

Minecraft reaches its signed-in main menu with its existing selected runtime. Accounts loads and sign-out cancellation preserves the session. The compatibility matrix retains each title's separate verification date and scope.

The automated Forza performance sample was constrained by another running game occupying about 9.4 GiB of VRAM. It is not an unconstrained frame-rate benchmark; the 240 FPS option is a selectable cap. No GPU-memory budget override or graphics-quality preset is imposed.
