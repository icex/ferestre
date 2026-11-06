# Ferestre 0.1.19

**Update** in Library → Updates now updates the existing installation directly. It no longer opens the new-install location picker. **Reload library** also works with the Store ticket format that Microsoft was rejecting.

## Fixes

- Installed-game updates keep their recorded destination, including custom folder names and nested locations. They queue behind active work, and existing paused jobs retain their saved destination and update identity.
- An unavailable drive, ambiguous relative path or recorded library containing other games produces an error instead of falling back to a fresh installation elsewhere. New installations still offer folder selection.
- Store Collections and licensing requests normalize delegationcompact user tickets at their request boundaries. Stored ticket representations, device-ticket headers and Xbox authentication keep their existing behavior.
- Library refresh errors explain Store authentication and service failures, retain the saved library, and omit the internal AppImage mount path and raw server responses.
- Builds pin the corrected client fork and include updated AUR client metadata.

## Verification

582 workspace tests passed, with two opt-in tests ignored. Formatting, Clippy, source graph verification, 103 client tests, 45 service tests and five new ticket-format tests passed, together with existing Xbox authentication and device-response tests.

Isolated GUI tests cover mouse/controller Update, exact custom destinations, queueing behind an active transfer, missing-folder refusal, existing Resume and Install flows, and library error/cache/recovery behavior. The packaged GUI passed the same update and error regressions.

A live comparison with the same Store account returned InvalidMsaTicketFormat from the original client, 113 library items from the fixed client, then the same error from the original client again. The local preview was confirmed working by the user. These checks do not establish completion of a new retail Minecraft update.

The signed public runtime remains **11.0.20261023.1**. See the [0.1.18 release notes](https://github.com/icex/ferestre/releases/tag/v0.1.18) for previous runtime, retail-game and controller qualification.
