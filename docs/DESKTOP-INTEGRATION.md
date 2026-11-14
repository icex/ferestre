# Desktop integration

Ferestre follows the desktop's GTK window-button layout automatically. KDE's
GTK integration and GNOME expose the preference through `gtk-decoration-layout`.
The controls stay at the outside edges of the window, follow the visible page
on narrow screens, and disappear in fullscreen.

Games detect the current KDE or GNOME Wayland display when they start. Detection
reads the active mode and HDR setting; it never changes the monitor configuration.
HDR-capable displays running in SDR are treated as SDR.

- Automatic launches use XWayland on a Wayland desktop. A complete native
  Wayland driver and an HDR monitor do not establish that a game works with
  that driver. Inherited global Proton Wayland/HDR flags are disabled for the
  compatibility launch only; desktop settings and Proton settings files stay
  unchanged.
  Tested recipes can opt into native Wayland with
  `FERESTRE_NATIVE_WAYLAND=1` in `[launch.env]`. Forza Horizon 6 retains its
  verified native HDR path this way. To test another game deliberately, set
  `FERESTRE_NATIVE_WAYLAND=1` for its launch. That opt-in still requires a
  complete driver and a suitable desktop. HDR defaults apply only to a native
  or explicitly selected Gamescope path, and HDR opt-outs remain effective.
  Older Proton launchers get driver overrides for that launch only; Xalia and
  the X11 fullscreen hack are disabled for native Wayland. Prefix graphics
  preferences are not rewritten.
  On scaled desktops, automatic native Wayland also requires the runtime's
  fractional-scaling protocol support. Older drivers can enlarge and crop a
  game's child rendering window; those runtimes keep the existing display path.
  Runtime builds verify both drivers and both default-prefix templates before
  packaging, and fail if the SDK cannot compile the native Wayland driver.
  Wine also exposes 3840x2160 and scaled ultrawide modes (3440x1440,
  3840x1080 and 5120x1440) when they fit the monitor. This allows practical
  rendering resolutions on an 8K ultrawide display. Runtime CI exercises the
  public Windows display API against a private headless output.
- Gamescope remains an experimental, explicit choice with `FERESTRE_GAMESCOPE=1`.
  Retail Forza testing found unusable scaling/progression in that path. An
  existing Gamescope session is never nested again.
- Within an explicitly selected native display path, environment settings and
  Proton's `user_settings.py` are respected.
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
