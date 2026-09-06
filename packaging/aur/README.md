# AUR packaging

The development platform is Arch-based, so this is the cheapest route to real
users — people who already install Proton-GE by hand and will not be surprised
by a compatibility tool showing up in `compatibilitytools.d`.

## Three packages, not one

| package | what it is | size | changes when |
|---|---|---|---|
| [`ferestre-runtime-bin`](ferestre-runtime-bin/) | the patched Proton, prebuilt, into `/usr/share/steam/compatibilitytools.d/ferestre` | ~264 MiB download, ~1.4 GB installed | Wine or `xgameruntime.dll` changes |
| [`ferestre-client`](ferestre-client/) | the Xodus client with our patches: `xodus-cli`, `xodus-service` | ~15 MB | the fork rebases |
| [`ferestre-git`](ferestre-git/) | the front end: `ferestre`, the launch scripts, the docs | ~400 KB | constantly |

`ferestre-git` depends on the other two, so `paru -S ferestre-git`
installs the lot. The other two are independently useful: the runtime is a
working Proton build on its own, and the client downloads titles without any of
the rest.

### Why not one package

Three release trains with wildly different sizes and cadences. A single package
would mean a 264 MiB download to fix a typo in a shell script, and a full
container build of Wine to change a dependency. The roadmap already treats them
as three, for the same reason.

### Why the client is separate, and not folded into the launcher

This is the split the task actually turns on, so the reasoning, in order of
weight:

1. **Licence.** Xodus is GPL-3.0; the launcher is MIT. The whole architecture
   rests on the launcher *shelling out* to `xodus-cli` rather than linking its
   crates. Keeping the GPL-3.0 binary in its own package, with its own `license`
   field and its own source, keeps that separation visible in the packaging
   instead of only in a document. Aggregation would be legal in one package —
   this is about it staying obvious.
2. **Build cost.** The client links `webkit2gtk-4.1` (its Microsoft sign-in
   window is a `wry`/`tao` webview). The launcher links nothing. Nobody should
   rebuild webkit-linked Rust to pick up a launcher fix.
3. **Provenance.** It is someone else's project, carried as a fork that
   [cannot be upstreamed](../../NOTICE). It moves on its own schedule.

The counter-argument is real: three packages is more to keep in step, and the
client is useless on its own. That is why `ferestre-git` hard-depends on
both, so nobody has to know the structure to install it.

## Where things go

```
/usr/bin/ferestre                                   front end (placeholder today)
/usr/bin/xodus-cli, /usr/bin/xodus-service      client
/usr/lib/ferestre/scripts/                          the launch path
/usr/lib/ferestre/patches/                          read by `ferestre install-runtime`
/usr/lib/ferestre/VERSION
/usr/share/steam/compatibilitytools.d/ferestre/     the runtime
/usr/share/doc/ferestre/                            README, NOTICE, RECIPES, ROADMAP
```

The layout under `/usr/lib/ferestre` is the AppImage's AppDir layout, deliberately:
`packaging/appimage/ferestre-placeholder.sh` finds its scripts through
`$0/../lib/ferestre/scripts`, so the same command behaves identically whether it
came from an AppImage or from pacman. **When `build-appimage.sh` starts staging
another directory, stage it here too** — `titles/` is the obvious next one.

### One thing the packaging cannot do for you

`scripts/xodus-env.sh` looks for the runtime in Steam's *per-user*
`compatibilitytools.d`, not in `/usr/share/steam`. So a packaged runtime is not
auto-detected, and both `.install` files say:

```sh
export XODUS_PROTON_DIR=/usr/share/steam/compatibilitytools.d/ferestre
```

Adding `/usr/share/steam/compatibilitytools.d/ferestre` to the `_first_existing`
probe list in `xodus-env.sh` would delete that manual step for every packaging
format at once. That file belongs to `scripts/`, so it is a request, not a
change made here.

## How the dependency lists were derived

