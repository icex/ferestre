# Separate Store and Player accounts

Status: implemented in draft [PR #11](https://github.com/icex/ferestre/pull/11)
for [issue #9](https://github.com/icex/ferestre/issues/9). Local fixture checks
pass. Real two-account RETAIL qualification is pending; this is not part of the
0.1.12 production release.

## Using the local test build

Open **Accounts** from the account menu. The **Store account** supplies purchases,
subscriptions, downloads and content licences. The **Player account** supplies
Xbox identity, achievements, social services, PlayFab and saves. Add another
account through Microsoft's sign-in window, choose the two roles and apply.
Both identities appear in the sidebar before launch. A cancelled sign-in keeps
the current selection.

Changes apply to future launches. Existing games retain their captured pair,
service socket and save prefix, including when Stop is pressed. Explicit sign-out
removes only the selected profile's user credentials; if both roles use that
profile, both are signed out. Device credentials remain separate.

A split pair requires the matching profile-aware client/service and a runtime
with `xgameruntime.store-player-accounts`. Choose the local accounts runtime using
the game's runtime button. Older runtimes are refused before launch. Microsoft
and publishers still decide whether the Store owner may share a particular title
with the selected player; Ferestre does not create an entitlement for the player.

## Implementation

- `patches/xodus-cli/0020-separate-store-and-player-accounts.patch` adds opaque
  profile metadata, per-profile keychain keys and isolated ephemeral token caches.
  Legacy migration copies and verifies both user entries before publishing the
  default profile; interruption is retryable and the original entries remain.
  Login uses a fresh temporary browser context, shared across that login's MFA
  pages, without carrying cookies into another account's login.
- Each launch captures Store/Player IDs and a selection generation. The service
  pins verified MSA bindings and cannot follow replacement credentials under a
  running title. Socket names hash the pair, generation, service binary and bound
  title/content. IPC does not accept a title-selected profile or credentials.
- Store library, subscription, download, content licence and XStore licence-token
  calls use Store credentials. XUser/MSA/Xbox/PlayFab calls use Player credentials.
  Public catalog/tier data stays shared; private library, account and update
  caches are scoped by Store profile. Replies from older selections are ignored.
- `patches/xgameruntime/0030-separate-store-and-player-accounts.patch` makes
  `XUserIsStoreUser` compare the verified MSA identities. Current-game entitlement,
  game licences and package licences come from an authenticated, title-bound
  Store content-licence check. Installed files do not imply ownership. Unrelated
  package licensing and unsupported purchase APIs stay unsupported.
- Store checks run on the async work queue and cache successful checks for up to
  60 seconds (denials for 10 seconds); local sign-out is checked before cached
  grants. Player and licence state is polled on the Work port once per second,
  then change/lost callbacks are delivered on the caller's Completion port.
  Unregister suppresses queued callbacks and can wait for active delivery.
- Local saves use a stable hash of the verified Player MSA for an independent
  Wine prefix. The imported account keeps its existing prefix. Reauthenticating
  the same identity reuses its profile and save namespace. The optional Windows
  save-identity override is not changed.

Microsoft documents the [Windows sharing model][sharing], the distinction
between [Store and Player identities][mismatch], [Store licence tokens][license]
and [the XUserIsStoreUser contract][is-store-user]. Those documents describe the
platform contract, not proof that Dungeons II accepts a particular account pair.

## Observed local validation

- Launcher workspace: 310 Rust checks, Clippy and formatting.
- Credential tests: isolated owner/player keys, unchanged device context, retry
  after interrupted migration and refusal to follow replacement credentials.
- Service: 22 checks passed, one existing manual fixture ignored. Includes actual
  Unix-stream routing, distinct identity comparison, sign-out, protocol-version
  refusal, unbound title refusal and cached-grant invalidation after sign-out.
- GTK through AT-SPI: cancelled login, adding/selecting Player B independently
  from Store A, visible roles, real launcher plan and service binding, separate
  prefix, changing selection while a game runs, Stop targeting the original
  prefix, Player-only sign-out retaining the Store library, old-runtime refusal
  and refusing a signed-out Player without falling back to the Store identity.
- Both 64-bit and 32-bit native probes: 47 checks each, including delayed async
  licensing, no fabricated grants, Store sign-out notifications, Player state
  changes and no callbacks after unregister. A previous production runtime fails
  the account-role probe. Modified native units also pass Wine's `-Werror` flags.
- Existing native regression and Store catalog/licence fixtures pass in legacy
  mode. All four patch series apply to their pinned upstream bases.

All account identities and entitlements in these automated checks are synthetic.
No second-account credentials were supplied by the tests. The local candidate
and production AppImage have separate filenames and desktop entries.

## Two-account RETAIL check still required

1. Sign in owner A and non-owner B, then select Store A / Player B.
2. Launch Minecraft Dungeons II with the accounts runtime and record its actual
   response. Check that the visible player and online identity are B.
3. Create progress as B, exit normally, restart and verify B's save reloads.
   Verify that A's existing save remains unchanged.
4. Restart the launcher and confirm the roles persist. Check owner sign-out and
   a genuine entitlement refusal/revocation without a fallback grant.
5. Repeat with a qualifying Game Pass subscription where applicable. Record any
   publisher or Microsoft refusal; successful token acquisition alone is not a
   successful shared-account game test.

Microsoft requires mismatched-account behavior to be checked in RETAIL. Keep
this PR draft and issue #9 open until that check has supplied real evidence.

[sharing]: https://learn.microsoft.com/en-us/gaming/gdk/docs/store/commerce/fundamentals/xstore-product-sharing-model-for-games
[mismatch]: https://learn.microsoft.com/en-us/gaming/gdk/docs/store/commerce/pc-specific-considerations/xstore-handling-mismatched-store-accounts
[license]: https://learn.microsoft.com/en-us/gaming/gdk/docs/store/commerce/pc-specific-considerations/xstore-license-tokens
[is-store-user]: https://learn.microsoft.com/en-us/gaming/gdk/docs/reference/system/xuser/functions/xuserisstoreuser
