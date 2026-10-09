# Independent runtime delivery

The GUI and patched Proton runtime have separate release trains.  A GUI release
must not embed Proton; it discovers installed runtime directories, downloads the
latest compatible runtime on first start, and shows byte progress.  Runtime
archives remain immutable GitHub-release assets and install below the Ferestre
state directory, without replacing a runtime a running title uses.

Each title defaults to the newest compatible runtime.  A user override stores
the selected runtime path in the title override under the XDG config directory;
the launch plan exports that path as `XODUS_PROTON_DIR`.  The Runtime page lists
installed versions and exposes install/update actions.  A title picker offers
only runtimes whose declared capabilities satisfy that recipe; older installed
runtimes stay available for rollback.

Downloads validate the release archive layout and capability manifest before
activation, extract into a temporary directory, then rename atomically.  Tests
cover archive discovery, compatibility filtering, override serialization, and
the launch environment.  The Ubuntu VM test installs the published runtime,
selects it for DOOM 64, and launches it.