Not copied from another Proton package. For the runtime, every ELF file under
`files/` was read for its `NEEDED` entries, the sonames the tree satisfies
itself were subtracted, and the remainder resolved to packages:

```sh
D=/usr/share/steam/compatibilitytools.d/ferestre
find "$D/files" -type f \( -name '*.so*' -o -perm -u+x \) |
  while read -r f; do
    case $(file -b "$f") in *"ELF 64-bit"*)
      objdump -p "$f" 2>/dev/null | awk '/NEEDED/{print $2}' ;;
    esac
  done | sort -u > needed
find "$D/files" -name '*.so*' -printf '%f\n' | sort -u > provided
comm -23 needed provided        # what the host has to supply
```

That leaves 50 sonames. `NEEDED` misses anything `dlopen`ed, which for Wine is
most of the interesting parts — Vulkan, fontconfig, freetype, the X extension
libraries, gnutls, SDL, CUPS — so the `lib/wine/x86_64-unix/*.so` files were
also scanned for soname-shaped strings and attributed to the module that dlopens
them. The PKGBUILD comments name the consumer for anything non-obvious.

For the client the same thing on the two built binaries: `objdump -p
target/release/xodus-cli | grep NEEDED` gives 17 entries, `xodus-service` 7.

Redo this after any runtime rebuild. It takes a minute and it is the difference
between a dependency list and a guess.

### Three sonames deliberately absent

| soname | wanted by | why not a dependency |
|---|---|---|
| `libnettle.so.8` | gstreamer `hlsdemux` | Arch ships `libnettle.so.9`. Listed as an optdepend on the AUR's `nettle3`. |
| `libvpx.so.9` | gstreamer `vpx` | Arch ships `libvpx.so.12`. Nothing in the repositories provides 9. |
| `libavcodec.so.58` (32-bit) | 32-bit `winedmo`, `libgstlibav` | `lib32-ffmpeg4.4` does not exist. See below. |

The first two are optional GStreamer plugins: the plugin fails to load, and
nothing a supported title does touches those codecs. A dependency that cannot be
installed is worse than a documented gap.

### The 32-bit half

The tree ships a complete i386 Wine (`files/lib/i386-linux-gnu`,
`files/lib/wine/i386-unix`) whose 45 host sonames would need the `lib32-*`
mirror of everything above — and one of them, `lib32-ffmpeg4.4`, is not in the
repositories at all, so the set cannot be completed.

It is not in `depends` because nothing supported needs it: `files/bin/wine` and
`files/bin/wineserver` are x86_64, and every MSIXVC title tested here is a
64-bit executable. A title that shells out to a 32-bit helper would find a
32-bit Wine that cannot load its media stack. If one ever turns up, that is the
moment to decide whether to ship the 32-bit tree at all.

## Cutting a runtime release

`ferestre-runtime-bin` is the only package with an artefact to publish, and its
PKGBUILD assumes a shape. The contract:

- **Tag** `runtime-<pkgver>`, where `pkgver` is `<proton major>.<minor>.<date>`
  — `11.0.20260803`. Upstream's own version string
  (`xodus-bleeding-edge-11.0-20260803-3-g7c0b4354`, in the tool's `version`
  file) cannot be a `pkgver`: hyphens are not allowed.
- **Asset** `ferestre-runtime-<pkgver>-x86_64.tar.zst`, containing exactly one
  top-level directory named `ferestre-runtime-<pkgver>`, which *is* the
  compatibility tool: `proton`, `toolmanifest.vdf`, `compatibilitytool.vdf`,
  `version`, `files/`.
- The `compatibilitytool.vdf` in the asset should name the tool `ferestre`, not
  `xodus-proton`. The PKGBUILD does not rewrite it.
- `package()` refuses to build if `proton` lacks the `close_fds=False` patch or
  if `xgameruntime.dll` is missing. Both have been shipped broken before by
  hand; both look like the title crashing rather than like a packaging mistake.

`scripts/package-runtime.sh` produces exactly that shape from an installed
runtime, and asserts every point of the contract above before writing the
tarball. It also slims and strips: 1416 MB installed becomes 968 MB, 187 MB
compressed. Run it, then upload `out/*.tar.zst` as the release asset.

