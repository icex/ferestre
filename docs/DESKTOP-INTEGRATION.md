# Desktop integration

Ferestre follows the desktop's GTK window-button layout automatically. KDE's
GTK integration and GNOME expose the preference through `gtk-decoration-layout`.
The controls stay at the outside edges of the window, follow the visible page
on narrow screens, and disappear in fullscreen.

Games detect the current KDE or GNOME Wayland display when they start. Detection
reads the active mode and HDR setting; it never changes the monitor configuration.
HDR-capable displays running in SDR are treated as SDR.

- A native Wayland runtime gets the Wayland/HDR defaults appropriate to the
  active desktop: `PROTON_ENABLE_WAYLAND=1`, `PROTON_ENABLE_HDR=1` and `DXVK_HDR=1`.
  HDR formats come from the host Vulkan WSI; Wine's driver does not need to
  embed the compositor's color-management protocol. Current Mesa implements it.
  Without a Wayland driver, the normal display path and game window preferences
  are preserved. Installing Gamescope does not silently wrap or fullscreen games.
  Older Proton launchers get driver overrides for that launch only; Xalia and
  the X11 fullscreen hack are disabled for native Wayland. Prefix graphics
  preferences are not rewritten.
- Gamescope remains an experimental, explicit choice with `FERESTRE_GAMESCOPE=1`.
  Retail Forza testing found unusable scaling/progression in that path. An
  existing Gamescope session is never nested again.
- Explicit environment settings and Proton's `user_settings.py` take precedence.
  `PROTON_ENABLE_HDR=0` or `DXVK_HDR=0` disables automatic HDR. A recipe can set
  `FERESTRE_GAMESCOPE=0` to decline the compositor fallback, or
  `FERESTRE_DESKTOP_INTEGRATION=0` to retain the legacy launch path entirely.
- CachyOS uses its installed `game-performance` utility to hold the performance
  profile while the game runs. The hold runs beside the game because the utility
  closes inherited descriptors, including those required by licensed images.
  On other distributions, an available `gamemoderun` is used. The two are not
  stacked. `FERESTRE_PERFORMANCE=0` disables this automatic selection.
- Optional helpers that cannot start do not prevent the original launch path.
  The launch log records the selected display/performance path.

The runtime's `graphics.xrandr-desktop-refresh` capability corrects a specific
XWayland limitation: its 32-bit synthetic pixel clock can clamp an ultrawide
7680×2160 display at 240 Hz to approximately 158 Hz. The launcher passes the
actual compositor mode, and the Wine correction applies only to a matching
resolution with that clamped clock. Other display modes keep their reported
rates. Older runtimes need this runtime update for the correction.

GPU memory budgets are not overridden. A game's available DXGI budget can be
smaller than the GPU's physical VRAM because the driver accounts for other
allocations and memory pressure.
