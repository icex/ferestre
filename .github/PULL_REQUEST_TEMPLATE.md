## What this changes, and why

<!-- The why is the part that is hard to recover later. What the diff does is
     already in the diff. -->

## Which layer

<!-- Tick what this touches. The licence of a change follows its directory --
     see NOTICE -- so this is not bookkeeping. -->

- [ ] `patches/wine/` or `patches/xgameruntime/` — the runtime (LGPL-2.1-or-later)
- [ ] `patches/xodus-cli/` — the client (GPL-3.0-or-later)
- [ ] `patches/proton/` — the Proton script (BSD-3-Clause)
- [ ] `scripts/`, `tools/`, `tests/` — tooling (MIT)
- [ ] `titles/` — a title recipe
- [ ] `docs/`, `notes/`, `.github/` — documentation and repository plumbing

## How this was tested

<!-- Say what you ran and what it printed, not that it "should" work. Several
     fixes here looked right and were not; the ones that stuck had a check that
     could tell the difference. -->

- [ ] `tests/run-tests.sh`
- [ ] `tests/run-gameinput-tests.sh`
- [ ] `tests/run-appmodel-tests.sh`
- [ ] Launched a real title, and it got to:
- [ ] Not tested, because:

## Checklist

- [ ] No hardcoded home directories, machine names, or personal identifiers.
      Paths come from `XODUS_*` with `$HOME`-relative defaults.
- [ ] No gamertag, XUID, PFID, token, licence, content key, package or save
      anywhere in the diff — including in a log pasted into the description, a
      test fixture, or a commit message.
- [ ] Shell passes `bash -n`; Python parses with `ast.parse`.
- [ ] A change to GDK behaviour comes with a test that fails without it.
- [ ] A change under `titles/` was followed by
      `.github/scripts/gen-compatibility.py`, and `docs/COMPATIBILITY.md` is
      committed with it. CI runs `--check` and will fail otherwise.
- [ ] A patch file is `git diff --relative` output from the relevant directory,
      one concern per file.

<!-- A title moving state in the matrix needs someone to have actually run it.
     "Should work now" is not a state. -->
