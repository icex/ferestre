# Neural rendering (experimental)

Ferestre can install [DLSSNR-AMD](https://github.com/TheAutomatic/DLSSNR-AMD)
into a game, per game and fully reversibly. DLSSNR-AMD runs NVIDIA's DLSS 5
neural rendering network on AMD Radeon RX 9000 (RDNA4) cards. It is
experimental: expect games where it does not load, or looks wrong.

Use it only for games you play single-player. Some anti-cheats ban accounts
that load mods, and Ferestre never enables it on a game whose anti-cheat is a
kernel-level one (see [Anti-cheat](#anti-cheat)).

## What you need

- An AMD Radeon RX 9000 (RDNA4) card. On Intel, NVIDIA or an older Radeon
  the option stays off: Ferestre refuses to enable it, the GUI's Enable
  button is greyed out with the reason, and a game that already has it on
  (say an eGPU was unplugged) launches without it, with your own files put
  back, and gets it again on its next launch on an RX 9000. A machine with an
  integrated GPU beside an RX 9000 qualifies.
- Mesa 26.2 or newer. Ferestre warns when it finds an older one, or cannot
  tell, but does not stop you.
- Your own copy of `nvngx_dlssnr.dll`, version 310.9.16 or 310.8.0. Ferestre
  checks it by its SHA-256 and accepts no other file. 310.9.16 carries both
  networks' model files; 310.8.0 only `CC_SILVER_AARDWOLD`.

Ferestre does not bundle, host or link to any NVIDIA file. It extracts the
model from your DLL once and keeps it in
`~/.local/state/ferestre/dlssnr/models`. It does not keep the DLL.

The DLSSNR-AMD package itself is built by Ferestre from a pinned upstream
commit (`scripts/build-dlssnr.sh`) and published as a Ferestre release asset,
checked by its SHA-256. Until that release is pinned, set
`FERESTRE_DLSSNR_PACKAGE_DIR` to the output folder of a local
`scripts/build-dlssnr.sh` run.

## Setting it up

```sh
ferestre neural setup                     # find the DLL and extract the model
ferestre neural setup --dll ~/nvngx_dlssnr.dll
ferestre neural enable <title>            # the route is detected from the game
ferestre neural status [<title>]
ferestre neural disable <title>
```

In the window, open a game's menu and choose **Neural rendering…**. The
download address field is under Settings, in **Neural rendering
(experimental)**.

### Where the DLL comes from

`setup` (and the first `enable`, when no model is cached yet) looks for the
DLL in this order and stops at the first copy whose SHA-256 matches:

1. The path you give with `--dll`: the DLL, or a `.zip` holding exactly one
   `nvngx_dlssnr.dll`.
2. The folders games are installed in: every Steam library's
   `steamapps/common`, Heroic's installs, and your local games' folders.
3. Installed Ferestre Xbox packages: the files their install already decrypted
   with your licence. A copy that the install keeps encrypted on disk, or that
   has not finished downloading, does not match and is skipped; the error
   lists it.
4. A download address you set yourself (below).

A scan stops after 30 seconds, 400,000 entries or 12 folder levels per step.
When nothing is found, the error lists what was tried and every file that was
not accepted.

### A download address

Only you can give one; this repository never holds one. Either set it in
`~/.config/ferestre/settings.toml`:

```toml
[dlssnr]
dll-url = "https://example.invalid/your-own-copy.zip"
```

or, for one run, in `FERESTRE_DLSSNR_DLL_URL`. It must be `https://`. It may
point at the DLL or a `.zip` holding it. `ferestre neural status` says
whether one is configured, never what it is.

## Routes and networks

The mod goes into the folder of the game's executable. The route is how it
hooks in:

| Route | When |
| --- | --- |
| `optiscaler` | The game ships DLSS, FSR or XeSS, and is 64-bit |
| `reshade-feeder` | Direct3D 10, 11 or 12 |
| `reshade-feeder-vulkan` | Vulkan |
| `reshade-feeder-dx9` | Direct3D 9 |

`--route` picks one yourself. An executable Ferestre cannot read is refused
with "pass --route", except an Xbox install's, which is kept encrypted and is
assumed to be x86-64 Direct3D 12.

`--model` picks the network it starts with: `CC_SMALL_FP8` (the default when
its model is cached), `CC_SMALL_FP8_EXPERIMENTAL`, `CC_SILVER_AARDWOLD` (the
default otherwise) or `CC_SILVER_AARDWOLD_OPTIMIZED`.

At launch Ferestre adds `dxgi=n,b` (or `winevulkan=n,b;vulkan-1=n,b` for the
Vulkan and Direct3D 9 routes) to `WINEDLLOVERRIDES`. An override you set
yourself for the same DLL wins, and the launch says the mod may then not load.

## Anti-cheat

- A kernel-level anti-cheat that Proton cannot run (Ricochet, Vanguard, ACE,
  GameGuard, mhyprot, EA AntiCheat and others): neural rendering is never
  enabled. There is no override.
- Any other anti-cheat, including Easy Anti-Cheat and BattlEye: enabling needs
  `--accept-anticheat-risk` (or the checkbox in the window). Ferestre
  remembers it for that game and that anti-cheat, until you remove a local
  game from Ferestre.

The check runs again before every launch. If a game update brings in an
anti-cheat that forbids the mod, Ferestre takes the mod out, puts your files
back and turns neural rendering off for the game. If it cannot take it out,
the launch stops.

## What is changed in the game folder, and how it comes back

Before installing, Ferestre copies every file and folder of the route's that
is already there (another mod's `dxgi.dll`, a `ReShade.ini`, a
`reshade-shaders` folder) into `.ferestre-dlssnr-backup/` beside the
executable, and lists it in `.ferestre-dlssnr-backup/manifest.json`. Then it
runs DLSSNR-AMD's own `install.sh`.

`ferestre neural disable <title>` runs `install.sh remove` and puts every
backed-up file back as it was. Removing a local game in Ferestre does the same
first; removing an Xbox title removes its folder anyway.

If a game update or file check removes the mod's files, the next launch
installs it again and keeps your `dlssnr-amd.ini`. A launch with `--dry-run`
only shows what it would add.

If the backup manifest has been edited so that it names something outside the
route's files, Ferestre refuses to restore and deletes nothing; restore by hand
from `.ferestre-dlssnr-backup/`.

## Logs

- `~/.local/state/ferestre/dlssnr/logs/<PRODUCT ID>.log`: every `install.sh`
  run for that game. A failed install's error ends with its last lines.
- `dlssnr-amd.log` in the game's executable folder: DLSSNR-AMD's own log.
