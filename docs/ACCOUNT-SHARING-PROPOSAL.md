# Proposal: separate Store and player accounts

Status: design for [issue #9](https://github.com/icex/ferestre/issues/9).
This document adds no multi-account implementation or claim of title support.

## User-visible behavior

Accounts settings expose two roles:

- **Store account**: the account that owns the game or qualifying subscription.
  Its library, package download, content licence and Store requests are used.
- **Player account**: the person playing, with their Xbox identity, achievements,
  friends, PlayFab session and saves.

Initially both roles select the existing account. A second sign-in creates a
separate profile, then the user selects its role. Changing one role does not
sign out the other or alter a running game's identities. The launcher displays
both roles before launching when they differ. Per-role sign-out is explicit.

This follows Windows' distinction between the Microsoft Store identity and the
Xbox services identity. It is not a promise that every title permits a mismatch:
Microsoft documents that a title may require matching accounts.
See [the Windows sharing model][sharing] and [mismatched Store accounts][mismatch].

## Why a second GUI sign-in is insufficient today

The client creates one `TokenManager::with_keychain_and_memory()`. Persistent
user tokens and metadata use global keys (`user-tokens`, `user-DA`), and the
service creates the same single manager at startup:

- `third_party/xodus-cli/crates/xodus/src/tokens/manager.rs`
- `third_party/xodus-cli/crates/xodus/src/tokens/backend/keychain.rs`
- `third_party/xodus-cli/crates/xodus-service/src/main.rs`
- `third_party/xodus-cli/crates/xodus-service/src/simple_context.rs`

`connection/license.rs::fetch` authorizes Store licensing through the generic
`connection/xml.rs::xsts_token` path. Player authentication uses that path too.
A second login would replace the first user's credentials instead of preserving
a Store owner and a player. Separately, Wine's
`xuser.c::x_user_XUserIsStoreUser` currently treats the local user as the Store
user; that assumption would become false with two profiles.

These are source observations, not results from a two-account service test.

## Proposed implementation

### Profile-aware credentials

Introduce an opaque local profile ID and a keychain namespace for each signed-in
account. Keep user STS credentials, user metadata and refreshed user tokens in
that namespace. Keep short-lived XSTS caches separate by authenticated identity,
title and relying party; request signatures still cover the actual request.
Do not copy user tokens between profiles or store them in launcher JSON.

Keep host device credentials under a separate device context, with explicit
account bindings where Microsoft requires them. Do not duplicate or replace the
host's device registration as a side effect of adding a player profile.

Add profile-aware login/account/logout commands to the client. Ferestre remains
a subprocess caller and never imports or reads the client's token store.
Migrate the legacy credentials into one default profile without deleting the
old credentials until migration completes. Interrupted migration is retryable.

### Pin a role pair for each launch

At launch, resolve and pin `(store_profile, player_profile, generation)`.
Start or select a service instance bound to that immutable pair. Extend the
existing service socket namespace with a digest of the profile pair/generation,
keeping opaque profile IDs out of publicly pasted socket paths. Reuse the
existing service-build fingerprint as well.

Changing selections affects subsequent launches; it cannot replace credentials
under a running title. The game cannot choose an arbitrary keychain namespace
through IPC. Launcher-selected contexts determine routing:

| Operation | Context |
|---|---|
| Owned library, subscriptions, download, package decryption/content licensing | Store |
| Production PC XStore entitlement queries and license-token requests | Store |
| XStore User Collections/Purchase IDs, where supported | Store |
| XUser identity, Xbox services/MSA title tokens, PlayFab authentication | Player |
| Achievements, social state and cloud/local game saves | Player |

In particular, split the authorization callback inside `license::fetch` from
ordinary player `XstsTokenRequest` handling. Selecting a different relying party
alone must not accidentally select the other user's credentials.

The Store/player split for production PC XStore is documented in [Microsoft's
mismatch guidance][mismatch]; [license tokens][license-tokens] identify the
Store account. Microsoft also distinguishes Store identity from player X-tokens
for [service authentication][store-auth]. This table is the proposed routing in
Ferestre, not a report that those code paths already support two identities.

### Runtime identity and compatibility

Make `XUserIsStoreUser` compare verified MSA identity bindings for the two roles,
rather than equating "local user" with "owner". No Store sign-in means false;
a shared identity means true. [The API contract][is-store-user] requires that
comparison on PC.

Report a new runtime/service capability for separate identities. Refuse a
split-role launch with an older runtime or service, with an actionable upgrade
message; do not silently launch as the owner or treat the player as licensed.
Ensure identity queries and licence-change notifications agree after sign-out
or revocation. Existing synthetic "running title is owned" fallbacks must not
be used as evidence of entitlement for the player.

Keep saved data bound to the player profile/XUID and title/SCID. Do not relabel
an owner's existing saves as the player's, or change the existing optional
Windows-save identity override automatically.

### Cache and failure behavior

Public catalog/tier data may be shared. Owned-library, subscription and private
account caches are scoped to the Store profile; player metadata and saves are
scoped to the Player profile. Invalidate only affected private caches when a
role changes, and ignore in-flight responses for an older identity generation.

If Store authentication or entitlement fails, report that failure as a Store
account problem. If player authentication fails, identify the Player role.
Preserve Microsoft/publisher refusals, expired subscriptions and revoked
licences; a second account is not a fallback source of fabricated entitlement.
Keep the owner and player visible anywhere a future purchase flow needs the
user to identify which account would pay.

## Implementation and qualification sequence

1. Add profile-aware token storage and migration with offline tests, preserving
   single-account CLI/service behavior.
2. Add explicit service role contexts and routing tests using different dummy
   identities. Exercise the same relying party under both identities, account
   expiry, role sign-out, cancelled login and concurrent launches of two pairs.
3. Add runtime identity comparison, capability negotiation and launcher role
   selectors/cache isolation. Verify old-runtime refusal and same-account
   compatibility before exposing the selectors normally.
4. Run a legitimate two-account RETAIL test for Minecraft Dungeons II: owner A
   supplies the licence while non-owner B supplies the player identity. Verify
   the visible player, B's save creation and reload, and that A's saves remain
   unchanged. Check restart, owner sign-out/revocation and subscription expiry.
   Record the game's actual response; a valid owner token alone does not prove
   the publisher accepts a different player.

No second-account credentials were requested or used for this proposal. Whether
Dungeons II's service accepts that pair remains untested. Microsoft notes that
mismatched-account behavior must be checked in RETAIL, not inferred from a
same-account developer-sandbox test.

[sharing]: https://learn.microsoft.com/en-us/gaming/gdk/docs/store/commerce/fundamentals/xstore-product-sharing-model-for-games
[mismatch]: https://learn.microsoft.com/en-us/gaming/gdk/docs/store/commerce/pc-specific-considerations/xstore-handling-mismatched-store-accounts
[license-tokens]: https://learn.microsoft.com/en-us/gaming/gdk/docs/store/commerce/pc-specific-considerations/xstore-license-tokens
[store-auth]: https://learn.microsoft.com/en-us/gaming/gdk/docs/store/commerce/service-to-service/xstore-authenticating-your-service
[is-store-user]: https://learn.microsoft.com/en-us/gaming/gdk/docs/reference/system/xuser/functions/xuserisstoreuser
