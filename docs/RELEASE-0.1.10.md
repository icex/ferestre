# Release 0.1.10 qualification

Launcher **0.1.10** pairs with the existing runtime **11.0.20261013.3**.
This release changes launcher activity reporting and shutdown; it includes no
new runtime or game compatibility claims.

## Included changes

- Active operations stay visible until they finish. Busy messages identify
  the operation and game that must finish before another action can start.
- The top-right spinner's tooltip names the current task, including account,
  library and update checks, and explains pending shutdown.
- Closing during work asks for confirmation. Downloads stop while retaining
  game files for resuming; removals finish before the window exits.
- The window waits for the downloader to exit and for launcher cleanup before
  closing. A stopped update keeps its existing files and install record.
- Pending close can be cancelled with Keep Window Open. A download already
  asked to stop continues its cleanup while the window stays open.

## Verification and limits

- 295 Rust workspace tests, formatting and Clippy with warnings denied passed
  locally.
- Offline subprocess tests exercised download cancellation and unexpected
  window disappearance, for both fresh downloads and existing installations.
  They checked downloader cleanup, exit status, retained files and unchanged
  install records. The installed AppImage passed the same cancellation tests.
- GUI checks exercised persistent removal status, spinner tooltip, a blocked
  second operation, dismissing close confirmation, cancelling pending close,
  removal completion before exit and waiting for download cleanup before exit.
  Local GUI regression checks used accessibility actions because mouse
  injection into the headless display did not work. Hosted GUI smoke runs the
  normal pointer path before this release is tagged.
- Runtime and gameplay qualification remains as documented for
  [0.1.9](RELEASE-0.1.9.md). No new real-game download or gameplay qualification
  is claimed by these offline fixtures. A blocked network read can delay
  runtime-download cancellation until its existing request timeout expires;
  the window stays open and reports cleanup while waiting.
