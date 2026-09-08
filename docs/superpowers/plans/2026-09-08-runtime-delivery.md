# Independent Runtime Delivery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use subagent-driven-development (recommended) or executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver and select patched Proton runtimes independently of the GUI.

**Architecture:** Runtime archives are fetched from immutable GitHub releases into the Ferestre state directory. `ferestre-core` discovers and validates each installed runtime; user recipe overrides select one compatible runtime for a title and launch exports it.

**Tech Stack:** Rust, GTK4/libadwaita, GitHub release HTTP API, existing XDG paths.

**Spec:** `docs/superpowers/specs/2026-09-08-runtime-delivery-design.md`

## Global Constraints

- Runtime downloads must be portable and never contain personal account data.
- A running title's runtime directory is never replaced.
- Only a runtime satisfying a recipe's required capabilities can be selected.

---

### Task 1: Runtime inventory and installation

**Files:** Modify `crates/ferestre-core/src/runtime.rs`, `crates/ferestre-core/src/paths.rs`, `crates/ferestre-cli/src/main.rs`.

- [ ] Write failing tests for discovering multiple state-directory runtimes, rejecting an invalid archive layout, and choosing the newest compatible runtime.
- [ ] Implement `runtime::installed(paths) -> Vec<InstalledRuntime>` and `runtime::select(recipe, runtimes, requested) -> Result<InstalledRuntime>`.
- [ ] Implement download-to-temporary, archive validation, atomic extraction, and a CLI `install-runtime` path that emits existing progress lines.
- [ ] Run `cargo test -p ferestre-core -p ferestre-cli` and commit `Deliver versioned runtime archives`.

### Task 2: Per-title runtime override

**Files:** Modify `crates/ferestre-core/src/recipe.rs`, `crates/ferestre-core/src/launch.rs`, `crates/ferestre-core/src/paths.rs`.

- [ ] Write failing recipe round-trip and launch-plan tests for a runtime-path override.
- [ ] Add an optional user runtime selection field, load it only from the XDG override recipe, and make the launch plan set `XODUS_PROTON_DIR` to the selected runtime.
- [ ] Refuse a selection missing required capabilities with the named requirement.
- [ ] Run core tests and commit `Select a runtime per title`.

### Task 3: Runtime UI

**Files:** Modify `crates/ferestre-gui/src/state.rs`, `crates/ferestre-gui/src/model.rs`, `crates/ferestre-gui/src/main.rs`.

- [ ] Write failing model tests for compatible choices, default selection, and an incompatible runtime hidden from a title.
- [ ] Render installed runtime versions and update/install controls on Runtime; render a title runtime dropdown that persists the override through the existing editor path.
- [ ] Use `spawn_cli_tracked` progress for downloads and refresh the model after success.
- [ ] Run `cargo test --workspace`, GTK smoke test, and commit `Choose title runtimes in the GUI`.

### Task 4: Packaging and end-to-end verification

**Files:** Modify `.github/workflows/release.yml`, `packaging/appimage/build-appimage.sh`, `packaging/appimage/AppRun`, `docs/ROADMAP.md`.

- [ ] Write a packaging assertion that the AppImage does not contain `usr/lib/ferestre/runtime` and still resolves its client.
- [ ] Remove embedded runtime staging, retain client bundling, and make first start install the published runtime with progress.
- [ ] Run workspace tests, patch checks, AppImage self-test, and install the release candidate in Ubuntu VirtualBox; verify runtime download and DOOM 64 launch.
- [ ] Commit `Decouple runtime releases from the AppImage`.
