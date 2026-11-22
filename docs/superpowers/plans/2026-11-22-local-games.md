# Local Games Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Windows games Ferestre did not install (hand-added `.exe`s and every Steam non-Steam shortcut) become recipes that launch through `ferestre run <id>` and the existing Proton shim, with no Xbox service touched.

**Architecture:** A recipe gets `[title] kind = "local"`, a synthetic `LOCAL-…` id that can never be a Store product id, an absolute game folder and an absolute or managed prefix. The few Xbox-only chokepoints (`launch::plan`, `prepare_launch`/`launch_prepared`, uninstall, catalog lookups, the GUI row) branch on `Recipe::is_local()`. Steam import reads `shortcuts.vdf` with the existing binary parser, `config.vdf`/`libraryfolders.vdf`/`compatibilitytool.vdf` with a new minimal text-VDF reader, and writes user recipes with the existing write-if-absent path, so a re-import never overwrites an edit.

**Tech Stack:** Rust 1.88 workspace (`ferestre-core`, `ferestre-cli` with clap, `ferestre-gui` with GTK4/libadwaita), serde/toml, the existing `steam::Value` tree, Python AT-SPI GUI tests (`tests/gui_*.py`).

**Spec:** `docs/superpowers/specs/2026-11-22-local-games-and-neural-rendering-design.md` (section A only; B is a later plan).

Cargo is `$HOME/.cargo/bin/cargo`; every command below is run from the worktree root. Every commit message ends with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Global Constraints

