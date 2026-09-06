# Contributing

## Before anything else

Do not put game content, licences, content keys, account identifiers or tokens
in this repository — not in code, not in logs, not in a test fixture. Traces
from a real session contain your gamertag, XUID and PFID; redact them before
pasting. The `.gitignore` covers the obvious build outputs, but it cannot catch
a log you paste into an issue.

## Layout

```
.github/   issue and PR templates, and the script that regenerates the
           compatibility matrix from the recipes
patches/   modifications to Wine/Proton and to the GDK DLL, as plain git diffs
           (LGPL-2.1-or-later, because Wine is; the Xodus ones are GPL-3.0-only)
scripts/   build, install and per-title launch
tests/     synthetic suites; run them before proposing a runtime change
titles/    one TOML recipe per Store product id -- the per-title data the
           compatibility matrix is generated from
tools/     debugging and automation
docs/      how to build, install and run
notes/     write-ups of problems that were hard to find
```

Everything resolves its paths from `XODUS_*` environment variables with
`$HOME`-relative defaults. If you find a hardcoded path, that is a bug.

## Running the tests

```sh
tests/run-tests.sh          # the GDK runtime suite
tests/run-gameinput-tests.sh
tests/run-appmodel-tests.sh
```

These build against the current sources and run under the locally built runtime.
A change to the GDK DLL that does not keep them green is not ready.

There is also an unattended end-to-end harness, `tools/auto-join.py`, which
launches a title, drives its menus and reports whether it got where it should.
It needs a real account, a GPU and a desktop session, so it cannot run in CI —
but it turns "does this still work?" into one command instead of several minutes
of clicking.

## Adding a title

Per-title knowledge is data: one `titles/<store-product-id>.toml` per title.
The product id is the twelve-character identifier in the Store URL, which is
also what the collections API returns, so a recipe joins straight to what an
account owns.

Start from an existing recipe, run `python3 titles/validate.py`, and let
`scripts/launch-gdk.sh` do the shared work. A recipe declares the runtime
*capabilities* it needs rather than a Proton version, so it keeps working across
runtime updates. `docs/COMPATIBILITY.md` is generated from these -- do not edit
it by hand.

If a title does not work, say precisely where it stops. "Does not launch" is not
actionable; "exits with no window after `XGameRuntimeInitialize`, with this in
the log" is.

## Changing the runtime

`patches/` are plain `git diff` output against the fork, regenerated with
`git diff --relative` from the relevant directory. Keep one concern per patch
file, and explain *why* in the commit message rather than restating the diff.

Anything that changes GDK behaviour should come with a test that fails without
it. Several fixes here looked right and were not; the ones that stuck were the
ones with a check that could tell the difference.
