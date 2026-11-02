# Application and runtime updates

Ferestre checks the public release feed at startup and every six hours while the window is open. **Runtime → Check for Updates** and the window's Refresh button check immediately. An offline check leaves the installed application and runtime alone and retries later.

When a newer stable release is available, a banner names the application and/or runtime version. **Update** performs one combined operation. The AppImage includes the Xbox client and service, so updating Ferestre updates those components together. The separate runtime archive updates Proton, Wine and xgameruntime together.

## What happens after clicking Update

1. Authenticate the release's `SHA256SUMS` with the minisign public key compiled into Ferestre. The signed comment must name the checksum manifest and exact release version.
2. Download the selected artifacts over HTTPS, verify their own minisign signatures and compare them with the authenticated checksum manifest. Draft releases, prereleases, older versions, unexpected asset names, duplicate assets and URLs outside the expected repository/tag are refused.
3. Stage the new AppImage beside the installed file, run its CLI startup/version check with the old mount's environment overrides removed, and retain the old executable. A lock prevents concurrent replacement; inode, size, timestamp and permission checks detect changes during download. The GUI also binds the operation to its original AppImage digest and the versions it offered, refusing stale windows or an older feed response.
4. Install the runtime in a new version directory. Existing runtimes, title-specific choices, prefixes and game files are retained. Automatic selection prefers the newest compatible managed release even when its upstream `version` file still names an older base build.
5. Save a recovery receipt before atomically replacing the AppImage. Restart automatically once the window has no running games or other work. If executing the new AppImage fails, restore the exact previous copy. A later independent update is never rolled back using an old receipt.

The application-update button waits while the window is busy or a game it started is running. Cancellation before application replacement keeps the old AppImage. A runtime that completed before cancellation remains installed beside the old runtimes.

## Supported installations

An inherited `APPIMAGE` from another launcher is ignored unless Ferestre itself runs inside its matching AppDir. One-click application replacement supports executable, user-owned type-2 AppImages in a user-owned writable directory without group/world write permissions. Root execution, symlink targets and shared-writable executables are refused. System packages, source checkouts and portable archives remain managed by their original installer; their banner identifies an available release without overwriting that installation. Managed runtime updates remain available independently.

A new public release containing the updater must be installed before subsequent launcher releases can be installed through this banner.

## Recovery and CLI

```sh
ferestre check-app-updates --json  # read-only application/runtime check
ferestre self-update             # apply available signed updates
ferestre rollback-app-update     # restore the previous AppImage
```

For an AppImage, use its filename in place of `ferestre`, for example `./ferestre.AppImage self-update`.

The recovery receipt is `last-application-update.json` in Ferestre's state directory. Backups stay beside the AppImage. Rollback checks both file digests and ownership before restoring anything. If the new application starts successfully and later crashes, the rollback command or the saved previous AppImage is the recovery path; the updater does not supervise later application sessions. Power-loss recovery has not been simulated.

Update requests are anonymous reads of the fixed Ferestre GitHub release API and release assets. They do not send Microsoft credentials, account data, installed-game listings or source files. The production CLI provides no release-URL or signing-key override. Test signers and loopback fixtures are confined to tests.

## Validation

Core tests cover signed metadata, wrong keys, damaged or mismatched signatures, replay across versions, numeric version ordering, asset/URL confusion, duplicate checksums, unsafe targets, concurrent updates, changed installed files, staged-file tampering, cancellation, atomic replacement, recovery receipts and conditional rollback. HTTP tests retain resumable-download and signature regressions.

The offline GTK workflow covers the visible banner, a single combined-update click, verification failure, cancellation, restart into the new executable, failed-restart restoration, package-managed installations, offline checks and manual refresh. It runs through AT-SPI in a private display/session with private state.

A separate local check used genuine signed 0.1.15 release artifacts to update a private copy of 0.1.14, verified the installed version and backup, then restored 0.1.14 through the recovery command. That check did not download/install a new runtime or change the production installation.
