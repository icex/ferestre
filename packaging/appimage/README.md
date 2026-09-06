# AppImage packaging

**This is the recommended first distribution format**, ahead of Flatpak, for one
reason: it does not sandbox anything.

## Why it fits better than Flatpak here

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

## What still has to be solved

- **libfuse2.** Classic (type 2) AppImages need it, and current Arch-family
  systems ship only libfuse3 — verified on the development machine, which has
  `libfuse3.so.3` and `fusermount3` but no libfuse2. Either ship a fuse3-capable
  runtime, or document `--appimage-extract-and-run`, or both. Do not make a
  first-time user debug a FUSE error.
- **Updates.** AppImage has no built-in update mechanism. zsync via
  AppImageUpdate is the usual answer; alternatively the launcher checks for its
  own new version, which it will already be doing for the runtime.
- **Desktop integration.** No `.desktop` entry or icon unless the user installs
  one, or the AppImage offers to. Fine for a first release.
- **GUI dependencies.** A CLI-only build is nearly dependency-free; a GTK4 or Qt
  GUI is what actually needs bundling, via linuxdeploy and its Qt/GTK plugins.

## Recommended order

1. **AppImage** — lowest risk, matches the tested configuration, no store review.
2. **AUR** — the development platform is Arch-based, and it is the cheapest route
   to real users who already run Proton-GE by hand.
3. **Flatpak** — best discovery, most work, and blocked on validating that Proton
   runs correctly inside a sandbox. See `../flatpak/README.md`.

The runtime is downloaded separately in every case (~264 MiB compressed) and
never bundled into the package.
