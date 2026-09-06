# Roadmap: from patches to a launcher

The goal is something a person can install and use: sign in, see the titles they
own, download one, keep it updated, and play it — the role mcpelauncher plays
for Bedrock on Android, for Microsoft Store titles on Linux.

## Where the work already is

Three layers. Two exist.

| Layer | Does | State |
|---|---|---|
| Client | Microsoft sign-in, licences and content keys, MSIXVC download and decrypt | Exists upstream (Xodus), plus local fixes |
| Runtime | Patched Proton/Wine + `xgameruntime.dll`, the GDK implementation | Exists here. ~1.4 GB installed |
| Launcher | Library, install, update, configure, launch | **Does not exist** |

One constraint shapes everything: a title's executable is encrypted on disk and
only exists decrypted inside a memfd passed to Wine. Nothing can start a title
except the client's `run` path, so the launcher has to drive the client rather
than treat it as an optional extra.

## Phase 0 — foundations

- [x] Repository with no personal data, licences, and a clear statement that
      users must own their games
- [x] Replace prose recipes with a **declarative per-title manifest** (product
      id, executable path, prefix layout, environment, DLL overrides, required
      GDK features, known issues). The launch surface turns out to be small —
      `scripts/launch-gdk.sh` is 75 lines and most of it is undoing environment
      that Steam injects — so this is a modest schema, not a framework.
      `titles/*.toml`, `titles/SCHEMA.md`, validated in CI.
- [ ] CI running the synthetic suites on every change. **Half done.** The Rust
      workspace, `cargo fmt`, `clippy` and the AT-SPI end-to-end GUI test run on
      every change, and so does recipe validation. `tests/xgr_tests.c` — the
      synthetic GDK suites — still runs only by hand.

## Phase 1 — ship the runtime

The runtime is the big artefact and it changes far less often than a launcher
would, so the two version independently and the launcher fetches runtimes. This
is the Proton-GE / ProtonUp-Qt model, which people on this platform already
understand.

- **Slim the build.** Measured: dropping `wine-gecko` (208 MB) and `wine-mono`
  (226 MB) takes an installed runtime from 1.4 GB to **983 MB**, and Minecraft
  still launches, navigates its menus and joins a server. Neither is needed by a
  native GDK title. Keep a full build available in case a title wants .NET or an
  embedded browser.
- Strip binaries (76 of the first 400 checked still carry symbols).
- Publish versioned tarballs that install as a Steam compatibility tool, so the
  runtime is useful on its own before any launcher exists.

## A constraint that rules out the obvious approach

