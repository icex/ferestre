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
- [x] **A runtime publishes what it provides, and proves it.** Every capability
      in `titles/capabilities.toml` carries `verify` markers — a file inside the
      installed runtime and a string that is only there once the patch is — and
      `scripts/write-capabilities.sh` checks them against the real tree and
      writes `files/share/ferestre/capabilities.json`. `install-xodus-proton.sh`
      runs it. The manifest is therefore measured, not asserted, and
      `validate.py` refuses a capability with no way to check it. Before this,
      probing found 4 of 12 and every title carried "may not start" wording it
      did not deserve.
- [x] CI, sized to what each check costs. Per push: the Rust workspace, `cargo
      fmt`, `clippy`, the AT-SPI end-to-end GUI test, recipe validation, and
      **`scripts/check-patches.sh`** — every patch series applied to a pristine
      checkout of the commit it targets, and, where the repository is a build
      tree, checked to reproduce it exactly.

      The synthetic C suites (`tests/run-*.sh`) are deliberately **not** per
      push and the roadmap should stop implying they could be: they need the
      built Wine tree, which is 26 GB and about an hour. What they test is the
      DLL's behaviour, which only changes when the runtime is rebuilt, so they
      belong to the runtime's release train rather than to every launcher
      commit.

      The patch check is the cheap substitute and it has earned its place three
      times already: two client patches exported from a dirty tree carried their
      predecessor's changes and could not apply in order, and the Ferestre
      rename leaked `FERESTRE_HC_TRACE` into a patch describing the
      *xgameruntime* project's own source, where the variable is `XGDK_HC_TRACE`
      and is not ours to rename. Nothing else would have caught that.

## Phase 1 — ship the runtime

The runtime is the big artefact and it changes far less often than a launcher
would, so the two version independently and the launcher fetches runtimes. This
is the Proton-GE / ProtonUp-Qt model, which people on this platform already
understand.

- [x] **Slim the build, strip it, and package it.** All three were measured
      long ago and none was implemented; `scripts/package-runtime.sh` now does
      them, from an installed runtime, without touching it — everything happens
      in a staging copy.

      Measured on this machine: **1416 MB installed → 968 MB, 187 MB
      compressed.** Dropping `wine-gecko` (208 MB) and `wine-mono` (226 MB) is
      most of it; a native GDK title needs neither, and `--full` keeps them for
      one that turns out to. Stripping the 82 ELF shared objects is the rest.
      Only the ELF objects: Wine resolves a builtin DLL through data a strip can
      remove, so the PE builtins are left alone deliberately.

      Verified rather than assumed: the slimmed tarball was extracted, pointed
      at with `XODUS_PROTON_DIR`, and Minecraft launched and rendered on it.

      One trap, because it cost a silent two-thirds of the saving: **the runtime
      ships its libraries read-only** (`-r-xr-xr-x`), and `strip` works by
      writing a copy alongside, so it failed with "Permission denied" on 73 of
      82 files and reported nothing. It took noticing "stripped 9" against a
      known 82 to catch it.

      The script also asserts the release contract in `packaging/aur/README.md`
      rather than trusting it — one top-level directory, a
      `compatibilitytool.vdf` naming the tool `ferestre`, the `close_fds=False`
      patch, a real `xgameruntime.dll`, and the capability manifest — because
      each of those has been shipped broken by hand before, and each looks like
      the title crashing rather than like a packaging mistake.

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

- [x] **A library you can find seven titles in.** Measured on the development
      account: of 102 owned games and apps, **7** ship as MSIXVC and the other
      95 are UWP — 31 AppxBundle, 25 MsixBundle, 20 Appx, 13 with no PC package
      at all, and the rest Msix or encrypted variants. Listing all of them in
      one alphabetical run is a list where the working seven cannot be found, so
      the ones this runtime cannot open are held back behind a switch that says
      how many there are and why. Held back, not hidden: the switch shows them,
      each naming its own container, and a title the catalog says nothing about
      is never held back — "we could not ask" is not "no".

      Two things that went with it, both reported from real use: a paged list
      drew product ids because nothing fetched the names of the rows the pager
      had put on screen, and install started a download the moment it was
      clicked. Installing now asks first, with the size, the free space, and an
      editable destination.

