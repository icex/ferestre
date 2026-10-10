# Launching titles without a tested recipe

An installed MSIXVC title can be launched from its package manifest. The launcher
writes a personal recipe with the declared executable and installation directory;
it does not label an automatically detected title playable. Newly detected
packages also declare the minimum loader requirements, and GDK packages require
package identity and user support.

At each launch, engine files are inspected beside the executable and at the
package root. A complete Ultralight/WebCore library set enables an optional
allocator lifetime profile. When the selected runtime provides
`d3d12.recording-allocator-lifetime`, the launcher adds
`VKD3D_CONFIG=retain_recording_allocators`. A runtime without that capability is
never sent an unknown setting. Explicit recipe or process `VKD3D_CONFIG` values,
including an empty value, take precedence over automatic defaults.

This profile keeps one retired allocator alive per affected command list until
its next successful reset or destruction. It does not change public COM reference
counts and does not retain ordinary allocators. A recording whose allocator is
lost without the compatibility setting is invalidated, so closing it fails
instead of submitting a null Vulkan command buffer.

GDK 2504+ save-interface compatibility is a runtime fix shared by titles, rather
than an executable-name workaround. The full fifty-slot interface layout was
checked against the public [Microsoft GDK 2510 package](https://www.nuget.org/packages/Microsoft.GDK.PC/2510.3.6286).
Legacy local save operations are implemented; the seventeen added PlayFab cloud
operations return `E_NOTIMPL`. Recognizing the interface does not claim cloud
uploads or sync UI support.

Retro Classics reached its authenticated catalogue and a Tennis stream with
these fixes. The previous runtime crashed on a null command buffer. A focused
D3D12 test verifies the public release count, successful close with a retained
buffer, reset to a replacement allocator and final device-reference cleanup.
Broader game/controller coverage and save restoration are not claimed by that
single streamed-game test.

New profiles should follow the same boundary: identify an installed engine,
require a measured runtime capability, respect user configuration, and validate
both the failure and the resulting user-visible behavior. Do not guess executable
names, replace licensed images on disk, silently run arbitrary installers or
report an unimplemented service as successful.
