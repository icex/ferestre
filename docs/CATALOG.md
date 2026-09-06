# Catalog: listing what an account owns

Design note for the one feature a launcher is judged on and this project does
not have. Today every command takes a product id typed by hand — `get-game.sh`
even carries `bedrock` as a hardcoded alias for `9NBLGGH2JHXJ` because there is
nothing to look it up in. This is the design for `ferestre library`.

No launcher code has been written from this yet. It is written against the code
in the Xodus checkout (`crates/xodus`, `crates/xodus-cli`, `crates/xodus-service`) and
against the captured service responses that checkout carries in its own tests
and docs. Every claim is marked **verified** (read out of that code or a
captured response committed to it) or **inference** (consistent with it, not
yet observed), with a third mark for the one fact a live probe has settled.
Section 9 is the full list; if you implement from this document, read it first.

---

## 1. What exists, and what the gap actually is

The client can already answer *"is this specific product downloadable by me?"*
three different ways, and cannot answer *"what have I got?"* at all.

| question | answered by | where |
|---|---|---|
| does product X have a PC package | `get_content_id` | `crates/xodus-cli/src/package.rs:8` |
| am I entitled to content id C | `get_license_content` → `satisfactionFailure` | `crates/xodus/src/licensing/content.rs:27` |
| what version of C is current, and how big | `GetBasePackage` | `crates/xodus-cli/src/package.rs:77` |
| **what do I own** | — | — |

So the gap is one call: something that returns a *set* of product ids. Everything
downstream of that set — names, artwork, PC-package filtering, version, size —
already exists in some form and only needs widening.

---

## 2. Routing: host → relying party → token

This is the part that is not guesswork, and it is worth being precise about
because getting it wrong costs a day.

The client fetches Microsoft's own endpoint routing table:

```
GET https://title.mgt.xboxlive.com/titles/default/endpoints?type=1     (no auth)
```

`get_title_management()` (`crates/xodus/src/api/xbox/title.rs:3`) does this, and
`get_endpoint(url, &response)` (`:11`) resolves a URL against it: filter to
entries whose `Protocol` matches the scheme, glob-match `Host`, then pick the
match with the **most non-`*` characters** in its host — longest literal wins.

A real captured response is committed as the fixture for that function's test
(`crates/xodus/src/api/xbox/title.rs:38`). Resolving the hosts this feature
needs against it gives:

| host | matched entry | relying party | token | signature policy |
|---|---|---|---|---|
| `collections.mp.microsoft.com` | fqdn | `http://licensing.xboxlive.com` | JWT | **0** |
| `licensing.mp.microsoft.com` | fqdn | `http://licensing.xboxlive.com` | JWT | 0 |
| `displaycatalog.mp.microsoft.com` | `*.mp.microsoft.com` | `http://mp.microsoft.com/` | JWT | none |
| `beige.xboxservices.com` | `*.xboxservices.com` | `http://mp.microsoft.com/` | JWT | none |
| `assets.xboxservices.com` | fqdn | *(none)* | *(none)* | none |
| `catalog.gamepass.com` | `*.gamepass.com` | `http://xboxlive.com` | JWT | 0 |
| `packagespc.xboxlive.com` | fqdn | `http://update.xboxlive.com` | JWT | 0 |

Signature policy 0 in that response is
`{Version: 1, SupportedAlgorithms: ["ES256"], MaxBodyBytes: 8192, SupportedSignatureTypes: ["XBL"]}`.

Header form for a JWT relying party is `XBL3.0 x=<uhs>;<token>`, built by
`get_xsts_auth_header` (`crates/xodus/src/api/xbox/auth.rs:155`).

### 2.1 Three traps in that table

**Collections is not an `mp.microsoft.com` relying party.** The obvious guess —
"it is a `*.mp.microsoft.com` host, so it is `http://mp.microsoft.com/`" — is
wrong, because the table carries a more specific fqdn entry pointing at
`http://licensing.xboxlive.com`. `get_endpoint`'s longest-literal rule picks it.
`http://mp.microsoft.com/` is right for DisplayCatalog and for the Xbox app's
own hosts, and wrong for collections and licensing.

**Relying-party strings are compared verbatim, trailing slash included.** It is
`http://licensing.xboxlive.com` with no slash and `http://mp.microsoft.com/`
with one. `xsts/authorize` gets the literal string
(`crates/xodus/src/api/xbox/auth.rs:123`); do not normalise it.