- [x] **What a subscription makes installable.** A library that lists only what
      was bought outright misses most of what can actually be installed.
      `catalog.gamepass.com` answers anonymously, so the PC Game Pass catalogue
      -- 524 titles in this market -- is its own section, with the sizes and the
      same "this runtime cannot open that container" filter the library uses.

      What it deliberately does not do is claim entitlement. Tier names have
      been renamed and re-sliced, catalogues differ by market, and entitlement
      lists carry revoked subscriptions beside live ones. The section states
      what the catalogue includes and which subscriptions the account holds; the
      licence request at install time decides, and already says why when it
      refuses.

- [x] **A download size for every title.** Thirteen of 101 owned products had
      none, for three unrelated reasons: Windows 8 store apps (a PC platform the
      parser did not count), console-only titles (no PC download exists, which
      is an answer rather than a gap), and bundles (no packages of their own,
      only children). All three now resolve, and the count of products with no
      size and no reason is zero.

- [x] **Progress and a time remaining.** The client draws a terminal bar that
      hides itself when stderr is not a terminal, so a window had nothing to
      read. Patch 0006 adds machine-readable progress behind
      `XODUS_PROGRESS=json`; rate and estimate are smoothed over twenty seconds
      here, because an unsmoothed one flips between four seconds and nine
      minutes several times a second. A stalled download says so instead of
      estimating.

- [x] **Removing a title.** There was no uninstall at all: a title that failed
      to install left a record claiming it was there, a directory with a partial
      container in it, and no way to say otherwise. `ferestre uninstall
      <product-id>` removes the files and the record, and the window offers it
      on any installed row. Reinstalling is then the ordinary Install button.

      The path is checked rather than trusted before anything is deleted -- a
      record is a JSON file anybody can edit, and `remove_dir_all` on a bad path
      is the one thing here that cannot be undone.

- [ ] **Appx and Msix packages — the other 95%.** This runtime opens MSIXVC,
      the container Xbox GDK titles ship in. Everything else in a typical Store
      library is UWP: `Appx`, `AppxBundle`, `Msix`, `MsixBundle` and the
      `E`-prefixed encrypted variants. Measured on a real account, that is 95 of
      102 owned titles, and the window currently holds all of them back behind a
      switch that says why.

      Not a small item, and worth being honest about the shape of it. A UWP app
      is not a Win32 executable with a manifest beside it: it expects the
      Windows app model -- package identity, an activation host, WinRT brokers,
      the app container -- which is a different thing to stand up than the GDK
      surface this project already builds. Some of that work exists in Wine and
      in this fork already (package identity, `RegionPolicyEvaluator`), which is
      the argument for it being possible rather than the argument for it being
      quick.

      Worth doing in the order the library suggests: `AppxBundle` first, since
      it is the single largest group.

- [x] **Differential updates — answered.** The roadmap said not to promise this
      until two builds of one large title had settled it. That experiment turned
      out to be unnecessary: Microsoft publishes the patch plans themselves, and
      26 real ones across three titles were fetched and parsed. Verdict:
      **possible, and worth doing — but as "check whether a patch plan exists
      for your exact build and say the byte count before starting", never as
      "updates download only what changed".**

      The three questions, answered against the containers on this machine:

      - **Hashes: yes**, and better than needed. A four-level Merkle tree of
        24-byte entries, one per 4096-byte page, at file offset `0x4000`, and
        **entirely inside the retained prefix** — 14 / 265 / 897 MiB for the
        three titles. The decisive measurement: *the prefix's tree indexes the
        whole package*, not just the prefix. Pages of `ForzaHorizon5.exe` at
        container pages 1,290,859–1,332,814 verify against a prefix that ends at
        page 260,341.
      - **Ranges: yes.** Every install already depends on them — the client
        sends a bounded `Range` and gates on `206` with no fallback.
      - **Alignment: no, and it does not matter.** The layout is stream-ordered,
        so one file growing shifts everything after it: in a real plan, 37.0M of
        38.1M reusable blocks sit at shift +45 and another has *zero* at shift 0.
        A fixed-offset comparison would report ~100% changed. What rescues it is
        that identity here is per-(file, page-in-file): the XTS data unit is
        **per-file, not positional** (49/49 and 41/41 file boundaries show no
        continuity), which is why Microsoft can copy 97.6% of a 149 GiB volume
        to new offsets.

      **Measured savings**, as this launcher's real payload:
      Minecraft 2.32 GiB — 318 MB (12.8%) from the previous build;
      Expedition 33 44 GiB — 1.65 GB (3.5%) for 1.5.5.0 → 1.5.6.0;
      Forza Horizon 5 149 GiB — 2.76 GB (1.7%) from 3.688.44.

      **And the finding that stops this being a promise:** coverage is a
      publishing policy, not a property of the content. Microsoft ships a
      template full-download fallback, and for 10 of FH5's 12 published source
      versions — *including the build immediately before the current one* —
      that is all there is. So the same title is a 2.8 GB update from one build
      and a 160 GB one from the next. The feature must therefore announce the
      exact number before transferring anything, and say "full download" when
      that is what it is.

      Two further findings worth more than the feature:

      - **Do not implement this per-file.** On the real Expedition 33 plan,
        page-granular costs 1.65 GB and per-file costs 45.78 GB — 28× worse, and
        96.8% of a full download, because the largest single file is 12 GB.
        Per-file is what `xodus-cli` does today.
      - **The update key is the version, not the content id** — see below. That
        was a defect in the shipped code, not a future concern.

      The full specification — every offset, the plan format, the algorithm and
      its verification step — is in the workflow transcript rather than here;
      what belongs in a roadmap is the decision, which is: implement it, gated
      behind an honest byte count, after update detection works at all.

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