- Base `origin/main`; PRs target `main`. Rust 1.88, `cargo test --locked --workspace` green after every task.
- No hardcoded machine paths; everything derives from `Paths` (Steam roots come from `Paths`, not literals in callers).
- Recipes keep `deny_unknown_fields`; every new key is optional (`#[serde(default, skip_serializing_if = …)]`) so old recipes parse and Xbox recipes serialise byte-for-byte as before.
- `ferestre-core` never links the Xodus client (runs it as a child).
- Any new `[[setup]]` action is mirrored in `titles/validate.py`, `titles/SCHEMA.md` and the setup-action test. (This plan adds none.)
- Anti-goal: "Remove" on a local game never deletes the game folder or its prefix — not with `--prefix`, not with `--yes`, not through `check_removable`.
- Anti-goal: a local launch never runs `xodus-cli`, `xodus-service`, presence, WinRT registration, PlayFab repair, account roles, related-content binding or catalog/Store lookups.
- Anti-goal: no change to any Xbox launch, install, update or runtime selection. The existing tests in `launch.rs`, `paths.rs`, `runtime.rs`, `install.rs`, `ferestre-cli` and `ferestre-gui` stay green unmodified, except where a struct gains a field and its test literal must name it, or a test helper is split so a new test can reuse part of it (`state.rs`'s `model_at`).
- Import never modifies Steam: no write, rename or backup under any Steam root.
- No download URL for `nvngx_dlssnr.dll` or any NVIDIA file anywhere in this PR.
- Non-goals: Lutris/Heroic/Epic import, SteamGridDB, editing Steam shortcuts.

## Review Focus

1. **A local game on a drive that is not mounted** (`/run/media/...` absent). Expect: the row says the executable was not found and offers no Install; `ferestre run` fails naming the path and creates no prefix and no folder. Pinned in Task 11 (`a_local_game_whose_drive_is_gone_is_not_launched`) and Task 14 (`a_missing_local_executable_blocks_play_without_offering_install`).
2. **Steam launch options with quoted paths, `~`/`$HOME` in `STEAM_COMPAT_DATA_PATH`, no `%command%`, or shell operators.** Expect: quotes honoured, home expanded only in the prefix value, no-`%command%` text becomes game arguments, shell operators make the import keep the game but drop the options with a warning. Pinned in Task 8.
3. **Re-importing after the person edited the imported recipe or chose another runtime.** Expect: the edit and the choice stay; the report says "kept". Pinned in Task 9 (`a_second_import_keeps_edits_and_runtime_choices`).
4. **Removing a local game whose prefix is Steam's `compatdata/<appid>` (or lies inside the games directory).** Expect: nothing on disk is deleted even with `--prefix --yes`; only the recipe, runtime choice and copied art go. Pinned in Task 4 and Task 12.
5. **The same Steam install reached twice** (`~/.steam/steam` is a symlink to `~/.local/share/Steam`) **or the same shortcut in two Steam users.** Expect: one root, one recipe per shortcut app id. Pinned in Task 2 (`steam_roots_are_deduplicated_by_their_real_path`) and Task 9 (`a_shortcut_in_two_steam_users_is_imported_once`).

---

### Task 1: Local recipes in the schema

**Files:**
- Create: `crates/ferestre-core/src/local.rs` (id helpers only in this task)
- Modify: `crates/ferestre-core/src/lib.rs` (add `pub mod local;`)
- Modify: `crates/ferestre-core/src/recipe.rs:49-62` (`Title`), `:87-99` (`Launch`), `:281-328` (`Recipe::parse`), `:556-615` (helpers), `:618` (`slugify` visibility)
- Modify: `titles/SCHEMA.md` (new section "Local games (user recipes only)")
- Test: in-module tests in `recipe.rs` and `local.rs`

**Interfaces:**
- Produces in `recipe.rs`:
  - `#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)] #[serde(rename_all = "kebab-case")] pub enum TitleKind { #[default] Xbox, Local }` and `impl TitleKind { pub fn is_xbox(&self) -> bool }`.
  - `Title.kind: TitleKind` with `#[serde(default, skip_serializing_if = "TitleKind::is_xbox")]`.
  - `Launch.args: Vec<String>` and `Launch.wrapper: Vec<String>`, both `#[serde(default, skip_serializing_if = "Vec::is_empty")]`. `wrapper` is a flat argv prefix (`["gamemoderun", "mangohud"]`), exactly what Steam puts before `%command%`.
  - `Recipe::is_local(&self) -> bool`.
  - `pub fn is_store_product_id(id: &str) -> bool` — 12 ASCII alphanumerics (the rule `ferestre-cli::is_product_id` has today).
  - `pub(crate) fn slugify(name: &str, product_id: &str) -> String` (was private).
- Produces in `local.rs`:
  - `pub const ID_PREFIX: &str = "LOCAL-";`
  - `pub fn is_local_id(id: &str) -> bool` — starts with `LOCAL-` (ASCII case-insensitive), at most 64 bytes, remainder non-empty and only `[A-Za-z0-9-]`.
  - `pub fn steam_id(app_id: u32) -> String` → `"LOCAL-STEAM-<app_id decimal>"`.
  - `pub fn new_id(name: &str, taken: &dyn Fn(&str) -> bool) -> String` → `"LOCAL-" + slugify(name, "game").to_ascii_uppercase()` (`slugify` falls back to its second argument lowercased, so an unsluggable name gives `LOCAL-GAME`; the slug is cut to 40 bytes and a trailing `-` trimmed), then `-2`, `-3`… until `taken` says no.

- [ ] **Step 1: Write the failing tests** (`recipe.rs` tests)

```rust
fn local(extra: &str) -> anyhow::Result<Recipe> {
    Recipe::parse(&format!(r#"schema = 1
[title]
product-id = "LOCAL-STEAM-3000000001"
name = "Cyberpunk 2077"
slug = "local-steam-3000000001"
kind = "local"
[install]
dir = "/run/media/me/Games/Cyberpunk 2077"
[launch]
executable = 'bin/x64/Cyberpunk2077.exe'
args = ["--launcher-skip"]
wrapper = ["gamemoderun"]
[status]
state = "untested"
summary = "Imported from Steam"
{extra}"#))
}

#[test]
fn a_local_recipe_parses_and_round_trips() {
    let recipe = local("").unwrap();
    assert!(recipe.is_local());
    assert_eq!(recipe.launch.args, ["--launcher-skip"]);
    assert_eq!(recipe.launch.wrapper, ["gamemoderun"]);
    let again = Recipe::parse(&recipe.to_toml().unwrap()).unwrap();
    assert!(again.is_local());
    assert_eq!(again.install.dir.as_deref(), Some("/run/media/me/Games/Cyberpunk 2077"));
}

#[test]
fn an_xbox_recipe_serialises_without_the_new_keys() {
    let text = Recipe::blank("9ZZTESTNEW1", "Test").to_toml().unwrap();
    for key in ["kind", "args", "wrapper"] {
        assert!(!text.contains(&format!("{key} =")), "{text}");
    }
}

/// No args, no wrapper, slug = lowercase id: only the kind/id rule can refuse it.
fn bare(id: &str, kind: &str) -> String {
    format!("schema = 1\n[title]\nproduct-id = \"{id}\"\nname = \"G\"\nslug = \"{}\"\n{kind}\
             [install]\ndir = \"/g\"\n[launch]\nexecutable = \"G.exe\"\n\
             [status]\nstate = \"untested\"\nsummary = \"s\"\n", id.to_ascii_lowercase())
}

#[test]
fn kind_and_id_must_agree_both_ways() {
    assert!(Recipe::parse(&bare("LOCAL-G", "kind = \"local\"\n")).is_ok());
    for text in [bare("LOCAL-G", ""), bare("9ZZTESTNEW1", "kind = \"local\"\n")] {
        let err = Recipe::parse(&text).unwrap_err().to_string();
        assert!(err.contains("kind"), "{err}\n{text}");
    }
}

#[test]
fn a_local_recipe_is_refused_unless_every_local_rule_holds() {
    let good = local("").unwrap().to_toml().unwrap();
    // toml writes plain strings double-quoted; every `from` below must occur in `good`.
    for (from, to) in [
        ("slug = \"local-steam-3000000001\"", "slug = \"cyberpunk\""),     // slug must be the lowercase id
        ("dir = \"/run/media/me/Games/Cyberpunk 2077\"", "dir = \"Cyberpunk\""), // folder must be absolute
        ("dir = \"/run/media/me/Games/Cyberpunk 2077\"", "dir = \"/run/media/../etc\""),
        ("executable = \"bin/x64/Cyberpunk2077.exe\"", "executable = \"../x.exe\""),
        ("executable = \"bin/x64/Cyberpunk2077.exe\"", "executable = '..\\x.exe'"), // `\` counts as a separator
        ("[launch]", "content = [\"9NBLGGH2JHXJ\"]\n[launch]"),           // no companion content
        ("[launch]", "dir-env = \"LOCAL_STEAM_3000000001_DIR\"\n[launch]"),
        ("[launch]", "prefix-env = \"LOCAL_STEAM_3000000001_PREFIX\"\n[launch]"),
    ] {
        assert!(good.contains(from), "fixture no longer serialises {from:?}:\n{good}");
        let text = good.replace(from, to);
        assert!(Recipe::parse(&text).is_err(), "accepted: {from} -> {to}");
    }
}

#[test]
fn only_local_recipes_take_arguments_or_wrappers() {
    let mut text = Recipe::blank("9ZZTESTNEW1", "Test").to_toml().unwrap();
    text = text.replace("executable = \"\"", "executable = \"Game.exe\"\nargs = [\"-x\"]");
    assert!(Recipe::parse(&text).unwrap_err().to_string().contains("local"));
}

#[test]
fn store_ids_and_local_ids_never_overlap() {
    assert!(is_store_product_id("9NBLGGH2JHXJ"));
    for id in ["LOCAL-STEAM-1", "LOCAL-ABCDEFG", "local-x", "9NBLGGH2JHX", "../../etc"] {
        assert!(!is_store_product_id(id), "{id}");
    }
}
```

The three last rows insert keys into `[install]` (the `to_toml` output has `[install]` directly before `[launch]`); if the serialised order differs, insert them after the `dir = …` line instead.

`local.rs` tests: `is_local_id` accepts `LOCAL-STEAM-1`, `local-cyberpunk-2077`, refuses `LOCAL-`, `LOCAL-a/b`, `LOCAL-` + 70 chars, `9NBLGGH2JHXJ`; `steam_id(2_876_543_210) == "LOCAL-STEAM-2876543210"`; `new_id("Cyberpunk 2077", &|id| id == "LOCAL-CYBERPUNK-2077") == "LOCAL-CYBERPUNK-2077-2"`; `new_id("ゲーム", &|_| false) == "LOCAL-GAME"`.

- [ ] **Step 2: Run the tests to see them fail**

Run: `$HOME/.cargo/bin/cargo test --locked -p ferestre-core --lib -- recipe::tests local::tests`
Expected: compile errors (`kind`, `args`, `wrapper`, `is_local`, `local` module unknown).

- [ ] **Step 3: Implement the fields, `is_local`, `is_store_product_id`, the `local.rs` id helpers, and the `Recipe::parse` rules**

Rules `Recipe::parse` adds after the existing `dir-env`/`prefix-env` loop (error text names the key):
- First, `kind == Local` ⇔ `local::is_local_id(product_id)`; the message names `[title].kind`.
- Local: `slug == product_id.to_ascii_lowercase()`; `install.dir` present, absolute, no `..` component; `install.prefix` absent, or absolute without `..`, or relative (left to `Paths::prefix_dir`'s containment check); `install.content` empty, `dir-env` and `prefix-env` absent; `launch.executable` non-empty with no `..` component after `\` is read as `/` (absolute, or relative to `install.dir`; either separator is accepted, as Steam and Windows paths mix them).
- Xbox: `launch.args` and `launch.wrapper` empty, message "only local games take arguments or wrappers: the client's run command takes none".

`Recipe::blank` sets `kind: TitleKind::Xbox`, `args`/`wrapper` empty.

`SCHEMA.md`: a new `## Local games (user recipes only)` section at the end, documenting `[title].kind`, `[launch].args`, `[launch].wrapper`, the id/slug rule, absolute folders, either path separator in `executable`, and that `titles/validate.py` refuses them in `titles/` (it already rejects unknown keys — leave `validate.py` unchanged). Write the three keys as a bullet list or a table whose rows sit under that `## ` heading with **no** `### \`[table]\`` heading after it: `every_key_the_schema_documents_is_accepted` (recipe.rs:970) inserts each `| \`key\` |` row found under a `### \`[…]\`` heading into a minimal Xbox recipe, and `kind = "x"` (or any local key on an Xbox recipe) would make it panic; a `## ` heading clears its current table. Also change the `[launch]` section's "There is no way to pass arguments to the title" paragraph to say this holds for Store titles, pointing at the new section.

- [ ] **Step 4: Run the tests to see them pass**

Run: `$HOME/.cargo/bin/cargo test --locked -p ferestre-core --lib -- recipe::tests local::tests` then `python3 titles/validate.py`
Expected: PASS, including the existing `recipe::tests::every_key_the_schema_documents_is_accepted` and `documented_environment_alias_matches_the_existing_env_key`; validator still clean.

- [ ] **Step 5: Commit**

```bash
git add crates/ferestre-core/src/local.rs crates/ferestre-core/src/lib.rs crates/ferestre-core/src/recipe.rs titles/SCHEMA.md
git commit -m "Describe local games in recipes with a LOCAL- id that is never a Store id

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 2: Paths for local games and Steam roots

**Files:**
- Modify: `crates/ferestre-core/src/paths.rs:499-519` (`install_dir`), `:521-538` (`prefix_dir`), add methods after `:476` (`cache_dir`)
- Test: `paths.rs` in-module tests

**Interfaces:**
- Consumes: `Recipe::is_local()` (Task 1).
- Produces:
  - `Paths::install_dir(&self, recipe)` — for a local recipe returns `install.dir` as given (already validated absolute); no env variable, no install record, no `under_games`.
  - `Paths::prefix_dir(&self, recipe)` — local with an absolute `install.prefix`: that path; otherwise the existing relative rule (default `<lowercase id>-proton` under the games directory, because the local slug is the lowercase id).
  - `pub fn steam_roots(&self) -> Vec<PathBuf>` — `steam_dir()` first, then the four `steam_candidates(home)`, keeping only existing directories, deduplicated by `canonicalize()` (first spelling wins).
  - `pub fn local_art_dir(&self, product_id: &str) -> Option<PathBuf>` — `state_dir()/local-art/<UPPERCASE ID>`, `None` unless `local::is_local_id`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_local_game_lives_where_its_recipe_says_and_nowhere_else() {
    let p = paths(&[]);
    let recipe = Recipe::parse(LOCAL_FIXTURE).unwrap(); // dir = "/mnt/games/Witcher 3", no prefix
    assert_eq!(p.install_dir(&recipe).unwrap(), Path::new("/mnt/games/Witcher 3"));
    assert_eq!(p.prefix_dir(&recipe).unwrap(),
               Path::new("/home/tester/xbox-games/local-witcher-3-proton"));
    let mut own = recipe.clone();
    own.install.prefix = Some("/home/tester/Games/prefixes/witcher".into());
    assert_eq!(p.prefix_dir(&own).unwrap(), Path::new("/home/tester/Games/prefixes/witcher"));
}

#[test]
fn steam_roots_are_deduplicated_by_their_real_path() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".local/share/Steam")).unwrap();
    std::fs::create_dir_all(home.path().join(".steam")).unwrap();
    std::os::unix::fs::symlink(home.path().join(".local/share/Steam"),
                               home.path().join(".steam/steam")).unwrap();
    let p = Paths::resolve(&Env::from_map([("HOME", home.path().to_str().unwrap())])).unwrap();
    assert_eq!(p.steam_roots().len(), 1);
}

#[test]
fn local_art_is_kept_per_local_id_only() {
    let p = paths(&[]);
    assert!(p.local_art_dir("LOCAL-STEAM-7").unwrap().ends_with("local-art/LOCAL-STEAM-7"));
    assert!(p.local_art_dir("9NBLGGH2JHXJ").is_none());
}
```

`LOCAL_FIXTURE` is a `const &str` in the test module with id `LOCAL-WITCHER-3`. The existing `a_recipe_cannot_escape_the_games_directory` must still pass unchanged (an Xbox recipe with an absolute `dir` is still refused).

- [ ] **Step 2: Run to see them fail**

Run: `$HOME/.cargo/bin/cargo test --locked -p ferestre-core --lib paths::tests`
Expected: FAIL (local dir refused by `under_games`; `steam_roots`/`local_art_dir` undefined).

- [ ] **Step 3: Implement the branches and the two methods**

- [ ] **Step 4: Run to see them pass**

Run: `$HOME/.cargo/bin/cargo test --locked -p ferestre-core --lib paths::tests`
Expected: PASS, including `a_recipe_cannot_escape_the_games_directory`.

- [ ] **Step 5: Commit** — `git add crates/ferestre-core/src/paths.rs` and commit "Locate local games and their prefixes, and list Steam roots once each" with the trailer.

### Task 3: Store-only chokepoints refuse local ids

**Files:**
- Modify: `crates/ferestre-core/src/catalog.rs:1387-1420` (`resolve_using`)
- Modify: `crates/ferestre-cli/src/main.rs:4978-4980` (`is_product_id`)
- Modify: `crates/ferestre-core/src/steam.rs:484-502` (`is_ferestre_entry` → public)

**Interfaces:**
- Consumes: `recipe::is_store_product_id`, `local::is_local_id`.
- Produces:
  - `catalog::resolve` / `catalog::revalidate` drop every id that is not `is_store_product_id` before the cache split and the fetch (so `want_names`, `fetch_icons`, artwork and Add-to-Steam can never send a local id to the display catalog).
  - CLI `is_product_id(s)` delegates to `ferestre_core::recipe::is_store_product_id(s)` (same answers: the existing `product_ids_are_checked_before_they_reach_the_client` stays green). Its callers — `cmd_install` (`:1079`), `uninstall_title` (`:1439`), `cmd_move` (`:5018`) — therefore reject `LOCAL-…` as a Store id; uninstall still finds a local recipe by slug (Task 12).
  - `pub fn is_ferestre_shortcut(entry: &steam::Value) -> bool` (renamed from `is_ferestre_entry`, callers updated); `runs_a_product` also accepts `["run", id]` with `local::is_local_id(id)`.

- [ ] **Step 1: Write the failing tests**
  - `catalog.rs`: `a_local_id_never_reaches_the_catalog` — call `resolve_using` with `["LOCAL-STEAM-1", "9NBLGGH2JHXJ"]` and a fetch closure that records its argument; assert it saw only `["9NBLGGH2JHXJ"]`, and with only the local id the closure is never called.
  - `steam.rs`: `a_shortcut_that_runs_a_local_game_is_ferestres` — an entry with `Exe = "\"/usr/bin/env\""`, `LaunchOptions = "run LOCAL-STEAM-12"` is ours; `"run 9NBLGGH2JHXJ"` still ours; someone's `"%command% -dx12"` is not.
  - CLI: extend `product_ids_are_checked_before_they_reach_the_client` with `assert!(!is_product_id("LOCAL-STEAM-1"))`.
- [ ] **Step 2: Run** `$HOME/.cargo/bin/cargo test --locked -p ferestre-core --lib -- catalog::tests steam::tests` and `$HOME/.cargo/bin/cargo test --locked -p ferestre-cli product_ids_are_checked` — expect FAIL on the new asserts.
- [ ] **Step 3: Implement** the filter, the delegation and the rename.
- [ ] **Step 4: Run the same commands** — expect PASS.
- [ ] **Step 5: Commit** "Keep local ids out of every Store lookup" with the trailer.

### Task 4: Forgetting a local game, and never deleting its files

**Files:**
- Modify: `crates/ferestre-core/src/install.rs:235-301` (`check_removable`)
- Modify: `crates/ferestre-core/src/local.rs`
- Test: `install.rs` tests (use the existing `Home` helper at `:1278`), `local.rs` tests

**Interfaces:**
- Consumes: `Paths::install_dir`, `Paths::prefix_dir`, `Paths::local_art_dir` (Task 2).
- Produces:
  - `install::check_removable(dir, paths)` additionally bails with "`<dir>` belongs to `<name>`, a game Ferestre did not install; Ferestre never deletes it" when, for any local recipe in `Recipe::load_layered_report(&paths.title_dirs())`, the resolved `dir` equals, contains, or lies inside that recipe's install dir or its **absolute** prefix. (A managed relative prefix under the games directory stays removable by the normal rules — it is Ferestre's.) `safe_to_remove` inherits this, so `relocation` is covered too.
  - `pub struct Forgotten { pub recipe_file: Option<PathBuf>, pub kept_dir: PathBuf, pub kept_prefix: PathBuf }`
  - `pub fn forget(paths: &Paths, product_id: &str) -> anyhow::Result<Forgotten>` — refuses non-local ids; removes the file(s) in `user_titles_dir()` that hold the id, whatever they are called (fails if there are none), the title's entry via `runtime::set_selected(config, id, None)`, and `local_art_dir(id)` (a directory Ferestre created); touches nothing else.

- [ ] **Step 1: Write the failing tests**

```rust
// install.rs
#[test]
fn a_local_games_folder_and_prefix_are_never_removable() {
    let home = Home::new("local-guard");
    let game = home.root.join("drive/Game");          // outside the games dir
    let prefix = home.root.join("steam/steamapps/compatdata/3000000001");
    for d in [&game, &prefix] { std::fs::create_dir_all(d.join("x")).unwrap(); }
    write_local_recipe(&home.paths, &game, Some(&prefix)); // helper writes LOCAL-T.toml into user_titles_dir
    for refused in [game.clone(), game.join("x"), prefix.clone(), home.root.join("drive")] {
        let err = check_removable(&refused, &home.paths).unwrap_err().to_string();
        assert!(err.contains("never deletes"), "{}: {err}", refused.display());
    }
}
```

```rust
// local.rs
#[test]
fn forgetting_a_local_game_removes_only_what_ferestre_wrote() {
    // tempdir HOME; local recipe with dir D (contains Game.exe) and absolute prefix P (contains pfx/);
    // a runtime selection for the id; a file in local_art_dir.
    let gone = forget(&paths, "LOCAL-T").unwrap();
    assert!(!paths.user_titles_dir().join("LOCAL-T.toml").exists());
    assert!(runtime::selected(paths.config_dir(), "LOCAL-T").is_none());
    assert!(!paths.local_art_dir("LOCAL-T").unwrap().exists());
    assert!(dir.join("Game.exe").is_file() && prefix.join("pfx").is_dir());
    assert_eq!(gone.kept_dir, dir);
    assert!(forget(&paths, "9NBLGGH2JHXJ").is_err());
}
```

- [ ] **Step 2: Run** `$HOME/.cargo/bin/cargo test --locked -p ferestre-core --lib -- install::tests local::tests` — expect FAIL.
- [ ] **Step 3: Implement** the guard (load recipes once per call; compare `canonicalize()`d paths with `starts_with` both ways) and `local::forget`.
- [ ] **Step 4: Run** the same command plus `$HOME/.cargo/bin/cargo test --locked -p ferestre-core --lib install::tests::the_home_and_games_directories_are_never_removable` — expect PASS.
- [ ] **Step 5: Commit** "Refuse to delete a local game's folder or prefix; forget only the recipe" with the trailer.

### Task 5: The launch plan for a local game

**Files:**
- Modify: `crates/ferestre-core/src/launch.rs:261-345` (`plan`), add `pub fn windows_path`
- Modify: `scripts/proton-wine-shim.sh:112-128` (the GamingServices `ServiceRetryTimeout` block)
- Test: `launch.rs` tests, `tests/test_proton_wine_shim.py` (`GamingServicesWaitTests`)

**Interfaces:**
- Consumes: `Recipe::is_local`, `Launch.args`, `Launch.wrapper`.
- Produces:
  - `pub fn windows_path(path: &Path) -> String` — `"Z:"` + the absolute path with `/` turned into `\` (`/games/Cyberpunk 2077/bin/x64/C.exe` → `Z:\games\Cyberpunk 2077\bin\x64\C.exe`).
  - `plan(recipe, dirs, options)` for a local recipe returns:
    - `program`: `wrapper[0]` when the wrapper is non-empty, else `dirs.shim()`;
    - `args`: `wrapper[1..]`, then `dirs.shim()` (only when a wrapper is present), then `windows_path(game_dir.join(executable.replace('\\', "/")))` (an absolute executable replaces `game_dir` in the join), then `recipe.launch.args`;
    - `cwd`: `game_dir` (`options.game_dir`, i.e. the recipe's folder);
    - `env_set`: the Xbox set **minus** `WINE_GAMEINPUT`, `XODUS_CLI_DIR` and `XGR_XUID` (ignore `options.xuid`): the two `DISABLE_VK_LAYER_VALVE_*` (unless Steam integration), `FERESTRE_DESKTOP_INTEGRATION=1`, `FERESTRE_LOCAL_GAME=1`, `STEAM_COMPAT_DATA_PATH`, `STEAM_COMPAT_CLIENT_INSTALL_PATH`, `PROTON_DIR`, `XODUS_GAMES_DIR`, `XODUS_PROTON_DIR`, `XODUS_STEAM_DIR`, then the recipe's `[launch.env]` last;
    - `env_unset`: the same `STEAM_ENVIRONMENT` rule as Xbox.
  - Xbox plans are unchanged.
- Shim: when `FERESTRE_LOCAL_GAME=1`, skip the `reg add HKLM\Microsoft\GamingServices … ServiceRetryTimeout` step. It exists for GDK titles; a local game's prefix may be Steam's own `compatdata/<appid>`, and Ferestre should not write a GDK setting into it.

- [ ] **Step 1: Write the failing tests**

```rust
fn local_recipe(extra: &str) -> Recipe { /* LOCAL-GAME, dir "/drive/My Game", executable 'bin/Game.exe', kind local, + extra */ }

#[test]
fn a_local_game_runs_through_the_shim_and_never_the_client() {
    let options = Options { game_dir: Some("/drive/My Game".into()),
                            prefix_dir: Some("/pfx/game".into()), ..Options::default() };
    let plan = plan(&local_recipe("args = [\"-dx12\"]"), &dirs(), &options);
    assert_eq!(plan.program, PathBuf::from("/usr/lib/ferestre/scripts/proton-wine-shim.sh"));
    assert_eq!(plan.args, vec![r"Z:\drive\My Game\bin\Game.exe", "-dx12"]);
    assert_eq!(plan.cwd.as_deref(), Some(Path::new("/drive/My Game")));
    assert_eq!(plan.get("STEAM_COMPAT_DATA_PATH"), Some("/pfx/game"));
    assert!(plan.get("WINE_GAMEINPUT").is_none());
    assert!(!plan.args.iter().any(|a| a.contains("xodus-cli")));
}

#[test]
fn wrappers_come_first_and_the_recipe_environment_wins() {
    let options = Options { game_dir: Some("/drive/G".into()), xuid: Some("2533274812345678".into()),
                            ..Options::default() };
    let plan = plan(&local_recipe("wrapper = [\"gamemoderun\", \"mangohud\"]\n[launch.env]\nPROTON_ENABLE_WAYLAND = \"0\""),
                    &dirs(), &options);
    assert_eq!(plan.program, PathBuf::from("gamemoderun"));
    assert_eq!(plan.args[..2], ["mangohud", "/usr/lib/ferestre/scripts/proton-wine-shim.sh"]);
    assert_eq!(plan.get("PROTON_ENABLE_WAYLAND"), Some("0"));
    assert!(plan.get("XGR_XUID").is_none());
}

#[test]
fn an_absolute_executable_outside_the_folder_is_used_as_it_is() { /* executable '/other/Launcher.exe' → arg "Z:\\other\\Launcher.exe" */ }

#[test]
fn a_backslash_executable_is_the_same_file() { /* executable 'bin\\Game.exe' → arg r"Z:\drive\My Game\bin\Game.exe", same as 'bin/Game.exe' */ }
```

`tests/test_proton_wine_shim.py`: add `test_a_local_game_prefix_is_not_given_the_gaming_services_value` to `GamingServicesWaitTests` (give `run_shim` an optional `extra_env` dict, pass `FERESTRE_LOCAL_GAME='1'`; assert the only recorded call is the game's, with no `run reg add`). The three existing GamingServices tests stay unchanged.

(`local_recipe` puts `extra` lines inside `[launch]`; adjust so `[launch.env]` follows.) `the_program_is_the_client_and_never_the_title` and the other existing tests must stay unchanged.

- [ ] **Step 2: Run** `$HOME/.cargo/bin/cargo test --locked -p ferestre-core --lib launch::tests` and `python3 tests/test_proton_wine_shim.py` — expect FAIL on the new tests.
- [ ] **Step 3: Implement** the local branch at the top of `plan` (a private `local_plan`), `windows_path`, and the shim guard. Update the module doc's "The program is always `xodus-cli run`" paragraph to say "for a Store title".
- [ ] **Step 4: Run** the same two commands — expect PASS (old and new).
- [ ] **Step 5: Commit** (`git add crates/ferestre-core/src/launch.rs scripts/proton-wine-shim.sh tests/test_proton_wine_shim.py`) "Plan a local game's launch through the Proton shim, wrappers first" with the trailer.

### Task 6: Anti-cheat detection by file signature

**Files:**
- Modify: `crates/ferestre-core/src/anticheat.rs` (new section after `has_child_dir`, `:104`)
- Test: `anticheat.rs` tests

**Interfaces:**
- Produces:
  - `#[derive(Debug, Clone, PartialEq, Eq)] pub struct Detection { pub name: &'static str, pub kernel: bool, pub proton_runtime: bool, pub evidence: PathBuf }`
  - `pub struct Signature { pub name: &'static str, pub kernel: bool, pub proton_runtime: bool, pub files: &'static [&'static str], pub dirs: &'static [&'static str] }`, `pub const SIGNATURES: &[Signature]`.
  - `pub fn detect(game_dir: &Path, executable: &str) -> Vec<Detection>` — looks in `game_dir` and in the executable's own folder (`game_dir.join(parent of executable)`, `\` treated as `/`, absolute executables honoured), comparing names without case; patterns are lowercase with one optional leading or trailing `*`; one `Detection` per signature (first evidence found), in `SIGNATURES` order.
  - `pub fn describe(found: &[Detection]) -> Option<String>` — one line for CLI and GUI that states evidence, not a verdict, because a file on disk does not prove the build starts it (publicly modded builds keep the driver file next to a Steam emulator): `"anti-cheat files found: Ricochet (Randgrid.sys, a kernel driver), Easy Anti-Cheat (EasyAntiCheat/, a kernel driver; Proton has a runtime for it). If this build uses them, play single-player offline only; online play may be refused or lead to a ban."` — the evidence is the file or folder name as found on disk; `None` when empty.
- `kernel` means what the spec says: the anti-cheat ships a Windows kernel driver. `proton_runtime` means Proton has a bridge for it (Easy Anti-Cheat and BattlEye — the runtimes this module already downloads). Part B's gate reads `kernel`; nothing in this plan refuses anything on it.

Signature table (exactly these):

| name | kernel | proton_runtime | files | dirs |
|---|---|---|---|---|
| Easy Anti-Cheat | true | true | `start_protected_game.exe`, `easyanticheat_x64.dll`, `easyanticheat_eos_setup.exe`, `eaclaunch.exe` | `easyanticheat`, `easyanticheat_eos` |
| BattlEye | true | true | `beservice*.exe`, `beclient*.dll`, `bedaisy.sys` | `battleye` |
| Ricochet | true | false | `randgrid.sys` | |
| Riot Vanguard | true | false | `vgk.sys`, `vgc.exe` | |
| EA AntiCheat | true | false | `eaanticheat.gameservicelauncher.exe`, `eaanticheat.gameservicelauncher.dll`, `eaanticheat.installer.exe` | |
| nProtect GameGuard | true | false | `npggsvc.exe`, `gameguard.des` | `gameguard` |
| ACE | true | false | `sguard64.exe`, `ace-base*` | `ace-base*` |
| XIGNCODE3 | true | false | `x3.xem` | `xigncode` |
| mhyprot | true | false | `mhyprot*.sys` | |

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_kernel_anti_cheat_is_reported_once_as_evidence_even_where_the_build_is_modded() {
    // The CoD sp26 layout: the driver at the root and beside the exe, plus a Steam emulator.
    let game = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(game.path().join("sp26/steam_settings")).unwrap();
    for f in ["Randgrid.sys", "sp26/Randgrid.sys", "sp26/sp26-cod.exe"] {
        std::fs::write(game.path().join(f), b"").unwrap();
    }
    let found = detect(game.path(), "sp26/sp26-cod.exe");
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].name, found[0].kernel, found[0].proton_runtime), ("Ricochet", true, false));
    let line = describe(&found).unwrap();
    assert!(line.contains("Randgrid.sys") && line.contains("If this build uses them"), "{line}");
    assert!(!line.contains("will not run"), "evidence, not a verdict: {line}");
}

