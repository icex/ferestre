# Ferestre 0.1.27

Pairs with runtime 11.0.20261124.1. **Settings → Choose default runtime** selects it once it is installed. A game with its own runtime choice keeps it: set it back to the default in **Edit launch settings → Proton** to get this runtime's fixes.

## Steam games

- **Import from Steam** also lists the games Steam installed. **Play** asks Steam to start them, so Steam's Proton, launch options, DRM and cloud saves stay as they are. Steam's files are only read.
- A Steam shortcut to a script or other Linux program of your own is imported too, and Steam starts it.
- Neural rendering (experimental) works with games Steam starts: the DLL override goes into the game's own Wine prefix, for that program alone, and is put back when it is turned off. See [Neural rendering](NEURAL-RENDERING.md).

## Compatibility labels

- Every tile says how well the game runs: **Playable**, **Playable with issues**, **Untested** or **Not playable**. A game's details show the same label, what was tested and when, and its known issues as a warning.
- Forza Horizon 6 was checked again on this runtime: sign-in, Continue, the in-game store's lists, the online tab and saving. Joining an online race and purchases were not tried.
- Halo Infinite and Gears of War: E-Day are listed as playable: their earlier intermittent problems were reported resolved by the player, and were not retested.
- Still with issues: Death Stranding Director's Cut (online features are not available; the opening plays offline) and Hogwarts Legacy (a first launch can close while it prepares shaders; start it again).

## The window

- Launch settings are tabs: display, compatibility, upscaling, Proton with the runtime, variables and neural rendering. They offer Proton's upscaling and compatibility flags; one the game's runtime does not read says so.
- Game details have tabs, pictures and links. A title that is not installed opens its details, which end in a large Install button.
- Lists scroll on instead of paging, and tiles size to fill the page. My Library opens on Installed, with counts on its tabs.
- Small screens get compact tiles and, in fullscreen, a top bar of tabs; larger screens keep the sidebar. Controller hints show only while a controller is in use.
- Featured titles follow the PC Game Pass "Most popular" list. Installed Game Pass titles show Play rather than Install.
- The window opens at the size it was left at. Starting a game is animated.

## Fixes

- Sign-in tokens for title services are matched whether or not the endpoint path begins with a slash, with the Store account and with a separate Player account. Minecraft Dungeons II reaches its online hero creation with different Store and Player accounts; joining another player was not tested.
- On a machine whose CPU numbering has gaps (offline cores), games get a dense CPU layout. From MW4-Proton.
- Reset to defaults is no longer offered for a local game, whose settings are the game; it used to make the game vanish from the library.
- A game's neural rendering route and network are kept when it is turned off and on again. OptiScaler's menu opens with **Home**.

## Runtime 11.0.20261124.1

- Wine's Wayland driver keeps a top-level window's surface while its client surface needs it. Cyberpunk 2077 with native Wayland was confirmed by the player.
- DXVK has GE-Proton11-11's DXGI fixes (fullscreen on focus loss, HDR detection and output state, D3D12 fullscreen resizes, swap chain backpressure, the layered overlay) and a DXGI memory-budget cap from MW4-Proton.
- vkd3d-proton no longer reserves a 4 MiB slot for each small aligned heap. From MW4-Proton.
- Call of Duty: Modern Warfare 4 and Battlefield 6 were not tested on this runtime.