- **Two titles crash on a null pointer the runtime handed them, and both fixes
  are runtime-wide rather than per-title.** Neither is fixed: both need a change
  inside the Wine fork and a rebuilt runtime, and there is no mingw toolchain on
  the machine they were diagnosed on, so neither could be compiled or tested.

  **Age of Empires Definitive Edition** — diagnosed, and the fix is clear.
  Thread `0224` asks for a WinRT class the fork does not implement, and
  dereferences what it gets back:

  ```text
  0224:err:combase:RoGetActivationFactory Failed to find library for
        L"Windows.Foundation.Diagnostics.LoggingChannelOptions"
  wine: Unhandled page fault on read access to FFFFFFFFFFFFFFFF at
        address 0000000140392304 (thread 0224), starting debugger...
  ```

  Same thread, consecutive lines, requested IID
  `{a93151da-7faf-4191-8755-5e86dc65d896}`. Nothing in the tree implements
  `Windows.Foundation.Diagnostics` -- no IDL, no class -- so the fix is to host
  `LoggingChannel` and `LoggingChannelOptions` in an existing WinRT dll and
  no-op the logging, exactly the way
  `patches/wine/0004-regionpolicyevaluator-winrt-class.patch` hosts
  `RegionPolicyEvaluator` in `windows.system.profile.systemid`. Every title gets
  it, no recipe involved.

  **DOOM 64** — narrowed, not solved. It writes its own crash log: an access
  violation reading address 0 at `DOOM64_x64.exe+0x55b03`, with rax, rdx, rsi,
  rdi, r8 and r9 all zero. The GDK calls it makes before that all succeed --
  `XStoreQueryGameLicense{Async,Result}` returns an active perpetual licence,
  `XSystemGetXboxLiveSandboxId` returns `RETAIL`, `XGameGetXboxTitleId` answers
  -- so nothing obviously hands it the null.

  The one suspicious thing is `XUserRegisterForChangeEvent`, which it calls once
  and which is a no-op stub returning a token that never fires. No `XUserAdd`
  ever follows, so the title never gets a user. That is the same shape as a bug
  already fixed once here -- a `Register*Changed` stub that returned `S_OK`
  without delivering the current value left libHttpClient believing there was no
  network -- but it is a hypothesis, and saying so is the point.

- **An install can fail while the client exits zero.** A title the account is
  not licensed for prints `not entitled to this content` and returns success,
  leaving a directory holding a partial container. The launcher used to believe
  the exit code and record an install that never happened. It now checks the
  destination before recording, and a record for a directory that holds no
  install is treated as stale -- but the underlying behaviour is the client's,
  and this is a check around it rather than a fix.

- **Update detection does not work yet, and the reason was a wrong premise.**
  It compared *content ids*, on the belief that a rebuild produces a new one. It
  does not: 26 published patch plans spanning 26 builds of three titles carry
  three content ids, one per title. A content id names the package; the version
  names the build. Detection is keyed on the version now, but the *available*
  version is not published anonymously — the catalog reports `"0"` for
  everything — so every installed title honestly reads "update checking is not
  wired up yet" until `GetBasePackage` on update.xboxlive.com is called with an
  XSTS token. That endpoint also returns the patch plans, so it is the same
  piece of work as the item above.
- **There are no delta updates.** MSIXVC downloads are resumable but not
  differential, so "update" currently means re-downloading the title. That is
  ~2.5 GB for Bedrock but tens of gigabytes for a large title, which makes
  update detection close to useless until it is solved. Epic's chunked manifests
  give Heroic deltas for free; this has no equivalent yet.