#[test]
fn every_signature_is_recognised_by_its_file_or_folder() {
    for (layout, name) in [("EasyAntiCheat/", "Easy Anti-Cheat"), ("BEService_x64.exe", "BattlEye"),
        ("vgc.exe", "Riot Vanguard"), ("EAAntiCheat.GameServiceLauncher.exe", "EA AntiCheat"),
        ("GameGuard/", "nProtect GameGuard"), ("SGuard64.exe", "ACE"), ("ACE-Base64.sys", "ACE"),
        ("x3.xem", "XIGNCODE3"), ("mhyprot3.sys", "mhyprot")] { /* create layout in a fresh tempdir, assert detect(.., "Game.exe")[0].name == name */ }
    assert!(SIGNATURES.iter().all(|s| s.kernel));
    assert!(SIGNATURES.iter().filter(|s| s.proton_runtime).map(|s| s.name).eq(["Easy Anti-Cheat", "BattlEye"]));
    assert!(detect(tempfile::tempdir().unwrap().path(), "Game.exe").is_empty());
    assert!(describe(&[]).is_none());
}
```

- [ ] **Step 2: Run** `$HOME/.cargo/bin/cargo test --locked -p ferestre-core --lib anticheat::tests` — expect FAIL.
- [ ] **Step 3: Implement** `SIGNATURES`, `detect`, `describe`. Do not change `needed`, `ALL` or the runtime downloads.
- [ ] **Step 4: Run** the same command — expect PASS (old runtime tests too).
- [ ] **Step 5: Commit** "Detect anti-cheat by file signature, marking kernel drivers" with the trailer.

### Task 7: Text VDF, Steam libraries, and Steam's Protons as runtimes

**Files:**
- Create: `crates/ferestre-core/src/vdf.rs`
- Modify: `crates/ferestre-core/src/lib.rs`, `crates/ferestre-core/src/steam.rs` (add `libraries`), `crates/ferestre-core/src/runtime.rs` (after `all`, `:387-400`)
- Test: in-module tests in all three

**Interfaces:**
- Consumes: `Paths::steam_roots` (Task 2).
- Produces:
  - `vdf::parse(text: &str) -> anyhow::Result<steam::Value>` — text KeyValues into `Value::Map`/`Value::Str` (so `Value::get`'s case-insensitive lookup applies): quoted or bare tokens, `{`/`}` nesting, `//` comments, escapes `\"` `\\` `\n` `\t`, `[$CONDITION]` tags after a value skipped, duplicate keys kept in order; unbalanced braces or an unterminated string is an error.
  - `steam::libraries(root: &Path) -> Vec<PathBuf>` — `root` first, then every library in `root/steamapps/libraryfolders.vdf` (new format: `"N" { "path" "…" }`; old: `"N" "…"`), existing directories only, deduplicated by real path.
  - `pub struct SteamTool { pub name: String, pub dir: PathBuf }` in `runtime.rs`.
  - `runtime::steam_tools(paths: &Paths) -> Vec<SteamTool>` — for every root in `paths.steam_roots()`: each `compatibilitytools.d/*` with a `proton` script, named by every key of `compatibilitytool.vdf`'s `compatibilitytools.compat_tools` (dir = vdf dir joined with its `install_path`, default `.`), or by the folder name when there is no vdf; and for every library of the root, each `steamapps/common/Proton*` holding both `proton` and `toolmanifest.vdf`, named by `steam_proton_name`.
  - `fn steam_proton_name(dir_name: &str) -> Option<String>` — `"Proton - Experimental"`→`proton_experimental`, `"Proton Hotfix"`→`proton_hotfix`, `"Proton X.Y…"`→`proton_X` when `X >= 7` or `Y == 0`, else `proton_XY`.
  - `runtime::steam_tool<'a>(tools: &'a [SteamTool], name: &str) -> Option<&'a SteamTool>` (exact name, then ASCII case-insensitive).
  - `runtime::external(paths: &Paths) -> Vec<InstalledRuntime>` — `InstalledRuntime::at` of each tool dir, minus any `same_path` as an entry of `all(paths)` (the legacy `compatibilitytools.d/xodus` is already there), deduplicated, sorted by path.
  - `runtime::for_recipe(paths: &Paths, recipe: &Recipe) -> Vec<InstalledRuntime>` — `all(paths)` for an Xbox recipe; `all(paths)` followed by `external(paths)` for a local one.
  - `runtime::all`, `discover`, `choose`, `select`, `requested` are unchanged, so an external path chosen for an Xbox title is "not installed" exactly as an unknown path is today.

