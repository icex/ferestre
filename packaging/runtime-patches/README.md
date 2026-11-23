# Runtime patches

`scripts/build-runtime.sh` applies these to the Proton checkout after the
source graph has passed the lock check and before `configure.sh`: every
`<submodule>/*.patch`, in file-name order, with `git apply` inside
`<work>/proton/<submodule>`. A patch that does not apply stops the build.
`scripts/runtime-inputs.py` hashes each file, so a changed patch is a new
runtime digest.

## dxvk

The DXGI fixes GE-Proton carries in `patches/dxvk/` at GE-Proton11-11
(`c0c2ee35e6d4d3bff4d39c6246ac46636434feb8`), applied in its
`protonprep-valve-staging.sh` order, and a memory-budget cap:

| Patch | From |
| --- | --- |
| 01-layered-overlay-dxvk | GE-Proton |
| 02-dxvk-preserve-swapchain-on-acquire-backpressure | GE-Proton |
| 03-dxgi-defer-initial-fullscreen-for-probe-swapchains | GE-Proton |
| 04-dxgi-follow-d3d12-fullscreen-client-resizes | GE-Proton, ported to the pinned DXVK (`Present` and `ResizeBuffers` differ) |
| 05-dxgi-keep-fullscreen-on-focus-loss | GE-Proton |
| 06-black-desert-keep-fullscreen-on-focus-loss | GE-Proton |
| 07-assassins-creed-keep-fullscreen-on-focus-loss | GE-Proton |
| 08-dxgi-respect-output-hdr-state | GE-Proton |
| 09-dxgi-auto-detect-hdr | GE-Proton |
| 10-starcraft2-disable-constant-buffer-streaming | GE-Proton |
| 11-dxgi-memory-budget | MW4-Proton 0.1.0-beta.3: `dxgi.maxDeviceMemory` also caps the budget `QueryVideoMemoryInfo` reports |

## vkd3d-proton

| Patch | From |
| --- | --- |
| 01-aligned-heaps | MW4-Proton 0.1.0-beta.3: a small heap that asks for 4 MiB alignment gets its own allocation instead of a 4 MiB slot in a shared pool, which kept most of that VRAM unused |
