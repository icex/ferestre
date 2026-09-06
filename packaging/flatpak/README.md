# Flatpak packaging

Draft. The launcher does not exist yet, so this records the constraints that
packaging has to satisfy rather than pretending to be a working manifest.

## Why Flatpak is awkward here, specifically

Most Linux game launchers run Proton through
[umu-launcher](https://github.com/Open-Wine-Components/umu-launcher), which sets
up a Steam Runtime container with pressure-vessel. Heroic's Flatpak works this
way. **We cannot.**

A GDK title's executable is encrypted on disk and only ever exists decrypted
inside a memfd, handed to Wine as an inherited file descriptor. Inherited fds do
not survive pressure-vessel's container layer — tested directly against the
entry point, so the result is about pressure-vessel and not about umu. Lose the
fd and there is no executable, for every title.

So the Flatpak has to run Proton **directly**, against the libraries the Flatpak
runtime provides, rather than inside a Steam Runtime container. That is known to
work outside Flatpak — all of this project's testing runs Proton directly on a
modern distribution with no Steam Runtime involved — but it has not been
validated inside a sandbox, and it is the single largest risk in this path.

How large: the binaries under the runtime's `lib/` pull in **66 distinct host
libraries** between them. Inside a Flatpak every one of those resolves against
the runtime's copy instead of the host's, on a graphics stack this project has
never tested. That is why `../appimage/` is the recommended first format — it
has no sandbox, so Proton runs exactly as it does today.

There is an escape hatch, with a cost: the launcher could run the game via
`flatpak-spawn --host`, putting Proton back on the host libraries. It needs
`--talk-name=org.freedesktop.Flatpak`, which is effectively full host access and
which Flathub scrutinises heavily — at which point the sandbox is providing
little, and an AppImage is the more honest package.

## Constraints the manifest must meet

| Need | Why | Permission |
|---|---|---|
| Network | Sign-in and download | `--share=network` |
| GPU | The titles render | `--device=dri` |
| Audio | ditto | `--socket=pulseaudio` |
| Secret Service | The client stores account tokens in the keyring, and aborts outright without one | `--talk-name=org.freedesktop.secrets` |
| A games directory | Installs are tens of gigabytes and belong outside `~/.var` | `--filesystem=` a user-chosen path |

The keyring one is not cosmetic: the client currently calls
`init_secrets().expect(...)`, so a session with no Secret Service — a headless
box, or Steam Deck Gaming Mode — does not degrade, it panics. That has to be
fixed before a Flatpak is usable, not after.

## The runtime is not bundled

The patched Proton runtime is ~264 MiB compressed and versions independently of
the launcher. It is downloaded on first use into the user's data directory, not
shipped inside the Flatpak — the same arrangement Heroic uses for Proton-GE, and
the reason the launcher and runtime can release on different schedules.

## Flathub, honestly

Flathub is plausible but not automatic. Heroic and mcpelauncher are both on it,
and both install content the user already owns, so the shape is not unprecedented.
What a submission would have to satisfy:

- An application id under a domain the submitter controls. The project uses
  `io.github.icex.ferestre`, matching its GitHub home and the AppImage build.
- A manifest that builds from source, with the runtime download declared as
  `extra-data` rather than fetched silently at first run.
- No game content, no keys, no licences in the package — which is already the
  rule here.
- An appstream metainfo file with screenshots and a clear description of what
  the user must own.

If Flathub declines, a self-hosted OSTree remote is the fallback, and an AppImage
covers people who want no store at all.