- [ ] **Step 1: Write the failing tests**
  - `vdf.rs`: `config_vdf_compat_tool_mapping_reads` — parse
    ```
    "InstallConfigStore" { "Software" { "Valve" { "Steam" { "CompatToolMapping" {
      "3000000001" { "name" "GE-Proton10-25" "config" "" "priority" "250" } } } } } }
    ```
    and walk `get("InstallConfigStore")…get("CompatToolMapping").get("3000000001").get("name")` to `"GE-Proton10-25"`; `comments_escapes_and_conditions` (the VDF text `"k" "a\"b"` gives `a"b`; `"k" "v" [$WIN32]` gives `v`; a `// x` line is ignored); `unbalanced_text_is_an_error` (`"a" {` and `"a" "b` are errors).
  - `steam.rs`: `libraries_come_from_both_libraryfolders_formats` (tempdir root with both shapes, one non-existent path dropped).
  - `runtime.rs`: `steam_protons_are_named_as_steam_names_them` — table: `"Proton 9.0 (Beta)"→"proton_9"`, `"Proton 10.0"→"proton_10"`, `"Proton 6.3"→"proton_63"`, `"Proton 5.13"→"proton_513"`, `"Proton 5.0"→"proton_5"`, `"Proton - Experimental"→"proton_experimental"`, `"Proton Hotfix"→"proton_hotfix"`, `"Proton EasyAntiCheat Runtime"→None`.
  - `runtime.rs`: `external_protons_are_offered_to_local_games_only` — tempdir HOME with `.local/share/Steam/compatibilitytools.d/GE-Proton10-25/{proton,compatibilitytool.vdf}` and `.local/share/Steam/steamapps/common/Proton 9.0/{proton,toolmanifest.vdf}`; assert `steam_tools` names `GE-Proton10-25` and `proton_9`; `for_recipe(&paths, &local)` contains both dirs after every `all()` entry; `for_recipe(&paths, &xbox)` contains neither; `choose(&xbox, &for_recipe(&paths, &xbox), Some(ge_dir), None)` is `Err`; `choose(&local, &for_recipe(&paths, &local), Some(ge_dir), None)` picks it.
- [ ] **Step 2: Run** `$HOME/.cargo/bin/cargo test --locked -p ferestre-core --lib -- vdf::tests steam::tests runtime::tests` — expect FAIL.
- [ ] **Step 3: Implement** `vdf.rs`, `steam::libraries`, and the runtime functions.
- [ ] **Step 4: Run** the same command — expect PASS, including every existing `runtime::tests` case.
- [ ] **Step 5: Commit** "Read text VDF and offer Steam's Proton builds to local games" with the trailer.

### Task 8: Steam launch options

**Files:**
- Create: `crates/ferestre-core/src/steam_import.rs` (parser only in this task)
- Modify: `crates/ferestre-core/src/lib.rs`

