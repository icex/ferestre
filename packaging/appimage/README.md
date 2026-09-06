# AppImage packaging

**This is the recommended first distribution format**, ahead of Flatpak, for one
reason: it does not sandbox anything.

## Build one

```sh
packaging/appimage/build-appimage.sh
```

That is the whole thing. It downloads two pinned build tools on first run, then
produces `out/xgdk-<version>-x86_64.AppImage` (about 1.1 MB) and runs it twice
to prove it works before telling you it is done.

The build host needs `bash`, `curl` or `wget`, coreutils and network access the
first time. It does **not** need appimagetool, mksquashfs, linuxdeploy, FUSE, or
a Rust toolchain installed — see below for why not.

Useful options:

| | |
|---|---|
| `--bin PATH` | package a real launcher binary instead of the placeholder |
| `--out DIR` | where the `.AppImage` lands (default `out/`) |
| `--version V` | override the version string |
| `--app-id ID` | reverse-DNS id, for a fork publishing its own builds |
| `--update-info STR` | appimagetool `-u` string; also writes the `.zsync` |
| `--offline` | never touch the network; the tools must already be cached |
| `--skip-self-test`, `--keep-appdir` | for debugging the packaging itself |

`--help` lists the `XGDK_*` environment equivalents. Build tools are cached in
`${XDG_CACHE_HOME:-~/.cache}/xgdk-appimage`, so every build after the first is
offline-capable.

## What is in it, and what is not

```
AppRun                        four lines, and deliberately so (below)
usr/bin/xgdk                  the launcher, or the placeholder shell command
usr/lib/xgdk/scripts/         the launch path: launch-gdk.sh, proton-wine-shim.sh, …
usr/lib/xgdk/patches/         so install-runtime works, and RECIPES.md is followable
usr/share/{applications,icons,metainfo,doc}
```

**The patched Proton runtime is not in there.** It is ~264 MiB compressed,
versions independently, and would tie a 1 MB application to a 264 MB release
train. It is built or downloaded separately; `xgdk doctor` says whether it is
installed and `xgdk install-runtime` builds it.

Nor is `xodus-cli`. It is GPL-3.0, it needs a Rust toolchain and it is what
signs in — carrying it is a decision for the launcher, not for the packaging.
`doctor` reports it as a hard requirement.

## The placeholder

The launcher binary does not exist yet (`docs/ROADMAP.md`, phase 2). Until it
does, `build-appimage.sh` packages `xgdk-placeholder.sh`: a thin front end over
the shell scripts in `scripts/`, using the subcommand names the CLI is planned
to have.

```
xgdk doctor                      check this machine for everything a launch needs
xgdk titles                      what has a launch recipe
xgdk download <product-id>       download and decrypt a title you own
xgdk run bedrock                 launch it
xgdk install-runtime             build and install the patched Proton
xgdk stop                        kill everything a launch left behind
```

A placeholder build is marked as one: the version gets a `-placeholder` suffix,
the filename carries it, and the build prints a warning. Pass `--bin` (or build
`target/release/xgdk`, which is auto-detected) and it disappears.

Packaging the placeholder is the point. It means the packaging is tested rather
than described, and the day the binary exists nothing about this directory has
to change.

## The libfuse2 problem, and why this does not have it

Classic AppImages embed a runtime linked against libfuse2, which Arch-family
distributions have not shipped for years. The usual result is a first-time user
seeing `dlopen(): error loading libfuse.so.2` and giving up. Two halves, solved
separately:

**Building.** appimagetool is itself an AppImage. The build never lets it mount
itself: it is unpacked once with `--appimage-extract`, which is a plain
userspace squashfs read and needs no FUSE at all, and the unpacked tree is run
directly. That also puts appimagetool's own bundled `mksquashfs`, `zsyncmake`
and `desktop-file-validate` on `PATH`, which is why the build host needs none of
them. Verified by running a complete build, cold cache and all, with **no
`fusermount` binary anywhere on `PATH`**.

