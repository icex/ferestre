# Ferestre 0.1.16

This release adds a durable install queue, verified resumable game updates and a console-style interface for desktop and handheld PCs.

## Downloads and updates

- Install Queue saves pending work across restarts and runs one transfer at a time. Pause, resume and stop retain verified progress; restarted transfers stay paused until resumed.
- Incremental updates can reuse installed pages only after validating them against the new package's authenticated hashes and encryption parameters. Resuming rechecks staged pages. Each original file stays in place until its replacement is complete, and an interrupted-update barrier prevents launching a mixed installation.
- Installed games with a confirmed update display an **Update available** badge on their cover. Unknown checks do not show a badge, and a matching installed version clears it.
- Bounded local diagnostics explain authentication, HTTP, disk and integrity failures without keeping raw tokens or download URLs.
- The app checks for new Ferestre and runtime releases. Its update banner verifies signatures before replacing a writable AppImage or installing a runtime, and supports restart recovery. Package-manager installations retain their package update path.

## Interface and controllers

- Opaque Xbox-style home and library layouts use high-resolution cover art. Game Pass artwork preloads across the full catalogue in the background after opening that section; visible pages have priority.
- Library has one navigation row: **My games**, **Installed**, **Updates** and **Install queue**.
- A remembered fullscreen layout fits 1280×720 and 1280×800 handheld screens, with larger touch controls, visible focus and instructions for moving between navigation and games.
- Controller navigation reaches banners, tabs, games and pagination, automatically scrolling the focused item into view. SDL detects Xbox and PlayStation button families. View/Share/Create opens navigation; Menu/Options opens game actions.
- Xbox Home/Guide or PlayStation PS brings the main Ferestre window forward. KDE Wayland uses a scoped activation request; other desktops follow their compositor's window activation policy. Ordinary controller input remains inactive while another application has focus.
- Starting games display a large animated loading ring over the whole dimmed card. Once that launch's executable is observed, the card shows **Running** and a prominent **Stop** overlay. The card can also stop startup. Failures and exits clear only that game's state.

The AppImage bundles the updated client and service. This release continues to use [runtime 11.0.20261023.1](https://github.com/icex/ferestre/releases/tag/runtime-11.0.20261023.1); these launcher features do not require a new runtime build.

## Verification

Workspace unit tests and Clippy cover launch state, update badges, queue persistence, progress ordering and controller mapping. The patched client has 150 passing tests covering authenticated local-page reuse, corrupted checkpoints, interrupted transfers, cancellation and resume.

GUI fixtures exercise the actual animated ring pixels, full-card overlay bounds, pointer Stop, separate game launches, startup failure/cancellation, normal exits, update-badge clearing, background Home/PS activation, queue controls, signed updater recovery and 720p fullscreen artwork. Controller activation also passed in an isolated native KWin Wayland session.

These checks use controlled fixtures. Full retail incremental-update completion, physical handheld/touch/controller qualification and all desktop compositor policies remain outside that evidence.