**Interfaces:**
- Produces:
  - `#[derive(Debug, Default, PartialEq, Eq)] pub struct LaunchOptions { pub env: BTreeMap<String, String>, pub wrapper: Vec<String>, pub args: Vec<String> }`
  - `pub fn parse_launch_options(text: &str) -> anyhow::Result<LaunchOptions>`.
  - Rules: split like a POSIX shell word splitter without expansion — whitespace separates; `'…'` literal; `"…"` with `\"`, `\\`, `\$`, `` \` `` escapes; a backslash outside quotes escapes the next character. Unquoted `;`, `&`, `|`, `<`, `>`, `` ` ``, `$(` anywhere → `Err` naming the operator. With a `%command%` word: leading `NAME=value` words (`NAME` is `[A-Za-z_][A-Za-z0-9_]*`) before it are `env`; every following word up to `%command%` is `wrapper` (an `env` word after the first wrapper word stays in the wrapper); words after it are `args`. Without `%command%`: every word is an argument (what Steam does — appends them).
  - `pub fn quote_words(words: &[String]) -> String` — the inverse of the splitter for display: a word made only of `[A-Za-z0-9_./=:,+@%-]` is written bare, any other is single-quoted with `'` written as `'\''`; words joined by one space. `parse_launch_options(&quote_words(w))` gives back `w` as `args` for any `w` without `%command%`. (The GUI editor uses this; `ferestre-cli`'s private `shell_quote` is not reachable from it.)
  - `pub fn expand_home(value: &str, home: &Path) -> String` — a leading `~/`, `$HOME/` or `${HOME}/` (or the whole value equal to one of those without the slash) becomes `home`; nothing else expands.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn environment_wrappers_and_arguments_split_at_command() {
    let o = parse_launch_options(
        r#"PROTON_ENABLE_WAYLAND=1 DXVK_HDR=1 STEAM_COMPAT_DATA_PATH="/home/me/Games/prefixes/cod mw4" gamemoderun %command% -dx12 "--profile=a b""#).unwrap();
    assert_eq!(o.env["STEAM_COMPAT_DATA_PATH"], "/home/me/Games/prefixes/cod mw4");
    assert_eq!(o.env.len(), 3);
    assert_eq!(o.wrapper, ["gamemoderun"]);
    assert_eq!(o.args, ["-dx12", "--profile=a b"]);
}

#[test]
fn without_command_every_word_is_an_argument() {
    assert_eq!(parse_launch_options("-skipintro 'two words'").unwrap(),
               LaunchOptions { args: vec!["-skipintro".into(), "two words".into()], ..Default::default() });
    assert_eq!(parse_launch_options("").unwrap(), LaunchOptions::default());
}

#[test]
fn shell_operators_are_refused_not_guessed() {
    for text in ["A=1 %command% ; rm x", "%command% | tee log", "X=$(id) %command%", "%command% > log"] {
        assert!(parse_launch_options(text).is_err(), "{text}");
    }
    assert!(parse_launch_options(r#"%command% "a;b""#).is_ok());
}

#[test]
fn quoted_words_split_back_to_the_same_words() {
    let words: Vec<String> = ["-dx12", "a b", "it's", ""].map(String::from).to_vec();
    assert_eq!(quote_words(&words), r#"-dx12 'a b' 'it'\''s' ''"#);
    assert_eq!(parse_launch_options(&quote_words(&words)).unwrap().args, words);
}

#[test]
fn only_the_home_directory_is_expanded() {
    let home = Path::new("/home/me");
    assert_eq!(expand_home("~/Games/p", home), "/home/me/Games/p");
    assert_eq!(expand_home("${HOME}/p", home), "/home/me/p");
    assert_eq!(expand_home("$XDG_DATA_HOME/p", home), "$XDG_DATA_HOME/p");
}
```

- [ ] **Step 2: Run** `$HOME/.cargo/bin/cargo test --locked -p ferestre-core --lib steam_import::tests` — expect FAIL.
- [ ] **Step 3: Implement** the tokenizer and split.
- [ ] **Step 4: Run** the same command — expect PASS.
- [ ] **Step 5: Commit** "Parse Steam launch options into environment, wrappers and arguments" with the trailer.

### Task 9: Import non-Steam shortcuts

**Files:**
- Modify: `crates/ferestre-core/src/steam_import.rs`
- Modify: `crates/ferestre-core/src/local.rs` (artwork store)

**Interfaces:**
- Consumes: `steam::shortcut_files`, `steam::load`, `steam::shortcut_app_id`, `steam::is_ferestre_shortcut`, `steam::libraries`, `vdf::parse`, `runtime::steam_tools`/`steam_tool`, `runtime::selected`/`set_selected`, `local::steam_id`, `Paths::steam_roots`/`local_art_dir`, `Recipe::save_new_to`, `anticheat::detect`.
- Produces in `local.rs`:
  - `pub fn artwork(paths: &Paths, product_id: &str) -> steam::ArtworkFiles` — whatever of `portrait|landscape|hero|logo|icon.{png,jpg,jpeg}` exists in `local_art_dir`.
  - `pub fn save_artwork(paths: &Paths, product_id: &str, files: &steam::ArtworkFiles) -> anyhow::Result<()>` — copies each present source to `<slot>.<source extension>` through `cache::write`, never over an existing slot.
- Produces in `steam_import.rs`:
  - `pub enum ImportStatus { New, Kept, Skipped(String) }` (not `Status`: the crate root already re-exports `recipe::Status`)
  - `pub enum RuntimeFound { Tool(PathBuf), Missing(String), Unset }`
  - `pub struct Candidate { pub id: String, pub app_id: u32, pub name: String, pub user: String, pub status: ImportStatus, pub recipe: Option<Recipe>, pub runtime: RuntimeFound, pub artwork: steam::ArtworkFiles, pub anticheat: Vec<anticheat::Detection>, pub warnings: Vec<String> }`
  - `pub fn plan(paths: &Paths, user: Option<&str>) -> anyhow::Result<Vec<Candidate>>` — reads only.
  - `pub struct Report { pub added: Vec<String>, pub kept: Vec<String>, pub skipped: Vec<(String, String)> }`
  - `pub fn apply(paths: &Paths, candidates: &[Candidate]) -> anyhow::Result<Report>`.
- Decisions `plan` applies, per shortcut entry, per user (`userdata/<id>/config/shortcuts.vdf` of every root; `--user` keeps one id):
  - app id = the entry's `appid` as `u32` when non-zero, else `shortcut_app_id(Exe, AppName)`; id = `local::steam_id(app_id)`. The first occurrence wins; later ones (another user or root) are dropped silently.
  - Skipped with a reason: `is_ferestre_shortcut`; `Exe` (unquoted) not ending in `.exe`/`.bat` (ASCII case-insensitive) → "not a Windows program".
  - Recipe: `kind = local`, slug = lowercase id, name = `AppName`, `install.dir` = unquoted `StartDir` or the exe's parent when empty, `launch.executable` = exe relative to that dir when inside it, else absolute; `status = untested`, summary `"Imported from Steam"`.
  - Launch options via `parse_launch_options`; on `Err`, keep the shortcut without options and push a warning `"<name>: launch options not imported (<error>); add them in Edit launch settings"`. `env` minus `STEAM_COMPAT_DATA_PATH` → `launch.env`; `wrapper`/`args` → recipe.
  - Prefix: `STEAM_COMPAT_DATA_PATH` from the options, through `expand_home` (a remaining `$` → warning, value used as written); else the first existing `<library>/steamapps/compatdata/<app id>` across all libraries of all roots; else `None` (managed default).
  - Runtime: `CompatToolMapping[<app id>].name` from each root's `config/config.vdf`; found by `steam_tool` → `Tool(dir)`; named but absent → `Missing(name)` with a warning "Steam's <name> is not installed; Ferestre's runtime will be used"; no mapping → `Unset` (no fallback to key `"0"`).
  - Steam's prefix, another Proton: when the prefix is a Steam `compatdata/<app id>` folder and the runtime is `Missing` or `Unset`, push the warning `"<name>: its Steam prefix will be started by a different Proton than Steam used, which may upgrade it; choose Steam's Proton for it in Choose runtime, or give it its own prefix in Edit launch settings"`. The prefix is still used as found (the person's saves and settings are there); the shim's GDK registry step is already skipped for local games (Task 5).
  - Artwork: `<shortcuts.vdf parent>/grid/<app id>{p,"",_hero,_logo,_icon}.{png,jpg,jpeg}` → portrait, landscape, hero, logo, icon; else the entry's `icon` field if it is a `.png`/`.jpg`/`.jpeg` file.
  - `status` = `Kept` when `user_titles_dir()/<ID>.toml` exists, else `New`.
- `apply`: for `New`, `recipe.save_new_to(user_titles_dir)` (`Ok(None)` counts as kept), `local::save_artwork`, and `runtime::set_selected(config, id, Some(dir))` for `Tool(dir)` only when `runtime::selected` is `None`; for `Kept`, nothing is written. Nothing under any Steam root is written, ever.

- [ ] **Step 1: Write the failing tests** (in-module; a `steam_home()` helper builds a tempdir HOME with `.local/share/Steam/userdata/{12345678,111}/config/shortcuts.vdf` from `steam::serialise`, a `config/config.vdf`, a `compatibilitytools.d/GE-Proton10-25`, `steamapps/compatdata/<appid>/pfx`, grid files, and game folders with `Game.exe`)

```rust
#[test]
fn a_shortcut_becomes_a_local_recipe_with_its_prefix_runtime_and_art() {
    let h = steam_home();
    let found = plan(&h.paths, None).unwrap();
    let cp = found.iter().find(|c| c.name == "Cyberpunk 2077").unwrap();
    let r = cp.recipe.as_ref().unwrap();
    assert_eq!(cp.id, format!("LOCAL-STEAM-{}", cp.app_id));
    assert_eq!(r.install.dir.as_deref(), Some(h.cyberpunk_dir.to_str().unwrap()));
    assert_eq!(r.launch.executable, "bin/x64/Cyberpunk2077.exe");
    assert_eq!(r.launch.wrapper, ["gamemoderun"]);
    assert_eq!(r.launch.env.get("DXVK_HDR").map(String::as_str), Some("1"));
    assert!(!r.launch.env.contains_key("STEAM_COMPAT_DATA_PATH"));
    assert_eq!(r.install.prefix.as_deref(), Some(h.compatdata(cp.app_id).to_str().unwrap()));
    assert!(matches!(&cp.runtime, RuntimeFound::Tool(dir) if dir.ends_with("GE-Proton10-25")));
    assert!(cp.artwork.portrait.is_some() && cp.artwork.hero.is_some());
}

#[test]
fn an_explicit_prefix_in_the_options_wins_and_ferestres_own_shortcuts_are_skipped() { /* COD entry: STEAM_COMPAT_DATA_PATH=~/Games/prefixes/cod-mw4 → expanded; "run 9NBLGGH2JHXJ" entry → Skipped; "/usr/bin/heroic" entry → Skipped("not a Windows program") */ }

#[test]
fn a_shortcut_in_two_steam_users_is_imported_once() { /* same appid in both users → one candidate */ }

#[test]
fn a_steam_prefix_without_steams_proton_is_flagged() { /* fixture entry with compatdata/<appid>/pfx and no CompatToolMapping → runtime Unset, warnings has one containing "different Proton"; the Cyberpunk entry (mapped to GE-Proton10-25) has no such warning */ }

#[test]
fn import_writes_recipes_and_never_touches_steam() {
    let h = steam_home();
    let before = h.snapshot_steam(); // path -> bytes for every file under the Steam root
    let report = apply(&h.paths, &plan(&h.paths, None).unwrap()).unwrap();
    assert!(!report.added.is_empty());
    assert_eq!(h.snapshot_steam(), before);
    for id in &report.added {
        assert!(h.paths.user_titles_dir().join(format!("{id}.toml")).is_file());
    }
    let cyberpunk = local::steam_id(h.cyberpunk_app_id);
    assert!(local::artwork(&h.paths, &cyberpunk).portrait.is_some()); // the fixture gives Cyberpunk grid art
}

#[test]
fn a_second_import_keeps_edits_and_runtime_choices() {
    let h = steam_home();
    let first = apply(&h.paths, &plan(&h.paths, None).unwrap()).unwrap();
    let id = first.added[0].clone();
    let file = h.paths.user_titles_dir().join(format!("{id}.toml"));
    std::fs::write(&file, std::fs::read_to_string(&file).unwrap().replace("Imported from Steam", "Mine")).unwrap();
    runtime::set_selected(h.paths.config_dir(), &id, Some(Path::new("/elsewhere"))).unwrap();
    let again = plan(&h.paths, None).unwrap();
    assert!(again.iter().any(|c| c.id == id && matches!(c.status, ImportStatus::Kept)));
    let second = apply(&h.paths, &again).unwrap();
    assert!(second.added.is_empty() && second.kept.contains(&id));
    assert!(std::fs::read_to_string(&file).unwrap().contains("Mine"));
    assert_eq!(runtime::selected(h.paths.config_dir(), &id).as_deref(), Some(Path::new("/elsewhere")));
}
```

- [ ] **Step 2: Run** `$HOME/.cargo/bin/cargo test --locked -p ferestre-core --lib -- steam_import::tests local::tests` — expect FAIL.
- [ ] **Step 3: Implement** `plan`, `apply`, `local::artwork`, `local::save_artwork`.
- [ ] **Step 4: Run** the same command — expect PASS.
- [ ] **Step 5: Commit** "Import Steam non-Steam shortcuts as local games, read-only towards Steam" with the trailer.

### Task 10: Add a game by hand (core)

**Files:**
- Modify: `crates/ferestre-core/src/local.rs`

**Interfaces:**
- Consumes: `new_id`, `Recipe::save_new_to`, `runtime::for_recipe`, `runtime::steam_tools`/`steam_tool`, `runtime::set_selected`.
- Produces:
  - `pub fn derive_name(exe: &Path) -> String` — walk up from the exe's folder past names (ASCII case-insensitive) `bin`, `binaries`, `win64`, `win32`, `x64`, `x86`, `x64_dx12`, `retail`, `shipping`, `game`; the first other folder name is the name; the exe stem when none.
  - `pub struct AddRequest { pub exe: PathBuf, pub name: Option<String>, pub dir: Option<PathBuf>, pub prefix: Option<PathBuf>, pub runtime: Option<String> }`
  - `pub fn add(paths: &Paths, request: &AddRequest) -> anyhow::Result<Recipe>` — `exe` must be an absolute existing file ending in `.exe`/`.bat`; `dir` defaults to the exe's folder and must be absolute and existing; name defaults to `derive_name`; id from `new_id` with `taken` = a recipe with that id exists in any `title_dirs()`; `prefix` absolute kept as is, relative kept relative (games-dir rule), `None` = managed; `runtime` is a path or a Steam tool name and must be in `runtime::for_recipe(paths, &recipe)` (else `Err` listing what is available); saved through `save_new_to` and `set_selected`. Summary `"Added by hand"`.
  - `pub fn executable_path(recipe: &Recipe) -> Option<PathBuf>` — `None` for Xbox; otherwise `install.dir` joined with `launch.executable.replace('\\', "/")` (the same reading `anticheat::needed` and Task 5's `plan` use, so a SCHEMA-style backslash path is found; an absolute executable replaces the dir). `pub fn is_present(recipe: &Recipe) -> bool` — `executable_path` is a file.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn names_come_from_the_games_folder_not_its_binaries_folder() {
    assert_eq!(derive_name(Path::new("/g/Cyberpunk 2077/bin/x64/Cyberpunk2077.exe")), "Cyberpunk 2077");
    assert_eq!(derive_name(Path::new("/g/Witcher 3/bin/x64_dx12/witcher3.exe")), "Witcher 3");
    assert_eq!(derive_name(Path::new("/x64/Game.exe")), "Game");
}

#[test]
fn a_backslash_executable_is_found_on_disk() {
    // tempdir D with bin/x64/Game.exe; local recipe dir = D, executable = 'bin\x64\Game.exe'
    assert_eq!(executable_path(&recipe).unwrap(), dir.join("bin/x64/Game.exe"));
    assert!(is_present(&recipe));
    // executable = 'bin/x64/Missing.exe' → not present; an Xbox recipe → executable_path is None
}

#[test]
fn adding_twice_gives_two_ids_and_an_unknown_runtime_is_refused() {
    // tempdir HOME, exe at <tmp>/Games/RDR2/RDR2.exe
    let first = add(&paths, &AddRequest { exe: exe.clone(), name: None, dir: None, prefix: None, runtime: None }).unwrap();
    assert_eq!((first.title.product_id.as_str(), first.title.name.as_str()), ("LOCAL-RDR2", "RDR2"));
    assert_eq!(first.install.dir.as_deref(), exe.parent().unwrap().to_str());
    let second = add(&paths, &AddRequest { exe: exe.clone(), name: None, dir: None, prefix: None, runtime: None }).unwrap();
    assert_eq!(second.title.product_id, "LOCAL-RDR2-2");
    let bad = AddRequest { exe, name: None, dir: None, prefix: None, runtime: Some("GE-Proton99".into()) };
    assert!(add(&paths, &bad).is_err());
    assert!(add(&paths, &AddRequest { exe: "relative.exe".into(), name: None, dir: None, prefix: None, runtime: None }).is_err());
}
```

- [ ] **Step 2: Run** `$HOME/.cargo/bin/cargo test --locked -p ferestre-core --lib local::tests` — expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the same command — expect PASS.
- [ ] **Step 5: Commit** "Add a local game from its executable" with the trailer.

### Task 11: `ferestre run` for a local game skips every Xbox step

**Files:**
- Modify: `crates/ferestre-cli/src/main.rs:632-682` (`cmd_run`), `:711-876` (`prepare_launch`), `:879-1017` (`launch_prepared`), `:2735-2754` (`launch_dirs_for_runtime`), `:4174-4201` (`warnings`)
- Test: CLI `tests` module, using `Home`, `Home::client`/`client_ran`, `Home::script`, `Home::runtime`

**Interfaces:**
- Consumes: `Recipe::is_local`, `runtime::for_recipe`, `anticheat::{detect, describe}`, `local::is_local_id`, `launch::plan` (Task 5).
- Produces (behaviour; signatures unchanged):
  - `prepare_launch`, local recipe:
    - skips `describe_from_disk` when `local::is_local_id(title)`;
    - skips `autofix::infer` and `engine_defaults` (the person's launch options are the recipe);
    - uses `runtime::for_recipe(paths, recipe)` instead of `runtime::all(paths)` (Xbox unchanged because `for_recipe` returns `all` for it);
    - skips `accounts::launch_selection`, the roles block, `accounts::require_service`, `player_prefix`, the `XGR_XUID` option, the `XODUS_*_PROFILE`/`XGR_ACCOUNT_ROLES_V1` env and `bind_service_title`;
    - skips `related::installed`, `XGR_RELATED_PACKAGE_PATHS` and `XODUS_RELATED_CONTENT`;
    - skips `plan_service_socket` (`service_socket: None`, `roles: None`, `profiles: None`, `related: vec![]`);
    - keeps the gate, `add_anticheat_runtimes`, and prints `-- {describe(..)}` when `detect` finds anything.
  - `warnings`: no "recorded as untested" line for a local recipe.
  - `launch_dirs_for_runtime(paths, runtime, local: bool)`: for a local launch a missing client directory is not an error (`cli_dir` becomes `PathBuf::new()`, never used by a local plan), so a machine with no `xodus-cli` can still play local games. Xbox callers pass `false` and keep today's error.
  - `launch_prepared`, local recipe:
    - keeps `TitleLock::take(.., LockMode::Run)`, the log, `run_setup("every-launch")`, `create_dir_all(prefix)`, `Forwarding`/`PrefixSession`, the libav check and `run_teed`;
    - replaces the "not installed … ferestre install" error with `"{name}'s executable is not at {path}; is its drive mounted?"`, checked against `local::executable_path` **before** the prefix is created;
    - skips `install::is_partial`, `register_winrt_classes`, the PlayFab repair, `ServiceUse::begin`/`start_service`, `accounts::fetch` and `presence::Presence::start`.
  - `cmd_run` keeps its body but delegates, after `Paths::from_env()`, to `fn run_title(cli: &Cli, paths: &Paths, title: &str, request: LaunchRequest, args: &[String]) -> Result<ExitCode>` so tests can pass their own `Paths`.
  - The trailing-args refusal stays **first**, before anything is read from or written to the disk by the launch: when `args` is non-empty, `run_title` calls `recipes_from(cli, paths)` (read-only) and refuses with today's usage error unless a recipe `matches(title)` and `is_local()`. So an Xbox title, or an unknown title that `describe_from_disk` would describe, never reaches `prepare_launch` (no recipe written, no `accounts::fetch`, no client run). This check lives in `fn title_args_allowed(recipes: &[Recipe], title: &str, args: &[String]) -> Result<()>`.
  - After `prepare_launch`, a local recipe's `args` are appended to `prepared.plan.args`. `prepare_content` is skipped for a local recipe.

- [ ] **Step 1: Write the failing tests**

```rust
/// `install_extra` lands in [install] (e.g. `prefix = "/abs"`), `launch_extra` in [launch].
fn local_game_recipe(home: &Home, dir: &Path, install_extra: &str, launch_extra: &str) {
    home.recipe("config/ferestre/titles", "LOCAL-TEST.toml", &format!(r#"schema = 1
[title]
product-id = "LOCAL-TEST"
name = "Local Test"
slug = "local-test"
kind = "local"
[install]
dir = "{}"
{install_extra}
[launch]
executable = "Game.exe"
{launch_extra}
[status]
state = "untested"
summary = "Added by hand"
"#, dir.display()));
}

#[test]
fn a_local_game_launches_through_the_shim_without_the_client_or_the_service() {
    let home = Home::new("run-local");
    let game = home.root().join("drive/Local Game");
    std::fs::create_dir_all(&game).unwrap();
    std::fs::write(game.join("Game.exe"), b"").unwrap();
    std::fs::write(game.join("Randgrid.sys"), b"").unwrap();
    local_game_recipe(&home, &game, "", "args = [\"-x\"]");
    home.runtime("ferestre-11.0.2", Some(&["loader.memfd-main-image"]));
    home.script("proton-wine-shim.sh",
        r#"{ pwd -P; printf '%s\n' "$@"; } > "$XODUS_GAMES_DIR/shim-args""#);
    let paths = home.paths();
    let prepared = prepare_launch(&cli(), &paths, "local-test", LaunchRequest::default()).unwrap();
    assert!(prepared.service_socket.is_none() && prepared.roles.is_none());
    assert!(prepared.plan.get("XGR_RELATED_PACKAGE_PATHS").is_none());
    assert_eq!(launch_prepared(&paths, prepared, false).unwrap(), ExitCode::SUCCESS);
    let seen = std::fs::read_to_string(home.games().join("shim-args")).unwrap();
    let real = game.canonicalize().unwrap();
    assert_eq!(seen.lines().collect::<Vec<_>>(),
               [real.to_str().unwrap(), &launch::windows_path(&game.join("Game.exe")), "-x"]);
    assert!(!home.client_ran(), "xodus-cli must never run for a local game");
    assert!(home.games().join("local-test-proton").is_dir());
}

#[test]
fn a_local_game_whose_drive_is_gone_is_not_launched() {
    let home = Home::new("run-local-gone");
    let gone = home.root().join("unmounted/Game");
    local_game_recipe(&home, &gone, "", "");
    home.runtime("ferestre-11.0.2", Some(&["loader.memfd-main-image"]));
    let paths = home.paths();
    let prepared = prepare_launch(&cli(), &paths, "LOCAL-TEST", LaunchRequest::default()).unwrap();
    let err = launch_prepared(&paths, prepared, false).unwrap_err().to_string();
    assert!(err.contains("drive mounted"), "{err}");
    assert!(!gone.exists() && !home.games().join("local-test-proton").exists());
    assert!(!home.client_ran());
}
```

No existing test runs `launch_prepared` to a successful end (they all stop at a refusal). If `Forwarding::install` or the tee cannot complete inside the test harness, keep the first test's prepare-side assertions, assert on `prepared.plan` (`program` is the stand-in shim, args as above, `cwd` is the game folder), and move the end-to-end shim check into Task 17's GUI test by pressing Play on the local row. A third test, `a_local_game_needs_no_xbox_client`, pushes `("XODUS_CLI_DIR", "")` onto `home.extra` (the `Env` map is a `BTreeMap`, so the later entry replaces `Home::paths()`'s hardcoded one, and an empty variable means unset — `paths::tests::an_empty_variable_means_unset`), asserts `paths.cli_dir()` is `None`, and asserts `prepare_launch` for the local recipe succeeds while the same call for `game_recipe("")` still fails with "no xodus-cli found".

Also `arguments_after_dashes_reach_a_local_game_only`: `title_args_allowed` accepts the local recipe and refuses an Xbox one (`game_recipe("")`) and an unknown title with the existing usage text. And `arguments_to_an_xbox_title_are_refused_before_anything_happens`: `Home` with `game_recipe("")`, no recipe for `9ZZUNKNOWN12`, a stand-in client (`home.client("")`); `run_title(&cli(), &paths, t, LaunchRequest::default(), &["-x".into()])` errs with the usage text for `t` in `["testgame", "9ZZUNKNOWN12"]`; afterwards `home.client_ran()` is false and `paths.user_titles_dir()` holds no new `.toml`. The existing `run_refuses_with_the_reason_and_force_gets_past_it`, `an_unfinished_download_is_not_launched` and `a_title_being_installed_is_not_launched` must stay green.

- [ ] **Step 2: Run** `$HOME/.cargo/bin/cargo test --locked -p ferestre-cli local` — expect FAIL.
- [ ] **Step 3: Implement** the branches (one `let local = recipe.is_local();` per function; each skipped step guarded by it), `run_title`, and `title_args_allowed`.
- [ ] **Step 4: Run** `$HOME/.cargo/bin/cargo test --locked -p ferestre-cli` — expect PASS.
- [ ] **Step 5: Commit** "Launch local games without the Xbox client, service, accounts or presence" with the trailer.

### Task 12: `ferestre uninstall` forgets a local game

**Files:**
- Modify: `crates/ferestre-cli/src/main.rs:1428-1594` (`uninstall_title`)

**Interfaces:**
- Consumes: `local::forget` (Task 4).
- Produces: in `uninstall_title`, right after `recipes` load, when a local recipe matches `title` (id or slug): `--prefix` (with or without `--prefix-path`) bails `"Ferestre never deletes a local game's files or prefix; remove {dir} yourself if you mean to"`; otherwise take `TitleLock::take(.., LockMode::Change)`, call `local::forget`, print `-- forgot {name}; its files in {dir} and its Wine prefix {prefix} are kept`, and with `--json` print `{"schema", "product_id", "local": true, "forgot": true, "kept": [dir, prefix]}`. No question is asked (nothing is deleted), so `--yes` is not required. `--keep-files` is accepted and changes nothing.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn removing_a_local_game_deletes_nothing_on_disk() {
    let home = Home::new("uninstall-local");
    let game = home.root().join("drive/Game");
    std::fs::create_dir_all(&game).unwrap();
    std::fs::write(game.join("Game.exe"), b"").unwrap();
    let prefix = home.root().join("steam/steamapps/compatdata/3000000001");
    std::fs::create_dir_all(prefix.join("pfx")).unwrap();
    local_game_recipe(&home, &game, &format!("prefix = \"{}\"", prefix.display()), ""); // helper from Task 11
    let paths = home.paths();
    let with_prefix = Removal { remove_prefix: true, ..YES };
    assert!(uninstall(&paths, "LOCAL-TEST", with_prefix).unwrap_err().to_string().contains("never deletes"));
    assert_eq!(uninstall(&paths, "local-test", YES).unwrap(), ExitCode::SUCCESS);
    assert!(game.join("Game.exe").is_file() && prefix.join("pfx").is_dir());
    assert!(!paths.user_titles_dir().join("LOCAL-TEST.toml").exists());
}
```

- [ ] **Step 2: Run** `$HOME/.cargo/bin/cargo test --locked -p ferestre-cli removing_a_local_game` — expect FAIL.
- [ ] **Step 3: Implement** the early branch.
- [ ] **Step 4: Run** `$HOME/.cargo/bin/cargo test --locked -p ferestre-cli uninstall` — expect PASS (all uninstall tests).
- [ ] **Step 5: Commit** "Make Remove on a local game forget it and keep every file" with the trailer.

### Task 13: `ferestre add` and `ferestre import-steam`

**Files:**
- Modify: `crates/ferestre-cli/src/main.rs:96-229` (`Cmd`), `:265-331` (`dispatch`), new `cmd_add`, `cmd_import_steam` after `cmd_uninstall`

**Interfaces:**
- Consumes: `local::{add, AddRequest}`, `steam_import::{plan, apply, Candidate, ImportStatus, RuntimeFound}`, `anticheat::{detect, describe}`.
- Produces:
  - `Cmd::Add { #[arg(value_name = "EXE")] exe: PathBuf, #[arg(long)] name: Option<String>, #[arg(long, value_name = "DIR")] dir: Option<PathBuf>, #[arg(long, value_name = "DIR")] prefix: Option<PathBuf>, #[arg(long, value_name = "RUNTIME")] runtime: Option<String> }` — doc: "Add a Windows game that is already on disk. Its files are never moved or deleted." A relative `exe`/`dir`/absolute-intended path is made absolute against the current directory before `local::add`. Prints `-- added {name} as {id}; start it with: ferestre run {id}` plus `-- {describe}` when anti-cheat is found; `--json` prints `{"schema", "product_id", "name", "dir", "executable", "prefix", "anticheat": [{"name","kernel","proton_runtime","evidence"}]}`.
  - `Cmd::ImportSteam { #[arg(long)] dry_run: bool, #[arg(long, value_name = "ID")] user: Option<String> }` — doc: "Import non-Steam shortcuts from Steam as local games. Steam's files are only read." Prints a `table` (existing helper) with columns `STATUS` (`new`/`kept`/`skipped: <why>`), `ID`, `NAME`, `PREFIX` (`managed` when none), `RUNTIME` (`Ferestre` / tool dir / `missing: <name>`), then each warning as `-- …`, then `-- would import N` (dry run) or `-- imported N, kept M, skipped K`. `--dry-run` calls only `plan`. `--json` prints `{"schema", "dry_run", "candidates":[{"id","name","user","status","prefix","runtime","anticheat","warnings"}]}`.

- [ ] **Step 1: Write the failing tests**
  - `every_subcommand_the_placeholder_had_still_parses`: add `["add", "/g/Game.exe", "--name", "G", "--prefix", "/p", "--runtime", "GE-Proton10-25"]` and `["import-steam", "--dry-run", "--user", "12345678"]` rows.
  - `import_steam_dry_run_writes_nothing`: a `Home` with `extra` `XODUS_STEAM_DIR=<root>/steam` and `<root>/steam/userdata/12345678/config/shortcuts.vdf` holding one `.exe` shortcut, built inline with `steam::serialise` (core test helpers are not visible from the CLI crate); run `cmd_import_steam` with `dry_run = true`; assert `paths.user_titles_dir()` holds no `LOCAL-STEAM-*.toml`; with `dry_run = false`, assert exactly one.
- [ ] **Step 2: Run** `$HOME/.cargo/bin/cargo test --locked -p ferestre-cli -- every_subcommand import_steam` — expect FAIL.
- [ ] **Step 3: Implement** the two commands (both take `&Cli` and build `Paths::from_env()`; give `cmd_import_steam` an inner `fn import_steam(cli: &Cli, paths: &Paths, dry_run: bool, user: Option<&str>) -> Result<ExitCode>` so the test passes its own `Paths`).
- [ ] **Step 4: Run** `$HOME/.cargo/bin/cargo test --locked -p ferestre-cli` — expect PASS.
- [ ] **Step 5: Commit** "Add the add and import-steam commands" with the trailer.

### Task 14: GUI model — local rows

**Files:**
- Modify: `crates/ferestre-gui/src/model.rs:164-233` (`Inputs`), `:266-336` (`action_for`), `:797-838` (`LibraryRow`), `:867-953` (`library`), `:963-1021` (`described_row`), `undescribed_row` literal, every `Inputs { … }` literal in the tests module and the `inputs()` helper at `:1620`
- Modify: `crates/ferestre-gui/src/main.rs:1320` (`draw_list`'s `Inputs`) — passes `&model.local_runtimes` and a closure over `model.anticheat` (fields added in Task 15; in this task pass `&[]` and `&|_| Vec::new()` from `draw_list`, replaced in Task 15)

**Interfaces:**
- Consumes: `Recipe::is_local`, `anticheat::{Detection, describe}`.
- Produces:
  - `Inputs.local_runtimes: &'a [InstalledRuntime]` — the runtimes a local game may use (`runtime::for_recipe` of a local recipe).
  - `Inputs.anticheat: &'a dyn Fn(&Recipe) -> Vec<anticheat::Detection>`.
  - `LibraryRow.local: bool`.
  - `action_for`, local recipe: Stop/Installing first as today; runtime judged with `inputs.local_runtimes`; `autofix::infer` skipped; then `Play` when `(inputs.installed)(recipe)`, else `Blocked(format!("Not found: {} — is its drive mounted?", <dir>/<executable>))`; never `Install`/`Resume`. The caution gets `describe(&(inputs.anticheat)(recipe))` joined in.
  - `library`: a local recipe is always a row.
  - `described_row`, local: `local: true`, `badge: Some(("Local", "dim-label"))`, `image: None`, `update: None`, subtitle `"Local game"` or `"Local game  ·  {caution}"` when installed, the Blocked reason otherwise.

- [ ] **Step 1: Write the failing tests** (model tests module; a `local_recipe(dir)` helper)
  - `a_local_game_is_listed_and_plays_without_ownership_or_a_record` — `Ownership::Unknown`, no records, `installed = |_| true` → one row, `local`, `Action::Play`, no `Install` anywhere.
  - `a_missing_local_executable_blocks_play_without_offering_install` — `installed = |_| false` → `Action::Blocked(reason)` with `reason.contains("drive mounted")`.
  - `a_local_game_may_use_a_steam_proton_and_an_xbox_title_may_not` — `runtimes = []`, `local_runtimes = [ge]`, `selected_runtime = |_| Some(ge.path)`: local → Play; the Xbox row → Blocked.
  - `anti_cheat_shows_as_a_caution_on_a_local_row` — `anticheat = |_| vec![Ricochet detection]` → subtitle contains "Ricochet" and "kernel driver".
- [ ] **Step 2: Run** `$HOME/.cargo/bin/cargo test --locked -p ferestre-gui model::tests` — expect FAIL.
- [ ] **Step 3: Implement**, adding `local_runtimes: &[]` and `anticheat: &|_| Vec::new()` (a `NO_ANTICHEAT` const like `NO_GAME_DIR`) to every existing test literal.
- [ ] **Step 4: Run** `$HOME/.cargo/bin/cargo test --locked -p ferestre-gui` — expect PASS.
- [ ] **Step 5: Commit** "Show local games in the library with Play and anti-cheat cautions" with the trailer.

### Task 15: GUI state — installed, runtimes, anti-cheat and art for local games

**Files:**
- Modify: `crates/ferestre-gui/src/state.rs:72-135` (`Model`), `:165-370` (`Model::load`), `:375-404` (`reload`), `:406-411` (`is_installed`), `:413-424` (`incomplete`), `:431-452` (`game_dir`), `:454-456` (`installed_version`), `:462-476` (`wineserver`), `:478-493` (`launch_runtime`), `:495-506` (`runtime_note`), test helper `model_at` (`:1110`)
- Modify: `crates/ferestre-gui/src/main.rs` — `draw_list` (`:1320`) passes the real fields; `fixture()` (`:6216`) Model literal; `runtime_chooser` (`:1982-2000`) lists `model.runtimes_for(id)`; `render_featured` (`:2302-2306`) falls back to `model.local_art[key].hero`

**Interfaces:**
- Consumes: `runtime::for_recipe`, `local::{is_present, artwork}`, `anticheat::detect`.
- Produces:
  - `Model.local_runtimes: Vec<InstalledRuntime>` (`all` + `external`, loaded once), `Model.anticheat: BTreeMap<String, Vec<anticheat::Detection>>` (local recipes only, computed in `load`), `Model.local_art: BTreeMap<String, steam::ArtworkFiles>`.
  - `pub fn runtimes_for(&self, product_id: &str) -> &[InstalledRuntime]` — `local_runtimes` for a local id, else `runtimes`.
  - `is_installed` → `local::is_present` for a local recipe; `incomplete` → `false` for a local id; `game_dir` → `paths.install_dir(recipe)` for local (no record lookup); `installed_version` → `None` for local; `launch_runtime` skips `autofix::infer` and uses `runtimes_for`.
  - `pub fn load() -> Self` becomes `Self::load_from(Paths::from_env())`, and the body moves to `pub(crate) fn load_from(paths: anyhow::Result<Paths>) -> Self` (the existing `match` on the `Result` is unchanged). `pub fn reload(&mut self)` becomes `self.reload_from(Model::load())`, and its body moves to `pub(crate) fn reload_from(&mut self, fresh: Model)`. Behaviour is unchanged; tests can now load a `Model` from a temp root.
  - `load_from` puts each local game's portrait (else icon) into `icons`; `reload_from` re-inserts `fresh.local_art`'s portraits into the carried-over `icons` map (it currently replaces `fresh.icons` with the old map).
  - `wineserver` uses `paths.prefix_dir(recipe)` as it is for a local recipe — no `accounts::player_prefix` even when roles are set — because Task 11's local launch runs in the plain prefix, and Stop must target the server of the prefix the game is actually in.

- [ ] **Step 1: Write the failing tests** (state tests, using `model_at`)
  - `a_local_game_is_installed_when_its_executable_exists` — local recipe with dir in the temp root; false before `Game.exe` is written, true after; `incomplete("LOCAL-T")` false; `game_dir` is the recipe's dir even when a stale record for that id exists.
  - `local_art_survives_a_reload` — build the same `Paths` as `model_at` (pull its `Paths::resolve(..)` into a `paths_at(root)` helper that `model_at` calls); write a local recipe into `paths.user_titles_dir()` and `state/ferestre/local-art/LOCAL-T/portrait.png`; `Model::load_from(Ok(paths.clone()))` → `icons["LOCAL-T"]` is that file; after `m.reload_from(Model::load_from(Ok(paths)))` it still is.
  - `stop_targets_a_local_games_own_prefix_even_with_account_roles` — `model_at` with a runtime holding `files/bin/wineserver`, a local recipe with an absolute prefix `P`, `roles: Some(accounts::Selection { store: OWNER, player: PLAYER, generation: 1 })` (two 32-hex ids) and `profiles: Some(accounts::parse_profiles(..))` of a schema-1 JSON listing both ids with the client capability (state.rs has no roles test yet; build the JSON the way `accounts.rs`'s tests do); `wineserver(&local)` is `Some((_, P/pfx))`; for an Xbox recipe the same model still gives the `players/…` prefix (the existing behaviour).
- [ ] **Step 2: Run** `$HOME/.cargo/bin/cargo test --locked -p ferestre-gui state::tests` — expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `$HOME/.cargo/bin/cargo test --locked -p ferestre-gui` and `$HOME/.cargo/bin/cargo build --locked -p ferestre-gui` — expect PASS.
- [ ] **Step 5: Commit** "Track local games' presence, runtimes, anti-cheat and art in the window" with the trailer.

### Task 16: GUI row menu, launch, Remove-as-Forget, Add to Steam and the editor

**Files:**
- Modify: `crates/ferestre-gui/src/main.rs:2689-3104` (`library_components`), `:3532-3600` (`spawn_title`), `:4947-5145` (`add_to_steam_titles`), `:4816-4860` (`open_editor`), new `confirm_forget_local` next to `confirm_forget_library` (`:4762`)
- Modify: `crates/ferestre-gui/src/editor.rs:27-108` (`Draft`), `:125-…` (`present`), and every `Draft { … }` literal in its tests (`:246`, `:313`, `:328`, `:344`, `:365`, `:378`, `:388`, `:420`), which gain `arguments: String::new(), wrapper: String::new()`
- Modify: `crates/ferestre-gui/src/model.rs` (two pure helpers, below)

**Interfaces:**
- Consumes: `LibraryRow.local`, `Model::runtimes_for`, `local::artwork`, `model::forget_args`.
- Produces:
  - `library_components` for `row.local`: no "Open Store page", no "Move location", no "Remove from disk"; "Choose runtime", "Add to Steam", "Edit launch settings" stay; a new item `("user-trash-symbolic", "Remove from library", confirm_forget_local)` when `row.action != Action::Stop`.
  - `fn confirm_forget_local(ui: &Ui, product_id: &str, name: &str)` — `adw::AlertDialog` titled `Remove {name} from Ferestre?`, body exactly `"{name} leaves your library. Nothing is deleted: the game's files in {dir} and its Wine prefix {prefix} are kept."`, responses `cancel`/`remove` (`remove` has `adw::ResponseAppearance::Destructive`), and on `remove` runs `spawn_cli_tracked(&ui, forget_args(id), "Removing {name}")`.
  - `model::launch_uses_accounts(recipe: Option<&Recipe>) -> bool` — `false` for a local recipe, `true` otherwise. `spawn_title` reads it once: when `false` it skips the account-roles block, always calls `command.env_remove("FERESTRE_ACCOUNT_ROLES")`, and skips `presence_ui::launch_started`.
  - `model::store_ids(ids: &[String]) -> Vec<String>` — keeps only `is_store_product_id` ids, in order. `add_to_steam_titles` passes its `stale` list through it before `catalog::revalidate`; for a local id the product is `None` and the artwork is `local::artwork(paths, id)` instead of `steam::prepare_artwork`.
  - `editor::Draft` gains `arguments: String` and `wrapper: String` (shown with `steam_import::quote_words` from Task 8; parsed back with `steam_import::parse_launch_options` on the text `"<wrapper> %command% <arguments>"` so the quoting rules are one). Because that parser moves leading `NAME=value` words into `env`, `apply` refuses a wrapper whose first word matches `[A-Za-z_][A-Za-z0-9_]*=…` with "put environment variables in Environment, not Wrapper commands", and refuses a non-empty `env` from the parse instead of dropping it; `Draft::apply` for a local recipe accepts an absolute executable, absolute install folder and absolute prefix (still refusing `..`), and keeps `install.dir` as given instead of dropping it when equal to the slug. `present` shows the two extra rows only for a local recipe, titled "Arguments" and "Wrapper commands".

- [ ] **Step 1: Write the failing tests**
  - `editor.rs`: `a_local_draft_keeps_absolute_paths_arguments_and_wrappers` — `Draft::from_recipe(&local)` then `apply` with `arguments = "-dx12 \"a b\""`, `wrapper = "gamemoderun"` → recipe args `["-dx12","a b"]`, wrapper `["gamemoderun"]`, dir unchanged; an Xbox draft with an absolute executable is still refused.
  - `editor.rs`: `an_environment_word_is_not_a_wrapper` — `wrapper = "DXVK_HDR=1 gamemoderun"` → `apply` errs naming Environment.
  - `model.rs`: `local_games_skip_accounts_and_the_store_catalog` — `launch_uses_accounts(Some(&local))` false, `Some(&xbox)` and `None` true; `store_ids(&["LOCAL-STEAM-1", "9NBLGGH2JHXJ"])` is `["9NBLGGH2JHXJ"]`.
  - The widget-level behaviour (no Store page, Remove forgets, Add to Steam writes a working shortcut) is covered by Task 17's GUI test.
- [ ] **Step 2: Run** `$HOME/.cargo/bin/cargo test --locked -p ferestre-gui -- editor model::tests::local_games_skip` — expect FAIL.
- [ ] **Step 3: Implement** the editor change, then the `main.rs` changes.
- [ ] **Step 4: Run** `$HOME/.cargo/bin/cargo test --locked -p ferestre-gui` and `$HOME/.cargo/bin/cargo build --locked --workspace` — expect PASS.
- [ ] **Step 5: Commit** "Give local game rows their own menu: no Store page, Remove forgets" with the trailer.

### Task 17: GUI — Add game… and Import from Steam

**Files:**
- Modify: `crates/ferestre-gui/src/folder_picker.rs:4-…` (`open`)
- Modify: `crates/ferestre-gui/src/main.rs:2113-2147` (`render_library_tabs`), new `add_game_dialog`, `import_from_steam`
- Create: `tests/gui_local_games.py`
- Modify: `.github/workflows/rust.yml` (both `paths:` lists near `:48` and `:109`; a step after `:515` with `--server-num=138`)

**Interfaces:**
- Consumes: `local::{add, AddRequest, derive_name}`, `steam_import::{plan, apply}`, `anticheat::describe`.
- Produces:
  - `folder_picker::open_file(ui: &Ui, target: &gtk::Editable, extensions: &'static [&'static str])` — the same browser (shared private `fn browse_dialog(ui, target, pick: Pick)`), titled "Choose the game's program", listing folders and files whose extension matches (ASCII case-insensitive); activating a file sets `target` to its path and closes. `open` keeps its behaviour and title.
  - `render_library_tabs` appends a `gtk::MenuButton` (`list-add-symbolic`, tooltip "Add games", widget name `library-add`) with two items: "Add game…" (`add_game_dialog`) and "Import from Steam" (`import_from_steam`).
  - `fn add_game_dialog(ui: &Ui)` — `adw::Dialog` with `adw::EntryRow`s "Program" (+ "Browse…" → `open_file(.., &["exe", "bat"])`), "Name" (filled from `derive_name` when the program changes and the name was not typed), "Folder" (filled with the program's folder), "Wine prefix" (empty = "Managed by Ferestre"); "Add" calls `local::add` in `gio::spawn_blocking`, then `reload` + `render` + toast `"Added {name}"`, and a second toast with `describe(..)` when anti-cheat is found; an `Err` is shown in the dialog, which stays open.
  - `fn import_from_steam(ui: &Ui)` — `begin(ui, "Reading Steam's shortcuts")`; `steam_import::plan` in `spawn_blocking`; nothing new → toast "Nothing new to import from Steam"; otherwise an `adw::AlertDialog` "Import from Steam" listing up to ten new names, then "and N more", then "M already in your library; K skipped (Ferestre's own shortcuts or not Windows programs)." and "Steam's files are only read.", responses `cancel`/`import`; `import` → `apply` in `spawn_blocking` → `reload` + `render` + toast `"Imported N games from Steam"`.
- GUI test `tests/gui_local_games.py` (pattern: `gui_runtime_selection.py`, reusing `gui_launch.prepare`): after `prepare`, copy the real `ferestre` CLI (`Path(args.binary).resolve().with_name('ferestre')`) over `h/bin/ferestre`; write `config/ferestre/titles/LOCAL-TEST.toml` (kind local, dir `h/'drive/Local Test'`, `Game.exe` and `Randgrid.sys` present, name "Local Test Game"). Checks: the row "Local Test Game" has a Play button; its "More actions" menu has "Remove from library" and has no "Open Store page"; its text mentions "Ricochet"; the add menu (`library-add`) offers "Add game…" and "Import from Steam"; pressing "Remove from library" shows a dialog containing "Nothing is deleted"; confirming removes `LOCAL-TEST.toml` while `drive/Local Test/Game.exe` still exists. Before the removal, with a Steam root set up the way `gui_smoke.py` does (`steam/userdata/1/config/`, `:1020`), pressing "Add to Steam" on the row writes `shortcuts.vdf` whose bytes contain `run LOCAL-TEST` and the art from `local-art/LOCAL-TEST/` when the test placed a `portrait.png` there. Prints `PASS: local game row, menu, anti-cheat caution, Add to Steam and forget-only removal`.

- [ ] **Step 1: Write `tests/gui_local_games.py`** with the checks above.
- [ ] **Step 2: Run it to see it fail**

Run: `$HOME/.cargo/bin/cargo build --locked --workspace && xvfb-run -a --server-args='-screen 0 1280x720x24 -noreset' dbus-run-session -- python3 tests/gui_local_games.py`
Expected: FAIL at the add menu (`library-add` not found) — the row checks already pass after Task 16.

- [ ] **Step 3: Implement** `open_file`, the add menu, `add_game_dialog`, `import_from_steam`; add the CI step.
- [ ] **Step 4: Run the GUI test again, then the workspace**

Run: the Step 2 command, then `python3 tests/gui_runtime_selection.py` the same way (must stay green), then `$HOME/.cargo/bin/cargo test --locked --workspace`
Expected: `PASS: local game row, …`; all green.

- [ ] **Step 5: Commit**

```bash
git add crates/ferestre-gui/src/folder_picker.rs crates/ferestre-gui/src/main.rs tests/gui_local_games.py .github/workflows/rust.yml
git commit -m "Add a game by hand or import Steam's shortcuts from the library page

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Self-review notes

- Spec A coverage: recipe kind/folder/prefix/args/wrappers/env (T1, T2); Steam import of every user's shortcuts with name, exe, start dir, options, prefix, Proton, art, own shortcuts skipped, never writing Steam (T7–T9, T13, T17); hand add with derived name and folder (T10, T13, T17); same `ferestre run` entry and shim with no Xbox service (T5, T11); external Protons for local only (T7, T11, T14, T15); library with installed = executable exists, Play, Add to Steam, edit, anti-cheat (T14–T17); signature list with kernel flag (T6); Remove never deletes (T4, T12, T16).
- Deliberate narrowing: shortcuts whose program is not `.exe`/`.bat` (native Linux games, other launchers) are reported as skipped rather than imported, because the shim can only run Windows programs. Steam's global default compat tool (`CompatToolMapping."0"`) is not applied, because Steam does not apply it to non-Steam shortcuts.
- Type names used across tasks: `TitleKind`, `local::{is_local_id, steam_id, new_id, forget, Forgotten, add, AddRequest, derive_name, executable_path, is_present, artwork, save_artwork}`, `launch::windows_path`, `anticheat::{Detection, Signature, SIGNATURES, detect, describe}` (fields `kernel`, `proton_runtime`), `vdf::parse`, `steam::{libraries, is_ferestre_shortcut}`, `runtime::{SteamTool, steam_tools, steam_tool, external, for_recipe}`, `steam_import::{LaunchOptions, parse_launch_options, expand_home, quote_words, Candidate, ImportStatus, RuntimeFound, plan, apply, Report}`, `Paths::{steam_roots, local_art_dir}`, `Inputs::{local_runtimes, anticheat}`, `LibraryRow::local`, `Model::{local_runtimes, anticheat, local_art, runtimes_for, load_from, reload_from}`, `model::{launch_uses_accounts, store_ids}`, CLI `run_title`/`title_args_allowed`.

## Review notes

First review round. All findings were checked against the worktree, and every one held. What changed:

- Task 1: the `from` strings match `to_toml`'s double-quoted output, and each one is asserted to be present. The kind⇔id cases moved to `kind_and_id_must_agree_both_ways`, which uses a fixture with no args or wrapper and a matching slug, and asserts the error names `kind`. SCHEMA.md's local keys go under a `## ` section with no `### \`[…]\`` key table, so `every_key_the_schema_documents_is_accepted` does not insert them into an Xbox recipe. `new_id` calls `slugify(name, "game")`.
- Commands with several test filters now pass them after `--` (Tasks 1, 3, 4, 7, 9, 13, 16). Checked in the worktree with `cargo test --locked -p ferestre-core --lib -- recipe::tests anticheat::tests`.
- Backslash executables: Task 1's `..` rule, Task 5's `plan` and Task 10's `executable_path` all read `\` as `/`. Tasks 5 and 10 each have a new test for this.
- Task 11: the trailing-args refusal stays ahead of `prepare_launch` (`title_args_allowed` over the read-only `recipes_from`). A new test shows that `run -- -x` on an Xbox or unknown title writes no recipe and does not run the client. The no-client test uses `XODUS_CLI_DIR=""` through `Home.extra`, because `Env` is a `BTreeMap` and an empty value means unset. No `Home::without` is needed. `local_game_recipe` takes separate `install_extra` and `launch_extra`.
- Task 15: `Model::load_from` and `reload_from` make loading testable. `wineserver` skips `player_prefix` for local games, and a test with roles set covers it.
- Task 6: `kernel` again means "ships a kernel driver", as the spec says, so Easy Anti-Cheat and BattlEye are `true`. A separate `proton_runtime` marks the two that Proton bridges. `describe` reports evidence (the file found), not a verdict. The CoD sp26 layout is a publicly modded build, so "will not run under Proton" would be untrue for it.
- Task 9 and Task 5: an imported Steam prefix run by a different Proton gets a warning in the import report. The shim skips its GDK-only GamingServices registry write when `FERESTRE_LOCAL_GAME=1`.
- Task 16: the pure helpers `model::launch_uses_accounts` and `model::store_ids` have unit tests. `steam_import::quote_words` (Task 8) replaces the unreachable CLI `shell_quote`. The editor test `Draft` literals are listed. A wrapper starting with `NAME=value` is refused. Task 17's GUI test now presses Add to Steam.
- `steam_import::Status` is renamed to `ImportStatus`.

Not done here (partly rejected):

- The suggestion of a per-game "this build is modded" acknowledgement that Part B can honour. The spec (B, "Anti-cheat gate") says a kernel-level anti-cheat *refuses* enabling, with no override. Every signature here is a kernel driver, so Part B as specified will refuse neural rendering on any game where one of these files is found, including publicly modded builds such as the CoD sp26 layout. Allowing an override is a spec change, so it belongs to the neural-rendering spec and plan, not to Part A. It is flagged for the person to decide. Part A refuses nothing on anti-cheat and only shows the evidence line.

