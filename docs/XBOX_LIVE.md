# Xbox Live authentication across titles

Client patch `0014` implements title authentication and service discovery from
package metadata. It contains no Forza product ID, title ID, service hostname,
audience, save-folder name or account identifier. Runtime patch `0017` corrects
the diagnostic for configurations without an `MSAAppId`; the authentication
behavior lives in `xodus-service`. Update the client and restart the service to
pick it up.

## Identity comes from the package

`MicrosoftGame.config` provides a hexadecimal, nonzero 32-bit `TitleId` and an
optional `MSAAppId`. The CLI accepts the filename case-insensitively, as Windows
does. The runtime sends the title ID in decimal over the existing IPC protocol.

With an MSA client ID, the service uses that title's SISU exchange. Without one,
it uses Windows title authentication: the numeric title ID, authenticated device
token and the device's proof key. User, device and title tokens belong to the
same key. This supports older Windows configurations without inventing another
application's identity. Requests with no title metadata use a user/device chain
with no substitute title claim.

Proof-bound user authentication, XSTS authorization and signed service requests
use the same P-256 key. Each request signs the exact transmitted method, path,
query, authorization and policy-limited body. Unsigned user-only callers remain
supported. XSTS refusal diagnostics retain HTTP status and numeric `XErr`,
without logging token contents.

## Service rules come from Microsoft

The default endpoint list is fetched from
`/titles/default/endpoints?type=1`. A claimed token authorizes the signed request
to `/titles/current/endpoints`, which supplies the running title's own rules.
These Network Service Access Lists (NSALs) determine the relying party (token
audience) and signature policy. A service hostname is not necessarily its token
audience.

Matching considers protocol, host and path. More specific rules win; equal
rules prefer the title's list. Each rule keeps its originating policy table,
because a signature-policy index is local to that list. Title responses may omit
the policy table entirely. Bare service-domain fallback respects dot boundaries
and never overrides an explicitly matched audience. WebSockets use the upgrade
scheme for lookup: `wss` maps to `https`, and `ws` to `http`.

A refused audience is not replaced with the generic Xbox audience. Where XSTS
rejects a title claim with HTTP 400/403, a retry may use the same identity's
user/device chain. That result is remembered only after the retry succeeds and
only for that account, title, audience and issuer key.

## Caches retain the identity and signing key

Tokens are scoped by signed-in account, title, MSA client, audience and issuer
key. A cache hit signs the new request with the cached token's own key. Changing
HTTP method, path, query or body always produces a new request signature.

Identity expiry is the earliest expiry of its user/device/title tokens, with
60 seconds of refresh headroom. Reading the persisted account and credentials
before a cache lookup prevents a different account or logged-out client from
using the old identity. Ordinary STS credential refresh does not change the
account cache key.

Discovery and identity work is bounded and coordinated per cache key. One
title's slow discovery cannot hold the cache lock for every other title.
Failed discovery remains retryable; an initial network error is not cached as
an empty configuration for the lifetime of the service.

## Checking another title

After sign-in, use the same package directory that the launcher runs:

```sh
xodus-cli xbl-check /path/to/game
python3 tools/xsts_probe.py --game-dir /path/to/game --check-endpoints
```

The CLI separates account-level probes from title authentication and scoped
service probes. The second command exercises the running service, including
metadata parsing, identity selection, discovery and request signing. Repeat it
for two titles and then the first again to exercise reuse across title switches.
It prints no account identity or authorization material.

Successful authentication does not establish complete multiplayer support.
Check the title's own service response and in-game behavior as well. An HTTP 401
alone does not establish a missing title claim, and an HTTP 403 alone does not
establish a server outage. Preserve the actual status and `XErr` before changing
authentication or blaming title protection.

## Evidence and limits

Forza Horizon 5's title-service calls changed from HTTP 403 to HTTP 200; a run
recorded 30 successful calls and loaded the live Festival Playlist. The player
confirmed the online fix. Gameplay uses the restored Windows career, and a
subsequent launch retained the selected vehicle. Multiplayer race completion
has not been automated.

Offline regression fixtures use invented title IDs and `.example.test` hosts.
They cover Windows/SISU identity selection, unsigned and proof-bound XSTS,
cryptographic verification of the transmitted request body, endpoint precedence,
policy-table ownership, WebSocket mapping and cache boundaries. Live discovery
is also checked across Windows and SISU titles; its success is an authentication
check, not a claim that every tested title's online modes work.

Protocol references:

- [Microsoft: title service authentication](https://learn.microsoft.com/en-us/gaming/gdk/docs/services/fundamentals/s2s-auth-calls/service-authentication/live-title-service-authentication)
- [Microsoft: Xbox Live token claims](https://learn.microsoft.com/en-us/gaming/gdk/docs/services/fundamentals/s2s-auth-calls/service-authentication/security-tokens/live-token-claims)
- Pinned XAL source, revision `6aa2420`: `authenticator.rs` implements Windows
  title authentication and signs user/XSTS exchanges; `request_signer.rs` maps
  WebSocket schemes before audience and policy lookup.
- [RFC 6455: WebSocket opening handshake](https://www.rfc-editor.org/rfc/rfc6455.html#section-4.1)
