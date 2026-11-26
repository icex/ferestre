# Per-game launch options

Open a game's **More actions → Edit launch settings**. Choose **Save** and
relaunch the game to try the changes. Ferestre saves your edits as a personal
copy of the title's recipe. **Reset to defaults** sets that copy aside as a
`.toml.bak` file and restores the shipped or detected settings. A local
game added by executable has no separate default recipe to restore.

The tables use the editor's labels. Variable names are the corresponding
keys in a recipe's `[launch.env]`. Most switches set their variable to `1`
when enabled and leave it out when disabled. The automatic switches in
**Display** work in reverse: clearing the switch writes `0`. Shipped
recipes may already choose settings for individual titles. For the file
format and other recipe fields, see [Recipes](RECIPES.md) and the
[recipe schema](../titles/SCHEMA.md).

## General

| Field | What it controls |
|---|---|
| Name | The game's display name. |
| Executable | The program to start. For a Store title, use a path inside the installed title. Local titles also accept full paths. |
| Game folder or Install directory | The path where the title's files are installed. |
| Wine prefix directory | The location for the game's Wine prefix. |
| Arguments | Command-line arguments for a local game. |
| Wrapper commands | Commands that wrap a local game's launch, such as `gamemoderun`. |

## Display

| Setting | Variable | What it controls |
|---|---|---|
| Native Wayland | `FERESTRE_NATIVE_WAYLAND` | Uses Wine's native Wayland driver and allows automatic HDR on that path. Use it for titles that work with native Wayland. |
| Gamescope | `FERESTRE_GAMESCOPE` | **Automatic** leaves Ferestre's selection in place. **Use Gamescope** (`1`) requests the compositor and **Never use Gamescope** (`0`) disables it. Gamescope integration is experimental. |
| Automatic HDR | `PROTON_ENABLE_HDR` | Allows HDR when supported by the monitor and display path. Turn it off to set `0` for the game. |
| Performance profile while playing | `FERESTRE_PERFORMANCE` | Allows Ferestre to use the CachyOS performance profile or GameMode while the game runs. Turn it off to set `0`. |
| Desktop integration | `FERESTRE_DESKTOP_INTEGRATION` | Lets Ferestre choose the display path, HDR and performance profile automatically. Turn it off (`0`) to use the older launch path. |

See [Desktop integration](DESKTOP-INTEGRATION.md) for display-path and host requirements.

## Compatibility

### Controllers

| Setting | Variable | What it controls |
|---|---|---|
| Raw HID for PlayStation controllers | `PROTON_ENABLE_HIDRAW` | Exposes the controller's HID device to the game. Try it with a PS4 DualShock 4 or PS5 DualSense when a title expects native PlayStation input or button prompts. Turn it off and relaunch if the game loses controller input. |

