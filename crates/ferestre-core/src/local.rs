//! Games Ferestre did not install: a Windows program somewhere on disk, added
//! by hand or imported from a Steam non-Steam shortcut.
//!
//! Such a game is a user recipe with `[title] kind = "local"` and an id that
//! starts with [`ID_PREFIX`]. The prefix has a dash, which a Store product id
//! (12 ASCII letters and digits) never has, so the two can never be confused.

use crate::paths::Paths;
use crate::recipe::Recipe;
use std::path::{Path, PathBuf};

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

    // The loader reads every `*.toml` whatever its name, so the file to
    // remove is the one that holds this id, not `<ID>.toml`.
    let files = user_recipe_files(paths, &id);
    if files.is_empty() {
        anyhow::bail!(
            "{id}'s recipe is not in {}; Ferestre only removes recipes kept there",
            paths.user_titles_dir().display()
        );
    }
    for file in &files {
        match std::fs::remove_file(file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => anyhow::bail!("{}: {e}", file.display()),
        }
    }
    let recipe_file = files.into_iter().next();
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

/// Every file in the user's titles directory whose recipe is `id`'s.
fn user_recipe_files(paths: &Paths, id: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(paths.user_titles_dir()) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("toml"))
        .filter(|p| {
            std::fs::read_to_string(p)
                .ok()
                .and_then(|text| Recipe::parse(&text).ok())
                .is_some_and(|r| r.title.product_id.eq_ignore_ascii_case(id))
        })
        .collect();
    files.sort();
    files
}

/// The picture slots a local game's art fills, as `ArtworkFiles` names them.
const ART_SLOTS: [&str; 5] = ["portrait", "landscape", "hero", "logo", "icon"];
const ART_EXTENSIONS: [&str; 3] = ["png", "jpg", "jpeg"];

fn art_slot<'a>(files: &'a mut crate::steam::ArtworkFiles, slot: &str) -> &'a mut Option<PathBuf> {
    match slot {
        "portrait" => &mut files.portrait,
        "landscape" => &mut files.landscape,
        "hero" => &mut files.hero,
        "logo" => &mut files.logo,
        _ => &mut files.icon,
    }
}

/// The art Ferestre keeps for a local game: whichever of
/// `portrait|landscape|hero|logo|icon.{png,jpg,jpeg}` is in its art folder.
pub fn artwork(paths: &Paths, product_id: &str) -> crate::steam::ArtworkFiles {
    let mut files = crate::steam::ArtworkFiles::default();
    let Some(dir) = paths.local_art_dir(product_id) else {
        return files;
    };
    for slot in ART_SLOTS {
        *art_slot(&mut files, slot) = ART_EXTENSIONS
            .iter()
            .map(|ext| dir.join(format!("{slot}.{ext}")))
            .find(|p| p.is_file());
    }
    files
}

/// Copy a local game's art into its art folder, each picture as
/// `<slot>.<its extension>`. A slot that already has a picture keeps it, so
/// art the person chose is never replaced by an import.
pub fn save_artwork(
    paths: &Paths,
    product_id: &str,
    files: &crate::steam::ArtworkFiles,
) -> anyhow::Result<()> {
    let dir = paths
        .local_art_dir(product_id)
        .ok_or_else(|| anyhow::anyhow!("{product_id} is not a local game's id"))?;
    let mut have = artwork(paths, product_id);
    let mut wanted = files.clone();
    for slot in ART_SLOTS {
        if art_slot(&mut have, slot).is_some() {
            continue;
        }
        let Some(source) = art_slot(&mut wanted, slot).clone() else {
            continue;
        };
        let Some(ext) = source
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .filter(|e| ART_EXTENSIONS.contains(&e.as_str()))
        else {
            continue;
        };
        let bytes =
            std::fs::read(&source).map_err(|e| anyhow::anyhow!("{}: {e}", source.display()))?;
        crate::cache::write(&dir.join(format!("{slot}.{ext}")), &bytes)?;
    }
    Ok(())
}

