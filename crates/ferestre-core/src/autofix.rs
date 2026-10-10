//! Launch defaults inferred from the engine a title ships, not from a title list.
//!
//! A failure that belongs to an engine reaches every title built on it, and most
//! of those titles have no tested recipe. Recognising the engine from the files
//! it installs lets a new title get the same fix as the one that was debugged,
//! without anybody writing down its product id first.
//!
//! The inference stays narrow on purpose. A profile only adds an optional
//! capability to the recipe and, when the selected runtime provides that
//! capability, one token to a backend setting. It never removes or replaces
//! something a person configured, and never sends a runtime a setting it does
//! not implement -- an unknown flag is ignored today and may mean something
//! else tomorrow.

use crate::capability::RECORDING_ALLOCATOR_LIFETIME;
use crate::recipe::Recipe;
use std::borrow::Cow;
use std::collections::BTreeSet;
use std::path::{Component, Path};

/// An engine recognised from its files, and what it needs from the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// Ultralight's WebCore renderer, as used by Retro Classics. It releases a
    /// command allocator while a list recorded from it is still open and then
    /// closes the list, which a stock vkd3d-proton turns into a fault in
    /// `vkEndCommandBuffer` on a null buffer.
    Ultralight,
}

impl Profile {
    const ALL: [Profile; 1] = [Profile::Ultralight];

    /// How the engine is named in a message.
    pub fn engine(self) -> &'static str {
        match self {
            Profile::Ultralight => "Ultralight",
        }
    }

    /// What enabling the profile does, for the same message.
    pub fn effect(self) -> &'static str {
        match self {
            Profile::Ultralight => "recording allocator lifetime compatibility",
        }
    }

    /// Files that must all be present, side by side, in one directory. Lower
    /// case: Windows does not care, and packages ship whatever case their
    /// build system produced (`ULTRALIGHT.DLL` and `Ultralight.dll` both
    /// occur). One library alone is not enough -- a title can carry a stray
    /// copy of a common DLL without running the engine it belongs to.
    fn markers(self) -> &'static [&'static str] {
        match self {
            Profile::Ultralight => &["ultralight.dll", "ultralightcore.dll", "webcore.dll"],
        }
    }

    /// The runtime capability that makes the setting below mean something.
    pub fn capability(self) -> &'static str {
        match self {
            Profile::Ultralight => RECORDING_ALLOCATOR_LIFETIME,
        }
    }

    /// The variable to extend and the token to add to it.
    pub fn setting(self) -> (&'static str, &'static str) {
        match self {
            Profile::Ultralight => ("VKD3D_CONFIG", "retain_recording_allocators"),
        }
    }
}

/// Which engine, if any, the installed title runs on.
///
/// Looks in two directories only: the package root and the directory of the
/// executable the recipe launches. Those are where Windows' loader would find
/// the engine's DLLs without help, so they are where an engine that is really
/// in use keeps them; walking the whole tree would match tools, redistributables
/// and leftovers. Names compare case-insensitively.
///
/// An executable path that climbs out of the package (`..`) or is absolute is
/// not followed, but the root is still examined: a bad recipe should cost the
/// executable's directory, not the whole inference. A directory that cannot be
/// read is reported on stderr unless it simply does not exist, which is the
/// ordinary state of a title that is not installed.
pub fn detect(game_dir: &Path, executable: &str) -> Option<Profile> {
    let mut directories = vec![game_dir.to_path_buf()];
    let executable = executable.replace('\\', "/");
    let executable = Path::new(&executable);
    let contained = executable
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    if let Some(parent) = executable
        .parent()
        .filter(|p| contained && !p.as_os_str().is_empty())
    {
        directories.push(game_dir.join(parent));
    }

    directories.iter().find_map(|directory| {
        let names = file_names(directory);
        Profile::ALL
            .into_iter()
            .find(|profile| profile.markers().iter().all(|m| names.contains(*m)))
    })
}

/// The lower-cased names of the regular files in one directory.
fn file_names(directory: &Path) -> BTreeSet<String> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return BTreeSet::new(),
        Err(e) => {
            eprintln!(
                "-- engine detection skipped for {}: {e}",
                directory.display()
            );
            return BTreeSet::new();
        }
    };
    entries
        .filter_map(Result::ok)
        // `Path::is_file` follows a symlink, which is how some installs share
        // one copy of a runtime between packages.
        .filter(|entry| entry.path().is_file())
        .map(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase())
        .collect()
}