Then, in `ferestre-runtime-bin/`:

```sh
updpkgsums                        # replaces the sha256sums SKIP placeholder
makepkg --printsrcinfo > .SRCINFO
```

**`sha256sums=('SKIP')` is a placeholder and must not reach the AUR.** No
release exists yet, and inventing a hash would fail later with a confusing
message rather than a clear one.

## Publishing

Each directory here becomes its own AUR git repository — the AUR has no concept
of a monorepo, and a `PKGBUILD` cannot reference a file outside its own
directory. That is why `ferestre-client` fetches its patches by URL from a pinned
commit of this repository rather than reaching up the tree.

```sh
git clone ssh://aur@aur.archlinux.org/ferestre-runtime-bin.git
cp packaging/aur/ferestre-runtime-bin/{PKGBUILD,.SRCINFO,*.install} ferestre-runtime-bin/
cd ferestre-runtime-bin && git add -A && git commit && git push
```

Keep this directory the source of truth and copy outward, or the two drift.

Before pushing anything:

```sh
makepkg --printsrcinfo > .SRCINFO   # .SRCINFO must match the PKGBUILD exactly
namcap PKGBUILD                     # not installed on the dev machine; install it
makepkg -f && namcap ./*.pkg.tar.zst
```

Expect namcap to complain about the sonames listed above as "dependency
detected and not included". Those are the documented gaps, not oversights.

## The client fork, pinned

**This package cannot build today.** Its patch sources are raw URLs into this
repository, and this repository is private, so `makepkg` gets a 404 for every
one of them. That is a visibility setting, not a packaging problem, and nothing
else in `packaging/` depends on it.

`ferestre-client` does not track upstream. It pins upstream commit `3e75c9f` and
applies four of the five patches in `patches/xodus-cli/`. That is not
arbitrary — it was verified by applying them, in order, into a clean worktree
at that commit, and type-checking the result:

- upstream `3e75c9f` + `0002` reproduces the fork's working tree **byte for
  byte, except `Cargo.lock`** (and the patch's lock is the better of the two: it
  carries the `p256`/`rand_core`/`base64` entries the fork's committed lock is
  missing, so `cargo build --locked` works after patching).
- `0003`, `0004` and `0005` apply cleanly on top of that, in that order.
- `0001` is an earlier export of work `0002` already contains. Applying both
  fails. It is not in the source array.

`0004` had the same disease as `0001` and was re-exported to cure it: it was cut
from a commit that had swept in the then-uncommitted collections work, so it
carried `0003`'s changes and could not apply after it. Exporting a patch from a
dirty tree is how this series keeps acquiring overlaps, and applying it into a
clean worktree is the only check that catches them.

Two consequences worth stating plainly. First, `patches/xodus-cli/` is not a
clean series against one base — `0001` applies to today's upstream `main`,
`0002` and `0003` to a base three commits older — and packaging it is what
exposed that. Second, bumping the pin means rebasing the patches by hand, which
is the maintenance cost the roadmap's open question 3 ("how the client is
carried — vendored, submodule, or a maintained fork") is really about. **When
the fork becomes a repository, this PKGBUILD loses `prepare()` and both patch
sources and becomes six lines shorter.**

## What is not done here

- **No release exists**, so `ferestre-runtime-bin` cannot be built as written. Its
  checksum is a placeholder and everything else about it is real.
- **The client was not built.** Patch application was verified; the compile was
  not. `0003` in particular arrived as a patch file with no corresponding commit
  in the client checkout.
- **No `namcap` run.** It is not installed on the development machine.
- **No systemd user unit for `xodus-service`.** `launch-gdk.sh` starts it with
  `nohup` if `pgrep` does not find it, and a unit would be a second way to do
  the same thing. Worth revisiting when the launcher stops being shell.
- **No `ferestre` (tagged, non-`-git`) package.** Nothing to tag yet.