**A listed signature policy is not necessarily enforced.**
`packagespc.xboxlive.com` also carries `SignaturePolicyIndex: 0`, yet
`get_packages` (`crates/xodus-cli/src/package.rs:77`) authenticates it with
`xodus::api::xbox::run(...)`, which passes `signer: None`, so the token is not
bound to a proof key and no `Signature` header is possible — and downloads work.
So: **try collections unsigned first.** It works — 3.2 records the probe that
settled it. Had it answered 401, that would have been the signal to switch to
`try_run_with_signer(..., Some(&signer))` and produce a `Signature`, not a
reason to doubt the relying party.

Producing that signature is not new work either: `sign_for_with`
(`crates/xodus-service/src/connection/xml.rs:121`) already looks up a URL's
signature policy and calls
`RequestSigner::sign(version, method, path_and_query, authorization, body, max_body)`.
Its comment records the constraint that costs the most time to rediscover: *the
signature must be made with the key the token was minted under*. A signer and a
token from different chains will be rejected.

---

## 3. Which endpoint lists entitlements

Four candidates, doing genuinely different jobs. Use the first; the rest are
corroboration and enrichment.

### 3.1 Primary — `collections.mp.microsoft.com`

The Store's entitlement service, and the only candidate that returns product ids
that feed straight into the existing `get_content_id` → `GetBasePackage` →
`streaming` path.

- **Relying party:** `http://licensing.xboxlive.com` (verified from the table).
- **Token:** XSTS JWT, presented as `XBL3.0 x=<uhs>;<token>`, **unsigned** — see
  the probe result in 3.2.
- **Path:** `POST https://collections.mp.microsoft.com/v6.0/collections/query`
  is what the probe in the fork now targets. Reported to reach the service;
  whether v6.0 is the right version for a PC entitlement listing is not settled.
- **Shape:** a JSON body with a `beneficiaries` array and a continuation token
  for paging (**inference** — see 3.2 and §10).
- **Header hygiene:** send `MS-CV`. `get_license_content` sets one from
  `xal::cvlib::CorrelationVector::new()` for its sibling host, and it is what
  you quote if you ever have to ask Microsoft why a call failed.

The body schema is the one thing this document cannot give you from the code.
§10 says how to settle it with a probe rather than by guessing.

### 3.2 The auth question, and why there are two answers

The endpoint table describes how an **Xbox title** reaches these hosts. The
**Store / Gaming Services client** reaches the same hosts a different way, and
this repo contains a working example of the second:

`get_license_content` (`crates/xodus/src/licensing/content.rs:27`) posts to
`https://licensing.mp.microsoft.com/v7.0/licenses/content` — a host the table
routes to `http://licensing.xboxlive.com` — and does **not** send an XSTS token.
It sends:

```
Authorization: <MSA compact device token>          // exchange_device_token(..., "www.microsoft.com", mbi_ssl)
from: XboxLicenseManager
user-agent: XboxLm-PC/Microsoft.GamingServices_32.107.4002.0_x64__8wekyb3d8bbwe
MS-CV: <correlation vector>

{ ..., "users": { "<suid>": [ { "identityType": "Msa",
                                "identityValue": "<MSA compact user token>",
                                "localTicketReference": "<user.puid>" } ] } }
```

That is verified, working code against the same host family. The user identity
travels *in the body*, as a beneficiary; the `Authorization` header carries the
**device**. Collections' query API takes a `beneficiaries` array of the same
shape, which is why this precedent matters more than the endpoint table does.

It also explains the note in the Xodus docs for
`beige.xboxservices.com` (`docs/xbox/xboxservices.md`): *"the Authorization
header is a `t=` token; `x-ms-authorization-social` contains Xbox style token —
`XBL3.0 x={hash};{token}`"*. An MSA compact ticket is `t=`-prefixed, so that
note is describing exactly the pattern above with the Xbox token demoted to a
secondary header (**inference**: the `t=` reading; the quoted note is verified
repo documentation).

**Order to try, and how to tell which failed:**

1. XSTS for `http://licensing.xboxlive.com`, unsigned, `Authorization: XBL3.0 …`.
2. Same, signed with a `RequestSigner`, if (1) gives 401 with a signature complaint.
3. MSA device token in `Authorization` + user token as a beneficiary, exactly as
   `get_license_content` does, if (1) and (2) give 401.

