# Linux dependencies

Ferestre runs both 64-bit and 32-bit Wine. Install your distribution's 32-bit
compatibility libraries before starting a title.

| Distribution | Command |
|---|---|
| Ubuntu / Debian | `sudo dpkg --add-architecture i386 && sudo apt update && sudo apt install libc6:i386 libstdc++6:i386` |
| Fedora | `sudo dnf install glibc.i686 libstdc++.i686` |
| Arch / CachyOS | Enable `[multilib]`, then `sudo pacman -Syu lib32-glibc lib32-gcc-libs` |

Run `ferestre doctor` afterwards. It reports **32-bit Linux loader available**
when Wine can start 32-bit helpers.

Halo Infinite also needs Python 3 and util-linux (`unshare`, `mount`, `umount`),
with unprivileged user and mount namespaces enabled. Its licensed executable
view exists only in private tmpfs mounts for the lifetime of the launch.

## Video playback with current runtimes

The GStreamer libav plugin in runtimes through `11.0.20261013.3` uses the
FFmpeg 4 ABI (`libavcodec.so.58`, `libavformat.so.58`, `libavfilter.so.7`,
`libavutil.so.56`). A newer system FFmpeg, including Fedora's
`libavcodec-freeworld`, does not supply these older library names. Without them,
intro videos and cutscenes can be black or silent even when the game runs.

On Fedora, enable [RPM Fusion](https://rpmfusion.org/Configuration), then run:

```sh
sudo dnf install compat-ffmpeg4
```

On Arch/CachyOS, install `ffmpeg4.4`. Restart the game afterwards and run
`ferestre doctor`: **GStreamer libav dependencies available** means the plugin's
linked libraries resolve. Playback still depends on the codecs in those
libraries; the diagnostic does not certify a particular movie.

Ferestre warns about missing plugin dependencies before launching a title.
Until the runtime ships a decoder-enabled FFmpeg build with its full dependency
set, these compatibility packages are required for its libav-based decoding.

## SDL2 for PlayStation controllers

Ferestre hands DualShock 4 and DualSense pads to Wine through SDL, because a
GDK title sees gamepads only through XInput and GameInput, and a pad Wine
reads as raw HID is neither. Wine loads the host's `libSDL2-2.0.so.0` for
this; the runtime does not include one. Without it a PlayStation pad does
not reach any title at all.

| Distribution | Command |
|---|---|
| Ubuntu / Debian | `sudo apt install libsdl2-2.0-0` |
| Fedora | `sudo dnf install sdl2-compat` |
| Arch / CachyOS | `sudo pacman -S sdl2-compat` (the `ferestre-runtime-bin` package depends on it) |

`sdl2-compat` provides SDL2 on top of SDL3, and is what current Fedora and
Arch install in place of SDL2. `ferestre doctor` reports **SDL2 available for
PlayStation pads** when the 64-bit library resolves, and a launch prints a
warning when it does not.

A title that reads Sony HID itself, for haptics, adaptive triggers or
PlayStation button prompts, can have raw HID back: set `PROTON_ENABLE_HIDRAW=1`
(or `PROTON_DISABLE_HIDRAW=0`) in the environment, in the recipe's
`[launch.env]`, or in Proton's `user_settings.py`. Ferestre then leaves the
choice to Proton.