/// Why `dir` is too broad to be one local game's folder or prefix: it is, or
/// holds, the home directory, the games directory or another directory the
/// launcher never removes. `None` when it is narrow enough.
///
/// Such a folder is not protected by the removal guard -- it would shield
/// every Store title -- and adding or importing a game should refuse it.
pub fn too_broad(paths: &Paths, dir: &Path) -> Option<String> {
    let resolve = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let dir = resolve(dir);
    if dir.components().count() < 3 {
        return Some(format!("{} is too close to the root", dir.display()));
    }
    crate::install::protected_dirs(paths)
        .into_iter()
        .find(|(_, protected)| resolve(protected).starts_with(&dir))
        .map(|(what, protected)| {
            format!(
                "{} is or holds {what} ({})",
                dir.display(),
                protected.display()
            )
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

    fn test_paths(home: &Path) -> Paths {
        Paths::resolve(&crate::paths::Env::from_map([(
            "HOME",
            home.to_str().unwrap(),
        )]))
        .unwrap()
    }

    #[test]
    fn forgetting_removes_the_recipe_whatever_its_file_is_called() {
        let home = tempfile::tempdir().unwrap();
        let paths = test_paths(home.path());
        let dir = home.path().join("drive/Witcher");
        std::fs::create_dir_all(&dir).unwrap();
        let text = format!(
            "schema = 1\n[title]\nproduct-id = \"LOCAL-W\"\nname = \"W\"\n\
             slug = \"local-w\"\nkind = \"local\"\n[install]\ndir = \"{}\"\n\
             [launch]\nexecutable = \"W.exe\"\n[status]\nstate = \"untested\"\n\
             summary = \"Hand written\"\n",
            dir.display()
        );
        std::fs::create_dir_all(paths.user_titles_dir()).unwrap();
        let file = paths.user_titles_dir().join("witcher.toml");
        std::fs::write(&file, text).unwrap();

        let gone = forget(&paths, "LOCAL-W").unwrap();
        assert_eq!(gone.recipe_file, Some(file.clone()));
        assert!(!file.exists() && dir.is_dir());
        let (left, _) = Recipe::load_layered_report(&paths.title_dirs());
        assert!(left.iter().all(|r| r.title.product_id != "LOCAL-W"));
        // Nothing left to forget is an error, not a quiet success.
        assert!(forget(&paths, "LOCAL-W").is_err());
    }

    #[test]
    fn a_folder_holding_home_or_the_games_dir_is_too_broad() {
        let home = tempfile::tempdir().unwrap();
        let paths = test_paths(home.path());
        for broad in [
            PathBuf::from("/"),
            PathBuf::from("/mnt"),
            home.path().to_path_buf(),
            paths.games_dir().to_path_buf(),
            home.path().parent().unwrap().to_path_buf(),
        ] {
            assert!(too_broad(&paths, &broad).is_some(), "{}", broad.display());
        }
        assert_eq!(too_broad(&paths, &home.path().join("drive/Game")), None);
    }

    #[test]
    fn artwork_is_copied_once_and_never_over_a_slot() {
        let home = tempfile::tempdir().unwrap();
        let paths = test_paths(home.path());
        let src = home.path().join("grid");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("1p.PNG"), b"portrait").unwrap();
        std::fs::write(src.join("1_hero.jpg"), b"hero").unwrap();
        let files = crate::steam::ArtworkFiles {
            portrait: Some(src.join("1p.PNG")),
            hero: Some(src.join("1_hero.jpg")),
            ..Default::default()
        };
        assert!(artwork(&paths, "LOCAL-STEAM-1").portrait.is_none());
        save_artwork(&paths, "LOCAL-STEAM-1", &files).unwrap();
        let art = paths.local_art_dir("LOCAL-STEAM-1").unwrap();
        let found = artwork(&paths, "LOCAL-STEAM-1");
        assert_eq!(found.portrait, Some(art.join("portrait.png")));
        assert_eq!(found.hero, Some(art.join("hero.jpg")));
        assert!(found.landscape.is_none() && found.logo.is_none() && found.icon.is_none());

        // A second copy, from another picture, keeps the first.
        std::fs::write(src.join("other.jpeg"), b"other").unwrap();
        let other = crate::steam::ArtworkFiles {
            portrait: Some(src.join("other.jpeg")),
            ..Default::default()
        };
        save_artwork(&paths, "LOCAL-STEAM-1", &other).unwrap();
        assert_eq!(
            std::fs::read(art.join("portrait.png")).unwrap(),
            b"portrait"
        );
        assert!(!art.join("portrait.jpeg").exists());
        assert!(save_artwork(&paths, "9NBLGGH2JHXJ", &files).is_err());
    }
}
