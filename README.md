<img src="packaging/icons/hicolor/scalable/apps/io.github.icex.ferestre.svg" width="96" align="left" alt="">

# Ferestre

Running **Xbox GDK / Microsoft Store (MSIXVC) titles on Linux**, on a patched
Wine/Proton with a from-scratch implementation of the Xbox Game Development Kit
runtime.

<br clear="left">

> **You need to own the games.** This project ships no game content and never
> will. It signs in with *your* Microsoft account, uses *your* licences, and
> downloads *your* purchases. If you do not own a title, or your account does
> not have a Game Pass entitlement for it, there is nothing here that will let
> you play it — and that is deliberate.

## Why

My son wanted to keep playing **Minecraft** after moving his machine from
Windows to Linux.

The usual answer is the Android build under an emulator, which is a different
game: different edition, different input, different performance, and it does not
join the servers his friends are on. The version he already owns is the Windows
Store one, and that is a `.msixvc` package — encrypted, licence-gated, and built
against the Xbox GDK, none of which Wine could open.

So the goal was narrow and specific: **let a kid play the Minecraft he already
owns, on Linux, without pretending to be a phone.** That works now, servers and
all. Everything else here — the GDK runtime, the launcher, the catalogue work —
grew out of that one requirement, and the same machinery turned out to run other
Store titles too.

## The name

**Ferestre** is Romanian for **windows** — the plural of *fereastră*, the thing
in a wall you look through, not the operating system. Romanian gets it from
Latin *fenestra*, the same root behind French *fenêtre*, Italian *finestra* and
the English word *defenestration*.

The joke is that this project runs Windows games, on Linux, by putting a window
around them — and that the pun only works in a language Microsoft did not name
anything after. The icon is an arched window rather than a four-pane rectangle
for the same reason: a four-pane rectangle is somebody else's trademark.

Pronounced roughly *feh-RESS-treh*.

## Status

Honest version, because this is early. Every row here was run on a real machine,
not inferred from the fact that it downloaded:

| Title | Source | State |
|---|---|---|
| **Minecraft for Windows** (Bedrock) | Store | Playable. Signs in to Xbox Live, loads the profile, and **joins third-party servers from the in-game list** |
| **Clair Obscur: Expedition 33** | Store / Game Pass | Playable — saves, video, full game |
| **DREDGE** | Game Pass | Playable |
| **Stardew Valley** | Game Pass | Installs and launches |
| **Overthrown** | Game Pass | Installs; not played through yet |
| **Age of Empires Definitive Edition** | Game Pass | Downloads; not played through yet |
| **Forza Horizon 5** | Store | Downloads, does not run: blocked by protection inside the title, not by anything here |

Notes worth having before you try your own library:

- **Only MSIXVC packages.** That is the container Xbox GDK titles ship in.
  Everything else in a typical Store library is UWP (`Appx`, `Msix` and their
  bundle and encrypted variants) and this runtime cannot open it — measured on
  one real account, that is 95 of 102 owned titles. The window holds them back
  behind a switch that says how many there are and why, rather than mixing them
  in. Support for them is on the roadmap and is not a small job.
- **Game Pass lists more than any one tier grants.** The PC catalogue is
  public — 524 titles at the time of writing — but a given subscription covers a
  subset, and installing outside it is refused at the licence step. Those rows
  are filtered out by default too.

There is a launcher: a CLI (`ferestre`) and a GTK4 window (`ferestre-gui`) that
signs in, lists what your account owns with real names, cover art and download
sizes, lists what a Game Pass subscription includes, installs with a progress
bar and a time remaining, detects updates, and launches. See
[docs/ROADMAP.md](docs/ROADMAP.md) for what is still missing and
[docs/RECIPES.md](docs/RECIPES.md) for doing any of it by hand.

Two things the window is careful about, because both are easy to get wrong in a
way that looks like it works. Titles whose package this runtime cannot open --
Appx and Msix, which is most of a typical Store library -- are held back behind
a switch that says how many there are, rather than mixed in among the ones that
run. And a title being in the Game Pass catalogue is not the same as your
account being entitled to it: the window says what the catalogue includes and
which subscriptions the account holds, and lets the licence request at install
time be the thing that decides.

## How it fits together

Three layers, only one of which is unusual:

1. **The client** — signing in, fetching your licences and content keys, and
   downloading and decrypting the MSIXVC package. This is what the upstream
   [Xodus](https://github.com/xodus-gaming) project already does.
2. **The runtime** — a Proton fork whose Wine is patched so an encrypted title
   can actually be *loaded*, plus `xgameruntime.dll`: an implementation of the
   GDK (task queues, async, users, storage, networking, save games, packages)
   written against the observable behaviour of the API.
3. **The launcher** — this repository. `ferestre-core` decides everything (which
   recipe applies, whether the runtime can satisfy it, where a title installs);
   the CLI and the window are two faces on the same decisions, so they cannot
   disagree. It never links the client: Xodus is GPL-3.0-only and this is MIT,
   so the client is run as a child process — which is also the only shape that
   works, because a decrypted executable is passed to Wine as an inherited file
   descriptor.

The awkward part is that a GDK title's executable is encrypted on disk and only
ever exists decrypted inside a memfd handed to Wine, so it cannot be started
directly. Everything has to go through the client's `run` path.

## What this is not

- Not a way to play games you do not own.
- Not a piracy tool: no content, no keys, no bypass of a title's own protection.
  Where a title defends itself against running under Wine (Forza Horizon 5), it
  stays unsupported.
- Not affiliated with Microsoft, Mojang, Valve, or the Xodus project.
- Not upstreamable to Xodus: they operate a clean-room policy that excludes
  AI-assisted contributions, so this stays a downstream fork.

## Documentation

- [docs/RECIPES.md](docs/RECIPES.md) — build the runtime, sign in, download a
  title, launch it. Machine-independent; every script takes its paths from
  `XODUS_*` environment variables.
- [docs/ROADMAP.md](docs/ROADMAP.md) — the plan for turning this into a launcher.
- [notes/](notes/) — the debugging write-ups. Worth reading if you want to know
  *why* something is the way it is; several were genuinely hard to find.
- [CONTRIBUTING.md](CONTRIBUTING.md) — adding a title, running the tests.

## Licence

Mixed, because the work is. MIT for the original tooling, tests, scripts and
notes; LGPL-2.1-or-later for the Wine patches; GPL-3.0-or-later for the Xodus
patches; BSD-3-Clause for the Proton one. [NOTICE](NOTICE) has the map.

One consequence worth knowing before building on this: Xodus is GPL-3.0, and it
is what signs in and downloads. A launcher that *shells out* to the `xodus-cli`
binary is aggregation and may carry any licence; one that *links* the `xodus` or
`msixvc` crates becomes GPL-3.0 itself.
