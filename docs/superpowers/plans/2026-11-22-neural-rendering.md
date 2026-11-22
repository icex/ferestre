# Neural Rendering (DLSSNR-AMD) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An experimental per-game "neural rendering" switch that installs DLSSNR-AMD (a Linux package Ferestre builds from a pinned upstream commit and hosts as a release asset) into a game's executable folder, with the NVIDIA model taken from the person's own `nvngx_dlssnr.dll`, gated on anti-cheat, re-checked before every launch, and fully reversible.

**Architecture:** A new `ferestre_core::neural` module owns everything that is not UI: the pinned package (download, verify, unpack into `state/dlssnr/packages`), the model cache (`state/dlssnr/models`, filled once from an accepted DLL found in a fixed discovery order), route detection from the game's files (a minimal PE reader plus upscaler file names), the per-game state file (`config/neural-rendering.toml`), and the install itself, which backs up what the route would overwrite and then drives upstream's own `install.sh` non-interactively. The CLI adds `ferestre neural …` and wires the launch (in `launch_prepared`: re-check anti-cheat, re-verify/reinstall, then merge the env only when that check says the mod is present and allowed; a dry run previews it read-only from `prepare_launch`) and the removal of a local or Xbox title; the GUI adds a per-game dialog and a Settings field, both through the CLI or the core. The package is built by `scripts/build-dlssnr.sh` in an `ubuntu:24.04` container (the image upstream's own release CI uses) and published by `.github/workflows/dlssnr.yml` on a `dlssnr-<version>` tag.

**Tech Stack:** Rust 1.88 (serde, toml 0.8, sha2, zip 2, ureq 3 already in `ferestre-core`; no new crates), GTK4/libadwaita, bash + Docker/Podman for the package build, GitHub Actions, Python 3 for repository tests.

**Spec:** `docs/superpowers/specs/2026-11-22-local-games-and-neural-rendering-design.md` (section B, plus its Non-goals, Anti-goals and Constraints).

**Builds on PR A** (`docs/superpowers/plans/2026-11-22-local-games.md`). This plan uses exactly these items from PR A and nothing else:
- PR A Task 6: `ferestre_core::anticheat::detect(game_dir: &Path, executable: &str) -> Vec<Detection>` with `#[derive(Debug, Clone, PartialEq, Eq)] pub struct Detection { pub name: &'static str, pub kernel: bool, pub proton_runtime: bool, pub evidence: PathBuf }`. Tests build detections with a helper `fn det(name: &'static str, kernel: bool, proton_runtime: bool) -> Detection` (evidence `PathBuf::new()`), so a later field change touches one helper per crate.
- PR A Task 2/7: `Paths::steam_roots()` and `steam::libraries(root: &Path) -> Vec<PathBuf>` (root first, every library of `libraryfolders.vdf`, deduplicated). This plan adds no Steam library reader of its own.
- PR A Task 12: the local branch of `uninstall_title` that calls `local::forget` and returns before `install::forget` (Task 7 hooks both paths).
- A local recipe resolves through the existing `Paths::install_dir(&recipe)` to its absolute game folder. `recipe.launch.executable` is relative to it (Windows or POSIX separators) **or absolute** (PR A writes an absolute path when a shortcut's exe is outside its StartDir). The mod always goes into the executable's own folder, which `apply::exe_dir` computes for both forms (tested in Task 6). Nothing here reads PR A's `kind` field.

**Spec deviation needing the spec owner's sign-off before Task 4 is executed:** discovery step 3 ("inside an installed Ferestre Xbox package … decrypted with the person's own licence through `xodus-cli`") is implemented as a scan of the files the install already decrypted with that licence, not as a new `xodus-cli` call; see "Xbox packages and `xodus-cli`" for why no `xodus-cli` command can do it today. The follow-up (a `streaming --only <path>` fetch in the `icex/xodus` fork, then a fourth `dll::Step`) is tracked as a ROADMAP line in Task 8. If the owner rejects this, Task 4 stops at the `Step::Xbox` scan and waits for that fork change; nothing else in this plan depends on it.

## Global Constraints

- Base `origin/main`; PRs target `main`. Rust 1.88, `cargo test --locked --workspace` (cargo is `$HOME/.cargo/bin/cargo`). `cargo fmt --all --check` and `cargo clippy --locked --workspace --all-targets -- -D warnings` must pass; every new shell script passes `bash -n` and `shellcheck --severity=error`.
- No hardcoded machine paths; everything derives from `Paths`.
- Recipes keep `deny_unknown_fields`; this plan adds no recipe key and no `[[setup]]` action (so `titles/validate.py`, `titles/SCHEMA.md` and the setup-action test are untouched).
- `ferestre-core` never links the Xodus client (runs it as a child). This plan does not run it at all (see "Xbox packages" below).
- **No URL for the DLL is ever written into this repository.** The only DLL URL comes from `[dlssnr] dll-url` in `~/.config/ferestre/settings.toml` or `FERESTRE_DLSSNR_DLL_URL`. No NVIDIA file is bundled or hosted; the package build fails if a model file is in a tarball.
- `nvngx_dlssnr.dll` is accepted only when its SHA-256 is 310.9.16 `2bc19bc09a7b89dd44f73e2ac0108fcc586e2154bdd0830f242f4f040e28789f` or 310.8.0 `e16bcf15e16e13f527491cdf7845b2fe6521a738d8f7c9c721866a8496e1fc8e`. Models: `CC_SILVER_AARDWOLD.bin` `2b41c888cf4155b8958c665ba64018ab0bd25c85fc71a2b6db86d0d04d1f7fbd` (147,756,560 bytes), `CC_SMALL_FP8.bin` `7e966e2639dbec6596aa0bac3b8761137d2c40ca8cb4aca30df7ca10043c6e33` (113,917,248 bytes). The model is extracted once and cached; the DLL itself is not kept.
- DLL discovery order, exactly: explicit path → scan of known game locations (Steam libraries, Heroic installs, local-game folders) → installed Ferestre Xbox packages → configured URL. A `.dll`, or a `.zip` containing exactly one `nvngx_dlssnr.dll`.
- Routes: `optiscaler` when the game ships DLSS/FSR/XeSS and is 64-bit; else `reshade-feeder` (DX10–12), `reshade-feeder-vulkan` (Vulkan), `reshade-feeder-dx9` (DX9).
- Anti-cheat gate: a kernel-level anti-cheat that Proton cannot run (`Detection { kernel: true, proton_runtime: false, .. }`: Ricochet, Vanguard, ACE, GameGuard, mhyprot, EA AntiCheat…) refuses enabling (no override); any other detected anti-cheat, including Easy Anti-Cheat and BattlEye (`proton_runtime: true`), requires an explicit acknowledgement remembered per game and anti-cheat. Never enable silently on a game with anti-cheat. (PR A as merged: `Detection { name, kernel, proton_runtime, evidence }`; `kernel` is true for EAC/BattlEye too, so the refusal must test `kernel && !proton_runtime`.)
- Hardware check warns, never blocks: no RDNA4 GPU, or Mesa older than 26.2.
- Upstream pin: `https://github.com/TheAutomatic/DLSSNR-AMD.git` commit `9503f6b180df45305b38fbd21cd9f705164f2ab1` ("Release 0.0.9"), version `0.0.9`. Ferestre release tag `dlssnr-0.0.9`, assets `ferestre-dlssnr-0.0.9-x86_64.tar.gz`, `ferestre-dlssnr-0.0.9-i686.tar.gz`, `SHA256SUMS`.
- Every commit message ends with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Decisions taken from investigation

- **`install.sh` (upstream, `linux/package/install.sh` at the pin).** Fully non-interactive when given `<dir> <route> --model <M> --renodx --no-lumenite` with stdin `</dev/null` (every prompt is behind `-t 0` or a positional route). Exit 0 on success, 1 on explicit errors, anything else from `set -euo pipefail`; a failure in `optiscaler/patch_ini.py` aborts half-installed with the manifest already written, so Ferestre runs `remove` after any non-zero exit. Manifest `dlssnr-amd-install.txt` lists relative paths; `remove` deletes each and does **not** restore anything it overwrote. `--dll` accepts a DLL or a zip and caches models *in the package's* `dlssnr-amd/`; Ferestre instead caches models itself and hard-links them into the package before each install, so `--dll` is never passed. A reinstall writes a fresh `dlssnr-amd.ini` (user settings lost), so Ferestre keeps the person's copy across reinstalls. Top-level names each route writes (Ferestre computes them from the unpacked package, not from a hardcoded list): common `dlssnr-amd-install.txt`, `dlssnr-amd/`, `dlssnr-amd.ini`, `dlssnr-amd.log`; `optiscaler` adds `dxgi.dll` (OptiScaler.dll), `OptiScaler.ini`, `OptiScaler/`, `OptiScaler.log` and `vulkan-1.dll` only when the game has none; `reshade-feeder` every entry of `<pkg>/reshade/` plus `ReShade.log`; `reshade-feeder-vulkan`/`-dx9` every entry of `<pkg>/reshade/` and `<pkg>/vulkan/` except `dxgi.dll` and `ReShadePreset-d3d9.ini`, plus `ReShade.log`.
- **Launch env (from `install.sh`'s printed line).** `optiscaler`, `reshade-feeder`: `WINEDLLOVERRIDES` gains `dxgi=n,b`. Vulkan/DX9 routes: `winevulkan=n,b;vulkan-1=n,b`. ReShade routes whose starting network is `CC_SILVER_AARDWOLD_OPTIMIZED` and whose install produced `dlssnr-amd/CC_SILVER_AARDWOLD_OPTIMIZED_INT4.bin`: `VK_ADD_LAYER_PATH=<exe dir>/dlssnr-amd/int4/layer` (a host path: the layer is a native `.so`) and `VK_INSTANCE_LAYERS=VK_LAYER_DLSSNR_int4`. Merge rule copied from `scripts/game-session.py:247-257`: keep every existing `;`-group, append ours only for DLL names not already specified.
- **The neural env is decided after the pre-launch check, never before.** `launch::plan()` computes `env_unset` from `STEAM_ENVIRONMENT` (`launch.rs:69-88`, which includes `VK_LAYER_PATH` and `VK_INSTANCE_LAYERS`) filtered by the keys set *at that moment* (`launch.rs:329-334`), and `Plan::to_command` applies `env_remove` after `env` (`launch.rs:213-219`). So the neural helper (a) removes every key it sets from `plan.env_unset`, and (b) takes a key's current value from `plan.env_set`, falling back to the inherited environment only when `!plan.is_unset(key)` — Steam's inherited `VK_INSTANCE_LAYERS` (overlay/Fossilize, stripped on purpose) never comes back. The env is merged in `launch_prepared` *after* `ensure_installed` says the install is present and the anti-cheat gate passed; a dry run shows what a launch would add without writing anything.
- **Package build.** `linux/build/build_package.sh` needs `fetch_deps.sh` (pinned Vulkan-Headers, glslang 16.5.0, slang 2026.19, imgui, DLSS5-Feeder, reshade-shaders, vort_Shaders, ReShade 6.8.0, OptiScaler nightly-20261030, Inter 4.1), `linux/build/build_vulkan_loader.sh` (Khronos loader v1.4.357, both arches), mingw-w64, g++-multilib, cmake, ninja, python3, p7zip, openssl and osslsigncode (`NR_SIGN=1`). Upstream's own GitLab release job runs in `ubuntu:24.04` with exactly `ca-certificates git curl cmake ninja-build mingw-w64 g++-multilib python3 p7zip-full openssl osslsigncode`; Ferestre uses the same image (pinned by digest) and the same flags, i686 included (one more `build_package.sh` run; the loader is built for both arches anyway). Upstream's tarball is `tar -czf` without sorting or fixed mtimes, so `build-dlssnr.sh` repacks it deterministically; compiled PE files are not guaranteed byte-identical between builds, so the trust anchor is the SHA-256 of the *published* asset, pinned in Rust, not a rebuild.
- **Source pin location.** `sources.lock.json` accepts exactly five `components`, all `icex` forks (`scripts/source-pins.py:load`), so the upstream pin goes into a new top-level `packages` table validated separately. Cost noted: any edit of `sources.lock.json` on `main` triggers the 2-hour runtime build (`runtime.yml` path filter); a DLSSNR bump is deliberate and rare, so this is accepted rather than adding a second lock file.
- **Xbox packages and `xodus-cli`.** `xodus-cli` *is* a fork Ferestre controls (`sources.lock.json` → `https://github.com/icex/xodus.git` `a41bb842…`), but its `extract` command (`crates/xodus-cli/src/commands/extract.rs`) is `streaming::run("file://"+path)`, i.e. it re-reads a *whole package container* and extracts every file. The `.xodus-streaming.msixvc` kept in an install folder is only the committed metadata-prefix record (`PrefixCacheFile` → `commit()` in `commands/streaming.rs:542`), not file data, so no filter flag could pull a file out of it. Files of an installed package are already decrypted on disk at install time with the person's licence, except files flagged `KEEP_ENCRYPTED_ON_DISK` (`msixvc/src/xvd.rs:1126`, in practice executables), and a stopped download simply lacks files. So step 3 of discovery scans the install folders' already-extracted files and accepts only a hash-matching `nvngx_dlssnr.dll`; an encrypted or partial copy fails the hash and is reported as such. No `xodus-cli` change in this plan; a `streaming --only <path>` fetch in the fork is the tracked follow-up. This is a deviation from the spec's wording and is listed above as needing sign-off.
- **Route detection on Xbox titles.** The main executable of an Xbox install is kept encrypted on disk, so the PE reader fails on it; when the game folder `install::looks_installed`, Ferestre assumes x86-64 D3D12 (GDK PC titles are both) and says it assumed so.
- **No new crate.** The workspace has no PE parser; a ~150-line reader (machine, optional-header magic, import directory names) is enough. `zip` 2 is already a dependency. Large downloads reuse `http.rs`'s resumable `fetch_resumable` (the 60 s global timeout of `download_pinned` would cut a 150 MB file).

## File Structure

- Create `crates/ferestre-core/src/neural/mod.rs` — public types (`Route`, `Network`, `Arch`, `DllVersion`, `ModelFile`, `Expect`, `Context`), pinned hashes, `overview()`.
- Create `crates/ferestre-core/src/neural/package.rs` — pinned release asset, local override, download/verify/unpack, `route_files()`.
- Create `crates/ferestre-core/src/neural/pe.rs` — minimal PE reader.
- Create `crates/ferestre-core/src/neural/detect.rs` — API, bitness, upscalers → route.
- Create `crates/ferestre-core/src/neural/dll.rs` — accept DLL/zip, bounded scan, discovery order.
- Create `crates/ferestre-core/src/neural/models.rs` — extract once, cache, stage into a package.
- Create `crates/ferestre-core/src/neural/state.rs` — `neural-rendering.toml`.
- Create `crates/ferestre-core/src/neural/hardware.rs` — RDNA4 / Mesa warnings.
- Create `crates/ferestre-core/src/neural/apply.rs` — gate, backup/restore, run `install.sh`, verify, enable/disable/ensure, launch env.
- Create `crates/ferestre-core/src/settings.rs` — `settings.toml` (`[dlssnr] dll-url`).
- Modify `crates/ferestre-core/src/lib.rs`, `http.rs`, `updater.rs` (expose `sha256` as `pub(crate) fn sha256_file`).
- Modify `crates/ferestre-cli/src/main.rs` — `Cmd::Neural`, launch wiring, uninstall (Xbox and local paths).
- Create `crates/ferestre-gui/src/neural_ui.rs`; modify `crates/ferestre-gui/src/main.rs`.
- Create `scripts/build-dlssnr.sh`, `.github/workflows/dlssnr.yml`, `tests/test_no_nvidia_urls.py`, `docs/NEURAL-RENDERING.md`; modify `sources.lock.json`, `scripts/source-pins.py`, `tests/test_source_pins.py`, `tests/test_workflows.py`, `.github/workflows/rust.yml`, `docs/RELEASING.md`, `NOTICE`, `docs/ROADMAP.md`.

## Review Focus

1. **A game folder that already has its own `dxgi.dll`, `ReShade.ini` or `reshade-shaders/` (another mod, DLSS Swapper, Special K).** Expected: enable backs each up, disable puts back byte-identical files and folders. Test: `apply::tests::another_mods_files_come_back_byte_identical_after_disable` (Task 6).
2. **A game update or file verification that deletes `dlssnr-amd/` or replaces `dxgi.dll` between launches.** Expected: the next launch reinstalls, keeps the person's `dlssnr-amd.ini`, and never overwrites the original backup with Ferestre's own files. Test: `apply::tests::a_wiped_install_is_redone_without_backing_up_our_own_files` (Task 6) and `tests::a_launch_reinstalls_neural_rendering_that_an_update_removed` (Task 7).
3. **The person's own `WINEDLLOVERRIDES` (recipe `[launch.env]` or a Steam shortcut's launch options), including one that already names `dxgi`.** Expected: theirs is kept and wins; the launch says the mod may not load. Test: `apply::tests::the_persons_overrides_win_and_are_reported` (Task 6).
4. **`install.sh` failing part-way (missing `python3`, `patch_ini.py` exit 1).** Expected: Ferestre runs `remove`, restores backups, records nothing as enabled, and shows the log tail. Test: `apply::tests::a_failed_install_is_rolled_back_and_not_recorded` (Task 6).
5. **An executable Ferestre cannot read (encrypted Xbox image, a launcher stub, OpenGL-only game).** Expected: Xbox installs fall back to "x86-64 D3D12 (assumed)"; anything else refuses with "pass --route", never guesses silently. Test: `detect::tests::an_unreadable_executable_is_assumed_only_for_an_xbox_install` (Task 3).
6. **A game update that adds a kernel or unacknowledged anti-cheat while the mod's files are intact.** Expected: the next launch takes the mod out (or refuses to launch if it cannot) and the plan carries no `dxgi`/`vulkan-1` override and no int4 layer. Tests: `apply::tests::a_new_anti_cheat_takes_an_intact_install_out` (Task 6), `tests::a_new_kernel_anti_cheat_leaves_no_neural_override_in_the_launch` (Task 7).
7. **Steam's own Vulkan layers versus ours.** Expected: with `steam_integration = false`, `VK_INSTANCE_LAYERS` reaches the game as exactly `VK_LAYER_DLSSNR_int4` (Steam's inherited value stays stripped), and is not in `env_unset`. Test: `tests::the_int4_layer_survives_the_steam_environment_strip` (Task 7, on `plan.to_command()`).
8. **Removing a local game that has neural rendering on.** Expected: the mod is taken out and the person's backed-up files are put back before Ferestre forgets the game; if that fails, nothing is forgotten. Test: `tests::removing_a_local_game_takes_neural_rendering_out_first` (Task 7).
9. **A backup manifest someone edited (`..`, an absolute path, `""`, a symlink).** Expected: restore refuses before deleting anything. Test: `apply::tests::a_tampered_backup_manifest_deletes_nothing` (Task 6).

---

### Task 1: Pin upstream, build the package reproducibly, publish it on a `dlssnr-*` tag

**Files:**
- Modify: `sources.lock.json` (new top-level `packages` table), `scripts/source-pins.py:14-28` (`load`) and `:64-105` (`main`), `tests/test_source_pins.py`
- Create: `scripts/build-dlssnr.sh`, `.github/workflows/dlssnr.yml`, `tests/test_no_nvidia_urls.py`
- Modify: `tests/test_workflows.py` (`test_only_a_tag_push_signs_or_publishes` `:83-91`, `test_releases_are_signed_before_they_are_published` `:118-131`, `test_signatures_carry_the_tags_version_and_manual_runs_say_they_are_unsigned` `:133-146`, plus two new tests), `.github/workflows/rust.yml:256-262` (run the new test), `docs/RELEASING.md` (tag table at line 8, rulesets at lines 92/121, a "DLSSNR package" section), `NOTICE`

**Interfaces:**
- Produces: `python3 scripts/source-pins.py package dlssnr-amd {repository|commit|version}`; `scripts/build-dlssnr.sh <output dir>` writing exactly `ferestre-dlssnr-<version>-x86_64.tar.gz`, `ferestre-dlssnr-<version>-i686.tar.gz`, `SOURCES.txt` and, last, `SHA256SUMS` listing all three (tarballs have no top-level directory, like upstream's). Every file in the output dir is listed in `SHA256SUMS`, because `packaging/sign-release.sh:62-68` refuses any unlisted file. The release URL shape `https://github.com/icex/ferestre/releases/download/dlssnr-<version>/ferestre-dlssnr-<version>-<arch>.tar.gz` is used in Task 2.

- [ ] **Step 1: Write the failing tests**

In `tests/test_source_pins.py` (same `subTest` + deep-copy pattern as `test_lock_requires_complete_commit_hashes_icex_forks_and_safe_paths`, `:16-26`):
```python
def test_packages_pin_an_exact_commit_and_version(self):
    pkgs=pins_module.load_packages()
    self.assertEqual(pkgs['dlssnr-amd']['commit'],'9503f6b180df45305b38fbd21cd9f705164f2ab1')
    self.assertEqual(pkgs['dlssnr-amd']['version'],'0.0.9')
    original=json.loads((ROOT/'sources.lock.json').read_text())
    for change in ('branch','version','url','extra','missing'):
        with self.subTest(change=change),tempfile.TemporaryDirectory() as tmp:
            data=copy.deepcopy(original)
            if change=='branch':data['packages']['dlssnr-amd']['commit']='main'
            if change=='version':data['packages']['dlssnr-amd']['version']='latest'
            if change=='url':data['packages']['dlssnr-amd']['repository']='http://x/y.git'
            if change=='extra':data['packages']['other']=dict(data['packages']['dlssnr-amd'])
            if change=='missing':del data['packages']
            path=Path(tmp)/'lock.json';path.write_text(json.dumps(data))
            with self.assertRaises(ValueError):pins_module.load_packages(path)
def test_components_still_require_exactly_the_five_forks(self):
    self.assertEqual(set(pins_module.load()),pins_module.NAMES)
def test_the_package_command_prints_one_field(self):
    out=subprocess.check_output(['python3','scripts/source-pins.py','package','dlssnr-amd','commit'],cwd=ROOT,text=True)
    self.assertEqual(out.strip(),'9503f6b180df45305b38fbd21cd9f705164f2ab1')
```
In `tests/test_workflows.py`, each existing test edited as follows:
- `test_only_a_tag_push_signs_or_publishes`: the path tuple gains `REPO / '.github/workflows/dlssnr.yml'`.
- `test_releases_are_signed_before_they_are_published`: the name tuple becomes `('release.yml', 'runtime.yml', 'dlssnr.yml')`.
- `test_signatures_carry_the_tags_version_and_manual_runs_say_they_are_unsigned`: the pair tuple gains `('dlssnr.yml', 'dlssnr-')`, so the publish step must contain `tag_version=${GITHUB_REF_NAME#dlssnr-}` and `packaging/sign-release.sh out "$tag_version"`.

New tests in the same class:
```python
def test_the_dlssnr_package_is_built_from_the_lock_and_never_becomes_latest(self):
    workflow=load(REPO/'.github/workflows/dlssnr.yml')
    text='\n'.join(runs(workflow['jobs']['build']))
    self.assertIn('scripts/build-dlssnr.sh out',text)
    self.assertIn('The tag must be on main',[s.get('name') for s in workflow['jobs']['build']['steps']])
    self.assertNotIn('${{',text)
    publish=next(t for t in runs(workflow['jobs']['publish']) if 'gh release create' in t)
    # The README's "latest release" link must keep pointing at the AppImage.
    self.assertIn('--latest=false',publish)
def test_the_dlssnr_build_sums_every_file_it_writes(self):
    text=(REPO/'scripts/build-dlssnr.sh').read_text()
    sums=[i for i,l in enumerate(text.splitlines()) if 'SHA256SUMS' in l and 'sha256sum' in l]
    self.assertEqual(len(sums),1)
    line=text.splitlines()[sums[0]]
    self.assertIn('sha256sum -- *',line)  # every file in the output dir, SOURCES.txt included
    # Nothing is written into the output dir after the sums.
    self.assertNotRegex('\n'.join(text.splitlines()[sums[0]+1:]),r'>\s*"?\$out')
```
Create `tests/test_no_nvidia_urls.py`: over `git ls-files`, assert no line matches `re.compile(r'https?://\S*(nvngx|dlssnr\.dll|nvidia\.com/\S*dlss)', re.I)`; plus a self-check that the regex catches a URL the test assembles at run time (`'https://example.invalid/' + 'nvngx_dlssnr.dll'`), so the guard's own source never contains one.

- [ ] **Step 2: Run them to verify they fail**

Run: `python3 tests/test_source_pins.py && python3 tests/test_workflows.py && python3 tests/test_no_nvidia_urls.py`
Expected: FAIL (`load_packages` undefined, `dlssnr.yml` and `build-dlssnr.sh` missing). `test_no_nvidia_urls.py` passes already: it is a guard.

- [ ] **Step 3: Implement the pin**

`sources.lock.json` gains `"packages": {"dlssnr-amd": {"repository": "https://github.com/TheAutomatic/DLSSNR-AMD.git", "commit": "9503f6b180df45305b38fbd21cd9f705164f2ab1", "version": "0.0.9"}}`. In `source-pins.py` add `PACKAGES = {'dlssnr-amd'}` and `load_packages(path=ROOT / 'sources.lock.json') -> dict` (raises `ValueError` unless: names exactly `PACKAGES`; repository matches `https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+\.git`; commit 40 hex; version `\d+(\.\d+){2,3}`); `load()` keeps its behaviour and ignores `packages`. Add the `package` command (`component` choices become NAMES ∪ PACKAGES and are checked per command; `field` gains `version`).

- [ ] **Step 4: Write `scripts/build-dlssnr.sh <output dir>`**

Decisions (body is the implementer's):
- `#!/usr/bin/env bash`, `set -euo pipefail`; reads repository/commit/version with `source-pins.py package`; `out` is the output dir, created empty (refuse a non-empty one, so nothing unlisted is ever in it).
- Runs the build in a container: `${FERESTRE_CONTAINER:-docker}` (podman works), image `IMAGE=docker.io/library/ubuntu:24.04@sha256:<digest>` — record the digest current at implementation time (`docker buildx imagetools inspect ubuntu:24.04`) as a constant with a comment saying it is bumped deliberately. Container user: host uid:gid unless rootless (copy `runtime.yml`'s check).
- Inside: `apt-get install -y --no-install-recommends ca-certificates git curl cmake ninja-build mingw-w64 g++-multilib python3 p7zip-full openssl osslsigncode`; `git init` + `git fetch --depth 1 <repo> <commit>` + checkout, then refuse unless `git rev-parse HEAD` equals the pin; `bash fetch_deps.sh`; `bash linux/build/build_vulkan_loader.sh`; for `x86_64` and `i686`: `NR_VERSION=<version> NR_ARCH=<arch> NR_SIGN=1 bash linux/build/build_package.sh`.
- Refuse a tarball containing any of `CC_SILVER_AARDWOLD.bin`, `CC_SMALL_FP8.bin`, `dlssnr.bin`, `dlssnr-small.bin` (upstream's own check still looks only for the last two names).
- Repack each `linux/package/DLSSNR-AMD-Vulkan-Linux-<version>-<arch>.tar.gz` deterministically: unpack, `tar --sort=name --owner=0 --group=0 --numeric-owner --mtime=@<commit time> -C <dir> -cf - . | gzip -n -9 > "$out/ferestre-dlssnr-<version>-<arch>.tar.gz"`.
- Write `"$out/SOURCES.txt"` (upstream repository+commit, and the line "OptiScaler (GPL-3.0) is shipped unmodified from its nightly-20261030 release; source: https://github.com/optiscaler/OptiScaler at e7e3872d410f704479d94d318d68d163b6b58947").
- Last line that touches `out`: `( cd "$out" && sha256sum -- * > SHA256SUMS )`, so the two tarballs and `SOURCES.txt` are all listed (this is what `test_the_dlssnr_build_sums_every_file_it_writes` checks).

- [ ] **Step 5: Write `.github/workflows/dlssnr.yml`** mirroring `runtime.yml`'s two-job shape: `on: push: tags: ['dlssnr-*']` and `workflow_dispatch`; `permissions: {contents: read}`; job `build` on `ubuntu-24.04`, `actions/checkout` pinned to the same SHA as `runtime.yml` with `persist-credentials: false`, step "The tag must be on main" (copy), step `bash scripts/build-dlssnr.sh out` (version check: on a tag, `${GITHUB_REF_NAME#dlssnr-}` must equal `source-pins.py package dlssnr-amd version`), step named exactly `Unsigned build` with `if: <NOT_TAG_PUSH>` whose `run` writes `UNSIGNED` to `$GITHUB_STEP_SUMMARY` (copy `runtime.yml`'s), upload-artifact pinned; job `publish` with `needs: build`, `environment: release`, `if: <TAG_PUSH>`, `permissions: contents: write`, download-artifact → a run step `tag_version=${GITHUB_REF_NAME#dlssnr-}` then `packaging/sign-release.sh out "$tag_version"` (copy `runtime.yml`'s publish step) → `gh release create "$GITHUB_REF_NAME" out/* --latest=false --title "DLSSNR-AMD $VERSION for Ferestre"` with notes stating the upstream commit, the licences (MIT; OptiScaler GPL-3.0 with source link; ReShade BSD-3; Vulkan-Loader Apache-2.0 with `PATCHES.diff`) and that no NVIDIA file is included. `--latest=false` keeps README's "latest release" link (`README.md:35`) on the AppImage release.

- [ ] **Step 6: Docs.** `docs/RELEASING.md`: table row `dlssnr-<version>` → `.github/workflows/dlssnr.yml` → the two tarballs, `SOURCES.txt` and `SHA256SUMS` (each with its `.minisig`), never marked latest; add `dlssnr-*` to the tag rulesets; a short "Bumping DLSSNR-AMD" section (edit `packages` in the lock → tag → Task 10's pin update). `NOTICE`: the DLSSNR-AMD package and its bundled licences. Wire `python3 tests/test_no_nvidia_urls.py` into `rust.yml`'s "Packaging and release tests".

- [ ] **Step 7: Verify**

Run: `python3 tests/test_source_pins.py && python3 tests/test_workflows.py && python3 tests/test_no_nvidia_urls.py && python3 tests/test_release_signing.py && bash -n scripts/build-dlssnr.sh && shellcheck --severity=error scripts/build-dlssnr.sh && scripts/check-sources.sh`
Expected: all OK. Then one real local build: `scripts/build-dlssnr.sh /tmp/dlssnr-out` produces both tarballs, `SOURCES.txt` and `SHA256SUMS`, and `(cd /tmp/dlssnr-out && sha256sum --strict -c SHA256SUMS)` lists all three (takes tens of minutes; needs network).

- [ ] **Step 8: Commit**

```bash
git add sources.lock.json scripts/source-pins.py scripts/build-dlssnr.sh tests/test_source_pins.py tests/test_workflows.py tests/test_no_nvidia_urls.py .github/workflows/dlssnr.yml .github/workflows/rust.yml docs/RELEASING.md NOTICE
git commit -m "Build and publish the DLSSNR-AMD Linux package from a pinned commit

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 2: Core package handling (`neural::package`) and a large checked download

**Files:**
- Create: `crates/ferestre-core/src/neural/mod.rs`, `crates/ferestre-core/src/neural/package.rs`
- Modify: `crates/ferestre-core/src/lib.rs` (add `pub mod neural;`), `crates/ferestre-core/src/http.rs:306-334` (beside `download_pinned`), `crates/ferestre-core/src/updater.rs:391` (`fn sha256` → `pub(crate) fn sha256_file`, callers updated)

**Interfaces:**
- Produces (`neural/mod.rs`):
  - `pub enum Arch { X86_64, I686 }` (`as_str()`: `"x86_64"`, `"i686"`; serde same strings)
  - `pub enum Route { Optiscaler, ReshadeFeeder, ReshadeFeederVulkan, ReshadeFeederDx9 }` with `as_str()`/`FromStr` = `"optiscaler"`, `"reshade-feeder"`, `"reshade-feeder-vulkan"`, `"reshade-feeder-dx9"`; serde same strings
  - `pub enum Network { SmallFp8, SmallFp8Experimental, SilverAardwold, SilverAardwoldOptimized }` with `as_str()`/`FromStr`/serde = `"CC_SMALL_FP8"`, `"CC_SMALL_FP8_EXPERIMENTAL"`, `"CC_SILVER_AARDWOLD"`, `"CC_SILVER_AARDWOLD_OPTIMIZED"`
  - Owned data, so tests can build expectations from hashes they compute at run time (no `Box::leak`): `#[derive(Debug, Clone, PartialEq, Eq)] pub struct DllVersion { pub version: String, pub sha256: String, pub small_model: bool }`; `#[derive(Debug, Clone, PartialEq, Eq)] pub struct ModelFile { pub file: String, pub sha256: String, pub size: u64 }`; `#[derive(Debug, Clone)] pub struct Expect { pub dlls: Vec<DllVersion>, pub aardwold: ModelFile, pub small: ModelFile }`
  - The pinned values stay `const` tables of `&'static str` (`const PINNED_DLLS: [(&str, &str, bool); 2]` = 310.9.16 small-model `true`, 310.8.0 `false`; `const PINNED_AARDWOLD`/`PINNED_SMALL: (&str, &str, u64)`), converted by `impl Expect { pub fn pinned() -> Expect }`
  - `pub fn root(state_dir: &Path) -> PathBuf` = `state_dir/dlssnr`
- Produces (`neural/package.rs`):
  - `pub enum Origin { Url(String), File(PathBuf) }`; `pub struct Package { pub version: String, pub arch: Arch, pub sha256: String, pub origin: Origin }`
  - `pub fn pinned(arch: Arch) -> Option<Package>` (None while the pin is empty)
  - `pub const LOCAL_ENV: &str = "FERESTRE_DLSSNR_PACKAGE_DIR"`; `pub fn local(dir: &Path, arch: Arch) -> Result<Package>` (exactly one `ferestre-dlssnr-<version>-<arch>.tar.gz` in `dir`; sha computed; version from the name)
  - `pub fn current(paths: &Paths, arch: Arch) -> Result<Package>` (local override first, else pinned, else error naming `LOCAL_ENV` and `scripts/build-dlssnr.sh`)
  - `pub fn installed(root: &Path, package: &Package) -> Option<PathBuf>` (`root/packages/<version>-<arch>` with stamp `.ferestre-sha256` == `package.sha256`, `install.sh` and `model-tools/extract_model.sh` present)
  - `pub fn ensure(root: &Path, package: &Package) -> Result<PathBuf>`
  - `pub struct RouteFile { pub name: String, pub dir: bool, pub source: Option<PathBuf>, pub only_if_absent: bool }`; `pub fn route_files(package_dir: &Path, route: Route) -> Result<Vec<RouteFile>>` (`only_if_absent` is true for exactly one entry: `optiscaler`'s `vulkan-1.dll`, which `install.sh` writes only when the game has none; its `source` is the file `install.sh` copies for it — read that from `install.sh` at the pin while implementing)
  - `pub struct Injectors { pub required: &'static [&'static str], pub when_listed: &'static [&'static str] }`; `pub fn injectors(route: Route) -> Injectors` — `optiscaler`: required `["dxgi.dll"]`, when_listed `["vulkan-1.dll"]`; `reshade-feeder`: required `["dxgi.dll"]`, when_listed `[]`; `reshade-feeder-vulkan`/`-dx9`: required `["vulkan-1.dll", "winevulkan.dll"]`, when_listed `[]`. `verify` (Task 6) compares every required name, and every when_listed name the install manifest lists, byte for byte with its `RouteFile.source`.
  - `pub fn expects_models(package_dir: &Path, expect: &Expect) -> bool` (its `install.sh` contains `model_sha256=<aardwold>` and `small_sha256=<small>`)
- Produces (`http.rs`): `pub fn download_large_checked(url: &str, dest: &Path, limit: u64, cancelled: impl Fn() -> bool, check: impl Fn(&Path) -> Result<()>) -> Result<()>` — HTTPS only, resumable (`fetch_resumable`, `LARGE` patience), temporary beside `dest`, renamed in only after `check` passes; nothing unchecked is left on disk.

- [ ] **Step 1: Write the failing tests**

`http::tests` (use the existing `serve`, `Served`, `QUICK`, `leftovers`; test through a private `download_checked(url, dest, limit, cancelled, patience, check)` that the public fn calls with `LARGE{https_only: true}`):
```rust
fn a_checked_download_is_kept_only_when_the_check_passes()   // check Ok -> dest has the bytes; check Err -> error, leftovers(dir).is_empty()
fn a_checked_download_past_its_limit_is_refused()             // limit 10, body 20 bytes -> Err containing "larger than"
```
`neural::package::tests` (fake tarballs built with `tar -czf` from a tempdir, as `anticheat::tests::ensure_unpacks_an_archive_already_downloaded` does):
```rust
fn a_package_unpacks_without_a_top_level_folder_and_is_stamped()    // installed(root,&pkg) == Some(root/"packages/0.0.9-x86_64")
fn a_tarball_missing_install_sh_is_refused_and_leaves_nothing()
fn a_copy_from_another_tarball_is_not_installed()                    // stamp "0000" -> None
fn a_local_package_dir_names_its_version_and_arch()                  // dir with ferestre-dlssnr-0.0.9-x86_64.tar.gz -> version "0.0.9"; two matches -> Err
fn the_pin_is_empty_or_a_ferestre_release_asset()                     // for both arches: url and sha both empty, or url starts "https://github.com/icex/ferestre/releases/download/dlssnr-" and sha is 64 lowercase hex
fn route_files_follow_the_package_contents()                          // fake pkg with reshade/{dxgi.dll,ReShade.ini,reshade-shaders/} and vulkan/{vulkan-1.dll,winevulkan.dll,ReShadePreset-d3d9.ini,vk-override/}
    // reshade-feeder: names ⊇ {dlssnr-amd-install.txt, dlssnr-amd, dlssnr-amd.ini, dlssnr-amd.log, ReShade.log, dxgi.dll, ReShade.ini, reshade-shaders}
    // reshade-feeder-vulkan: contains vulkan-1.dll, winevulkan.dll, vk-override; does NOT contain dxgi.dll or ReShadePreset-d3d9.ini
    // optiscaler: {.., dxgi.dll (source = optiscaler/release/OptiScaler.dll), OptiScaler.ini, OptiScaler, OptiScaler.log, vulkan-1.dll}; only vulkan-1.dll has only_if_absent
fn injectors_name_what_each_route_loads_through()                     // optiscaler required ["dxgi.dll"] when_listed ["vulkan-1.dll"]; reshade-feeder-dx9 required ["vulkan-1.dll","winevulkan.dll"]
fn a_package_expecting_other_models_is_detected()                     // install.sh text with other hashes -> false
```

- [ ] **Step 2: Run to verify they fail**

Run: `$HOME/.cargo/bin/cargo test --locked -p ferestre-core -- neural::package http::tests::a_checked`
Expected: FAIL to compile (`neural` missing).

- [ ] **Step 3: Implement** the items in the Interfaces block. Unpack exactly as `anticheat::unpack` does (staging beside the target via `cache::temporary_beside`, `tar -xzf … --no-same-owner --no-same-permissions`, stamp, one rename, keep a copy another process finished first). Download into `root/downloads/` with a 1 GiB limit, verify against `package.sha256` in the `check` closure; `Origin::File` copies nothing and unpacks the file in place. The pinned constants hold empty `url`/`sha256` until Task 10.

- [ ] **Step 4: Run to verify they pass** — same command, Expected: PASS. Then `$HOME/.cargo/bin/cargo test --locked -p ferestre-core` stays green.

- [ ] **Step 5: Commit** — `git add crates/ferestre-core/src/neural crates/ferestre-core/src/lib.rs crates/ferestre-core/src/http.rs crates/ferestre-core/src/updater.rs` and commit `Fetch and unpack the pinned DLSSNR-AMD package` (with the Co-Authored-By line).

### Task 3: Route detection (`neural::pe`, `neural::detect`)

**Files:**
- Create: `crates/ferestre-core/src/neural/pe.rs`, `crates/ferestre-core/src/neural/detect.rs`

**Interfaces:**
- Consumes: `Route` (Task 2), `install::looks_installed(dir: &Path) -> bool` (`install.rs:943`)
- Produces:
  - `pub struct PeInfo { pub bits: u8, pub imports: Vec<String> }` (imports lowercase); `pub fn pe::read(path: &Path) -> Result<PeInfo>` (seek-based; reads headers, section table and the import directory only; at most 4096 descriptors, names ≤ 256 bytes; `0x14c` → 32, `0x8664` → 64, anything else an error)
  - `pub enum Api { Dx10To12, Vulkan, Dx9, OpenGl, Unknown }`
  - `pub struct Detected { pub bits: u8, pub api: Api, pub upscalers: Vec<String>, pub assumed: bool, pub route: Option<Route> }`
  - `pub fn detect::detect(game_dir: &Path, exe: &Path) -> Detected`
  - `pub fn detect::route_for(bits: u8, api: Api, has_upscaler: bool) -> Option<Route>`
  - `pub const UPSCALER_FILES: &[&str]` (lowercase exact names `nvngx_dlss.dll`, `sl.interposer.dll`, `sl.dlss.dll`, `libxess.dll`, `libxess_dx11.dll`, `amd_fidelityfx_dx12.dll`, `amd_fidelityfx_vk.dll`) and prefixes `ffx_fsr2_api`, `ffx_fsr3`, `amd_fidelityfx_upscaler`

Rules: imports are the exe's plus those of each DLL the exe imports that sits in the exe's folder (one level: catches `UnityPlayer.dll`). `d3d12`/`d3d11`/`d3d10`/`d3d10_1`/`dxgi` → `Dx10To12`; else `d3d9` → `Dx9`; else `vulkan-1` → `Vulkan`; else `opengl32` → `OpenGl`; else `Unknown`. Upscalers: case-insensitive file-name match under `game_dir`, depth ≤ 8, ≤ 50,000 entries, symlinked directories not followed. `route_for`: upscaler && 64 → `Optiscaler`; `Dx10To12` → `ReshadeFeeder`; `Vulkan` → `ReshadeFeederVulkan`; `Dx9` → `ReshadeFeederDx9`; `OpenGl`/`Unknown` → `None`. If `pe::read` fails: when `looks_installed(game_dir)` → `bits 64, Dx10To12, assumed: true`; otherwise `route: None`.

- [ ] **Step 1: Write the failing tests** (PE fixtures are built in-test by a helper `fn tiny_pe(machine: u16, magic: u16, imports: &[&str]) -> Vec<u8>` writing a DOS header, `PE\0\0`, one `.idata` section and an import table — no binary fixtures in the repo):
```rust
// pe::tests
fn a_64_bit_image_lists_its_imports()            // tiny_pe(0x8664, 0x20b, ["KERNEL32.dll","d3d12.dll"]) -> bits 64, imports ["kernel32.dll","d3d12.dll"]
fn a_32_bit_image_is_read_too()                  // 0x14c/0x10b -> bits 32
fn garbage_and_truncated_files_are_errors_not_panics()   // b"MZ", 64 random bytes, an import RVA past EOF
// detect::tests
fn routes_follow_the_spec_table()   // route_for(64,Dx10To12,true)==Some(Optiscaler); (32,Dx10To12,true)==Some(ReshadeFeeder); (64,Vulkan,true)==Some(Optiscaler); (64,Vulkan,false)==Some(ReshadeFeederVulkan); (32,Dx9,false)==Some(ReshadeFeederDx9); (64,OpenGl,false)==None
fn an_engine_dll_beside_the_exe_names_the_api()   // Game.exe imports UnityPlayer.dll; UnityPlayer.dll imports d3d11.dll -> Dx10To12
fn an_unreal_upscaler_deep_in_the_tree_is_found() // Engine/Plugins/Runtime/Nvidia/DLSS/Binaries/ThirdParty/Win64/nvngx_dlss.dll -> upscalers == ["nvngx_dlss.dll"], route Optiscaler for a 64-bit exe
fn an_unreadable_executable_is_assumed_only_for_an_xbox_install()  // garbage exe: plain dir -> route None; dir with ".xodus-streaming.msixvc" -> route Some(ReshadeFeeder), assumed true
```

- [ ] **Step 2: Run to verify they fail** — `$HOME/.cargo/bin/cargo test --locked -p ferestre-core -- neural::pe neural::detect` → FAIL (missing items).
- [ ] **Step 3: Implement** the items above.
- [ ] **Step 4: Run to verify they pass** — same command → PASS.
- [ ] **Step 5: Commit** `Detect a game's DLSSNR-AMD route from its files` (with the Co-Authored-By line).

### Task 4: Settings, DLL discovery and the model cache (`settings`, `neural::dll`, `neural::models`)

**Files:**
- Create: `crates/ferestre-core/src/settings.rs`, `crates/ferestre-core/src/neural/dll.rs`, `crates/ferestre-core/src/neural/models.rs`
- Modify: `crates/ferestre-core/src/lib.rs` (`pub mod settings;`)

**Interfaces:**
- Consumes: `http::download_large_checked`, `package::ensure`, `Expect`/`DllVersion`/`ModelFile` (Task 2); `install::all(state_dir) -> Vec<Record>` (`install.rs:1132`); `Paths::{home, config_dir, games_dir, state_dir, install_dir, var}`; from PR A: `Paths::steam_roots()` and `steam::libraries(root)`
- Produces:
  - `settings::FILE = "settings.toml"`; `pub const DLL_URL_ENV: &str = "FERESTRE_DLSSNR_DLL_URL"`; `pub fn settings::dlssnr_dll_url(paths: &Paths) -> Result<Option<String>>` (env wins over the file); `pub fn settings::file_dlssnr_dll_url(config_dir: &Path) -> Result<Option<String>>` (the file only, for the GUI's field, which must not show or save a value that came from the environment); `pub fn settings::set_dlssnr_dll_url(config_dir: &Path, url: Option<&str>) -> Result<()>` (edits a `toml::Table` so unknown tables survive; written with `cache::write`; a value that does not start with `https://` is refused; `None` removes the key)
  - `pub struct dll::Accepted { pub path: PathBuf, pub version: DllVersion, pub found_at: PathBuf, _work: Option<TempDir-like guard> }` (a downloaded or unzipped copy is deleted on drop: "the DLL itself is not kept")
  - `pub fn dll::accept(candidate: &Path, work: &Path, expect: &Expect) -> Result<Accepted>` (`.zip`: exactly one entry whose base name is `nvngx_dlssnr.dll` case-insensitively, subfolders allowed, extracted with a 512 MiB cap; otherwise the file itself; then SHA-256 must be one of `expect.dlls`, error text `"<path> is not nvngx_dlssnr.dll 310.9.16 or 310.8.0 (SHA-256 <hex>)"`)
  - `pub struct dll::Budget { pub deadline: Instant, pub entries: usize, pub depth: usize }`, `impl Budget { pub fn standard() -> Self }` (30 s, 400,000 entries, depth 12)
  - `pub struct dll::Rejected { pub path: PathBuf, pub reason: String }`
  - `pub fn dll::scan(roots: &[PathBuf], work: &Path, budget: &mut Budget, expect: &Expect) -> (Option<Accepted>, Vec<Rejected>)` (file name match case-insensitive; size 50 MiB–512 MiB before hashing; directories named `compatdata`, `shadercache`, `downloading`, `temp`, `.ferestre-dlssnr-backup` and symlinked directories skipped)
  - `pub struct dll::Locations { pub steam: Vec<PathBuf>, pub heroic: Vec<PathBuf>, pub local: Vec<PathBuf>, pub xbox: Vec<PathBuf> }`; `pub fn dll::locations(paths: &Paths, recipes: &[Recipe]) -> Locations` (steam: `<lib>/steamapps/common` for every `lib` in `steam::libraries(root)` over every `root` in `paths.steam_roots()` (native and Flatpak Steam), deduplicated; heroic: every `install_path`/`path` string found in `<xdg config>/heroic/{legendaryConfig/legendary,gog_store,nile_config/nile}/installed.json` and the Flatpak equivalent under `~/.var/app/com.heroicgameslauncher.hgl/config/heroic`, plus `~/Games/Heroic`; local: `paths.install_dir(r)` for recipes whose folder is not `install::looks_installed`; xbox: install-record dirs, recipe dirs that `looks_installed`, and `paths.games_dir()`; only existing directories)
  - `pub enum dll::Step { Explicit, Scan, Xbox, Url }`; `pub fn dll::find(locations: &Locations, explicit: Option<&Path>, url: Option<&str>, work: &Path, expect: &Expect, progress: &mut dyn FnMut(&str)) -> Result<(Accepted, Step)>` — order exactly: explicit (a rejection is the error, no fall-through) → `scan(steam ++ heroic ++ local)` → `scan(xbox)` → URL (downloaded with `download_large_checked`, 512 MiB, then `accept`) → error listing what was tried, each rejection, and how to set `[dlssnr] dll-url` / `FERESTRE_DLSSNR_DLL_URL` / `--dll`. One `Budget::standard()` per scan step. A rejection inside an Xbox folder adds "(this copy may be encrypted on disk or still downloading)".
  - `pub fn models::dir(root: &Path) -> PathBuf` = `root/models`
  - `pub fn models::cached(root: &Path, expect: &Expect) -> Vec<ModelFile>` (present with the expected size; `verify` adds the SHA-256)
  - `pub fn models::verify(root: &Path, expect: &Expect) -> Result<Vec<ModelFile>>`
  - `pub fn models::extract(package_dir: &Path, dll: &Accepted, root: &Path, expect: &Expect) -> Result<Vec<ModelFile>>` (runs `bash <pkg>/model-tools/extract_model.sh <dll> <staging>` with stdin null; each output must match its expected hash; renamed into `models/`; staging removed whatever happens)
  - `pub fn models::stage(package_dir: &Path, root: &Path, expect: &Expect) -> Result<()>` (hard-link, else copy, each cached model into `<pkg>/dlssnr-amd/`; replaces a mismatching file there)

- [ ] **Step 1: Write the failing tests** (tests build an owned `Expect` whose `dlls` hold the SHA-256 of `b"fake dll 310.9.16"` etc., and fake models whose hashes they compute at run time; a fake package's `model-tools/extract_model.sh` is a 3-line bash script writing known bytes into `$2`):
```rust
// settings::tests
fn the_environment_wins_over_the_file()
fn the_file_value_is_read_without_the_environment()          // env set, file has another https URL -> file_dlssnr_dll_url returns the file's
fn setting_the_url_keeps_other_tables()                     // file has [other] x=1 -> still there after set
fn a_non_https_url_is_refused()                               // "http://…", "file:///…" -> Err; file unchanged
// neural::dll::tests
fn a_dll_with_an_accepted_hash_is_accepted_and_others_say_their_hash()
fn a_zip_must_hold_exactly_one_nvngx_dlssnr_dll()             // one in sub/ -> ok; two -> Err; none -> Err
fn the_scan_finds_a_dll_deep_in_a_library_and_skips_prefixes() // under common/Game/Engine/.../Win64; another copy under compatdata/ never hashed
fn every_steam_root_and_library_is_a_location()               // two roots (native + Flatpak layout), the second with a libraryfolders.vdf naming a third dir -> locations.steam has all three common/ dirs once
fn a_scan_stops_at_its_budget()                               // entries: 10 over a tree of 100 -> returns, no panic, found None
fn a_symlink_loop_does_not_hang_the_scan()
fn discovery_tries_explicit_then_games_then_xbox_then_url()   // explicit bad -> Err (no fall-through); with only an xbox copy -> Step::Xbox; with only a URL served by a local test server -> Step::Url and the download is gone after drop
fn an_encrypted_xbox_copy_is_reported_not_accepted()          // wrong-hash file in xbox root -> Err text contains "encrypted on disk or still downloading"
// neural::models::tests
fn models_are_extracted_once_and_verified()
fn a_wrong_extracted_hash_keeps_nothing()
fn staging_links_the_cached_models_into_the_package()         // same inode when on one filesystem
```
`dll::find` with a URL takes the URL as a parameter, so the test server's `http://` URL needs the private `https_only: false` path: give `find` the download function as a parameter in the private `find_with(…, download: &dyn Fn(&str, &Path) -> Result<()>)` and make the public `find` call it with `http::download_large_checked`.

- [ ] **Step 2: Run to verify they fail** — `$HOME/.cargo/bin/cargo test --locked -p ferestre-core -- settings:: neural::dll neural::models` → FAIL.
- [ ] **Step 3: Implement** the items above.
- [ ] **Step 4: Run to verify they pass** — same command → PASS.
- [ ] **Step 5: Commit** `Find nvngx_dlssnr.dll in the documented order and cache its model` (with the Co-Authored-By line).

### Task 5: Per-game state and the hardware check (`neural::state`, `neural::hardware`)

**Files:**
- Create: `crates/ferestre-core/src/neural/state.rs`, `crates/ferestre-core/src/neural/hardware.rs`

**Interfaces:**
- Produces:
  - `pub struct state::Entry { pub enabled: bool, pub route: Route, pub model: Network, pub package: String, pub arch: Arch, pub install_dir: PathBuf }` (serde kebab-case)
  - File `config_dir/neural-rendering.toml`: `[titles.<KEY>]` = `Entry`, `[acknowledged]` `<KEY> = ["<Detection.name>", …]`; no `deny_unknown_fields`; keys are `product_id.to_ascii_uppercase()`; written with `cache::write` (atomic, like `runtime-selections.toml`, `runtime.rs:406-497`); an unreadable file is an error naming it, never a silent blank slate.
  - `pub fn state::file(config_dir: &Path) -> PathBuf`; `pub fn get(config_dir, key: &str) -> Result<Option<Entry>>`; `pub fn put(config_dir, key, entry: &Entry) -> Result<()>`; `pub fn acknowledged(config_dir, key) -> Result<BTreeSet<String>>`; `pub fn acknowledge(config_dir, key, names: &[&str]) -> Result<()>`; `pub fn record(config_dir, key, entry: &Entry, acknowledged: &[&str]) -> Result<()>` (the entry and new acknowledgements in one atomic write: an acknowledgement is only ever stored with a successful enable); `pub fn forget(config_dir, key) -> Result<()>` (drops the entry, keeps acknowledgements); `pub fn all(config_dir) -> Result<BTreeMap<String, Entry>>`
  - `pub struct hardware::Gpu { pub vendor: u16, pub device: u16, pub gfx_major: Option<u32> }`; `pub const RDNA4_DEVICES: &[u16] = &[0x7550, 0x7551, 0x7590]`; `pub const MIN_MESA: (u32, u32) = (26, 2)`
  - `pub fn hardware::gpus(drm: &Path) -> Vec<Gpu>` (each `card*/device/{vendor,device}` hex; `gfx_major` from `card*/device/ip_discovery/die/0/GC/0/major` when present — verify that path on the RX 9070 XT machine while implementing)
  - `pub fn hardware::mesa_version(vulkan_summary: &str) -> Option<(u32, u32)>` (first `Mesa <major>.<minor>` in a `driverInfo` line)
  - `pub fn hardware::warnings(gpus: &[Gpu], mesa: Option<(u32, u32)>, checked_mesa: bool) -> Vec<String>`: no AMD GPU with an RDNA4 device id or `gfx_major == Some(12)` → `"No AMD RX 9000 (RDNA4) GPU was found; DLSSNR-AMD runs only on RDNA4 and stays off in game without it."`; Mesa older than 26.2 → `"Mesa <x.y> is older than 26.2, which DLSSNR-AMD needs."`; not checkable → `"Could not check the Mesa version (vulkaninfo is not installed); DLSSNR-AMD needs Mesa 26.2 or newer."`
  - `pub fn hardware::check() -> Vec<String>` (reads `/sys/class/drm`, runs `vulkaninfo --summary` with a 5 s limit when on `PATH`)

- [ ] **Step 1: Write the failing tests**
```rust
// state::tests
fn an_entry_round_trips_and_other_titles_survive_a_write()
fn acknowledgements_outlive_forgetting_the_entry()
fn record_writes_the_entry_and_its_acknowledgements_together()
fn an_unreadable_file_is_an_error_naming_it_and_is_not_overwritten()
// hardware::tests (fake sysfs trees in a tempdir)
fn an_rx_9070_xt_with_mesa_26_2_has_no_warnings()            // vendor 0x1002 device 0x7550, "driverInfo = Mesa 26.2.0" -> []
fn an_rdna3_card_and_old_mesa_both_warn()                     // device 0x744c, Mesa 25.3 -> two warnings
fn a_gfx12_device_id_not_in_the_list_still_counts()           // device 0x75ff, GC major 12 -> no GPU warning
```
- [ ] **Step 2: Run to verify they fail** — `$HOME/.cargo/bin/cargo test --locked -p ferestre-core -- neural::state neural::hardware` → FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run to verify they pass** → PASS.
- [ ] **Step 5: Commit** `Remember neural rendering per game and warn about unsupported GPUs` (with the Co-Authored-By line).

### Task 6: Enable, disable, verify and the launch env (`neural::apply`)

**Files:**
- Create: `crates/ferestre-core/src/neural/apply.rs`
- Modify: `crates/ferestre-core/src/neural/mod.rs` (`Context`, `overview`)

**Interfaces:**
- Consumes: Tasks 2–5; `anticheat::detect(game_dir, executable) -> Vec<Detection>` (PR A Task 6; `Detection { name, kernel, evidence }`)
- Produces:
  - `pub struct Context { pub root: PathBuf, pub config_dir: PathBuf, pub packages: Box<dyn Fn(Arch) -> Result<Package>>, pub expect: Expect }`; `impl Context { pub fn from_paths(paths: &Paths) -> Self }` (`packages` = `package::current(paths, arch)`, `expect` = `Expect::pinned()`)
  - `pub fn exe_dir(game_dir: &Path, executable: &str) -> PathBuf` (executable with `\` → `/`; an absolute executable is taken as is, a relative one is joined to `game_dir`; then its parent). Enabling refuses when that folder is not an existing directory.
  - `pub enum Gate { Allowed, NeedsAcknowledgement(Vec<&'static str>), Refused(Vec<&'static str>) }`; `pub fn gate(found: &[Detection], acknowledged: &BTreeSet<String>) -> Gate` (any `kernel` → `Refused(names of kernel ones)`; else unacknowledged names → `NeedsAcknowledgement`; else `Allowed`)
  - `pub const BACKUP_DIR: &str = ".ferestre-dlssnr-backup"`; `pub const INSTALL_MANIFEST: &str = "dlssnr-amd-install.txt"`; backup manifest `<exe dir>/.ferestre-dlssnr-backup/manifest.json` = `{"schema":1,"package":"0.0.9","route":"optiscaler","saved":[{"path":"dxgi.dll","dir":false}],"absent":["OptiScaler.ini", …]}`
  - `pub fn backup(exe_dir: &Path, files: &[RouteFile], package: &str, route: Route) -> Result<()>` — idempotent merge: a name already saved stays as it is; a name that exists now and is neither in the current install manifest nor (for a file) byte-equal to its package `source` is saved (copied, directories recursively) and leaves `absent`; everything else not existing is `absent`.
  - `pub fn restore(exe_dir: &Path, allowed: &BTreeSet<String>) -> Result<()>` — `allowed` is the set of `route_files` names for the manifest's recorded route. First validates the whole manifest and refuses (deleting nothing) unless every `saved[].path` and `absent[]` name is exactly one `Component::Normal` (not `""`, `.`, `..`, absolute, or containing `/` or `\`), is in `allowed`, is not `BACKUP_DIR`, and its saved copy inside `BACKUP_DIR` exists and is not a symlink. Then, for each saved entry, removes what is at `exe_dir/<name>` (via `symlink_metadata`: a symlink is unlinked, never followed) and renames the copy back; then removes `BACKUP_DIR`. The error names the manifest and says to restore by hand from `BACKUP_DIR`.
  - `pub fn run_install(package_dir: &Path, exe_dir: &Path, route: Route, model: Network, log: &Path) -> Result<()>` — `bash <pkg>/install.sh <exe_dir> <route> --model <M> --renodx --no-lumenite`, stdin null, stdout+stderr appended to `log`; on non-zero exit run `run_remove` and return an error whose text ends with the last 20 log lines.
  - `pub fn run_remove(package_dir: &Path, exe_dir: &Path, log: &Path) -> Result<()>` (`install.sh <exe_dir> remove`)
  - `pub fn verify(package_dir: &Path, exe_dir: &Path, route: Route) -> Result<(), String>` — install manifest present; every listed entry exists except names ending `.log`; every `injectors(route).required` name, and every `injectors(route).when_listed` name the install manifest lists, is byte-equal to its `RouteFile.source`. Nothing else is compared (so `optiscaler` with the game's own `vulkan-1.dll`, unlisted, verifies).
  - `pub struct EnableRequest<'a> { pub key: &'a str, pub game_dir: &'a Path, pub executable: &'a str, pub route: Option<Route>, pub model: Option<Network>, pub accept_anticheat_risk: bool }`
  - `pub struct Enabled { pub entry: Entry, pub detected: Detected, pub warnings: Vec<String> }`
  - `pub fn enable(ctx: &Context, req: &EnableRequest) -> Result<Enabled>` — order: anti-cheat gate (Refused → error `"<game> uses <names>, a kernel-level anti-cheat; neural rendering is never enabled for it."`; NeedsAcknowledgement without `accept_anticheat_risk` → error listing names and `--accept-anticheat-risk`; with it → the names are carried to the final `state::record`, nothing is written yet) → refuse a foreign `INSTALL_MANIFEST` with no Ferestre backup manifest (`"…was installed by hand; remove it with its own install.sh … remove first"`) → detect route (explicit wins; `None` → error asking for `--route`; `optiscaler` with a 32-bit exe → error) → arch from bits → `ensure` package → `expects_models` → `models::verify` (error `"no model cached; run: ferestre neural setup"` when empty) → model default `SmallFp8` if the small model is cached else `SilverAardwold`; asking for a small-model network without it is an error → `stage` → `backup` → `run_install` (on error: `restore`, nothing recorded) → `state::record(config_dir, key, &entry /* enabled: true */, &newly_acknowledged)`; any earlier failure stores neither the entry nor the acknowledgement. `warnings` = `hardware::check()` plus "assumed x86-64 D3D12" when `detected.assumed`.
  - `pub fn disable(ctx: &Context, key: &str, game_dir: &Path, executable: &str) -> Result<()>` — if the exe dir is gone, only `state::forget`; else `run_remove` with the entry's package (error if that package dir is gone: say to re-run `ferestre neural setup`), then `restore(exe_dir, names of route_files(pkg, entry.route))`, then `state::forget`. Any error leaves the state entry in place.
  - `pub enum Ensured { NotEnabled, Present(Entry), Reinstalled(Entry), Skipped(String), TakenOut(String), Blocked(String) }`; `pub fn ensure_installed(ctx: &Context, key: &str, game_dir: &Path, executable: &str) -> Ensured` — under an exclusive `flock` on `root/locks/<KEY>.lock`, in this order:
    1. No enabled entry → `NotEnabled`.
    2. **The anti-cheat gate runs on every call, `Present` included** (`gate(&anticheat::detect(..), &acknowledged)`). `Refused` or a new unacknowledged anti-cheat: when Ferestre's files are in the exe dir (install manifest or `BACKUP_DIR` present), run `disable` → `TakenOut(reason)` (the mod is out, the person's files are back, the entry is forgotten; the reason says how to turn it on again); if `disable` fails → `Blocked(reason)`. When no files are there → `Skipped(reason)`.
    3. `verify` passes and the entry's `package`/`install_dir` equal the current ones → `Present(entry)`.
    4. Otherwise reinstall: keep the person's `dlssnr-amd.ini` (copied into `BACKUP_DIR/dlssnr-amd.ini.user` before, put back after), `backup` (merge) → `run_install` → `put` with the new `package`/`install_dir` → `Reinstalled(entry)`. A failure here → `Skipped(reason)` (and `run_install` has already removed its partial files).
    Only `Present`/`Reinstalled` carry an entry, and only an entry makes the caller add the launch env (Task 7), so `Skipped`, `TakenOut` and `Blocked` never launch with `dxgi=n,b`, `vulkan-1=n,b` or the int4 layer. `Blocked` (Ferestre's files are still in a game whose anti-cheat now forbids them) is the one result that stops a launch.
  - `pub fn preview(ctx: &Context, key: &str, game_dir: &Path, executable: &str) -> Option<Entry>` — reads only (no lock, no install): the enabled entry when the gate allows it. Used by a dry run.
  - `pub fn dll_overrides(route: Route) -> &'static [(&'static str, &'static str)]` (`[("dxgi","n,b")]` or `[("winevulkan","n,b"),("vulkan-1","n,b")]`)
  - `pub fn merge_dll_overrides(existing: Option<&str>, ours: &[(&str, &str)]) -> (String, Vec<String>)` (returns the merged value and the names the person already set)
  - `pub struct LaunchEnv { pub set: Vec<(String, String)>, pub notes: Vec<String> }`; `pub fn launch_env(entry: &Entry, exe_dir: &Path, current: &dyn Fn(&str) -> Option<String>) -> LaunchEnv` — `current` is the value a key would have at launch (the caller decides what that is, see Task 7); `WINEDLLOVERRIDES` merged; for ReShade routes with `model == SilverAardwoldOptimized` and `exe_dir/dlssnr-amd/CC_SILVER_AARDWOLD_OPTIMIZED_INT4.bin` present, `VK_ADD_LAYER_PATH` gets `exe_dir/dlssnr-amd/int4/layer` appended with `:` and `VK_INSTANCE_LAYERS` gets `VK_LAYER_DLSSNR_int4` appended with `:`, each only if not already there; a note `"-- neural rendering: your WINEDLLOVERRIDES sets dxgi itself, so DLSSNR-AMD may not load"` per conflicting name.
  - `pub struct Overview { pub entry: Option<Entry>, pub detected: Detected, pub anticheat: Vec<Detection>, pub gate: Gate, pub installed: Option<Result<(), String>>, pub models: Vec<ModelFile>, pub package_ready: bool, pub hardware: Vec<String>, pub dll_url_configured: bool }`; `pub fn neural::overview(ctx: &Context, paths: &Paths, key: &str, game_dir: &Path, executable: &str) -> Overview` (reads only)

The test fixture is a fake package (helper `fake_package(dir) -> PathBuf`): `reshade/{dxgi.dll,ReShade.ini,reshade-shaders/x.fx}`, `vulkan/{vulkan-1.dll,winevulkan.dll}`, `optiscaler/release/{OptiScaler.dll,OptiScaler.ini,OptiScaler/}`, `dlssnr-amd/`, `model-tools/extract_model.sh`, and an `install.sh` (bash, ~40 lines) that emulates upstream: `remove` deletes manifest entries then the manifest; any route first removes a previous install, writes the manifest, copies `dlssnr-amd/`, the route's files and a default `dlssnr-amd.ini`, and exits with `${FAKE_INSTALL_EXIT:-0}` after writing (so failures are part-way, like `patch_ini.py`). The anti-cheat input is injected by testing `gate` directly and by private seams `enable_with(ctx, req, detections: &[Detection])` and `ensure_with(ctx, key, game_dir, executable, detections: &[Detection])` that `enable` and `ensure_installed` call with `anticheat::detect(…)`.

- [ ] **Step 1: Write the failing tests**
```rust
// fn det(name: &'static str, kernel: bool) -> Detection { Detection { name, kernel, evidence: PathBuf::new() } }
fn a_kernel_anti_cheat_refuses_even_when_risk_is_accepted()   // [det("Ricochet", true)] + accept=true -> Err, no file written in exe dir, state has no entry
fn another_anti_cheat_needs_an_acknowledgement_that_is_remembered() // [det("Easy Anti-Cheat", false)]: accept=false -> Err mentions "--accept-anticheat-risk"; accept=true -> Ok; acknowledged() contains "Easy Anti-Cheat"; second enable with accept=false -> Ok
fn another_mods_files_come_back_byte_identical_after_disable()  // exe dir has dxgi.dll "theirs", ReShade.ini, reshade-shaders/a.fx; enable reshade-feeder; dxgi.dll is the package's; disable -> all three byte-identical, BACKUP_DIR and INSTALL_MANIFEST gone
fn a_failed_install_is_rolled_back_and_not_recorded()           // [det("Easy Anti-Cheat", false)], accept=true, FAKE_INSTALL_EXIT=1 -> Err text has the log tail; dxgi.dll "theirs" back; state::get == None; acknowledged() is empty
fn a_tampered_backup_manifest_deletes_nothing()                 // enable; rewrite manifest.json saved[] to each of "..", "/tmp/x", "", "a/b", a name not in route_files -> disable is Err; exe dir and its parent unchanged (a canary file beside the exe dir survives); BACKUP_DIR still there
fn an_absolute_executable_installs_beside_itself()              // executable = "<tmp>/elsewhere/Game.exe" (absolute), game_dir another folder -> exe_dir == <tmp>/elsewhere; enable writes INSTALL_MANIFEST there and nothing in game_dir
fn optiscaler_verifies_vulkan_1_only_when_it_installed_it()     // game ships vulkan-1.dll: manifest does not list it, game's copy differs -> verify Ok; game has none: install writes it, corrupt it -> verify Err
fn a_game_folder_with_spaces_and_unicode_works()               // exe dir "Mé Game/Bin 64"
fn a_hand_made_install_is_refused()                             // INSTALL_MANIFEST present, no backup manifest -> Err "installed by hand"
fn a_wiped_install_is_redone_without_backing_up_our_own_files() // enable; delete dlssnr-amd/ and INSTALL_MANIFEST; ensure_installed -> Reinstalled(_); backup still holds the original "theirs" dxgi.dll
fn the_persons_ini_survives_a_reinstall()                        // edit dlssnr-amd.ini; break verify; ensure_installed -> ini content unchanged
fn a_present_install_is_left_alone()                             // ensure_with(.., []) twice -> Present(_), install.sh not run again (log line count unchanged)
fn a_launch_never_installs_past_a_new_kernel_anti_cheat()        // ensure_with(.., [det("Ricochet", true)]) after deleting the install -> Skipped(_), nothing reinstalled
fn a_new_anti_cheat_takes_an_intact_install_out()                // enable; files intact; ensure_with(.., [det("Ricochet", true)]) -> TakenOut(_): INSTALL_MANIFEST gone, "theirs" dxgi.dll back, state::get == None; same with an unacknowledged [det("BattlEye", false)]
fn the_persons_overrides_win_and_are_reported()                  // merge_dll_overrides(Some("dxgi=b;d3d11=n"), [("dxgi","n,b")]) == ("dxgi=b;d3d11=n", vec!["dxgi"]); (None, vulkan) == ("winevulkan=n,b;vulkan-1=n,b", [])
fn the_int4_layer_is_added_only_for_reshade_routes_starting_optimized() // optiscaler+OPTIMIZED -> no VK_*; reshade-feeder+OPTIMIZED+int4 file -> VK_ADD_LAYER_PATH ends "dlssnr-amd/int4/layer", existing "/x" kept as "/x:<ours>"
```
- [ ] **Step 2: Run to verify they fail** — `$HOME/.cargo/bin/cargo test --locked -p ferestre-core neural::apply` → FAIL.
- [ ] **Step 3: Implement** the items above.
- [ ] **Step 4: Run to verify they pass** — same command → PASS; then `$HOME/.cargo/bin/cargo test --locked -p ferestre-core` → PASS.
- [ ] **Step 5: Commit** `Install, verify and remove DLSSNR-AMD per game with backups` (with the Co-Authored-By line).

### Task 7: Launch integration and uninstall

**Files:**
- Modify: `crates/ferestre-cli/src/main.rs` — `prepare_launch` (dry run only, after the `add_anticheat_runtimes(…)` call at `:856-862`), `launch_prepared` (`:879`: destructure `mut plan`; the hook right after `run_setup(&recipe, "every-launch", …)` at `:947`, before `start_service` at `:974`), `uninstall_title` (`:1428-1603`: the Xbox path beside `install::forget` at `:1577`, and PR A Task 12's local branch before its `local::forget`), new helpers beside `add_anticheat_runtimes` (`:3288`)

**Interfaces:**
- Consumes: `apply::{launch_env, ensure_installed, preview, disable, exe_dir, Ensured}`, `state::{get, forget}`, `Context::from_paths` (Task 6); `launch::Plan::{get, is_unset, to_command}`; PR A's `local::forget`
- Produces:
  - `fn add_neural_rendering(plan: &mut launch::Plan, entry: &Entry, exe_dir: &Path)` — applies `launch_env(entry, exe_dir, &current)` where `current = |k| plan.env_set.get(k).cloned().or_else(|| if plan.is_unset(k) { None } else { std::env::var(k).ok() })` (a key the plan strips, such as Steam's `VK_INSTANCE_LAYERS`, is never read back from the inherited environment); inserts each result into `plan.env_set` **and removes that key from `plan.env_unset`**; prints each note on stderr.
  - `fn neural_before_launch(plan: &mut launch::Plan, ctx: &neural::Context, product_id: &str, game_dir: &Path, executable: &str) -> Result<()>` — `ensure_installed`; `Present(e)`/`Reinstalled(e)` → `add_neural_rendering(plan, &e, &exe_dir(..))` (plus `:: neural rendering: reinstalled` for the latter); `Skipped(r)` → `-- neural rendering is off for this launch: <r>`, plan untouched; `TakenOut(r)` → `-- neural rendering was turned off for <name>: <r>`, plan untouched; `Blocked(r)` → `Err` (the launch stops before anything starts); `NotEnabled` → nothing.
  - `fn launch_prepared_with(paths: &Paths, prepared: PreparedLaunch, force: bool, neural: &neural::Context) -> Result<ExitCode>`; `launch_prepared(paths, prepared, force)` becomes `launch_prepared_with(paths, prepared, force, &neural::Context::from_paths(paths))`. Tests pass a `Context` whose `expect` holds their fake model hashes.
  - Dry run: in `prepare_launch`, only when `request.dry_run`, `apply::preview(..)` → `Some(e)` → `add_neural_rendering(&mut plan, &e, &exe_dir)`, so `--dry-run` shows what a launch would add, writing nothing. A real launch adds nothing in `prepare_launch`.
  - `uninstall_title`, Xbox path: when the files are removed, `neural::state::forget(paths.config_dir(), &product_id)`; with `--keep-files` nothing changes. Local path (PR A Task 12, which never reaches `:1577`): before `local::forget`, if `state::get` has an enabled entry, `apply::disable(&ctx, key, &paths.install_dir(&recipe), &recipe.launch.executable)` (mod out, the person's files back, entry forgotten) and print `-- neural rendering removed from <name>`; if that fails, bail with the error and `turn it off first: ferestre neural disable <title>` and forget nothing.

- [ ] **Step 1: Write the failing tests** in `main.rs` `mod tests`, using `Home` (`:6320`) and its `installed_package`/`game_recipe` helpers as the existing launch tests at `:7069-7128` do: write `config/ferestre/neural-rendering.toml` by `ferestre_core::neural::state::put`, a fake package via `FERESTRE_DLSSNR_PACKAGE_DIR` in `Home.extra`, fake cached models, and a `det(name, kernel)` helper.
```rust
fn an_enabled_title_gets_its_dll_overrides_merged_into_the_plan()
    // neural_before_launch on a plan with WINEDLLOVERRIDES "d3d11=n" from the recipe; entry route reshade-feeder, install intact
    // -> plan.get("WINEDLLOVERRIDES") == Some("d3d11=n;dxgi=n,b")
fn the_int4_layer_survives_the_steam_environment_strip()
    // steam_integration false, VK_INSTANCE_LAYERS in plan.env_unset (as launch::plan leaves it); reshade-feeder, CC_SILVER_AARDWOLD_OPTIMIZED, int4 file present
    // -> !plan.is_unset("VK_INSTANCE_LAYERS"); plan.to_command().get_envs() has VK_INSTANCE_LAYERS == Some("VK_LAYER_DLSSNR_int4") exactly
fn a_disabled_or_unknown_title_leaves_the_plan_alone()
fn a_new_kernel_anti_cheat_leaves_no_neural_override_in_the_launch()
    // enabled, files intact, then a Ricochet driver file appears in the game dir -> after neural_before_launch the plan has no "dxgi=n,b"
    // and no VK_INSTANCE_LAYERS; the install manifest is gone and the state entry forgotten
fn a_dry_run_reinstalls_nothing()                       // cmd_run with --dry-run on an enabled title whose install is gone: exe dir unchanged
fn a_launch_reinstalls_neural_rendering_that_an_update_removed()
    // enabled, install manifest deleted; launch_prepared_with(&paths, prepare_launch(..)?, false, &ctx) -> Err (Home has no
    // xodus-service binary, so start_service stops the launch after the hook, before anything starts); the install manifest is back
fn removing_a_title_forgets_its_neural_rendering()
fn removing_a_local_game_takes_neural_rendering_out_first()
    // local recipe (PR A), exe dir with "theirs" dxgi.dll; enable through apply::enable; uninstall_title -> dxgi.dll is "theirs" again,
    // no INSTALL_MANIFEST, no BACKUP_DIR, state::get == None, the game folder still exists; with the package dir removed first,
    // uninstall_title is Err mentioning "ferestre neural disable" and the local recipe is still there
```
- [ ] **Step 2: Run to verify they fail** — `$HOME/.cargo/bin/cargo test --locked -p ferestre-cli -- neural int4 an_enabled_title_gets a_disabled_or_unknown_title a_dry_run_reinstalls_nothing` → FAIL (every test above matches one filter).
- [ ] **Step 3: Implement** the items above.
- [ ] **Step 4: Run to verify they pass** — same command → PASS; then `$HOME/.cargo/bin/cargo test --locked -p ferestre-cli` → PASS.
- [ ] **Step 5: Commit** `Re-check neural rendering before every launch and merge its overrides` (with the Co-Authored-By line).

### Task 8: `ferestre neural` commands

**Files:**
- Modify: `crates/ferestre-cli/src/main.rs` — `Cmd` (`:96-232`, new variant before `Env`), `dispatch` (`:265-335`), new `cmd_neural_*` fns after `cmd_inspect_package` (`:338`), tests in `mod tests` (`every_subcommand_the_placeholder_had_still_parses` at `:5694` stays; add new tests)
- Create: `docs/NEURAL-RENDERING.md`; modify `docs/ROADMAP.md`

**Interfaces:**
- Consumes: everything in Tasks 2–7; `recipes_from(cli, paths)`, `Recipe::matches`, `Paths::install_dir`, `TitleLock::take(paths, id, LockMode::Change)`, `print_json`, `JSON_SCHEMA`, `usage()`
- Produces (clap):
```rust
/// Experimental: NVIDIA's DLSS 5 neural rendering on AMD RX 9000 cards, through DLSSNR-AMD.
Neural { #[command(subcommand)] command: NeuralCmd },

enum NeuralCmd {
    /// What is set up, and per game what is enabled; with a title, its route, anti-cheat and checks.
    Status { #[arg(value_name = "TITLE")] title: Option<String> },
    /// Download the package and extract the model from nvngx_dlssnr.dll (found, or given with --dll).
    Setup { #[arg(long, value_name = "PATH")] dll: Option<PathBuf> },
    Enable { #[arg(value_name = "TITLE")] title: String,
             #[arg(long, value_name = "ROUTE")] route: Option<String>,
             #[arg(long, value_name = "NETWORK")] model: Option<String>,
             #[arg(long)] accept_anticheat_risk: bool },
    Disable { #[arg(value_name = "TITLE")] title: String },
}
```
Behaviour: `setup` = `package::ensure` for x86_64 (i686 is fetched by `enable` when a 32-bit game needs it), then if `models::verify` is empty, `dll::find` with `dll::locations(paths, &recipes)`, `settings::dlssnr_dll_url(paths)?` and `--dll`, printing progress lines `:: <step>`; then `models::extract`. `enable` runs `setup`'s model part first when no model is cached (no explicit path), takes the `Change` lock, calls `apply::enable`, prints warnings with `-- ` and the final line `:: neural rendering enabled for <name> (<route>, <network>)`. `disable` takes the lock and calls `apply::disable`. `status` text and `--json` (`{"schema":…, "package":…, "models":[…], "dll_url_configured":bool, "hardware":[…], "titles":{KEY:{…}}}`; with a title, `overview` fields). Bad `--route`/`--model` → `usage()` error listing the valid values.

- [ ] **Step 1: Write the failing tests**
```rust
fn the_neural_commands_parse()   // ["ferestre","neural","status"], ["…","setup","--dll","/x/nvngx_dlssnr.dll"], ["…","enable","bedrock","--route","reshade-feeder","--model","CC_SMALL_FP8","--accept-anticheat-risk"], ["…","disable","bedrock"]
fn an_unknown_route_is_a_usage_error_naming_the_routes()
fn enabling_without_a_model_says_how_to_get_one()     // Home with no models and no URL, explicit setup skipped -> Err text contains "ferestre neural setup" and "dll-url"
fn status_json_reports_an_enabled_title()             // state put -> json["titles"]["9NBLGGH2JHXJ"]["route"] == "optiscaler"
fn status_never_prints_a_dll_url_it_was_not_given()   // no setting -> "dll_url_configured": false
```
- [ ] **Step 2: Run to verify they fail** — `$HOME/.cargo/bin/cargo test --locked -p ferestre-cli -- neural an_unknown_route enabling_without_a_model status_json_reports status_never_prints` → FAIL (every test above matches one filter).
- [ ] **Step 3: Implement**, and write `docs/NEURAL-RENDERING.md`: what it is (experimental, RX 9000 + Mesa 26.2, single-player), where the DLL comes from in the exact discovery order, `[dlssnr] dll-url` / `FERESTRE_DLSSNR_DLL_URL` (with a placeholder URL `https://example.invalid/…`, never a real one), the anti-cheat rules, what is backed up and where, how to turn it off, logs (`state/dlssnr/logs/<KEY>.log`, the game's `dlssnr-amd.log`), and the Xbox caveat (an encrypted or not-yet-downloaded DLL in an install is skipped). Add two lines to `docs/ROADMAP.md`: neural rendering (experimental), and the tracked follow-up "fetch `nvngx_dlssnr.dll` from an installed Xbox package through `xodus-cli streaming --only <path>` (needs the `icex/xodus` fork change)".
- [ ] **Step 4: Run to verify they pass** — `$HOME/.cargo/bin/cargo test --locked -p ferestre-cli` → PASS; `python3 tests/test_no_nvidia_urls.py` → OK.
- [ ] **Step 5: Commit** `Add ferestre neural status, setup, enable and disable` (with the Co-Authored-By line).

### Task 9: GUI — per-game dialog and the Settings field

**Files:**
- Create: `crates/ferestre-gui/src/neural_ui.rs`
- Modify: `crates/ferestre-gui/src/main.rs` — `mod neural_ui;` (module list `:12-33`), the per-row menu in `library_components`: a separate block after the `(row.has_recipe || row.installed)` loop (`:2886-2902`, which also runs for titles that are not installed, so the item is not added to its array): `if row.installed && row.action != Action::Content { item("video-display-symbolic", "Neural rendering…", …neural_ui::open…) }`, `render_runtime` (`:1149-1231`: a new `adw::PreferencesGroup` after the runtime group)

**Interfaces:**
- Consumes: `neural::{Context, overview, Overview, Route, Network}`, `neural::apply::Gate`, `settings::{file_dlssnr_dll_url, set_dlssnr_dll_url}`; GUI helpers `spawn_cli_tracked(ui, args, what)` (`:4229`), `toast(text)` (`:196`), `plain_row` (`:204`), `render(ui)`, `Ui::model` (`state::Model::{paths, recipe_for, game_dir}`), the dialog shape of `runtime_chooser` (`:1982-2100`: `adw::Dialog`, vertical `gtk::Box` with 18 px margins, `ScrolledWindow`, Cancel button) and the background pattern of `fetch_disk_usage` (`glib::spawn_future_local` + `gio::spawn_blocking`)
- Produces:
  - `pub fn neural_ui::open(ui: &Ui, product_id: &str, name: &str)` — computes `overview` with `gio::spawn_blocking`, then presents the dialog.
  - `pub(crate) struct View { pub summary: String, pub can_enable: bool, pub blocked_because: Option<String>, pub acknowledge: Vec<&'static str>, pub warnings: Vec<String>, pub route: Option<Route>, pub enabled: bool }`; `pub(crate) fn view(o: &Overview) -> View` (pure).
  - `pub(crate) fn enable_args(product_id: &str, route: Option<Route>, model: Option<Network>, acknowledged: bool) -> Vec<String>`.

Dialog contents: status line (`view.summary`), warnings (hardware, "assumed"), route `gtk::DropDown` ("Automatic (<detected>)" + the four routes), network `gtk::DropDown` (Automatic + four), for `NeedsAcknowledgement` a `gtk::CheckButton` "I understand <names> may ban accounts that load mods, and this game is single-player for me" that the Enable button's sensitivity follows, for `Refused` an insensitive Enable with the reason as its tooltip and text, Enable → `spawn_cli_tracked(ui, &["neural","enable",pid,…], "Enabling neural rendering")`, Disable (when enabled) → `["neural","disable",pid]`, Cancel. Settings group "Neural rendering (experimental)": an `adw::EntryRow` "nvngx_dlssnr.dll download address" prefilled with `settings::file_dlssnr_dll_url(config_dir)` (the file, never the environment), `show_apply_button(true)`, `connect_apply` → `set_dlssnr_dll_url` (empty text → `None`), toast on error; a row showing which models are cached with a "Set up" button → `spawn_cli_tracked(ui, &["neural","setup"], "Setting up neural rendering")`.

- [ ] **Step 1: Write the failing tests** in `neural_ui.rs` `#[cfg(test)]`:
```rust
// Overview fixtures build anti-cheat lists with a det(name, kernel) helper (Detection has `evidence`)
fn a_kernel_anti_cheat_disables_enable_and_says_why()     // Gate::Refused(["Ricochet"]) -> can_enable false, blocked_because contains "Ricochet"
fn another_anti_cheat_asks_for_an_acknowledgement()        // NeedsAcknowledgement(["Easy Anti-Cheat"]) -> acknowledge == ["Easy Anti-Cheat"], can_enable true
fn an_undetectable_route_still_lets_the_person_pick_one()  // detected.route None -> route None, can_enable true
fn enable_args_carry_only_what_was_chosen()                // (None, None, false) == ["neural","enable","9NBLGGH2JHXJ"]; with ack adds "--accept-anticheat-risk"
```
- [ ] **Step 2: Run to verify they fail** — `$HOME/.cargo/bin/cargo test --locked -p ferestre-gui neural_ui` → FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Verify** — `$HOME/.cargo/bin/cargo test --locked -p ferestre-gui` → PASS; `$HOME/.cargo/bin/cargo clippy --locked --workspace --all-targets -- -D warnings` → clean; launch the window, open a game's menu → "Neural rendering…" shows status; Settings shows the new group; `python3 tests/gui_smoke.py` still passes.
- [ ] **Step 5: Commit** `Add the neural rendering dialog and its Settings field` (with the Co-Authored-By line).

### Task 10: Publish `dlssnr-0.0.9` and pin it

Needs a maintainer: the tag push runs the `release`-environment job.

**Files:**
- Modify: `crates/ferestre-core/src/neural/package.rs` (the two pinned constants), add `#[cfg(test)] mod network` inside it (path `neural::package::network`) with an `#[ignore = "downloads from github.com"]` test like `anticheat::network` (`anticheat.rs:372-374`)

- [ ] **Step 1:** After Tasks 1–9 are merged to `main`: `git tag -a dlssnr-0.0.9 -m 'DLSSNR-AMD 0.0.9 (9503f6b) for Ferestre' && git push origin dlssnr-0.0.9`; wait for `.github/workflows/dlssnr.yml` to publish; download `SHA256SUMS` and check that it lists both tarballs and `SOURCES.txt`, and each `.minisig` with the key in `crates/ferestre-core/src/signing.rs`.
- [ ] **Step 2: Write the test** `network::both_pinned_packages_download_and_unpack` (ensure → `installed` is `Some`, `install.sh` and `model-tools/extract_model.sh` exist, no `CC_*.bin` inside).
- [ ] **Step 3:** Fill both pins: `url` = `https://github.com/icex/ferestre/releases/download/dlssnr-0.0.9/ferestre-dlssnr-0.0.9-<arch>.tar.gz`, `sha256` from `SHA256SUMS`.
- [ ] **Step 4: Verify** — `$HOME/.cargo/bin/cargo test --locked -p ferestre-core -- --include-ignored neural::package::network` → PASS; `$HOME/.cargo/bin/cargo test --locked --workspace` → PASS; on the RX 9070 XT machine: `ferestre neural setup --dll <own copy>`, `ferestre neural enable <a single-player DX12 game>`, play, F6 toggles NR, `ferestre neural disable <game>` leaves the folder as it was (`diff -r` against a copy taken before).
- [ ] **Step 5: Commit** `Pin the published DLSSNR-AMD 0.0.9 package` (with the Co-Authored-By line).

---

## Self-review notes

- Spec coverage: package built from a pin and hosted (T1, T10); pinned SHA-256 download (T2); DLL hashes, discovery order, zip, no URL in repo (T1 guard, T4); model cached once, DLL not kept (T4); per-game enable/disable/route/network, `install.sh` non-interactive, backups, env merge, re-check every launch (T6, T7); anti-cheat gate with remembered acknowledgement (T5, T6); hardware warning (T5); CLI (T8); GUI dialog and Settings field (T9); Xbox and local recipes both go through `Paths::install_dir` + `recipe.launch.executable` (T6–T8).
- Not done literally: discovery step 3 does not run `xodus-cli`; it reads files the install already decrypted (see "Xbox packages and `xodus-cli`"). A DLL kept encrypted on disk or not yet downloaded is reported, not fetched. This is a spec deviation flagged at the top for the spec owner's sign-off, with the follow-up tracked in `docs/ROADMAP.md` (Task 8).
- Anti-goals re-checked after review: never enabled silently on an anti-cheat game (gate on enable *and* every launch, `TakenOut`/`Blocked`, env only after the gate: T6, T7); never deletes the person's files (validated restore manifest, local Remove takes the mod out first: T6, T7); no NVIDIA URL (T1 guard; settings/env only: T4).
- Type consistency: `Expect`/`DllVersion`/`ModelFile` are owned everywhere (T2, T4, T6, T7 tests); `Ensured::{Present, Reinstalled}` carry the `Entry` that `add_neural_rendering` takes (T6, T7); `restore` takes the allowed names in every caller (`enable` rollback, `disable`); `Detection` has `evidence` and is built through `det()` in tests (T6, T7, T9); `injectors` returns `Injectors` (T2, T6).
- Commands: every `cargo test` with more than one filter puts the filters after `--`.

## Review notes

Review of the first draft; every issue was checked against the worktree. Accepted and fixed: unsigned-able release (`SOURCES.txt` now in `SHA256SUMS`, plus a test), the int4 env stripped by `env_unset` and Steam's inherited layers read back, the anti-cheat gate skipped for an intact install and env added before the check, local Remove leaving the mod and backups, invalid multi-filter `cargo test` commands, `Detection` missing `evidence`, the unvalidated restore manifest, the duplicate Steam library reader, the placeholder pin-refusal test, the vague `test_workflows` edits and `$version` vs `$tag_version`, the hook test not going through `launch_prepared`, the GUI item shown for titles not installed, the GUI prefill needing a file-only getter, `&'static str` hashes in test-built expectations, the contradictory `verify` rule for `optiscaler`'s `vulkan-1.dll`, the acknowledgement saved before a failed enable, absolute executables from PR A, `--latest=false`, and the over-broad Task 10 filter.

Accepted in part: "a spec goal has no task" (the `xodus-cli` Xbox step). The deviation is real and is now flagged at the top as needing the spec owner's sign-off, and the follow-up is tracked in `docs/ROADMAP.md` (Task 8). No task was added for it: the change belongs in the `icex/xodus` fork (its `extract` re-reads a whole package container, and the `.xodus-streaming.msixvc` kept in an install folder holds no file data), so it cannot be planned or tested from this repository, and this plan does not amend the spec on its own authority.

Rejected: none.