A 400 means the *body* is wrong and the auth was accepted — stop changing tokens
and change the body. A 401 means the reverse. Log status and `MS-CV` for both.

**Result so far:** the `xodus-cli collections` probe in the fork records that (1)
is enough — a malformed body comes back as **400 with a schema complaint, not
401**, so the unsigned XSTS token for `http://licensing.xboxlive.com` is
accepted. That settles the routing, the relying party and the signature question
in one shot, and confirms the packagespc analogy in 2.1. Keep (2) and (3)
documented anyway: they are what to reach for if that ever regresses, and (3)
remains the shape to copy for the body's beneficiary bag.

**Still open: which identifier names the beneficiary.** A token minted for
`http://licensing.xboxlive.com` carries a user hash but no `xid` claim —
`XstsResponse::xuid()` returns `Option` for exactly this reason
(`crates/xodus/src/models/xbox.rs`) — so the XUID is not available from the token
that authenticates the call. The candidates are `user.puid` from the keychain
(which is what `get_license_content` uses as `localTicketReference`, verified) or
an XUID fetched separately. Prefer the PUID: it is the identifier the Store side
of these services keys on, and it is already stored.

### 3.3 Corroboration — `beige.xboxservices.com/pcgafd/mygames`

```
GET https://beige.xboxservices.com/pcgafd/mygames?market={MARKET}&language={LANGUAGE}&appVersion=2605.1001.14.0
```

Documented in the Xodus checkout (`docs/xbox/xboxservices.md`) as returning "a
list of IDs and summaries for games" — this is the Xbox PC app's own *My games*
view, already joined against the catalog. Relying party `http://mp.microsoft.com/`,
no signature policy (verified from the table, via `*.xboxservices.com`).

Do **not** make this the primary. It is a UI aggregation with an `appVersion` in
its query string, so it will drift with the Xbox app and it returns what that app
chooses to show, not what the account holds. Use it to check that the collections
result is complete: if `mygames` lists a title collections did not, the
collections query is filtering too hard.

### 3.4 Subscriptions — `catalog.gamepass.com`

An entitlement to a Game Pass subscription is not an entitlement to the titles in
it. Expanding one into the other needs the catalog side:

```
GET https://catalog.gamepass.com/pcsubscriptions?market={MARKET}&language={LANGUAGE}
GET https://catalog.gamepass.com/misc/pc-pfns-list          # supports If-None-Match / ETag
```

Both documented in the Xodus checkout (`docs/xbox/gamepass.md`); the second is
a flat list of PC package family names, which joins to the catalog record's
`PackageFamilyName` (§5) and to the package identity the launcher already derives
from `AppxManifest.xml` at install time.

The subscription product ids you will see in an entitlement list are already in
the code, unused: `GAMEPASS_SUBS` at
`crates/xodus/src/models/xbox/subscriptions.rs:6` — `CFQ7TTC0K5DJ` Essential,
`CFQ7TTC0P85B` Premium, `CFQ7TTC0KHS0` Ultimate, `CFQ7TTC0KGQ8` PC Game Pass,
`CFQ7TTC0K6L8` for Console. Treat a match against that table as "this row is a
subscription, not a title" rather than as a title with no package.

Relying party for `catalog.gamepass.com` is `http://xboxlive.com` — a *third*
relying party, so a library refresh that touches all three mints three tokens.
That is the main argument for §8.4.

### 3.5 The oracle that already works

