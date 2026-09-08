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
