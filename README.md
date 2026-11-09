<img src="packaging/icons/hicolor/scalable/apps/io.github.icex.ferestre.svg" width="64" align="left" alt="">

# Ferestre

Play compatible **Xbox PC and Microsoft Store games on Linux** using your Microsoft account, purchases and Game Pass subscription.

<br clear="left">

**[Download the AppImage](https://github.com/icex/ferestre/releases/download/v0.1.20/ferestre-0.1.20-x86_64.AppImage)** · [Compatible games](docs/COMPATIBILITY.md#the-matrix) · [Quick start](#quick-start) · [Report a problem](https://github.com/icex/ferestre/issues)

![Ferestre game library in handheld fullscreen mode, with controller navigation and game-update badges](docs/screenshots/library-handheld.png)

<table>
  <tr>
    <td width="50%"><img src="docs/screenshots/running-game.png" alt="Resume or stop a running game from its library card"><br><strong>Switch between running games</strong></td>
    <td width="50%"><img src="docs/screenshots/install-game.png" alt="Choose a game installation folder inside Ferestre"><br><strong>Install with a controller-friendly folder picker</strong></td>
  </tr>
</table>

<details>
<summary>See the Stop confirmation dialog</summary>

![In-app Stop confirmation with Cancel selected and controller button hints](docs/screenshots/stop-confirmation.png)

</details>

*Screenshots use a demo library and synthetic account data.*

## Quick start

### 1. Download and open Ferestre

You need **x86_64 Linux**, a Vulkan-capable GPU with working drivers, and the [host libraries](docs/DEPENDENCIES.md) used by the launcher and runtime. Steam can supply some video-playback dependencies.

Download the `.AppImage` from the [latest release](https://github.com/icex/ferestre/releases/latest). Save it as `ferestre.AppImage` to use the commands below, or substitute its downloaded filename.

```sh
chmod +x ferestre.AppImage
./ferestre.AppImage
```

You can also make it executable through your file manager's **Properties → Permissions**, then open it normally. If FUSE mounting is unavailable, run:

```sh
./ferestre.AppImage --appimage-extract-and-run
```

Arch users can use the [AUR packages](packaging/aur/README.md). For source builds, see [CONTRIBUTING.md](CONTRIBUTING.md).

### 2. Set up the runtime and sign in

Let Ferestre download its **signed runtime** when prompted. The AppImage includes the launcher and Xbox client; the Proton/GDK runtime is a separate download.

Open the profile menu and choose **Sign in**. If the sign-in window needs WebKitGTK, Ferestre offers the appropriate package installation or a command for your distribution.

In **Accounts**, choose the Microsoft account supplying purchases/Game Pass (**Store**) and the Xbox account you want to play as (**Player**). They can be the same account. See the [account guide](docs/ACCOUNTS.md) for separate accounts and saves.

### 3. Install and play

1. Open **My Library** for your account's games and local installations, or **Game Pass** to browse the current PC catalogue.
2. Choose a game, select **Install**, choose a library folder and confirm.
3. Follow progress in **My Library → Install Queue**. Choose **Play** when installation finishes.

Check the [compatibility list](docs/COMPATIBILITY.md#the-matrix) before a large download. Ferestre supports compatible **GDK/MSIXVC** games; many other Store apps use UWP/Appx/Msix formats that are currently unsupported. Game Pass access depends on your subscription, region and the title still being included. Microsoft checks the licence when installing or launching.

## Quick guides

### Pause or resume a download

Open **My Library → Install Queue** and choose **Pause** or **Resume**. The queue survives restarts and runs one transfer at a time. Resume keeps the saved destination and verified progress.

See [downloads and controls](docs/DOWNLOADS.md) for progress, reused data and recovery.

### Update a game

Open **My Library → Updates**, then choose **Update**. Ferestre updates the existing installation in its current folder. A paused update offers **Resume** and continues that update.

### Switch between games

Running game cards show **Resume** and **Stop**. Resume returns to the existing game window. Stop opens an in-app confirmation with **Cancel** selected first.

Keep Ferestre open while playing to use the controller's **Xbox/Home** or **PS** button to return to the launcher. Select another game, resume one already running, or stop it from its card.

### Use a controller or handheld PC

Use the header's fullscreen button or **F11** to enter the handheld layout. Button hints adapt to Xbox and PlayStation controllers.

| Action | Xbox | PlayStation |
|---|---|---|
| Move focus | D-pad / left stick | D-pad / left stick |
| Select | A | ✕ |
| Back / cancel | B | ○ |
| Game options | Menu | Options |
| Return to Ferestre | Xbox/Home | PS |

Text entry, destination selection, installation, queues and confirmation dialogs work inside the app. PlayStation controllers need the [host SDL2 library](docs/DEPENDENCIES.md#sdl2-for-playstation-controllers).

### Update Ferestre and its runtime

Open **Settings → Check for Updates**, then use the **Update** banner. Downloads are signature-checked; a writable AppImage is replaced with a backup kept for recovery. New runtimes install alongside previous versions, and a restart waits for current work to finish.

Existing per-game runtime choices remain selected. To change one, use the game's **More actions → Choose runtime**. **Settings → Choose default runtime** sets the default; **Use default runtime** clears a game's override. Changes apply to the next launch.

See [signed updates and rollback](docs/UPDATES.md) for supported installations. Update package-managed installations through your package manager.

### Enable HDR

Enable HDR in your desktop's display settings, install the current Ferestre runtime, then enable HDR in the game's video settings when available. Native Wayland HDR is supported on tested KDE/Mesa configurations; support varies by desktop, driver and game.

Choose a suitable rendering resolution in the game. The runtime includes 4K and scaled ultrawide modes and preserves the display's native refresh rate. See [desktop integration](docs/DESKTOP-INTEGRATION.md) for detection, window controls and performance helpers.

### Move a game or add it to Steam

- **Move:** open the game's **More actions → Move location**, choose an existing library folder and confirm after the space check. Ferestre verifies the move, and keeps the original if it fails or is cancelled. Saves and prefixes retain their existing locations.
- **Steam:** close Steam, then choose **Add to Steam** in the game's menu. **Add All to Steam** in the Installed list adds all installed games. Shortcuts include available artwork and retain their identity when updated.

### Refresh your library

Use the top-right **Refresh** button, or **Reload library** in the profile menu, after purchases or subscription changes. Lists open from cache for speed; refresh checks them again. A failed refresh retains the last complete library.

**My Library** contains account titles, local installations and transfers in progress. **Game Pass** is the separate public PC catalogue. A game leaving Game Pass can remain installed and manageable, but still needs a valid licence to play. The [compatibility list](docs/COMPATIBILITY.md#the-matrix) lists tested games independently of what your account owns.

## Troubleshooting

| Problem | Start here |
|---|---|
| Sign-in will not open | Install the WebKitGTK package offered by Ferestre; see [host dependencies](docs/DEPENDENCIES.md#webkitgtk-for-sign-in). |
| A game is missing or access is refused | Refresh the library, check the selected Store account and current Game Pass coverage. A previous subscription download does not grant permanent ownership. |
| A game will not start | Run `./ferestre.AppImage doctor`, check the game's [compatibility notes](docs/COMPATIBILITY.md#the-matrix), and select a suitable runtime in game options. |
| Videos are black or silent | Check [video-playback dependencies](docs/DEPENDENCIES.md#video-playback-with-current-runtimes). |
| Controller input is missing | Check [SDL2 dependencies](docs/DEPENDENCIES.md#sdl2-for-playstation-controllers). Steam shortcuts can use Steam Input. |
| An update needs recovery | See [update backups and rollback](docs/UPDATES.md). |

When [reporting a problem](https://github.com/icex/ferestre/issues), include the game, Ferestre/runtime versions, Linux distribution and what happened. Review logs for personal information before sharing them.

## Command line

The AppImage also accepts CLI commands:

```sh
./ferestre.AppImage doctor
./ferestre.AppImage library
./ferestre.AppImage install PRODUCT-ID
./ferestre.AppImage run PRODUCT-ID
./ferestre.AppImage updates
./ferestre.AppImage check-app-updates
```

Use `./ferestre.AppImage --help` for all commands. The window and CLI use the same installation records and launch decisions. [Recipes and advanced setup](docs/RECIPES.md) cover custom paths and manual troubleshooting.

## Documentation and development

| Guide | Covers |
|---|---|
| [Game compatibility](docs/COMPATIBILITY.md) | Tested titles, limitations and verification dates |
| [Dependencies](docs/DEPENDENCIES.md) | Distribution-specific sign-in, graphics, video and controller libraries |
| [Downloads](docs/DOWNLOADS.md) | Install queues, pause/resume and progress |
| [Updates](docs/UPDATES.md) | Signed application/runtime updates and recovery |
| [Desktop integration](docs/DESKTOP-INTEGRATION.md) | HDR, scaling, refresh rate, window controls and performance helpers |
| [Accounts](docs/ACCOUNTS.md) | Sign-in, Store/Player selection, saves and sign-out |
| [Online presence](docs/PRESENCE.md) | What Xbox friends see while you play |
| [Pinned source forks](docs/SOURCES.md) | Component repositories, commits and source verification |
| [Package verification](docs/PACKAGE-SIGNATURES.md) | Microsoft package signatures and download integrity |
| [Contributing](CONTRIBUTING.md) | Build, test and add game recipes |
| [Roadmap](docs/ROADMAP.md) | Current work and planned support |
| [Releasing](docs/RELEASING.md) | Release builds and signing |
| [Debugging notes](notes/) | Technical investigations |

Current releases: **[Ferestre 0.1.20](https://github.com/icex/ferestre/releases/tag/v0.1.20)** and **[runtime 11.0.20261107.1](https://github.com/icex/ferestre/releases/tag/runtime-11.0.20261107.1)**. Read the [release notes](docs/RELEASE-0.1.20.md) for tested scope.

## About the project

Ferestre began with a simple goal: let my son play the Windows Store version of Minecraft he already owned after moving to Linux. The launcher, catalogue and runtime support grew from that work.

It is a personal open-source project, with support as time allows. Reports, game recipes and pull requests are welcome. **Ferestre** is Romanian for **windows**, pronounced roughly *feh-RESS-treh*.

Ferestre uses a Microsoft Store/Xbox client for authentication and downloads, a Wine/Proton runtime with an implementation of the Xbox GDK, and this launcher. It signs in with your account and ships no game content. It is independent of Microsoft, Mojang, Valve and the upstream Xodus project.

## Licence

MIT for the original launcher, scripts and tests; LGPL-2.1-or-later for the Wine, xgameruntime and vkd3d-proton forks; GPL-3.0-only for the Xodus client; BSD-3-Clause for Proton's top-level components. [NOTICE](NOTICE) records component provenance and licences. The launcher invokes the client as a separate process.