**Running.** The AppImage is built with `--runtime-file` against
[type2-runtime](https://github.com/AppImage/type2-runtime) `20251108`, which
links libfuse 3 statically and uses `fusermount3`. Verified on the development
machine, which has `libfuse3.so.3` and `fusermount3` and no libfuse2 or
`fusermount` at all: it mounts and runs.

Hosts with no FUSE whatsoever — most containers, some hardened setups — still
cannot mount anything, and no runtime choice fixes that. For those the answer is
`--appimage-extract-and-run`:

```sh
./xgdk-*.AppImage --appimage-extract-and-run doctor
```

The build's self-test tries both paths and says which worked, so this is
reported at build time rather than discovered from a stack trace.

Both build tools are pinned to a tagged release **and** a SHA-256. Upstream has
been known to re-upload assets under an existing tag; when that happens the
build stops with the expected hash, the hash it got, and the three ways to
proceed, instead of quietly building against something else.

## Why AppImage fits better than Flatpak here

The runtime has to run on the host. Measured: the binaries under the runtime's
`lib/` pull in **66 distinct host libraries** between them — Vulkan, X11 and
Wayland, audio, and the rest of a graphics stack. `wine` itself only needs libc,
but the stack around it does not.

Inside a Flatpak, Proton runs as a child of the launcher and therefore against
the *Flatpak runtime's* copies of those libraries rather than the host's. That is
66 opportunities for a mismatch with a graphics stack the project has never
tested against. An AppImage has no sandbox: the launcher runs on the host and
spawns Proton on the host, which is exactly the configuration every bit of
testing here has used.

The corollary is worth stating plainly: **the launcher's packaging barely
matters; what matters is that Proton runs on the host.** Choose formats by
whether they get in the way of that.

## Two things the AppDir gets deliberately wrong-looking

**`AppRun` exports almost nothing.** The conventional AppRun sets
`LD_LIBRARY_PATH=$APPDIR/usr/lib` so a bundled toolkit is found. This one must
not: every variable it exports is inherited by the Proton process tree, and a
bundled library winning over the host's is a rendering or audio bug three layers
away from anything that looks like packaging. `scripts/launch-gdk.sh` already
spends fifteen lines unsetting what Steam injects for the same reason; do not
give it more to undo.

**The launcher must stay in the foreground.** An AppImage is mounted for exactly
as long as its process lives, and `usr/lib/xgdk/scripts` lives on that mount —
including `proton-wine-shim.sh`, which `xodus-cli` execs as its "wine" binary.
A launcher that daemonised itself and exited would pull the shim out from under
the running game.

## Still open

- **Desktop integration.** The AppImage carries a `.desktop` entry, a 256px
  icon and AppStream metainfo, but nothing installs them unless the user runs
  `appimaged` or Gear Lever. Fine for a first release; offering to install a
  launcher entry is a launcher feature, not a packaging one.
- **Updates.** Pass `--update-info` and the build emits a `.zsync` alongside, so
  AppImageUpdate works. Nothing is set by default because the update string
  names a specific repository and release, which a fork must not inherit. The
  launcher will be checking for its own runtime updates anyway, and can check
  for itself at the same time.
- **A GUI build.** A CLI-only AppImage is nearly dependency-free. A GTK4 or Qt
  GUI is what actually needs bundling, via linuxdeploy and its toolkit plugins,
  and that is when this script grows a second act.
- **Signing.** `--sign` passes through to appimagetool's gpg2 signing. No key
  and no published fingerprint exist yet, so it is off by default.

## Recommended order

1. **AppImage** — lowest risk, matches the tested configuration, no store review.
2. **AUR** — the development platform is Arch-based, and it is the cheapest route
   to real users who already run Proton-GE by hand.
3. **Flatpak** — best discovery, most work, and blocked on validating that Proton
   runs correctly inside a sandbox. See `../flatpak/README.md`.

## Licensing note

The AppImage carries the patch series, which is not all MIT: `patches/wine` and
`patches/xgameruntime` are LGPL-2.1-or-later and `patches/xodus-cli` is GPL-3.0.
They travel as source diffs, which is aggregation and satisfies both. `NOTICE`,
`LICENSE` and `LICENSE.LGPL-2.1` ship in `usr/share/doc/xgdk/` so the map is
inside the artefact and not just in the repository.

The application id defaults to `io.github.icex.xgdk`, derived from this
project's own repository. A fork that publishes its own builds should pass
`--app-id`; the build rewrites the desktop entry, the metainfo and every
filename to match rather than shipping someone else's id.