/// A recipe as the launcher assesses it, and the engine that shaped it.
#[derive(Debug, Clone)]
pub struct Inferred<'a> {
    pub recipe: Cow<'a, Recipe>,
    pub profile: Option<Profile>,
}

/// The recipe with what its installed engine implies, for every caller that
/// asks whether a runtime can run it.
///
/// One function rather than `detect` and [`add_wants`] at each call site,
/// because the CLI and the window must reach the same verdict about the same
/// title: an inferred want that one of them forgot would make a runtime look
/// complete in one place and short of something in the other. `None` for the
/// directory means it is not known, and the recipe is used as written.
pub fn infer<'a>(recipe: &'a Recipe, game_dir: Option<&Path>) -> Inferred<'a> {
    let profile = game_dir.and_then(|dir| detect(dir, &recipe.launch.executable));
    let recipe = match profile {
        Some(profile) if !names(recipe, profile.capability()) => {
            let mut owned = recipe.clone();
            add_wants(&mut owned, profile);
            Cow::Owned(owned)
        }
        _ => Cow::Borrowed(recipe),
    };
    Inferred { recipe, profile }
}

/// Record the profile's capability as wanted, once.
///
/// Wanted rather than required: a runtime without it still starts the title,
/// and a title on such a runtime is no worse off than before the engine was
/// recognised. A recipe that already requires it is left alone.
pub fn add_wants(recipe: &mut Recipe, profile: Profile) {
    let capability = profile.capability();
    if !names(recipe, capability) {
        recipe.runtime.wants.push(capability.to_string());
    }
}

fn names(recipe: &Recipe, capability: &str) -> bool {
    let runtime = &recipe.runtime;
    runtime
        .requires
        .iter()
        .chain(&runtime.wants)
        .any(|c| c == capability)
}

/// What [`apply`] did, so the launcher can say exactly that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// No known engine was found.
    NotDetected,
    /// The setting is set and empty, in the recipe or the environment. Empty
    /// is how somebody turns every backend flag off, inferred ones included.
    SkippedExplicitEmpty,
    /// The setting already carries the token.
    AlreadyPresent,
    /// The selected runtime does not provide the capability, so the token
    /// would be meaningless to it.
    RuntimeLacks,
    /// The setting was unset and now holds only the token.
    Applied,
    /// The token was appended to the value already in effect, kept here so
    /// the message can show what was extended.
    Merged { previous: String },
}

/// Turn a detected profile into launch configuration, if that is safe.
///
/// `inherited` is the variable in the launcher's own environment, empty
/// included. It matters because a recipe's `env` replaces the inherited value
/// rather than extending it (`launch::plan` applies recipe variables last), so
/// writing the token on its own would silently drop flags the person set. The
/// value in effect -- the recipe's if it has one, otherwise the inherited one
/// -- is therefore extended, and the result goes into the recipe.
pub fn apply(
    recipe: &mut Recipe,
    profile: Option<Profile>,
    provided: &BTreeSet<String>,
    inherited: Option<&str>,
) -> Outcome {
    let Some(profile) = profile else {
        return Outcome::NotDetected;
    };
    let (key, token) = profile.setting();
    let current = recipe.launch.env.get(key).map(String::as_str).or(inherited);
    match current {
        Some("") => return Outcome::SkippedExplicitEmpty,
        Some(value) if has_token(value, token) => return Outcome::AlreadyPresent,
        _ => {}
    }
    if !provided.contains(profile.capability()) {
        return Outcome::RuntimeLacks;
    }
    let (value, outcome) = match current {
        None => (token.to_string(), Outcome::Applied),
        Some(previous) => {
            let separator = if previous.ends_with([',', ';']) {
                ""
            } else {
                ","
            };
            (
                format!("{previous}{separator}{token}"),
                Outcome::Merged {
                    previous: previous.to_string(),
                },
            )
        }
    };
    recipe.launch.env.insert(key.to_string(), value);
    outcome
}

