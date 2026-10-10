# Launching titles without a tested recipe

`ferestre run` can start an installed title nobody has written a recipe for, and
can give it fixes that were found on a different title built on the same
engine. Neither makes the title tested: a recipe written this way says
`untested`, and nothing below changes that.

## Recipes written from a package

When no recipe matches an installed title, the launcher writes a personal one
from the package: the executable its manifest declares and the directory it is
installed in. It also records what the title will ask of the runtime:

| | when | why |
|---|---|---|
| `requires = loader.memfd-main-image` | always | every package is an encrypted image that only starts from the memfd |
| `requires = xgameruntime.user` | the package has `MicrosoftGame.config` (any case) | a GDK title signs its user in through the runtime |
| `wants = appmodel.package-identity` | the same | see below |
| `wants` from an engine profile | an engine is recognised | see the next section |

Package identity is wanted rather than required because a runtime that
publishes no capability list is probed, and the probe can see the
inherited-fd change in `proton` and the presence of our `xgameruntime.dll`,
but never package identity. A requirement the probe cannot see would stop the
launcher selecting that runtime at all, before it could say "could not be
verified" and launch anyway.

## Engine profiles

An engine is recognised from its libraries, not from a title list. The launcher
looks in two directories only: the package root and the directory of the
executable it launches. File names compare case-insensitively, and every
library of a profile has to be in the same one of those directories. An
executable path that leaves the package (`..`, or absolute) is not followed;
the package root is still examined.

| profile | libraries | capability | setting |
|---|---|---|---|
| Ultralight | `Ultralight.dll`, `UltralightCore.dll`, `WebCore.dll` | `d3d12.recording-allocator-lifetime` | `retain_recording_allocators` in `VKD3D_CONFIG` |

A recognised engine adds its capability to the recipe's `wants`. The CLI and
the window assess that same recipe, so they agree about the runtime.

At launch, the setting is decided in this order:

1. The value in effect is the recipe's `[launch] env` entry when it has one,
   otherwise the launcher's own environment. (A recipe's entry replaces the
   inherited variable at launch; it does not extend it.)
2. Set and empty, in either place: an explicit opt-out. Nothing is added.
3. Already carrying the token (`,` and `;` separate entries, as vkd3d-proton
   reads them): nothing to add.
4. The selected runtime does not provide the capability: nothing is added. A
   runtime that publishes no capability list never provides it, because the
   probe cannot see it.
5. Otherwise the token is appended to the value in effect, or becomes the
   value when there was none. Existing flags are kept.

The launch prints one `-- detected Ultralight: ...` line saying which of these
happened.

## What `retain_recording_allocators` does

Ultralight releases its last reference to a command allocator while a command
list recorded from it is still open, then closes the list. The allocator owns
the list's Vulkan command buffer. The vkd3d-proton patch
(`patches/vkd3d-proton/`) handles that in two ways:

- **With the setting**, the list keeps one internal reference to that
  allocator until its next successful `Reset` onto another allocator, or until
  it is destroyed. Public COM reference counts do not change, and allocators
  released at any other time are not retained.
- **Without it**, the list is invalidated when its allocator goes away, and the
  log says so once per list, naming the setting. `Close` then fails instead of
  ending a null `VkCommandBuffer`. Recording calls the engine makes between the
  release and `Close` are not protected; the setting is the fix, the
  invalidation only stops the crash at `Close`.

## The newer save interface

Retro Classics also asks the runtime for `IXGameSaveImpl4` before its legacy
local save calls. That is a runtime fix shared by every title, not a profile:
the runtime exposes the complete fifty-slot interface, with slot numbers taken
from the Microsoft.GDK.PC 2510.3.6286 XGameSave and PFXGameSave headers; the
boundaries between the legacy and PlayFab methods are pinned by
`tests/xgr_tests.c`. Legacy local saves work through it. Its
seventeen PlayFab cloud methods return `E_NOTIMPL`; cloud saves and the sync
UI are not implemented.

## Evidence

- Retro Classics: the player confirmed it playable on 2026-10-10. An isolated
  run reached the authenticated catalogue and streamed Tennis. Broader game and
  controller coverage and save restoration are not separately qualified.
- `tests/d3d12_allocator_lifetime.c` reproduces the allocator release in both
  modes. It needs a GPU, so it is run by hand; `tests/run-tests.sh` only
  compiles it.
- `tests/xgr_tests.c` checks the save interface's layout, a container round
  trip through it, and that every cloud method fails.
