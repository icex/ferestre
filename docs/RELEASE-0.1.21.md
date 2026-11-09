# Ferestre 0.1.21

My Library now shows your owned and locally managed games. A compatibility recipe alone no longer adds an unowned game to the library after sign-in. This removes phantom entries such as an uninstalled Clair Obscur: Expedition 33 after its Game Pass access ends.

## Changes

- Filter recipe-backed library entries using active account ownership, including games in owned bundles. Expired and revoked entries no longer keep an otherwise absent game in My Library.
- Keep installed, recorded, downloading, paused and running games visible so you can still manage them when subscription access changes or a game drive is offline. Visibility does not grant a licence; launching still requires valid access.
- Move the approved screenshots and download link to the top of the README. Add quick guides for installation, updates, controllers, HDR, switching games and troubleshooting, plus a separate account guide.

## Verification and scope

The library regressions cover expired access, purchases, bundles and locally managed games. The isolated native Wayland library test reproduces the recipe-only entry in the published 0.1.20 GUI and passes with this fix, including failed and successful library refreshes and preservation of installed games. These tests use synthetic account data and do not modify the signed-in user's library or games.

Before release preparation, the workspace suite passed 591 tests with two opt-in tests ignored; hosted Rust, MSRV, source-pin and GUI checks passed. The final release build runs the workspace tests again against the tagged source.

This is an application update. The signed runtime remains **11.0.20261107.1**, including the HDR support shipped with 0.1.20. No runtime or game graphics settings are changed by this fix.