/// Whether a vkd3d-style flag list contains a token. The same reading as
/// vkd3d-proton's `vkd3d_debug_list_has_member`: `,` and `;` separate, and
/// nothing is trimmed.
fn has_token(list: &str, token: &str) -> bool {
    list.split([',', ';']).any(|t| t == token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    const TOKEN: &str = "retain_recording_allocators";
    const ULTRALIGHT: [&str; 3] = ["ULTRALIGHT.DLL", "UltralightCore.dll", "WebCore.dll"];

    /// A fresh directory per test, so tests running in parallel never share
    /// a fixture and a failed run leaves nothing for the next one to trip on.
    fn scratch(name: &str) -> PathBuf {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ferestre-autofix-{}-{name}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn files_in(dir: &Path, names: &[&str]) {
        std::fs::create_dir_all(dir).unwrap();
        for name in names {
            std::fs::write(dir.join(name), []).unwrap();
        }
    }

    fn capable() -> BTreeSet<String> {
        [RECORDING_ALLOCATOR_LIFETIME.to_string()].into()
    }

    fn blank() -> Recipe {
        Recipe::blank("9ZZUNKNOWN01", "An unknown title")
    }

    // --- detection -----------------------------------------------------------

    #[test]
    fn the_engine_is_found_beside_an_executable_at_the_package_root() {
        // Retro Classics' layout: RetroClassics.exe and the three libraries
        // all at the root of the package.
        let dir = scratch("root");
        files_in(
            &dir,
            &[
                "RetroClassics.exe",
                "Ultralight.dll",
                "UltralightCore.dll",
                "WebCore.dll",
            ],
        );
        assert_eq!(detect(&dir, "RetroClassics.exe"), Some(Profile::Ultralight));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_engine_is_found_beside_an_executable_in_a_subdirectory() {
        let dir = scratch("nested");
        files_in(&dir.join("bin"), &ULTRALIGHT);
        assert_eq!(detect(&dir, "bin\\Anything.exe"), Some(Profile::Ultralight));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_incomplete_set_is_not_the_engine() {
        let dir = scratch("partial");
        files_in(&dir.join("bin"), &ULTRALIGHT[..2]);
        assert_eq!(detect(&dir, "bin\\Anything.exe"), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_libraries_must_share_one_directory() {
        let dir = scratch("split");
        files_in(&dir, &ULTRALIGHT[..1]);
        files_in(&dir.join("bin"), &ULTRALIGHT[1..]);
        assert_eq!(detect(&dir, "bin\\Anything.exe"), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_executable_outside_the_package_is_not_followed_but_the_root_still_counts() {
        let dir = scratch("escape");
        let outside = dir.join("outside");
        files_in(&outside, &ULTRALIGHT);
        let game = dir.join("game");
        std::fs::create_dir_all(&game).unwrap();
        // The fixture is real: where `../outside` leads, the engine is found.
        assert_eq!(detect(&outside, "Elsewhere.exe"), Some(Profile::Ultralight));
        assert_eq!(detect(&game, "../outside/Elsewhere.exe"), None);
        let absolute = outside.join("Elsewhere.exe");
        assert_eq!(detect(&game, &absolute.to_string_lossy()), None);

        // With the engine at the root, a rejected path does not hide it.
        files_in(&game, &ULTRALIGHT);
        assert_eq!(
            detect(&game, "..\\outside\\Elsewhere.exe"),
            Some(Profile::Ultralight)
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_title_that_is_not_installed_has_no_engine() {
        let dir = scratch("absent");
        assert_eq!(detect(&dir.join("not-there"), "Game.exe"), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    // --- inference -----------------------------------------------------------

    #[test]
    fn inference_adds_the_capability_as_a_want_once() {
        let dir = scratch("infer");
        files_in(&dir, &ULTRALIGHT);
        let recipe = blank();
        let inferred = infer(&recipe, Some(&dir));
        assert_eq!(inferred.profile, Some(Profile::Ultralight));
        assert_eq!(
            inferred.recipe.runtime.wants,
            [RECORDING_ALLOCATOR_LIFETIME]
        );
        let again = infer(&inferred.recipe, Some(&dir));
        assert!(matches!(again.recipe, Cow::Borrowed(_)));
        assert_eq!(again.recipe.runtime.wants, [RECORDING_ALLOCATOR_LIFETIME]);
        // Recognising an engine says nothing about whether the title works.
        assert_eq!(
            inferred.recipe.status.state,
            crate::recipe::TitleState::Untested
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_required_capability_is_not_also_wanted() {
        let mut recipe = blank();
        recipe.runtime.requires = vec![RECORDING_ALLOCATOR_LIFETIME.into()];
        add_wants(&mut recipe, Profile::Ultralight);
        assert!(recipe.runtime.wants.is_empty());
    }

    #[test]
    fn an_unknown_directory_leaves_the_recipe_as_written() {
        let recipe = blank();
        let inferred = infer(&recipe, None);
        assert_eq!(inferred.profile, None);
        assert!(matches!(inferred.recipe, Cow::Borrowed(_)));
    }

    // --- applying ------------------------------------------------------------

    fn ultralight(
        recipe: &mut Recipe,
        provided: &BTreeSet<String>,
        inherited: Option<&str>,
    ) -> Outcome {
        apply(recipe, Some(Profile::Ultralight), provided, inherited)
    }

    #[test]
    fn nothing_detected_changes_nothing() {
        let mut recipe = blank();
        assert_eq!(
            apply(&mut recipe, None, &capable(), None),
            Outcome::NotDetected
        );
        assert!(recipe.launch.env.is_empty());
    }

    #[test]
    fn an_unset_variable_gets_the_token_on_a_capable_runtime() {
        let mut recipe = blank();
        assert_eq!(ultralight(&mut recipe, &capable(), None), Outcome::Applied);
        assert_eq!(recipe.launch.env["VKD3D_CONFIG"], TOKEN);
    }

    #[test]
    fn a_runtime_without_the_capability_is_never_sent_the_token() {
        let mut recipe = blank();
        assert_eq!(
            ultralight(&mut recipe, &BTreeSet::new(), Some("dxr")),
            Outcome::RuntimeLacks
        );
        assert!(recipe.launch.env.is_empty());
    }

    #[test]
    fn an_empty_inherited_variable_is_an_explicit_opt_out() {
        let mut recipe = blank();
        assert_eq!(
            ultralight(&mut recipe, &capable(), Some("")),
            Outcome::SkippedExplicitEmpty
        );
        assert!(recipe.launch.env.is_empty());
    }

    #[test]
    fn an_empty_recipe_value_is_an_explicit_opt_out_whatever_was_inherited() {
        let mut recipe = blank();
        recipe
            .launch
            .env
            .insert("VKD3D_CONFIG".into(), String::new());
        assert_eq!(
            ultralight(&mut recipe, &capable(), Some("force_static_cbv")),
            Outcome::SkippedExplicitEmpty
        );
        assert_eq!(recipe.launch.env["VKD3D_CONFIG"], "");
    }

    #[test]
    fn inherited_flags_are_kept_when_the_token_is_added() {
        // The recipe's env replaces the inherited variable at launch, so the
        // inherited flags have to be carried into it.
        let mut recipe = blank();
        assert_eq!(
            ultralight(&mut recipe, &capable(), Some("force_static_cbv")),
            Outcome::Merged {
                previous: "force_static_cbv".into()
            }
        );
        assert_eq!(
            recipe.launch.env["VKD3D_CONFIG"],
            format!("force_static_cbv,{TOKEN}")
        );
    }

    #[test]
    fn a_recipe_value_wins_over_the_inherited_one_and_is_extended() {
        let mut recipe = blank();
        recipe
            .launch
            .env
            .insert("VKD3D_CONFIG".into(), "dxr;".into());
        assert_eq!(
            ultralight(&mut recipe, &capable(), Some("force_static_cbv")),
            Outcome::Merged {
                previous: "dxr;".into()
            }
        );
        assert_eq!(recipe.launch.env["VKD3D_CONFIG"], format!("dxr;{TOKEN}"));
    }

    #[test]
    fn a_token_already_present_is_left_alone() {
        let mut recipe = blank();
        let inherited = format!("dxr;{TOKEN}");
        assert_eq!(
            ultralight(&mut recipe, &capable(), Some(&inherited)),
            Outcome::AlreadyPresent
        );
        assert!(recipe.launch.env.is_empty());

        // A longer flag that merely contains the token's text is not it.
        let mut recipe = blank();
        assert!(matches!(
            ultralight(
                &mut recipe,
                &capable(),
                Some("no_retain_recording_allocators")
            ),
            Outcome::Merged { .. }
        ));
    }
}
