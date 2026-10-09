# Store licence tokens through the service

`XStoreQueryLicenseTokenAsync` now asks `xodus-service` for a real licence token.
The DLL runs IPC on the runtime's async provider machinery. It copies the input
strings before returning, retains successful results until retrieval, and cleans
up failed providers after their worker and callback finish. A failed request
never reports success with an empty token.

The service owns JSON serialization, signed authorization, HTTPS and response
validation. The request preserves the signed-XSTS approach proposed in PR #1:
`POST https://licensing.mp.microsoft.com/v8.0/licenseToken`, with the running
package's StoreId, `enforceSellableBy: true`, `relatedProductIds` as a **string
containing a JSON array**, and the caller's `customDeveloperString`. It serializes
once, asks the existing title/account-aware XSTS path to sign those exact bytes,
and sends the same buffer. The existing MSA-based licensing helper has a different
request shape and is not substituted for this flow.

The HTTP client does not follow redirects. Its total request/body timeout is
20 seconds, within a 45-second deadline for authentication plus the request. It
limits the response to 64 KiB, requires a nonempty top-level string token, and
never logs response contents or signed credentials. The production endpoint is
fixed; there is no environment override. The game's service remains responsible
for checking Microsoft's JWT signature and the account's entitlements.

## IPC and compatibility

This requires the matching **client patch 0017 and runtime patches 0018–0019**.
Rebuild/restart the service when deploying the runtime. The installed DLL and
Unix library must also come from the same build.

The existing `XSDX` framing uses request/response types 7/8. Requests contain an
XML `LicenseTokenRequest` with `ParentProductId`, a `ProductIds` container of
`ProductId` elements, `CustomBase64`, `TitleId`, and `TitleClientId`. Base64 keeps
XML from normalizing or rejecting control characters in the developer string;
the service decodes UTF-8 and JSON-escapes it. Replies carry the raw validated
ASCII token. An empty reply denotes failure. Unknown message types, wrong
response types, truncated frames and oversized replies all fail closed.

The DLL bounds developer strings to 16 KiB, product lists to 256 IDs (64 bytes
each), and tokens to 60,000 bytes so requests/results fit the 16-bit IPC frame.
Its entire socket exchange has a 50-second deadline, including reads. An older
service's unknown-message reply is rejected. There is no fallback to the old
successful-empty-token stub.

## Verification

```sh
# Pure service tests; requires the client patch series to be applied.
cd /path/to/xodus
cargo test --locked -p xodus-service

# Built DLL -> Unix IPC -> test-only Rust service -> local HTTP -> DLL result.
cd /path/to/ferestre
CLIENT_REPO=/path/to/xodus tests/run-tests.sh
# Also exercise certificate verification and redirects over local HTTPS:
LICENSE_FIXTURE_TLS=1 CLIENT_REPO=/path/to/xodus tests/run-tests.sh
```

The TLS fixture requires Python/OpenSSL, rejects its self-signed certificate
before trusting it only in the test client, and leaves host trust stores unchanged.

The Rust service tests check JSON escaping, malformed/duplicate/nested fields,
exact transmitted bytes, redirects, HTTP failures, response limits and timeouts.
The Wine suite uses the actual DLL and Unix library with synthetic credentials.
It verifies input lifetime, nonblocking starts, result sizes, short buffers,
refused/malformed/oversized responses, redirects, old-service replies, partial
IPC and the real stalled-service deadline. Provider tests cover cleanup after a
callback frees its async block, including failures that never call GetResult.

The fixture's synthetic signature is the base64 encoding of the signed body,
which the local HTTP endpoint verifies against the actual bytes received.
Existing service signer tests separately verify method/path/body/authorization
binding and account/title/issuer isolation. The test-only fixture cannot mint
real credentials; the shipped service cannot enable its fixture mode.

These tests qualify the implementation and failure behavior. They do not prove
that Microsoft's live endpoint accepts this authentication flow or that Minecraft
Dungeons II is playable. Real-game testing is unavailable on the maintainer's
current entitlement. No new compatibility/playability claim is added.