Ownership of a *known* product needs no new endpoint at all.
`get_license_content` returns `LicenseContentResponse::SatisfactionFailure` when
the account is not entitled (`crates/xodus/src/models/licensing.rs:73` — "not
owned, or not covered by the account's current subscription tier"), and
`LicenseContentError::NotEntitled` carries its description.

That gives a working `ferestre owns <product-id>` today, and a degenerate library:
probe a fixed list of ids. It is the right thing to ship first, because it makes
the JSON schema, the cache and the CLI real while the collections query is still
being settled, and it keeps working as a fallback afterwards.

---

## 4. Call sequence

Types are from the Xodus crate as it stands. `?` elides the real error handling;
note that `xodus::api::xbox::run` **panics** on failure and
`try_run_with_signer` (`crates/xodus/src/api/xbox/mod.rs:108`) is the variant to
use — the panicking one was already responsible for one class of bug the service
had to fix.

```
0.  xodus-cli login                        # once per machine; persists to the keychain

1.  let tokens = TokenManager::with_keychain_and_memory();
    xodus::tokens::device::ensure_device_credentials(&client, &tokens).await;

2.  let Token::Legacy(dev) = tokens.get_device_sts_token()?;   // models::secrets::LegacyToken
    let Token::Legacy(usr) = tokens.get_user_sts_token()?;
    let user: User        = tokens.get_user()?;                // { puid, username }

3a. // preferred: XSTS for the collections relying party
    let xsts: XstsResponse = xodus::api::xbox::try_run_with_signer(
        &client, dev.clone(), usr.clone(),
        "http://licensing.xboxlive.com", None).await?;
    tokens.cache_xsts("http://licensing.xboxlive.com", &xsts);      // see 9.4
    let auth = xodus::api::xbox::get_xsts_auth_header(xsts);        // "XBL3.0 x=<uhs>;<jwt>"

3b. // fallback: MSA pair, mirroring crates/xodus-cli/src/license.rs
    let dev_msa = api::live::exchange_device_token(&client, dev.clone(),
        "{d6d5a677-0872-4ab0-9442-bb792fce85c5}".into(), "www.microsoft.com".into(),
        Some(soap::PolicyReference::mbi_ssl())).await?;             // -> Token::Compact
    let usr_msa = api::live::exchange_user_token(&client, usr, user.username.clone(),
        dev, None, Some("Silent".into()),
        "{d6d5a677-0872-4ab0-9442-bb792fce85c5}".into(),
        &[("www.microsoft.com".into(), Some(soap::PolicyReference::mbi_ssl()))]).await?;
    // beneficiary: { identityType: "Msa", identityValue: usr_msa,
    //                localTicketReference: user.puid }

4.  POST the collections query, paging on the continuation token, MS-CV per request.
    -> Vec<Entitlement { product_id, sku_id, kind, acquired, expires, via }>

5.  for each product_id (and each subscription's expansion, §3.4):
      find_products_by_id(&client, product_id, market, languages).await?
      -> DisplayCatalogProductsResponse                           (anonymous GET)

6.  classify: playable | bundle | other                            (§6)
      bundle -> recurse into the `big:<productId>:` entitlement keys, depth-limited

7.  optional, only when a size or an update check is wanted:
      get_packages(&client, &tokens, content_id)                   (mints a *second*
      -> PackageDetails { version, version_id, package_files[] }    relying party)
```

Step 7 is deliberately optional and off by default: it is one authenticated
request per title against a different relying party, and a hundred-title library
does not need a hundred version checks to draw a list.

---

## 5. Joining to DisplayCatalog for names and artwork

```
GET https://displaycatalog.mp.microsoft.com/v7.0/products/{productId}?market={m}&languages={l}
```

`find_products_by_id` (`crates/xodus/src/api/displaycatalog.rs:3`) already does
this and sends **no `Authorization` header** — verified: it is a plain
`client.get(...)`. So the entire presentation half of a library listing is
anonymous, needs no token, and can be developed and tested without an account.

### 5.1 The model is narrow; the request is not

`DisplayCatalogProductsResponse` (`crates/xodus/src/models/displaycatalog.rs`)
deserialises only `Product.DisplaySkuAvailabilities`, because that is all
`get_content_id` needs. serde ignores unknown fields by default and the struct
does not set `deny_unknown_fields`, so **the names and artwork are already in the
response being thrown away**. Widening the model is not a new request.

### 5.2 Fields to add

All **inference** — the shapes below are what the v7.0 product document is
expected to contain, and one anonymous `curl` of a known product id confirms or
corrects the lot in a minute (§10). Add them as `Option<…>` / `#[serde(default)]`
so a shape change degrades to a missing name rather than a failed parse.

| field | use |
|---|---|
| `LocalizedProperties[0].ProductTitle` | display name |
| `LocalizedProperties[0].PublisherName`, `.DeveloperName` | list subtitle |
| `LocalizedProperties[0].ShortDescription` | detail view |
| `LocalizedProperties[0].Images[] { ImagePurpose, Uri, Width, Height }` | artwork |
| `Properties.PackageFamilyName` | joins to `pc-pfns-list` and to the installed `AppxManifest.xml` identity |
| `ProductType` / `ProductFamily` | drop `Durable` (DLC) and `Pass` rows from the list |
| `AlternateIds[] { IdType, Value }` | Xbox title id, for cross-referencing traces |

Two practical notes on images: catalog image `Uri`s are protocol-relative
(`//store-images…`), so prefix `https:` before use; and pick by `ImagePurpose`
with an explicit preference order plus a fallback to the largest image present,
because not every product carries every purpose.

`assets.xboxservices.com` appears in the endpoint table with **no relying party
and no token type**, i.e. artwork hosts are anonymous. Nothing in a library
refresh should ever attach a token to an image fetch.

### 5.3 Market and language

`get_content_id` calls with `market = "neutral"` and `languages = ["en", "neutral"]`.
That is the combination proven to work for finding a downloadable package, and it
should not be changed: a real market can drop availabilities or 404 a product.

But `neutral` is the wrong market for *presentation* — localized properties and
prices are selected by it. So:

- presentation lookup: `market` from `FERESTRE_MARKET`, else derived from the
  session locale, else `US`; `languages` from `LANG` plus `en` plus `neutral`.
- if that lookup fails, or yields no Windows.Desktop package, retry once with
  `market=neutral&languages=en,neutral` — today's exact behaviour — and mark the
  record as having fallen back.

Cache the two under different keys (§8). Never let a presentation failure block
a download: a title with no name is still installable.

---

## 6. Filtering to Windows.Desktop

### 6.1 What the code does now

`get_content_id` (`crates/xodus-cli/src/package.rs:8`) walks
`Product.DisplaySkuAvailabilities[] → Sku.Properties.Packages[]` and takes the
**first** package with a `PlatformDependencies[].PlatformName == "Windows.Desktop"`,
then requires its `ContentId`. If none is found it falls back to bundle
expansion: it collects
`Availabilities[].LicensingData.SatisfyingEntitlementKeys[].EntitlementKeys[]`,
splits each on `:`, and for keys of the form `big:<productId>:<…>` offers those
child products in an **interactive picker**, then recurses.

That bundle path is the important half. An account is frequently entitled to a
bundle or edition product whose own SKU carries no PC package; the playable
child is only reachable through those `big:` keys.

### 6.2 What changes for enumeration

- **Never prompt.** `docs/RECIPES.md` already lists "`Selection failed` from
  `xodus-cli download`" as a known failure for exactly this reason. A library
  refresh runs unattended. Expand every `big:` child, do not pick one.
- **Classify instead of erroring.** Enumeration wants three outcomes, not
  success-or-failure:
  - `playable` — a SKU package with a `Windows.Desktop` dependency **and** a `ContentId`
  - `bundle` — no such package, but `big:` entitlement keys; list the resolved children
  - `other` — everything else
- **Bound the recursion.** `get_content_id` recurses through `Box::pin` with no
  depth limit and no visited set. For enumeration, cap depth (2 is enough for
  every bundle seen so far) and keep a visited set of product ids: a cycle in the
  entitlement graph would otherwise be an infinite loop over the network.
- **Deduplicate on `ContentId`, not product id.** A bundle and its child can both
  resolve to the same content, and the library should show it once.

### 6.3 `Windows.Desktop` is necessary, not sufficient

`Windows.Desktop` is the only platform name this codebase asserts anything about,
so treat every other value as "not a PC package" rather than enumerating a list
of platform names nobody here has observed.

More importantly it does not mean *this project can run it*. An ordinary Store
desktop app also carries `Windows.Desktop`. The honest field is a three-state
`runnable`, and it is **not** derived from the catalog:

- `known-good` — a manifest exists at `titles/<product-id>.toml` (roadmap Phase 0)
- `known-broken` — a manifest exists and records the title as blocked (Forza
  Horizon 5 is the case in point: it downloads and does not run)
- `unknown` — everything else, which will be almost everything

Do not infer compatibility from catalog metadata. The project's credibility rests
on the status table being accurate; a library that claims a hundred playable
titles when three are known to work would undo that in one screenshot.

---

## 7. CLI surface

```
ferestre library [--json] [--refresh] [--offline] [--all]
             [--market <m>] [--language <l>]
ferestre show <product-id> [--json]
ferestre owns <product-id>
```

`ferestre library` prints one aligned line per playable title: product id, name,
entitlement kind, installed state. `--all` adds the rows normally filtered out
(subscriptions, DLC, console-only) with a reason column.

`--json` is the contract other things build on, so it gets rules:

- stdout is **only** the JSON document. Progress, warnings and HTTP diagnostics
  go to stderr. `ferestre library --json | jq` must work with no flags.
- never interactive. If sign-in is needed, fail with a message telling the user
  to run `xodus-cli login`, exit non-zero.
- exit `0` on a complete list, `0` with `"stale": true` on a cache-served list
  when the network failed, non-zero only when there is nothing to print.
- **no account identity in the output, ever** — no PUID, XUID, gamertag, e-mail,
  token, licence blob or content key. The document must be safe to paste into a
  bug report unedited. This is the same rule the repo's probe scripts already
  follow (`tools/xsts_probe.py` masks the XUID it prints).

### 7.1 `--json` schema

```json
{
  "schema": 1,
  "generated": "2026-09-06T09:12:04Z",
  "source": "collections",
  "stale": false,
  "market": "US",
  "languages": ["en-US", "en", "neutral"],
  "titles": [
    {
      "product_id": "9NBLGGH2JHXJ",
      "content_id": "…",
      "title": "Minecraft for Windows",
      "publisher": "Mojang Studios",
      "product_type": "Game",
      "package_family_name": "Microsoft.MinecraftUWP_8wekyb3d8bbwe",
      "platform": "Windows.Desktop",
      "entitlement": { "kind": "purchase", "acquired": "…", "expires": null, "via": null },
      "parent_product_id": null,
      "art": [ { "purpose": "BoxArt", "url": "https://…", "width": 300, "height": 300 } ],
      "installed": { "path": "…", "version": "…", "version_id": "…" },
      "available": { "version": "…", "version_id": "…", "size_bytes": 0 },
      "manifest": "titles/9NBLGGH2JHXJ.toml",
      "runnable": "known-good"
    }
  ],
  "subscriptions": [
    { "product_id": "CFQ7TTC0KGQ8", "name": "PC Game Pass", "expires": "…" }
  ],
  "skipped": [
    { "product_id": "…", "reason": "no Windows.Desktop package" }
  ]
}
```

Notes on the shape:

- `entitlement.kind` is `purchase` or `subscription`; when it is `subscription`,
  `via` names the subscription product id. That is what lets a UI say "this
  leaves Game Pass on the 15th" and lets the launcher warn before a licence stops
  renewing — the distinction is the whole reason to model entitlements rather
  than a flat id list.
- `parent_product_id` is set on a row that came out of a bundle expansion, so the
  provenance of a product id the user never bought directly is visible.
- `installed` is filled from the local install layout, `available` only when
  §4 step 7 ran; both are `null` otherwise. Absent, not zero.
- `skipped` exists so the filter is auditable. When someone reports a missing
  game, the first question is whether it was skipped and why, and the answer
  should be in the same document.
- `schema` is a plain integer. Bump it on any incompatible change; a GUI built
  against `1` should refuse `2` rather than misread it.

### 7.2 `ferestre owns`

Backed by §3.5, so it works before collections does. Exit `0` entitled, `1` not
entitled, `2` could not tell (network, no sign-in). Do not print the licence.

---

## 8. Caching

### 8.1 What, where, how long

Everything lives under `${XDG_CACHE_HOME:-$HOME/.cache}/ferestre/`, mode `0700`,
files `0600`. `FERESTRE_LIBRARY_CACHE_DIR` overrides. Per-account files are named by a
truncated hash of the PUID, not the PUID — cache paths end up in screenshots and
`ls` output.

| what | path | TTL | invalidated by |
|---|---|---|---|
| endpoint table | `endpoints.json` | 24 h | — |
| entitlement list | `library/<slot>.json` | 6 h soft, 30 d hard | `--refresh`, login, logout, install, purchase |
| catalog record | `catalog/<market>/<productId>.json` | 7 d | `--refresh`, market or language change |
| artwork | `art/<sha256(url)>` | indefinite | the URL changing |
| package version | `packages/<contentId>.json` | 1 h | `--refresh`, and always before a download |
| XSTS token | memory only | `not_after` − 60 s | process exit |

### 8.2 Why those numbers

Entitlements change only when a human buys something or a subscription lapses, so
a soft TTL in hours is generous and a refresh after any action that could change
them matters more than the number. Past the soft TTL, serve the cache and set
`"stale": true` rather than failing — being able to look at your library on a
train is most of the point of caching it at all. The hard TTL exists so a
long-abandoned cache is discarded rather than shown as current.

Catalog records are marketing metadata that changes on title updates; a week is
fine, and if DisplayCatalog sends `ETag` or `Last-Modified`, revalidate instead
of refetching. Artwork is immutable per URL, so it needs no expiry — a changed
image arrives as a changed URL in the catalog record.

Package version is the only one that must be fresh, because it gates a download
decision. One hour for display, and re-fetched unconditionally immediately before
an install or update.

### 8.3 Never cached

Licences, content keys, `PackageFile.key_blob`, CDN URLs, and tokens. Tokens
already live in the keychain via `KeychainBackend`; do not create a second
on-disk copy of credentials for the sake of a faster list. CDN URLs and key blobs
are per-request and short-lived, so a cached one is a bug that presents as a
mysterious download failure much later.

### 8.4 The token cache does not help a launcher that shells out

`TokenManager` has a per-relying-party XSTS cache — `get_cached_xsts` /
`cache_xsts` at `crates/xodus/src/tokens/manager.rs:130`, expiry taken from
`XstsResponse.not_after`. Two things are true about it and both matter:

- **Nothing calls it.** Grep the whole workspace: the only occurrences are its
  own definition. Every XSTS token minted today is minted from scratch.
- **Its backend is `MemoryBackend`** — process-local, gone on exit
  (`crates/xodus/src/tokens/backend/memory.rs`). `xodus-service` needed a token
  cache badly enough to add a *second*, separate one
  (`XSTS_CACHE`, `crates/xodus-service/src/connection/xml.rs:19`, with 60 s of
  headroom).

The launcher shells out to `xodus-cli`, so each invocation is a fresh process
with a cold cache, and each pays the full chain: MSA exchange → Xbox user auth →
device auth → title-scoped RPS → title auth → `xsts/authorize`. A naive library
refresh that runs one `xodus-cli` per title pays that per title, against up to
three relying parties.

Two ways out, in order:

1. **One process per refresh.** The catalog command does the whole enumeration in
   a single invocation and prints one JSON document. This is the reason
   `ferestre library --json` is one call rather than a loop over `ferestre show`, and it
   is the recommended shape.
2. **Ask the running service for tokens.** `xodus-service` is *already* started by
   `scripts/launch-gdk.sh` before any title runs, and its
   `XSTS_TOKEN_REQUEST` (message type 5) takes a URL and returns the ready
   `XBL3.0` header plus the correct `Signature` for that URL's signature policy,
   from a warm cache (`crates/xodus/src/models/xgameruntime/xuser.rs:26` for the
   message, `crates/xodus-service/src/connection/xml.rs:241` for the handler,
   `tools/xsts_probe.py` for a working client of it in this repo).

   Leaving `<RelyingParty>` empty makes the service resolve it from the live
   endpoint table, which means the launcher never hardcodes a relying party at
   all — and it solves the signature-policy question in the same move, because
   the service signs with the key the token was minted under.

Option 2 is strictly better once the service is running anyway, but it couples
the launcher to the IPC protocol. Do option 1 first; revisit when `ferestre install`
and `ferestre run` already depend on the service.

---

## 9. What is verified and what is not

**Verified — read out of committed code or a captured response in it:**

- The endpoint table endpoint, its lack of auth, and `get_endpoint`'s
  longest-literal-match rule (`api/xbox/title.rs:3,11`).
- Every row of the table in §2, from the fixture at `api/xbox/title.rs:38`,
  including collections → `http://licensing.xboxlive.com` with
  `SignaturePolicyIndex: 0`, and signature policy 0's parameters.
- `packagespc.xboxlive.com` carries policy 0 and is nonetheless used with an
  unsigned token in working code (`package.rs:77,92`).
- `XBL3.0 x=<uhs>;<token>` header form (`api/xbox/auth.rs:155`).
- The full MSA-token auth shape used against `licensing.mp.microsoft.com`,
  including `localTicketReference = user.puid` and the beneficiary bag
  (`licensing/content.rs:27`, `xodus-cli/src/license.rs`).
- `satisfactionFailure` as the not-entitled signal (`models/licensing.rs:73`).
- DisplayCatalog v7.0 by-product-id is anonymous (`api/displaycatalog.rs:3`).
- The `Windows.Desktop` filter and the `big:<productId>:` bundle expansion,
  including its interactive picker (`package.rs:8`).
- `GetBasePackage` with `x-xbl-contract-version: 3` under
  `http://update.xboxlive.com`, and `PackageDetails`'s fields (`package.rs:77`,
  `models/packagespc.rs`).