SDL is the regular path for PlayStation pads in GDK titles. See
[SDL2 for PlayStation controllers](DEPENDENCIES.md#sdl2-for-playstation-controllers)
for host packages.

### Runtime compatibility

| Setting | Variable | What it controls |
|---|---|---|
| HTTP/2 for WinHTTP | `FERESTRE_WINHTTP_HTTP2` | Enables HTTP/2 for title services that require it. Needs `winhttp.http2`. |
| Strict manual task queues | `XGR_STRICT_MANUAL_QUEUES` | Keeps manual work on the dispatching thread and executes it when the game dispatches it. Needs `xgameruntime.manual-queue-thread-affinity`. |
| Parallel thread-pool queues | `XGR_PARALLEL_THREADPOOL_QUEUES` | Allows thread-pool work to run concurrently when callbacks wait on one another. |
| Wait for completion callbacks | `XGR_WAIT_FOR_ASYNC_CALLBACKS` | Makes a blocking status check also wait for the operation's completion callback. Needs `xgameruntime.async-callback-wait`. |
| Live Store catalog | `XGR_STORE_CATALOG` | Answers Store product queries with live Microsoft catalog data. Needs `xgameruntime.store-catalog`. |
| Game files at their real paths | `FERESTRE_IMAGE_VIEW` | **Automatic** follows the title's default. **Always** (`real-path`) presents the licensed game files at their installed paths in a private view. |

### Anti-cheat

| Setting | Variable | What it controls |
|---|---|---|
| Easy Anti-Cheat Linux library | `FERESTRE_EAC_LINUX_LIBRARY` | **Not needed** leaves it out. **Download it** (`pinned`) uses the known, hash-checked Linux EAC library when the title's package does not contain it. |

## Upscaling

| Setting | Variable | What it controls |
|---|---|---|
| OptiScaler from Proton | `PROTON_USE_OPTISCALER` | Asks Proton to download and load its OptiScaler. Use one OptiScaler installation route per game. |
| FSR 4 upgrade | `PROTON_FSR4_UPGRADE` | Uses FSR 4 where the game uses FSR 3.1. |
| FSR 4 indicator | `PROTON_FSR4_INDICATOR` | Shows an on-screen indication when FSR 4 is active. |
| FSR 3 upgrade | `PROTON_FSR3_UPGRADE` | Uses a newer FSR 3 implementation than the game ships. |
| FSR frame generation upgrade | `PROTON_MLFG_UPGRADE` | Uses the machine-learning implementation where the game uses FSR frame generation. |
| DLSS upgrade | `PROTON_DLSS_UPGRADE` | Uses a newer DLSS library for supported NVIDIA GPUs. |
| DLSS indicator | `PROTON_DLSS_INDICATOR` | Shows an on-screen indication when DLSS is active. |
| XeSS upgrade | `PROTON_XESS_UPGRADE` | Uses a newer XeSS implementation than the game ships. |

## Proton

**Runtime** selects the installed Proton build for this game. **Default**
follows the launcher setting under **Settings → Choose default runtime**.
For games started by Steam, Steam chooses the runtime. Proton builds
recognize different flags. The editor reports when a flag is unknown to
the selected runtime.

| Setting | Variable | What it controls |
|---|---|---|
| No NVIDIA API | `PROTON_DISABLE_NVAPI` | Leaves out Proton's NVAPI implementation. Some games expose DLSS when OptiScaler provides its own NVAPI. |
| Force the NVIDIA API | `PROTON_FORCE_NVAPI` | Requests NVAPI for a title Proton would normally start without it. |
| Hide NVIDIA cards | `PROTON_HIDE_NVIDIA_GPU` | Reports an NVIDIA GPU to the game as an AMD one. |
| No ntsync | `PROTON_NO_NTSYNC` | Disables Wine's kernel ntsync path. |
| No fsync | `PROTON_NO_FSYNC` | Disables futex-based synchronization. |
| No esync | `PROTON_NO_ESYNC` | Disables eventfd-based synchronization. |
| WineD3D | `PROTON_USE_WINED3D` | Uses OpenGL-based WineD3D for Direct3D 8–11 in place of DXVK. |
| DXVK for Direct3D 8 | `PROTON_DXVK_D3D8` | Uses DXVK for Direct3D 8 titles. |
| WoW64 mode | `PROTON_USE_WOW64` | Uses Proton's WoW64 mode to run 32-bit games without 32-bit host libraries. |
| Shader cache per game | `PROTON_LOCAL_SHADER_CACHE` | Keeps the shader cache in the game's own prefix. |
| No Steam Input | `PROTON_NO_STEAMINPUT` | Requests that Proton exclude Steam Input for the game. |
| Controllers through SDL | `PROTON_PREFER_SDL` | Requests that Proton prefer SDL to the controllers' HID devices. |
| Delay freeing memory | `PROTON_HEAP_DELAY_FREE` | Keeps freed memory around longer for games that still reference it. |
| 4 GB for 32-bit games | `PROTON_FORCE_LARGE_ADDRESS_AWARE` | Lets a 32-bit game address up to 4 GB. |
| Proton log | `PROTON_LOG` | Writes `steam-<id>.log` in your home directory. Logging can slow the game. |

## Variables

Use **Other variables** to add, edit or remove arbitrary environment
variables. A recipe value that a switch or menu cannot represent stays
here. Each variable name can appear once.

## Neural rendering (experimental)

**Set up** opens the DLSSNR-AMD options. **Route** selects **Automatic**,
`optiscaler`, `reshade-feeder`, `reshade-feeder-vulkan` or
`reshade-feeder-dx9`. **Starting network** selects **Automatic**,
`CC_SMALL_FP8`, `CC_SMALL_FP8_EXPERIMENTAL`, `CC_SILVER_AARDWOLD` or
`CC_SILVER_AARDWOLD_OPTIMIZED`. The setup can also ask for an anti-cheat
acknowledgement. **Enable** applies the chosen setup, **Reinstall** applies
it again and **Disable** restores the backed-up files. The **nvngx_dlssnr.dll
download address** field accepts a source for the model on supported setups.
See [Neural rendering](NEURAL-RENDERING.md) for requirements, routes and recovery.
