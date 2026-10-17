# Ferestre 0.1.12

This launcher update uses the same runtime **11.0.20261017.1** as 0.1.11.

- All game views show the access source in the second row: Purchased, Game Pass,
  Trial, Shared licence, Owned, or Access not checked as supported by account data.
- Installed Game Pass titles no longer show “not in this account’s library
  listing” just because Store collections omit their direct-purchase entry.
  Catalogue inclusion alone is labelled Game Pass catalogue, separately from
  access through an active matching subscription. Bundle grants include the
  bundled game. Actual licensing failures still come from Microsoft.
- The release workflow publishes the complete notes matching the release version.

Includes all [0.1.11 fixes](https://github.com/icex/ferestre/releases/tag/v0.1.11):
two-line rows, persistent disk usage, daily library/catalogue/update caches with
forced Refresh, close during read-only refresh, Steam shortcut repair and
artwork, numeric runtime ordering, video dependency diagnostics, connected
GameInput v2 support in the matching runtime, and Appx/MSIX inspection.
The qualification limits documented for that release still apply.

Separate Store/player accounts remain under development in PR #11 and are not
included in this release.