- `get_cached_xsts`/`cache_xsts` exist, are memory-backed, and have no callers;
  `xodus-service` keeps a separate cache.
- `GAMEPASS_SUBS`, its five ids, and that nothing uses it.
- The `XSTS_TOKEN_REQUEST` IPC message, its fields, and that the response carries
  a `Signature`.
- The `mygames`, `pcsubscriptions` and `pc-pfns-list` URLs and the `t=` /
  `x-ms-authorization-social` auth note — verified **as documentation** in the
  Xodus checkout (`docs/xbox/xboxservices.md`, `docs/xbox/gamepass.md`), not as
  running code. Nobody here has executed them.

**Confirmed by probe — observed against the live service, not read out of code:**

- An unsigned XSTS token for `http://licensing.xboxlive.com` is **accepted** by
  `collections.mp.microsoft.com`: a malformed body returns 400 with a schema
  complaint rather than 401. Recorded in the `xodus-cli collections` probe in the
  fork. This promotes the §2.1 packagespc analogy from reasoning to fact.
- `POST .../v6.0/collections/query` reaches the service. That the v6.0 query is
  the *right* API for a PC entitlement listing is not established by a 400.

**Inference — plausible, consistent with the above, unconfirmed:**

- The collections request body: the `beneficiaries` array, its `identitytype`
  value, the paging/continuation field, and which product types to ask for.
