# Compatibility

Which Xbox GDK / Microsoft Store titles have been run on Linux with this
project, and exactly where the ones that fail stop.

## Read this first

**Most titles will not work, and that is the expected result today.** Three
have been tried. The GDK runtime here is written against the *observable*
behaviour of the API — task queues, async, users, storage, networking, save
games, packages — so it covers what those three titles happened to call.
A fourth title will call something else. That is not a bug in the report, it is
the state of the work.

There are two separate reasons a title fails, and telling them apart is most of
the value of a report:

- **A gap in this project.** The title asks for an API that is missing, stubbed,
  or subtly wrong, and says so in its own log. This is fixable, and the log line
  usually names the thing.
- **The title defending itself.** Some Store builds carry code protection that
  refuses to run under Wine. Forza Horizon 5 dies inside its own protection in a
  static initialiser before any runtime API is reached. Nothing here will try to
  defeat that.

So: **a clear failure report is worth as much as a success report.** "Does not
launch" is worth nothing. "Exits with no window, last line is
`Failed to Register TaskQueue Monitor`" points at one function. If you own a
title nobody has tried, running it once and writing down where it stopped is a
real contribution even — especially — when the answer is that it does not run.

**Before you paste anything: a session log contains your gamertag, your XUID and
your save-folder ids.** See [Reporting a title](#reporting-a-title) below.

## The matrix

<!-- BEGIN GENERATED MATRIX (.github/scripts/gen-compatibility.py) -->

| Title | Store id | State | Notes | Verified |
|---|---|---|---|---|
| Age of Empires: Definitive Edition | `9NJWTJSVGVLJ` | Playable | Boots, starts a custom game and plays. | 2026-09-06 |
| Clair Obscur: Expedition 33 | `9PPT8K6GQHRZ` | Playable | Playable start to finish, with working saves and video. | 2026-09-06 |
| DOOM 64 | `9MXND4PQLK3W` | Playable | Plays. | 2026-09-06 |
| DREDGE | `9MSVVM5NS9L6` | Playable | Plays. | 2026-09-06 |
| Minecraft for Windows | `9NBLGGH2JHXJ` | Playable | Signs in to Xbox Live, loads the profile, plays, and joins third-party servers from the in-game list. | 2026-09-06 |
| Overthrown | `9MT5KSV3RCWD` | Playable | Plays. | 2026-09-06 |
| Stardew Valley | `9MWR1NC6VQ6L` | Playable | Plays. | 2026-09-06 |
| Goat Simulator 3: Windows Edition | `9PDS2N82QNXG` | Playable, with issues | Boots and plays; the online features do not work. | 2026-09-06 |
| Forza Horizon 5 | `9NNX1VVR3KNQ` | Does not run | Downloads and decrypts; dies inside its own code protection before any runtime API is reached. Stops at: an illegal instruction (0xc000001d) at ForzaHorizon5.exe+0x53d5cec, reached through ucrtbase -- the C++ static-initialiser path -- with no GDK call made first. | 2026-09-05 |

Generated from `titles/*.toml`. Each row's recipe is the file named after its Store id, and `titles/SCHEMA.md` says what is in one.

<!-- END GENERATED MATRIX -->

### What the states mean

| State | Meaning |
|---|---|
| Playable | Played far enough that nothing is known to be broken. |
| Playable, with issues | Plays, but something in the recipe's issue list is wrong or has to be worked around. |
| Starts, menus only | Reaches a menu and goes no further. |
| Does not run | Downloads and decrypts, never reaches a window. |
| Untested | A recipe exists. Nobody has reported running it. |

The vocabulary is defined in `titles/validate.py`, which rejects a recipe using
anything else — and rejects any state but `untested` that carries no date,
because a matrix entry without one is a rumour.

## Reporting a title

Two forms, both under
[New issue](https://github.com/icex/ferestre/issues/new/choose):

- **Title report: it works** — for a title that reaches gameplay. What is
  wanted is the recipe: the product id, the executable path inside the package,
  and anything you had to do that is not in `docs/RECIPES.md`. That becomes a
  `titles/<store id>.toml` and a row in the matrix above.
- **Title report: it does not work** — for everything else, including downloads
  that never start. Where it stopped and the last lines of the log are the whole
  report.

Before you open either one:

1. Read the troubleshooting table in
   [RECIPES.md](RECIPES.md#5-troubleshooting-every-error-we-actually-hit-and-its-fix).
   Most of the failures seen so far already have a cause and a fix there.
2. Reproduce with `WINEDEBUG=+debugstr`, which is by far the most useful channel
   for a GDK title: the game's own `[INFO]`/`[ERROR]` lines say which API failed.
3. **Redact the log.** Do not skip this.

### Redacting a log

A launch log contains, at minimum, your gamertag, your XUID, your save-folder
ids, and — if you turned up the logging — Xbox Live tokens. None of it helps
anyone read the report, and a token in a public issue is a live credential until
it expires.

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

Replace `YourGamertag` with yours: that one cannot be guessed by a pattern, and
it is the field people forget. Then **read the result** before pasting it. A
regex catches shapes, not everything; a game that prints your account name in
its own format will still be in there.

Never attach a whole log file, a package, a `.msixvc`, a licence, a content key,
or a save. Fifty lines around the failure is what gets read anyway. If a
maintainer needs more they will ask.

If you have already pasted a token somewhere public, see
[SECURITY.md](../.github/SECURITY.md) — editing the comment is not enough,
revoke it.

## How this page is generated

Nothing in the table is typed by hand. `titles/*.toml` holds the recipes the
launcher runs from; `titles/validate.py --matrix` renders them, and refuses to
render a recipe that does not validate; `.github/scripts/gen-compatibility.py`
puts the result between the markers above. So the matrix cannot quietly
disagree with what the code does.

```sh
titles/validate.py                              # check the recipes
.github/scripts/gen-compatibility.py            # rewrite the table above
.github/scripts/gen-compatibility.py --check    # what CI runs on every change
```

Editing the table by hand will be undone by the next run, and CI will have
failed before then. Edit the recipe instead.

A title moves state when someone runs it, not when it seems likely to work.
