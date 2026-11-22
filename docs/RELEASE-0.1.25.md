# Ferestre 0.1.25

Pairs with runtime 11.0.20261122.2, unchanged from 0.1.24.

## Local games

Ferestre can now be the one library for the Windows games you already have, not only the ones it downloads from the Store.

- **Import from Steam** (in the window's Add games menu, or `ferestre import-steam`) brings in every non-Steam shortcut from every Steam user on the machine. Each game keeps its name, executable, launch options (environment before `%command%`, wrapper commands such as `gamemoderun`, arguments after it), its prefix (the `STEAM_COMPAT_DATA_PATH` in its launch options, or Steam's own `compatdata` folder for the shortcut), its Proton build and its grid artwork. Your saves stay where they are. Ferestre's own shortcuts are skipped, and Steam's files are only read, never changed. `--dry-run` shows what would be imported.
- **Add game…** (or `ferestre add <path to .exe>`) adds any Windows game on disk; its name and folder are filled in and can be edited.
- Local games launch through `ferestre run` like any other title, without the Xbox client, service, licence, presence or accounts. A game starts in its folder, as Steam starts a shortcut in its start folder; a recipe whose executable skips a launcher can name another with `[launch] working-dir` (GOG's The Witcher 3 run without its launcher needs `bin\x64_dx12`). A game on a drive that is not mounted says so instead of starting.
- The Proton builds Steam knows about (`compatibilitytools.d`, Steam's own Proton) can run local games, so an imported game keeps its GE-Proton or Proton version. Xbox titles keep their own runtime rules.
- Play, Add to Steam and Edit launch settings work for local games; the editor also has their Arguments and Wrapper. **Remove** only takes a local game out of Ferestre: its folder and prefix are never deleted.
- Anti-cheat files in a game's folder are named before you play: Easy Anti-Cheat, BattlEye, Ricochet, Riot Vanguard, EA AntiCheat, nProtect GameGuard, ACE, XIGNCODE3 and mhyprot, with a caution for kernel-level ones. They are a caution, never a block.

## Neural rendering (experimental)

NVIDIA's DLSS 5 neural rendering network on AMD Radeon RX 9000 (RDNA4) cards, through [DLSSNR-AMD](https://github.com/TheAutomatic/DLSSNR-AMD). See [Neural rendering](NEURAL-RENDERING.md).

- Per game: **Neural rendering…** in a game's menu, or `ferestre neural enable <id>`. The route is detected from the game (OptiScaler for a 64-bit game with DLSS, FSR or XeSS; ReShade for other Direct3D 10–12, Vulkan or Direct3D 9 games). Every file it would overwrite is backed up, and Disable puts them back byte for byte.
- In game, **Ctrl+Y** opens DLSSNR-AMD's NR Control menu (upstream's End is missing from many compact keyboards); `ferestre neural menu-key <key>` chooses another for every game. A game that crashes with OptiScaler as `dxgi.dll` can take another name it loads as: `ferestre neural enable <id> --proxy winmm` (Call of Duty: Modern Warfare 4 needs this).
- The model comes from your own `nvngx_dlssnr.dll` (310.9.16 or 310.8.0, checked by SHA-256): a path you give, a copy found in your Steam, Heroic or local game folders, or a download address you set in Settings. Ferestre does not ship or link to any NVIDIA file.
- The install is checked before every launch and redone after a game update.
- Only on an RX 9000 card: on Intel, NVIDIA or an older Radeon it cannot be turned on, and a game that has it launches without it until the card is back.
- A game with anti-cheat needs your explicit acknowledgement, remembered per game. A kernel-level anti-cheat gets the strongest warning: accept it only for a build that never goes online.
- The DLSSNR-AMD Linux package is built by Ferestre from a pinned upstream commit and published as its own `dlssnr-0.0.9` release.

## Fixes

- **Jump back in** on Home showed a blank tile for an installed game whose banner had never been downloaded (only featured games' banners are): it now shows the banner only once it is on disk, and the game's cover until then. Local games show their own landscape art there and in the featured carousel.