- Which identifier the beneficiary takes (PUID vs XUID) — see the end of §3.2.
- Every DisplayCatalog field in §5.2 beyond what the current model parses.
- That a batch `?bigIds=` form of the DisplayCatalog products call exists. The
  join in §4 is specified as N single-product calls precisely because that form
  is *verified* to work; treat batching as an optimisation to confirm, not a
  dependency.
- That catalog image URIs are protocol-relative.
- That an MSA compact ticket is what "a `t=` token" refers to.
- All response field names in the §7.1 schema that come from collections rather
  than from DisplayCatalog or `PackageDetails`.

---

## 10. Settling the inferences

What is left is the request body. There is a probe for exactly this in the Xodus
fork — `xodus-cli collections <url> [--body <json>] [--relying-party <rp>]`
— which mints a token, substitutes placeholders (`{XUID}`, `{PUID}`, `{UHS}`) so
no account identifier has to appear on a command line, and prints the status, the
`MS-CV` and the raw body. Iterate from the shell; do not recompile per guess.

Order of work, cheapest first, and only two steps touch an account:

1. **Confirm the catalog half offline.** `curl` the anonymous DisplayCatalog URL
   for `9NBLGGH2JHXJ` and read the document. That settles every §5.2 inference,
   the image URI form, and the batch-`bigIds` question, with no sign-in and no
   account risk. Do this first; it unblocks the whole presentation layer.
