# Security

This project signs in to Microsoft accounts, holds Xbox Live tokens, and
decrypts packages with a licence belonging to a real person. That is a small
surface but not a trivial one, so it is worth being clear about what to report,
where, and what never to put in the report.

## Supported versions

`main`, and only `main`. There are no releases yet and nothing is backported.
If you are running something older than the current `main`, the first question
will be whether the problem still reproduces on it.

## Reporting

**Use GitHub's private vulnerability reporting on this repository:
Security → Advisories → Report a vulnerability.** It is private to the
maintainers and it is the only private channel this repository has.

Do not open a public issue for anything involving credentials, tokens, licences,
content keys, or a way to read another user's data. Public issues are indexed
within minutes and are mailed to watchers immediately.

A useful report says: what an attacker can do, what they need first (local user?
a malicious title? nothing?), and the smallest set of steps that shows it. A
patch is welcome but not required.

Expect a human, eventually. This is volunteer work: there is no service level,
no bounty, and no PGP key. If something is being actively exploited, say so in
the first line.

## Never send a credential — not even privately

**Describe the shape, not the value.** "The service writes the XSTS token to
`$XDG_RUNTIME_DIR/xodus.sock` with mode 0666" is a complete report. Pasting the
token adds nothing and creates a second incident.

The same goes for logs. A launch log contains your gamertag, your XUID, your
save-folder ids, and — with verbose logging — Xbox Live tokens. Redact it:

```sh
sed -E \
  -e 's/\b[0-9]{16}\b/<redacted>/g' \
  -e 's/\b[0-9a-fA-F]{16}_/<redacted>_/g' \
  -e 's/eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9._-]+/<redacted>/g' \
  -e 's/(XBL3\.0 x=)[^;" ]*/\1<redacted>/g' \
  -e 's/(Bearer )[A-Za-z0-9._~+\/=-]{12,}/\1<redacted>/g' \
  -e 's/YourGamertag/<gamertag>/gI' \
  ~/xbox-games/<title>-launch.log > /tmp/report.log
```

Then read the output. A pattern catches shapes, not everything: a gamertag is
just a word, and a title that prints your account name in its own format will
still be in there.

## If you have already pasted a token somewhere public

Treat it as compromised and revoke it. Do that **before** worrying about the
comment.

1. `xodus-cli logout` — removes the stored credentials from your desktop
   keyring. `xodus-cli logout --device` also drops the device licence.
2. Sign out everywhere from your Microsoft account security settings, and
   review the device list at `account.microsoft.com/devices`. Xbox Live tokens
   expire in hours, but the refresh token behind them does not: signing out
   everywhere (or changing the password) is the step that actually invalidates
   it.
3. Only then, ask a maintainer to **delete** the comment. Editing it is not
   enough — the original text stays in the comment's edit history, in the API,
   and in the notification emails that already went out.

Nobody will be told off for this. It happens because logs are long and tokens
look like noise, which is exactly why the templates say it three times.

## In scope

Anything that could expose a user's account, tokens, device licence or content
keys, or let one process read what it should not:

- Token handling: how credentials reach the desktop keyring, and the
  `xodus-service` IPC socket that hands XSTS tokens to the runtime
  (`patches/xodus-cli/0001-xsts-token-ipc-endpoint.patch`) — permissions,
  authentication, what any local process can ask it for.
- The launch path: the memfd holding the decrypted executable, the file
  descriptors inherited by Wine, `WINE_DLL_FILE_MAP`, and anything that lets a
  title or another local process get at either.
- Patches that weaken a boundary Wine or Proton was relying on.
- Scripts that fetch and install a binary. `scripts/fix-xcurl.sh` downloads a
  curl build over HTTPS and installs it into a game directory; it verifies the
  exports the title imports, not a pinned checksum. If you can turn that into
  code execution, that is a report.
- Anything in the tooling that writes a token, key or identifier to a file, a
  log, or the terminal where it can be pasted by accident.

## Not in scope

- **A title's own protection.** Reports about defeating anti-tamper in a game
  are out of scope and will be closed. Forza Horizon 5 stays unsupported for
  exactly this reason.
- **"This downloads games."** It downloads titles the signed-in account is
  entitled to, with that account's own licence, the same way the Store app
  does. Concerns about the entitlement model itself belong with Microsoft.
- **Someone with your unlocked desktop session.** The Secret Service keyring is
  readable by your own session by design; that is the platform's boundary, not
  one this project claims to add.
- **Upstream bugs in Wine, Proton or Xodus** that this project only carries.
  Report them upstream; a note here so the patch set can be adjusted is
  welcome, but the fix belongs there.
- Missing hardening with no attack behind it. Say what an attacker gains.

## What this project stores

Nothing in this repository, ever: no content, no packages, no licences, no
keys, no account identifiers. Test fixtures are hand-written and carry only the
identifier fields the tests need.

On your machine, credentials live in the desktop keyring (D-Bus Secret Service:
KWallet, GNOME Keyring). Games, Wine prefixes and launch logs live under
`XODUS_GAMES_DIR` (`~/xbox-games` by default) — the logs are the part that
contains identifiers, and they are the reason `.gitignore` excludes `*.log`.
