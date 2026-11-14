# Ferestre 0.1.22

Automatic desktop detection no longer forces an unverified native Wayland/HDR path merely because the runtime contains a driver and the monitor uses HDR. This fixes the launch regression observed with Minecraft Dungeons II and Minecraft for Windows.

- Games use XWayland by default on Wayland desktops. Inherited global Proton Wayland/HDR flags are disabled for the game process on this compatibility path.
- Tested recipes explicitly opt into native Wayland. Forza Horizon 6 retains its verified native HDR path; other games can be tested with `FERESTRE_NATIVE_WAYLAND=1`.
- Desktop HDR, color, scale and refresh settings, Proton settings files, accounts and saves are not changed. Explicit HDR opt-outs and the experimental Gamescope option remain available.

Regression coverage checks driver presence versus game compatibility, inherited global flags, Proton settings, explicit native selection and Gamescope fallback. During diagnosis, the current runtime rendered Dungeons II with native Wayland disabled, and the player confirmed both Minecraft games loaded. This does not qualify long-session gameplay or cloud saves.

This is an application update. The signed runtime remains **11.0.20261107.1**.