2. **Ship `ferestre owns` and the schema** on the §3.5 licensing oracle. Real output,
   real cache, real CLI, no unknown endpoint.
3. **Vary the collections body.** The auth question is closed (§3.2), so every
   remaining 400 is about the body and each one names the field it disliked. Work
   the schema complaint, not the token. Try the PUID and the XUID as beneficiary.
4. **Cross-check against `mygames`** once collections answers, per §3.3. If the
   two disagree, the query is filtering too hard.
5. **Only then** widen the model and wire `ferestre library` to it.

Record what each probe returned — status, `MS-CV`, and the *shape* of the body —
in `notes/`, the way the rest of this project's hard-won findings are recorded.
Never paste a response containing an account identifier, a token or a licence.

---

## 11. What this design deliberately leaves out

- **Purchasing, redeeming, and anything that spends money.** Read-only.
- **Delta updates.** `available.version_id` makes "an update exists" answerable;
  acting on it still means a full re-download, which the roadmap already records
  as unsolved and which makes update detection close to useless for large titles.
- **A local search index.** A library is hundreds of rows, not millions. Filter
  the JSON.
- **Guessing compatibility.** `runnable` comes from a title manifest or is
  `unknown` (§6.3).
- **Console titles, DLC and durables as first-class rows.** They appear in
  `skipped` with a reason, so the filtering is auditable, and nowhere else.
- **Any on-disk copy of credentials.** The keychain is the only token store.
