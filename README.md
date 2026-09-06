<img src="packaging/appimage/io.github.icex.ferestre.svg" width="96" align="left" alt="">

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

Honest version, because this is early:

| Title | State |
|---|---|
| **Minecraft for Windows** (Bedrock, Store) | Signs in to Xbox Live, loads the profile, plays, and **joins third-party servers from the in-game list** |
| **Clair Obscur: Expedition 33** (Store / Game Pass) | Playable — saves, video, full game |
| **Forza Horizon 5** (Store) | Downloads, does not run: blocked by protection inside the title, not by anything here |

There is a launcher: a CLI (`ferestre`) and a GTK4 window (`ferestre-gui`) that
signs in, lists what your account owns with real names and cover art, installs,
detects updates, and launches. See [docs/ROADMAP.md](docs/ROADMAP.md) for what is
still missing and [docs/RECIPES.md](docs/RECIPES.md) for doing any of it by hand.

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
