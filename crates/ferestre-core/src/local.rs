//! Games Ferestre did not install: a Windows program somewhere on disk, added
//! by hand or imported from a Steam non-Steam shortcut.
//!
//! Such a game is a user recipe with `[title] kind = "local"` and an id that
//! starts with [`ID_PREFIX`]. The prefix has a dash, which a Store product id
//! (12 ASCII letters and digits) never has, so the two can never be confused.

use crate::paths::Paths;
use crate::recipe::Recipe;
use std::path::PathBuf;

/// What every local game's id starts with.
pub const ID_PREFIX: &str = "LOCAL-";

/// Whether `id` names a local game: `LOCAL-` (any case), then letters, digits
/// and dashes, at most 64 bytes in all.
pub fn is_local_id(id: &str) -> bool {
    id.len() <= 64
        && id.len() > ID_PREFIX.len()
        && id.is_char_boundary(ID_PREFIX.len())
        && id[..ID_PREFIX.len()].eq_ignore_ascii_case(ID_PREFIX)
        && id[ID_PREFIX.len()..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The id of a game imported from a Steam shortcut with this app id.
pub fn steam_id(app_id: u32) -> String {
    format!("{ID_PREFIX}STEAM-{app_id}")
}

/// A new id for a game added by hand: `LOCAL-` and its name as a slug,
/// uppercased, then `-2`, `-3`... while `taken` says the id is in use.
pub fn new_id(name: &str, taken: &dyn Fn(&str) -> bool) -> String {
    let mut slug = crate::recipe::slugify(name, "game");
    slug.truncate(40);
    let slug = slug.trim_end_matches('-').to_ascii_uppercase();
    let base = format!("{ID_PREFIX}{slug}");
    if !taken(&base) {
        return base;
    }
    (2u32..)
        .map(|n| format!("{base}-{n}"))
        .find(|id| !taken(id))
        .expect("some suffix is free")
}

/// What forgetting a local game removed, and the folders it left alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forgotten {
    /// The recipe file that was removed, if there was one.
    pub recipe_file: Option<PathBuf>,
    /// The game's folder, which is kept.
    pub kept_dir: PathBuf,
    /// Its Wine prefix, which is kept.
    pub kept_prefix: PathBuf,
}

/// Take a local game out of the library: its recipe, its runtime choice and
/// the art Ferestre copied for it. The game's folder and its prefix are the
/// person's, or Steam's, and are never touched.
pub fn forget(paths: &Paths, product_id: &str) -> anyhow::Result<Forgotten> {
    if !is_local_id(product_id) {
        anyhow::bail!(
            "{product_id} is not a local game's id; a Store title is uninstalled, not forgotten"
        );
    }
    let (recipes, _) = Recipe::load_layered_report(&paths.title_dirs());
    let recipe = recipes
        .iter()
        .find(|r| r.is_local() && r.title.product_id.eq_ignore_ascii_case(product_id))
        .ok_or_else(|| anyhow::anyhow!("there is no local game {product_id}"))?;
    let id = recipe.title.product_id.to_ascii_uppercase();
    let kept_dir = paths.install_dir(recipe)?;
    let kept_prefix = paths.prefix_dir(recipe)?;

    let file = paths.user_titles_dir().join(format!("{id}.toml"));
    let recipe_file = match std::fs::remove_file(&file) {
        Ok(()) => Some(file),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => anyhow::bail!("{}: {e}", file.display()),
    };
    crate::runtime::set_selected(paths.config_dir(), &id, None)?;
    if let Some(art) = paths.local_art_dir(&id) {
        match std::fs::remove_dir_all(&art) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => anyhow::bail!("{}: {e}", art.display()),
        }
    }
    Ok(Forgotten {
        recipe_file,
        kept_dir,
        kept_prefix,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime;

    #[test]
    fn local_ids_are_recognised_and_bounded() {
        for id in ["LOCAL-STEAM-1", "local-cyberpunk-2077"] {
            assert!(is_local_id(id), "{id}");
        }
        let long = format!("LOCAL-{}", "A".repeat(70));
        for id in ["LOCAL-", "LOCAL-a/b", long.as_str(), "9NBLGGH2JHXJ"] {
            assert!(!is_local_id(id), "{id}");
        }
    }

    #[test]
    fn a_steam_shortcut_id_is_its_app_id() {
        assert_eq!(steam_id(2_876_543_210), "LOCAL-STEAM-2876543210");
    }

    #[test]
    fn a_new_id_is_the_name_and_never_one_taken() {
        assert_eq!(
            new_id("Cyberpunk 2077", &|id| id == "LOCAL-CYBERPUNK-2077"),
            "LOCAL-CYBERPUNK-2077-2"
        );
        assert_eq!(new_id("ゲーム", &|_| false), "LOCAL-GAME");
        let long = new_id(&"word ".repeat(20), &|_| false);
        assert!(is_local_id(&long) && !long.ends_with('-'), "{long}");
    }

    #[test]
    fn forgetting_a_local_game_removes_only_what_ferestre_wrote() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::resolve(&crate::paths::Env::from_map([(
            "HOME",
            home.path().to_str().unwrap(),
        )]))
        .unwrap();
        let dir = home.path().join("drive/Game");
        let prefix = home.path().join("steam/steamapps/compatdata/3000000001");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Game.exe"), b"").unwrap();
        std::fs::create_dir_all(prefix.join("pfx")).unwrap();
        let text = format!(
            "schema = 1\n[title]\nproduct-id = \"LOCAL-T\"\nname = \"Test Game\"\n\
             slug = \"local-t\"\nkind = \"local\"\n[install]\ndir = \"{}\"\n\
             prefix = \"{}\"\n[launch]\nexecutable = \"Game.exe\"\n[status]\n\
             state = \"untested\"\nsummary = \"Added by hand\"\n",
            dir.display(),
            prefix.display()
        );
        Recipe::parse(&text)
            .unwrap()
            .save_to(&paths.user_titles_dir())
            .unwrap();
        runtime::set_selected(paths.config_dir(), "LOCAL-T", Some(&prefix)).unwrap();
        let art = paths.local_art_dir("LOCAL-T").unwrap();
        std::fs::create_dir_all(&art).unwrap();
        std::fs::write(art.join("portrait.png"), b"png").unwrap();

        let gone = forget(&paths, "LOCAL-T").unwrap();
        assert!(!paths.user_titles_dir().join("LOCAL-T.toml").exists());
        assert!(runtime::selected(paths.config_dir(), "LOCAL-T").is_none());
        assert!(!paths.local_art_dir("LOCAL-T").unwrap().exists());
        assert!(dir.join("Game.exe").is_file() && prefix.join("pfx").is_dir());
        assert_eq!(gone.kept_dir, dir);
        assert_eq!(gone.kept_prefix, prefix);
        assert_eq!(
            gone.recipe_file,
            Some(paths.user_titles_dir().join("LOCAL-T.toml"))
        );
        assert!(forget(&paths, "9NBLGGH2JHXJ").is_err());
    }
}
