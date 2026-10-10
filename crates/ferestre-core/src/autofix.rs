//! Conservative launch defaults inferred from an installed engine, not a title list.
//! A profile is optional: it never overrides an explicit config or emits a flag
//! the selected runtime does not implement.
use crate::recipe::Recipe;
use std::collections::BTreeSet;
use std::path::{Component, Path};

pub const ULTRALIGHT_ALLOCATORS: &str = "d3d12.recording-allocator-lifetime";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Ultralight,
}

pub fn detect(game_dir: &Path, executable: &str) -> Vec<Profile> {
    let executable = executable.replace('\\', "/");
    let path = Path::new(&executable);
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Vec::new();
    }
    let mut directories = vec![game_dir.to_path_buf()];
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        directories.push(game_dir.join(parent));
    }
    for directory in directories {
        let files: BTreeSet<String> = std::fs::read_dir(directory)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_file())
            .map(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase())
            .collect();
        if ["ultralight.dll", "ultralightcore.dll", "webcore.dll"]
            .iter()
            .all(|n| files.contains(*n))
        {
            return vec![Profile::Ultralight];
        }
    }
    Vec::new()
}

pub fn add_wants(recipe: &mut Recipe, profiles: &[Profile]) {
    if profiles.contains(&Profile::Ultralight)
        && !recipe
            .runtime
            .wants
            .iter()
            .any(|c| c == ULTRALIGHT_ALLOCATORS)
    {
        recipe.runtime.wants.push(ULTRALIGHT_ALLOCATORS.to_string());
    }
}

/// Return whether a compatibility setting was added. An environment supplied
/// by the user takes precedence, including an explicitly empty value.
pub fn apply(
    recipe: &mut Recipe,
    profiles: &[Profile],
    provided: &BTreeSet<String>,
    user_configured: bool,
) -> bool {
    if profiles.contains(&Profile::Ultralight)
        && provided.contains(ULTRALIGHT_ALLOCATORS)
        && !user_configured
        && !recipe.launch.env.contains_key("VKD3D_CONFIG")
    {
        recipe
            .launch
            .env
            .insert("VKD3D_CONFIG".into(), "retain_recording_allocators".into());
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_title_gets_engine_defaults_only_on_a_capable_runtime() {
        let dir = std::env::temp_dir().join(format!("ferestre-engine-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        for name in ["ULTRALIGHT.DLL", "UltralightCore.dll", "WebCore.dll"] {
            std::fs::write(dir.join("bin").join(name), []).unwrap();
        }
        let profiles = detect(&dir, "bin\\Anything.exe");
        assert_eq!(profiles, vec![Profile::Ultralight]);
        let mut recipe = Recipe::blank("9ZZUNKNOWN01", "An unknown title");
        add_wants(&mut recipe, &profiles);
        add_wants(&mut recipe, &profiles);
        assert_eq!(recipe.runtime.wants, vec![ULTRALIGHT_ALLOCATORS]);
        assert!(!apply(&mut recipe, &profiles, &BTreeSet::new(), false));
        assert!(recipe.launch.env.is_empty());
        let provided = [ULTRALIGHT_ALLOCATORS.to_string()].into_iter().collect();
        assert!(apply(&mut recipe, &profiles, &provided, false));
        assert_eq!(
            recipe.launch.env["VKD3D_CONFIG"],
            "retain_recording_allocators"
        );
        assert_eq!(recipe.status.state, crate::recipe::TitleState::Untested);
        std::fs::remove_file(dir.join("bin/WebCore.dll")).unwrap();
        assert!(detect(&dir, "bin\\Anything.exe").is_empty());
        assert!(detect(&dir, "../Elsewhere.exe").is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn explicit_recipe_and_user_environment_are_preserved() {
        let provided = [ULTRALIGHT_ALLOCATORS.to_string()].into_iter().collect();
        let profiles = [Profile::Ultralight];
        let mut recipe = Recipe::blank("9ZZUNKNOWN01", "Unknown");
        assert!(!apply(&mut recipe, &profiles, &provided, true));
        assert!(recipe.launch.env.is_empty());
        recipe
            .launch
            .env
            .insert("VKD3D_CONFIG".into(), String::new());
        assert!(!apply(&mut recipe, &profiles, &provided, false));
        assert_eq!(recipe.launch.env["VKD3D_CONFIG"], "");
    }
}
