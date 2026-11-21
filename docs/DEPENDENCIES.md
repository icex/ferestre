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

Licensed Xbox packages detected by their Easy Anti-Cheat bootstrapper or
Easy Anti-Cheat directory also need Python 3 and util-linux (`unshare`, `mount`,
`umount`), with unprivileged user and mount namespaces enabled. Ferestre
automatically makes the licensed images available at their installed paths
through private tmpfs mounts for the lifetime of the launch. Protected play on
Proton requires the publisher to enable Linux support for that game build.

On the first launch Ferestre also downloads Valve's Proton Easy Anti-Cheat and
BattlEye runtimes from the same Lutris URLs that Lutris and Heroic use. It keeps
each one only if its SHA-256 matches the pinned value, unpacks it under
`~/.local/state/ferestre/anticheat` (or your state directory), and passes it to
Proton as `PROTON_EAC_RUNTIME` and `PROTON_BATTLEYE_RUNTIME`. A value you set
yourself, or one set by the title's recipe, takes precedence. The Easy
Anti-Cheat runtime is used only when the game ships EAC's Linux library
(`easyanticheat_x64.so`) next to its Windows one.

The Xbox PC build of Gears 5 does not ship that library. Its recipe asks for
it (`FERESTRE_EAC_LINUX_LIBRARY = "pinned"`), so Ferestre downloads a copy,
keeps it only if its SHA-256 matches the pinned value, and stores it under
`~/.local/state/ferestre/anticheat/eac_linux`. During the launch the private
image view shows it in the game's `EasyAntiCheat` directory through a
read-only overlay; the installed package is not changed. To use your own copy
instead, set `FERESTRE_EAC_LINUX_LIBRARY` to its path.

## WebKitGTK for sign-in

The Xbox client (`xodus-cli`) shows Microsoft's sign-in page in a WebKitGTK
window. A browser cannot stand in for it: the page needs request headers and a
JavaScript bridge back to the client. The client therefore links
`libwebkit2gtk-4.1.so.0`, and through it `libjavascriptcoregtk-4.1.so.0`,
`libsoup-3.0.so.0` and `libgtk-3.so.0`. Without them the system's loader
refuses to start the client for any command, not only sign-in, and the exit
status is 127. The AppImage does not bundle WebKitGTK. Install your
distribution's package:

| Distribution | Command |
|---|---|
| Ubuntu / Debian (and Mint, Pop!_OS, KDE neon) | `sudo apt install libwebkit2gtk-4.1-0` |
| Fedora (and Nobara) | `sudo dnf install webkit2gtk4.1` |
| Arch (and CachyOS, EndeavourOS, Manjaro) | `sudo pacman -S webkit2gtk-4.1` |
| openSUSE | `sudo zypper install libwebkit2gtk-4_1-0` |
| Fedora Silverblue, Kinoite, Bazzite and other rpm-ostree images | `rpm-ostree install webkit2gtk4.1`, then restart |

The package brings the other three libraries with it.

SteamOS keeps its system read-only, and every system update replaces it, so a
package installed into it does not last. Ferestre gives no command there.

`ferestre doctor` reports **Xbox client libraries available** when the client's
libraries resolve. When they do not, it lists the missing ones and the command
for the distribution named in `/etc/os-release`. The window checks the same
thing when it opens. It shows a banner while the client cannot start, and
**Sign in** explains the problem instead of failing. Where PackageKit is
available (KDE Discover and GNOME Software use it), the banner's **Install…**
button installs the package through it, and your desktop asks for your
password. Otherwise the dialog offers the command to copy into a terminal.
An rpm-ostree image is never offered the PackageKit install, because the
package only takes effect after a restart.

Ferestre stays open until an install it started has finished.
Once the library is installed the banner goes away, and an install started
from Sign in carries on signing in. If you install it some other way, from a
terminal or a software centre, the window notices when you come back to it,
press Refresh, or press Sign in or the banner's button.

## Video playback with current runtimes

The GStreamer libav plugin in runtimes through `11.0.20261013.3` uses the
FFmpeg 4 ABI (`libavcodec.so.58`, `libavformat.so.58`, `libavfilter.so.7`,
`libavutil.so.56`). A newer system FFmpeg, including Fedora's
`libavcodec-freeworld`, does not supply these older library names. Without them,
intro videos and cutscenes can be black or silent even when the game runs.

A Steam install already provides them. Steam ships these four sonames in
`ubuntu12_64/video` and `ubuntu12_32/video`, and Proton puts those directories
first on its library path when Ferestre hands it the Steam install. `ferestre
doctor` checks the same directories, in the same order, so it reports nothing
missing when Steam is found. A shell `LD_LIBRARY_PATH` does not count for
`ferestre doctor`, because a default launch removes it before starting Proton.
`ferestre run --steam` keeps it, and a recipe can set its own in `launch.env`;
Proton searches either one after Steam's and the runtime's directories, and the
check `ferestre run` makes before a launch searches it there too.

Without a Steam install (or with one that lacks these files), install a
compatibility package. On Fedora, enable [RPM Fusion](https://rpmfusion.org/Configuration), then run:

```sh
sudo dnf install compat-ffmpeg4
```

On Arch/CachyOS, install `ffmpeg4.4`. Restart the game afterwards and run
`ferestre doctor`: **GStreamer libav dependencies available** means the plugin's
linked libraries resolve. Playback still depends on the codecs in those
libraries; the diagnostic does not certify a particular movie.

Ferestre warns about missing plugin dependencies before launching a title.
Until the runtime ships a decoder-enabled FFmpeg build with its full dependency
set, Steam's copy or these compatibility packages are required for its
libav-based decoding. Set `XODUS_STEAM_DIR` if your Steam install is somewhere
Ferestre does not look.

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

## Microsoft's package signing key, fetched at run time

The client checks the signature on every game package's header with
Microsoft's game package key. Nothing installs it and nothing ships it: the
first install fetches Microsoft Gaming Services anonymously from the Store
(about 200 MB, a few seconds on a fast connection), keeps the 539-byte key
if its fingerprint is the pinned one, and deletes the rest. It needs
`displaycatalog.mp.microsoft.com`, `fe3.delivery.mp.microsoft.com`,
`www.microsoft.com` and the download host the delivery service names, which
must be under `*.delivery.mp.microsoft.com` (so far
`tlu.dl.delivery.mp.microsoft.com`), to be reachable,
once. `ferestre doctor` reports **package signatures are checked** when the
key is in place, and `xodus-cli xvd-key fetch` fetches it by hand. See
[PACKAGE-SIGNATURES.md](PACKAGE-SIGNATURES.md) for what is checked and the
settings.
