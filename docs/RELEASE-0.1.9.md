# Release 0.1.9 qualification

Launcher **0.1.9** pairs with runtime **11.0.20261013.3**.

## Included changes

- Game Pass entitlements refresh independently of catalog data at startup,
  reload and periodically, so tier upgrades update the library. PC subscription
  metadata fills gaps in the catalog feeds. Search and paging use the complete
  catalog, with stable matching and playable status in results.
- Once downloading or installing starts, its button is disabled and reads
  `Installing..`. Other install and update actions wait until that operation
  finishes; existing Play actions remain available.
- Forza Horizon 6 is **Playable** from player-confirmed gameplay and Continue
  after a normal exit. The runtime implements the concurrent queue iterators
  used after shader compilation and bridges the title's native PlayFab file
  save SDK. Normal shutdown completes an upload before closing the save config.
  A launcher recovery path preserves wholly unsynced legacy save caches before
  allowing the SDK to rescan the actual game files.
- Dune: Awakening is **Playable** from player confirmation.
- Large signed Xbox requests and independently versioned service sockets keep
  newer service sessions from interrupting older games or installations.
- An opt-in, read-only XStore catalog bridge returns real Microsoft product,
  SKU, price, offer-token and image data, with paging and retained query handles.
  Forza's marketplace remains a known issue pending an in-game confirmation.

## Live evidence and limits

| Path | Observed result | Remaining scope |
|---|---|---|
| Forza Horizon 6, package 3.461.691.0 | Player reached gameplay beyond the intro; Continue loaded saved progress after normal exit | Full online-mode coverage and long-session stability |
| Forza file saves | Actual title SDK authenticated, uploaded and reopened files; 12 game files stayed byte-identical across an SDK reopen. Later gameplay exits completed real cloud manifest finalization | Crash-time background cloud upload is unavailable |
| Microsoft catalog | Real Windows API probe returned 28 add-ons over independent two-item page handles; matching runtime and service also returned them through Proton in the actual title prefix | In-game marketplace recovery remains unconfirmed; purchases are unsupported |
| Marketplace mismatch | The last failing player run sent the new protocol to an older service, which logged `Unknown magic`. The matching pair passed the same catalog requests | Relaunch Ferestre after upgrading so it uses the bundled matching service |
| Dune: Awakening | Player-confirmed playable | No additional automated gameplay qualification for this metadata update |

Catalog listings never grant DLC ownership. The bridge is enabled per title;
older recipes retain their existing Store behavior. Legacy save APIs and the
native file-save bridge are qualified separately.

## Verification

- 292 Rust workspace tests and workspace Clippy with warnings denied.
- Runtime suite: 266 general checks, 51 manual-queue checks, 59 native SDK
  fixture checks, concurrent queue iterator tests and 59 catalog fixture checks.
- Live catalog probe: 585 checks, zero failures, 28 products.
- 19 service unit tests passed; the credential-free licensing server fixture
  is exercised separately by the runtime suite.
- Every published patch series applies to its pinned source baseline; runtime
  capabilities are measured from the packaged files.
- The complete GUI smoke test is a separate hosted release check. Targeted
  install/progress checks passed locally; source tests do not qualify gameplay.

The launcher and runtime are separate release assets. Upgrade both. The release
keeps Forza marked Playable while documenting the marketplace limitation.
