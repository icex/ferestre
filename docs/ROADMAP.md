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
- [ ] Replace prose recipes with a **declarative per-title manifest** (product
      id, executable path, prefix layout, environment, DLL overrides, required
      GDK features, known issues). The launch surface turns out to be small —
      `scripts/launch-gdk.sh` is 75 lines and most of it is undoing environment
      that Steam injects — so this is a modest schema, not a framework.
- [ ] CI running the synthetic suites on every change

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

## Phase 2 — the launcher

Smallest useful version, in order:

1. Sign in and list what the account owns
2. Download with resume, and detect updates
3. Install into a predictable layout, one Wine prefix per title
4. Launch using the title's manifest
5. Save management, then uninstall

Two decisions worth stating up front:

- **Steam integration is a toggle, not a policy.** By default the launcher runs
  an isolated runtime and strips Steam's injected environment, because the
  overlay, Fossilize capture and `STEAM_COMPAT_*` collide with a Proton instance
  we start ourselves. But people add these titles as non-Steam shortcuts for the
  overlay, controller config and Remote Play, so keeping that environment must
  be an option per title rather than something the launcher decides.
- **The client is carried, not assumed.** Because launching requires the
  decrypt path, "install Xodus separately" is not a real option for an end user.

## Phase 3 — packaging

- AUR first: the development platform is Arch-based and it is the cheapest way
  to get real users.
- AppImage or Flatpak for everyone else. Whether a public Flatpak remote is
  appropriate is a posture decision, not a technical one — see below.
- The runtime is downloaded, never bundled into the launcher package.

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