Modern Linux launchers (Heroic, Lutris) run Proton through
[umu-launcher](https://github.com/Open-Wine-Components/umu-launcher), which
executes inside a Steam Runtime container via pressure-vessel. **That cannot
work here**, and it was tested rather than assumed: the entry point was invoked
directly, bypassing umu's downloader so the result is about pressure-vessel
itself, and **inherited file descriptors do not survive the container layer**.

A GDK title's executable only exists decrypted inside a memfd passed to Wine as
an inherited fd. Lose the fd, lose the executable — for every title, not as an
edge case. So the launcher must invoke the runtime directly, and the runtime
tarball must carry what it needs (a full ffmpeg included) rather than relying on
a container to supply it.

## Phase 2 — the launcher

Shape: a **CLI core with a GUI over it**, the split legendary/Heroic uses. The
CLI subsumes the thirteen shell scripts in `scripts/` into one binary with
subcommands, which is also the only way CI can test a launch path at all.

The largest genuinely missing piece is **enumerating what an account owns** —
minting a token for the collections relying party and joining entitlements
against product ids. Everything else exists in some form.

Smallest useful version, in order:

1. Sign in and list what the account owns
2. Download with resume, and detect updates
3. Install into a predictable layout, one Wine prefix per title
4. Launch using the title's manifest
5. Save management, then uninstall

Three decisions worth stating up front:

- **Shell out to the client, do not link it.** Xodus is GPL-3.0. Linking its
  crates makes the launcher GPL-3.0 too; running `xodus-cli` as a child process
  is aggregation and leaves the licence free. That is also the practical shape:
  the decrypt-and-launch path lives in the *binary* crate, not a library, so
  linking would mean lifting code out of a fork that cannot be upstreamed and
  rebasing it forever.

- **Steam integration is a toggle, not a policy.** By default the launcher runs
  an isolated runtime and strips Steam's injected environment, because the
  overlay, Fossilize capture and `STEAM_COMPAT_*` collide with a Proton instance
  we start ourselves. But people add these titles as non-Steam shortcuts for the
  overlay, controller config and Remote Play, so keeping that environment must
  be an option per title rather than something the launcher decides.
- **The client is carried, not assumed.** Because launching requires the
  decrypt path, "install Xodus separately" is not a real option for an end user.

### Per-title configuration as data

Adding a title today means editing five places, which is what caps this at three
titles. Replace it with one `titles/<store-product-id>.toml` per title — the
product id is the identifier a user actually has, since it is in the Store URL.

Do **not** pin a Proton version per title. Ship a generated capability list with
the runtime (`loader.memfd-main-image`, `appmodel.package-identity`, and so on)
and have a title declare the capabilities it needs. That way a title keeps
working across runtime updates instead of being frozen to one build.

Turn the twelve symptom/cause/fix rows in `docs/RECIPES.md` into a
fingerprint file the runner consults automatically on a failed launch, so a new
title diagnoses itself rather than needing someone who remembers.

### The window

The GTK4 window exists and launches titles. Most of what this section once listed
as missing is now built; what remains is at the bottom, and the hardest of it is
honestly marked as unproven rather than planned.

- [x] **Real names and cover art for owned titles.** Collections returns product
      ids and nothing else — no name, no image — so a library of 100+ titles
      currently reads as a list of twelve-character codes. The names and art
      come from DisplayCatalog (`displaycatalog.mp.microsoft.com/v7.0/products`),
      which is anonymous: no token, no account, so it can be called for a title
      nobody owns and cached on disk without touching auth. Batch the ids, cache
      per product, and never block the window on it.

- [x] **Pagination for the owned list.** A hundred rows in one `AdwPreferencesGroup`
      is neither usable nor fast. Pages plus a search box; search matters more
      than the pager once names exist.

- [x] **A login flow, and the signed-in account on screen.** Today signing in is
      a side effect of asking for the library. It should be explicit: a
      Sign in / Sign out control, the gamertag and gamerpic in the header, and a
      window that says plainly when nobody is signed in. The client has `login`
      and `logout`; the gamertag and picture come from
      `profile.xboxlive.com/users/me/profile/settings`, which needs an XSTS
      token for `http://xboxlive.com` rather than the licensing relying party
      the library uses.

- [x] **Updates.** Working end to end. The key is the catalog's `ContentId`,
      and the installed build is read from the package header the client leaves
      at `<install>/.xodus-streaming.msixvc` -- its VDUID *is* the ContentId, so
      a title installed before this launcher existed is adopted exactly, from a
      4 KiB read, with nothing assumed and nothing to ask.

      Two things had to be right or it would have been worse than useless.
      **The catalog set is filtered to `Windows.Desktop`**: most titles ship an
      Xbox package beside the desktop one with its own content id, and comparing
      against the union reports a permanent update that no download clears --
      measured, that was wrong on two of the three titles installed here. And
      **"cannot tell" is worded differently from "up to date"**, because the row
      and the button are identical in both, so silence would read as
      reassurance.

- [x] **A recipe editor.** The catalog will list a hundred titles with three
      recipes between them, so the common case is a title nobody has described.
      Someone should be able to fill in an executable path and a couple of
      environment variables in the window, try it, and hand the result back as
      an issue — not learn a TOML schema first. Edits go to
      `$XDG_CONFIG_HOME/ferestre/titles/` and win over the packaged recipe, so an
      upgrade never reverts them and "what did I change" stays answerable.

- [x] **Add to Steam.** A non-Steam shortcut for the overlay, controller
      configuration and Remote Play. Writing `shortcuts.vdf` means rewriting a
      file full of shortcuts that have nothing to do with this launcher, so it
      round-trips the whole document and keeps every field it did not write.
      Steam rewrites that file from memory on exit, so the launcher has to
      refuse while Steam is running rather than write an edit that vanishes.

- [x] **A sidebar, and a layout that survives a hundred titles.** One flat
      preferences page was right for three recipes and is wrong for a library.
      Library / Installed / Updates / Runtime as sidebar sections.

- [ ] **Differential updates — download only what changed.** Not confirmed
      possible yet, and worth saying so rather than promising it. What is known:
      an MSIXVC is an XVC container whose header carries per-block hashes, the
      client already has a chunk-granular `streaming` path rather than only a
      whole-file download, and the delivery endpoints are Azure blob URLs, which
      support HTTP range requests. If those three hold together, an update is
      "fetch the new header, compare block hashes against the local file, refetch
      the blocks that differ" — no delta format from Microsoft required. The open
      questions are whether block boundaries survive a content change (an insert
      that shifts everything defeats fixed-block comparison) and whether the
      published package is rebuilt wholesale between versions. Answer those with
      two versions of one large title before writing any code.

## Phase 3 — packaging

- AUR first: the development platform is Arch-based and it is the cheapest way
  to get real users.
- AppImage or Flatpak for everyone else. Whether a public Flatpak remote is
  appropriate is a posture decision, not a technical one — see below.
- The runtime is downloaded, never bundled into the launcher package.

Three independently versioned release trains: the launcher (small, frequent),
the runtime (~264 MiB compressed, when Wine or the GDK DLL changes), and title
recipes (continuously, community-contributed).

**CI is affordable.** Measured against the upstream Proton fork's own workflow:
a warm-cache build completed on free public runners in ~35 minutes, and a cold
build in ~1h12m. A self-hosted runner is a nice-to-have for a GPU smoke test,
not a requirement to build at all. Building from forked repositories rather than
applying a directory of loose patches removes a whole class of "the patch no
longer applies" breakage.

## Phase 4 — showing it works

The claim invites scepticism, so the evidence should be hard to wave away:

- A compatibility matrix that says exactly where each title stops.
- A demo produced by `tools/auto-join.py`, which launches the game, drives the
  menus and screenshots the result without anyone touching the keyboard.
- The write-ups in `notes/`. The best of them: a stubbed callback made
  libHttpClient believe the machine had no network, so it refused every
  WebSocket connect — while plain HTTP kept working, which hid it — and its
  failure path then used a destroyed object, which surfaced as the game
  freezing.

## Open decisions

These are judgement calls, not engineering ones:

1. **How public, and hosted where.** A private repo, a public repo, and a
   Flatpak remote are three quite different levels of visibility for a tool that
   signs in to Microsoft accounts and decrypts packages with the user's own
   licence. The technical work is the same; the exposure is not.
2. **Name.** Something that does not imply endorsement by Microsoft, Mojang,
   Valve or Xodus.
3. **How the client is carried** — vendored, submodule, or a maintained fork —
   given upstream will not take these changes.

## Known gaps, stated plainly

- **The runtime does not publish a capability list.** `files/share/ferestre/capabilities.json`
  is the manifest a build is supposed to ship; no build writes one, so the
  launcher falls back to probing, finds four of the capabilities it looks for,
  and every title carries "could not be found" wording it does not deserve. The
  launcher already refuses to *block* on probed evidence, which is the correct
  behaviour, but the wording will keep looking like a warning until
  `install-xodus-proton.sh` writes the manifest.
- **There are no delta updates.** MSIXVC downloads are resumable but not
  differential, so "update" currently means re-downloading the title. That is
  ~2.5 GB for Bedrock but tens of gigabytes for a large title, which makes
  update detection close to useless until it is solved. Epic's chunked manifests
  give Heroic deltas for free; this has no equivalent yet.
