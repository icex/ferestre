# Package signatures

Every game package from the Xbox package CDN (an MSIXVC, or XVD, container)
starts with a header page that Microsoft signs. The header names the package,
says how it is laid out, and holds the top hash of the hash tree that every
other page is checked against. When the client has the key that signs retail
PC game packages, it checks that signature before it believes anything in the
header, and refuses a download whose header does not verify.

Neither Ferestre nor the client ships that key. It is Microsoft's public key,
and the client finds it on your machine the first time it needs it.

## Where the key comes from

The key is in Microsoft Gaming Services, the free Store app that Windows
installs for every PC game (Store product `9MWPM2CQNLHN`). The client
downloads its current package the way Windows fetches a free app, through the
Store's delivery service, without signing in:

1. The Store catalogue (`displaycatalog.mp.microsoft.com`) gives the product's
   Windows Update category.
2. The delivery service (`fe3.delivery.mp.microsoft.com`) lists the package
   and its SHA-256, and gives a download URL. Every request carries an empty,
   anonymous ticket; no account, token or device identity is sent.
3. The package (about 200 MB) is downloaded from a
   `*.dl.delivery.mp.microsoft.com` host into a temporary file, and checked
   against the size and SHA-256 the delivery service gave.
4. The client reads the binaries of the x64 package inside it without
   unpacking the rest, and keeps only an RSA-4096 public key whose SHA-256 is
   the one compiled into the client:
   `183F0AE05431E4AD91554E88946967C872997227DBE6C85116F5FD2FD2D1229E`.
   Any other key, or a package that was changed on its way, yields nothing.
5. The key (539 bytes) is stored readable only by you, and the download is
   deleted. On a fast connection the whole thing takes a few seconds.

The delivery service's TLS certificate chains to Microsoft Root Certificate
Authority 2011, which Linux certificate stores do not include. The client
fetches that root certificate from the address Microsoft's own intermediate
certificate gives for it, accepts it only if its SHA-256 is
`847DF6A78497943F27FC72EB93F9A637320A02B561D0A91B09E87A7807ED7C61`, and
trusts it for the delivery service host only. Nothing else the client does
trusts it.

The stored key is hashed again every time it is used. A key file that has
changed, belongs to someone else or is readable by others is not used.

## When it happens

- `ferestre doctor` reports whether package signatures are checked.
- An install (`ferestre install`, or the client's `streaming`) uses the stored
  key. If there is none, it tries once to fetch it, for at most three minutes,
  and says so on stderr. An automatic attempt that failed is not repeated for
  a day.
- `xodus-cli xvd-key fetch` fetches it now; `xodus-cli xvd-key status` and
  `xodus-cli xvd-key forget` report and remove it. All three take `--json`,
  and none of them needs an account.

The key is kept in `~/.local/share/xodus/xvd-key/` (`$XDG_DATA_HOME/xodus/xvd-key`),
and the download goes to `~/.cache/xodus/xvd-key-fetch/` while it is read.
`XODUS_XVD_KEY_DIR` puts both somewhere else.

## What is checked

With the key, before anything in the header is used:

- the signature over the header page (RSASSA-PSS, SHA-256) must verify; an
  unsigned header is refused;
- a header that turns the package's integrity checks off is refused;
- for a package the authenticated package service named, the header must name
  the same package: its content id, its version id and its version;
- then, as before, the header's top hash must match the package service's
  `HashOfHashes` when it gives one, and every page must match the hash tree.

A refused download stops with an error that says the package was refused, and
nothing from it is installed.

## Settings

| Variable | Effect |
|---|---|
| `XODUS_REQUIRE_SIGNED_PACKAGE=1` | Refuse to install when the header signature cannot be checked because there is no key. |
| `XODUS_XVD_KEY_AUTOFETCH=0` | Never fetch the key on the way to an install; only `xodus-cli xvd-key fetch` does. |
| `XODUS_XVD_KEY_DIR` | Where the key, and the download it comes from, are kept. |

Without a key and without `XODUS_REQUIRE_SIGNED_PACKAGE=1`, an install goes
ahead as it did before signatures were checked, with a warning on stderr.

## Limits

- The key is the one that signed every retail PC package checked so far
  (eleven titles). A package signed with another key would be refused; if that
  happens, `xodus-cli xvd-key forget` with `XODUS_XVD_KEY_AUTOFETCH=0` turns
  checking off until it is fixed, and a report is welcome.
- The relation between the header's identity fields and the package service's
  answer was confirmed on Stardew Valley. The header's `product_id` is not
  compared: how it is encoded is not known.
- Only the header's first page is signed. The rest of the header is not, and
  nothing the client relies on is taken from it.
- If Microsoft changes Gaming Services so that it no longer holds the key, or
  the Store delivery service changes, fetching fails and installs go on
  unchecked, with a warning, unless `XODUS_REQUIRE_SIGNED_PACKAGE=1`.
