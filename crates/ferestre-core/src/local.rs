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

/// The id of a game Steam installed, with this app id.
pub fn steam_app_id(app_id: u32) -> String {
    format!("{ID_PREFIX}STEAMAPP-{app_id}")
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

/// Folder names that say where a game keeps its binaries, not what it is.
const BINARY_FOLDERS: [&str; 10] = [
    "bin", "binaries", "win64", "win32", "x64", "x86", "x64_dx12", "retail", "shipping", "game",
];

/// A game's name from its executable: the first folder above it that is not
/// a binaries folder (`bin`, `x64`, `Binaries`...), else the program's own
/// name.
pub fn derive_name(exe: &Path) -> String {
    exe.ancestors()
        .skip(1)
        .map_while(|dir| dir.file_name().and_then(|n| n.to_str()))
        .find(|name| {
            !BINARY_FOLDERS
                .iter()
                .any(|known| name.eq_ignore_ascii_case(known))
        })
        .map(str::to_owned)
        .or_else(|| exe.file_stem().and_then(|s| s.to_str()).map(str::to_owned))
        .unwrap_or_else(|| "Game".to_owned())
}

/// A game to add by hand.
#[derive(Debug, Clone, Default)]
pub struct AddRequest {
    /// The program to start: absolute, existing, `.exe` or `.bat`.
    pub exe: PathBuf,
    /// Defaults to [`derive_name`].
    pub name: Option<String>,
    /// The game's folder; defaults to the program's folder.
    pub dir: Option<PathBuf>,
    /// Its Wine prefix: absolute as it is, relative under the games
    /// directory, `None` for a managed one.
    pub prefix: Option<PathBuf>,
    /// A runtime's path, or the name Steam gives one of its tools.
    pub runtime: Option<String>,
}

/// Add a local game: write its user recipe and, when one is named, its
/// runtime choice. The game's files are only looked at.
pub fn add(paths: &Paths, request: &AddRequest) -> anyhow::Result<Recipe> {
    let exe = &request.exe;
    let lower = exe.to_string_lossy().to_ascii_lowercase();
    if !exe.is_absolute() {
        anyhow::bail!("{}: give the program's full path", exe.display());
    }
    if !(lower.ends_with(".exe") || lower.ends_with(".bat")) {
        anyhow::bail!("{}: not a Windows program (.exe or .bat)", exe.display());
    }
    if !exe.is_file() {
        anyhow::bail!("{}: no such file", exe.display());
    }
    let dir = match &request.dir {
        Some(dir) => dir.clone(),
        None => exe
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| anyhow::anyhow!("{}: has no folder", exe.display()))?,
    };
    if !dir.is_absolute() || !dir.is_dir() {
        anyhow::bail!(
            "{}: the game's folder must be an existing absolute folder",
            dir.display()
        );
    }
    if let Some(why) = too_broad(paths, &dir) {
        anyhow::bail!("{why}; choose the game's own folder");
    }
    if let Some(why) = request
        .prefix
        .as_deref()
        .filter(|p| p.is_absolute())
        .and_then(|p| too_broad(paths, p))
    {
        anyhow::bail!("{why}; choose a folder for this game's prefix alone");
    }
    let name = request
        .name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| derive_name(exe));

    let (recipes, _) = Recipe::load_layered_report(&paths.title_dirs());
    let id = new_id(&name, &|id| {
        recipes
            .iter()
            .any(|r| r.title.product_id.eq_ignore_ascii_case(id))
    });
    let executable = match exe.strip_prefix(&dir) {
        Ok(inside) => inside.display().to_string(),
        Err(_) => exe.display().to_string(),
    };
    let mut recipe = Recipe::blank(&id, &name);
    recipe.title.kind = crate::recipe::TitleKind::Local;
    recipe.title.slug = id.to_ascii_lowercase();
    recipe.install.dir = Some(dir.display().to_string());
    recipe.install.prefix = request.prefix.as_ref().map(|p| p.display().to_string());
    recipe.launch.executable = executable;
    recipe.status.summary = "Added by hand".into();
    let recipe = Recipe::parse(&recipe.to_toml()?)?;

    let chosen = match &request.runtime {
        None => None,
        Some(wanted) => {
            let available = crate::runtime::for_recipe(paths, &recipe);
            let tools = crate::runtime::steam_tools(paths);
            let path = match crate::runtime::steam_tool(&tools, wanted) {
                Some(tool) if !wanted.contains('/') => tool.dir.clone(),
                _ => PathBuf::from(wanted),
            };
            let resolve = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
            let found = available
                .iter()
                .find(|rt| rt.path == path || resolve(&rt.path) == resolve(&path));
            match found {
                Some(rt) => Some(rt.path.clone()),
                None => {
                    let mut names: Vec<String> = available
                        .iter()
                        .map(|rt| rt.path.display().to_string())
                        .collect();
                    names.extend(
                        tools
                            .iter()
                            .filter(|t| available.iter().any(|rt| rt.path == t.dir))
                            .map(|t| t.name.clone()),
                    );
                    anyhow::bail!(
                        "{wanted} is not an installed runtime; available: {}",
                        if names.is_empty() {
                            "none".to_owned()
                        } else {
                            names.join(", ")
                        }
                    );
                }
            }
        }
    };

    if recipe.save_new_to(&paths.user_titles_dir())?.is_none() {
        anyhow::bail!("{id} was added at the same moment by something else; try again");
    }
    if let Some(path) = chosen {
        crate::runtime::set_selected(paths.config_dir(), &id, Some(&path))?;
    }
    Ok(recipe)
}

/// Where a local game's program is: its folder joined with its executable,
/// `\` read as `/` (an absolute executable is used as it is). `None` for a
/// Store title.
pub fn executable_path(recipe: &Recipe) -> Option<PathBuf> {
    if !recipe.is_local() {
        return None;
    }
    let dir = recipe.install.dir.as_deref()?;
    Some(Path::new(dir).join(recipe.launch.executable.replace('\\', "/")))
}

/// Whether a local game's program is on disk. A game on a drive that is not
/// mounted is not.
pub fn is_present(recipe: &Recipe) -> bool {
    executable_path(recipe).is_some_and(|p| p.is_file())
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
/// launcher never removes, or a Steam install or one of its libraries.
/// `None` when it is narrow enough.
///
/// Adding or importing a game refuses such a folder.
pub fn too_broad(paths: &Paths, dir: &Path) -> Option<String> {
    if let Some(why) = shields_store_titles(paths, dir) {
        return Some(why);
    }
    // A shortcut started in a Steam library's root is that library's, not a
    // game's: every Steam game and prefix in it would be "its" files.
    let resolve = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let resolved = resolve(dir);
    paths
        .steam_roots()
        .iter()
        .flat_map(|root| crate::steam::libraries(root))
        .find(|library| resolve(library).starts_with(&resolved))
        .map(|library| {
            format!(
                "{} is or holds a Steam library ({})",
                resolved.display(),
                library.display()
            )
        })
}

/// Why `dir` is too broad for the removal guard to protect as one local
/// game's: it is, or holds, the home directory, the games directory or
/// another directory the launcher never removes. Protecting it would shield
/// every Store title. `None` when it is narrow enough.
pub(crate) fn shields_store_titles(paths: &Paths, dir: &Path) -> Option<String> {
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
    fn a_steam_install_or_library_is_too_broad_but_a_game_in_it_is_not() {
        let home = tempfile::tempdir().unwrap();
        let steam = home.path().join(".local/share/Steam");
        let library = home.path().join("drive/SteamLibrary");
        std::fs::create_dir_all(steam.join("steamapps")).unwrap();
        std::fs::create_dir_all(library.join("steamapps/common/Game")).unwrap();
        std::fs::write(
            steam.join("steamapps/libraryfolders.vdf"),
            format!(
                "\"libraryfolders\" {{ \"0\" {{ \"path\" \"{}\" }} \"1\" {{ \"path\" \"{}\" }} }}",
                steam.display(),
                library.display()
            ),
        )
        .unwrap();
        let paths = test_paths(home.path());
        for broad in [&steam, &library, &home.path().join("drive")] {
            assert!(too_broad(&paths, broad).is_some(), "{}", broad.display());
        }
        assert_eq!(
            too_broad(&paths, &library.join("steamapps/common/Game")),
            None
        );
        // The removal guard still protects a library a recipe names.
        assert_eq!(shields_store_titles(&paths, &library), None);
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

    #[test]
    fn names_come_from_the_games_folder_not_its_binaries_folder() {
        assert_eq!(
            derive_name(Path::new("/g/Cyberpunk 2077/bin/x64/Cyberpunk2077.exe")),
            "Cyberpunk 2077"
        );
        assert_eq!(
            derive_name(Path::new("/g/Witcher 3/bin/x64_dx12/witcher3.exe")),
            "Witcher 3"
        );
        assert_eq!(
            derive_name(Path::new("/g/Gears/Game/Binaries/Win64/Shipping/G.exe")),
            "Gears"
        );
        assert_eq!(derive_name(Path::new("/x64/Game.exe")), "Game");
    }

    fn local_recipe_at(dir: &Path, executable: &str) -> Recipe {
        Recipe::parse(&format!(
            "schema = 1\n[title]\nproduct-id = \"LOCAL-T\"\nname = \"T\"\n\
             slug = \"local-t\"\nkind = \"local\"\n[install]\ndir = \"{}\"\n\
             [launch]\nexecutable = '{executable}'\n[status]\nstate = \"untested\"\n\
             summary = \"Added by hand\"\n",
            dir.display()
        ))
        .unwrap()
    }

    #[test]
    fn a_backslash_executable_is_found_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("D");
        std::fs::create_dir_all(dir.join("bin/x64")).unwrap();
        std::fs::write(dir.join("bin/x64/Game.exe"), b"").unwrap();
        let recipe = local_recipe_at(&dir, r"bin\x64\Game.exe");
        assert_eq!(
            executable_path(&recipe).unwrap(),
            dir.join("bin/x64/Game.exe")
        );
        assert!(is_present(&recipe));
        let missing = local_recipe_at(&dir, "bin/x64/Missing.exe");
        assert!(!is_present(&missing));
        let elsewhere = tmp.path().join("Other/Launcher.exe");
        std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        std::fs::write(&elsewhere, b"").unwrap();
        let absolute = local_recipe_at(&dir, elsewhere.to_str().unwrap());
        assert_eq!(executable_path(&absolute), Some(elsewhere));
        let xbox = Recipe::blank("9NBLGGH2JHXJ", "Xbox Game");
        assert_eq!(executable_path(&xbox), None);
        assert!(!is_present(&xbox));
    }

    #[test]
    fn adding_twice_gives_two_ids_and_an_unknown_runtime_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let paths = test_paths(home.path());
        let exe = home.path().join("Games/RDR2/RDR2.exe");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"").unwrap();
        let request = |runtime: Option<&str>| AddRequest {
            exe: exe.clone(),
            name: None,
            dir: None,
            prefix: None,
            runtime: runtime.map(String::from),
        };
        let first = add(&paths, &request(None)).unwrap();
        assert_eq!(
            (first.title.product_id.as_str(), first.title.name.as_str()),
            ("LOCAL-RDR2", "RDR2")
        );
        assert_eq!(first.install.dir.as_deref(), exe.parent().unwrap().to_str());
        assert_eq!(first.launch.executable, "RDR2.exe");
        assert_eq!(first.status.summary, "Added by hand");
        assert!(paths.user_titles_dir().join("LOCAL-RDR2.toml").is_file());
        let second = add(&paths, &request(None)).unwrap();
        assert_eq!(second.title.product_id, "LOCAL-RDR2-2");

        let e = add(&paths, &request(Some("GE-Proton99"))).unwrap_err();
        assert!(format!("{e:#}").contains("GE-Proton99"), "{e:#}");
        assert!(!paths.user_titles_dir().join("LOCAL-RDR2-3.toml").exists());
        for bad in ["relative.exe", "/nonexistent/ferestre/Game.exe"] {
            let request = AddRequest {
                exe: bad.into(),
                name: None,
                dir: None,
                prefix: None,
                runtime: None,
            };
            assert!(add(&paths, &request).is_err(), "{bad}");
        }
        let not_windows = home.path().join("Games/RDR2/run.sh");
        std::fs::write(&not_windows, b"").unwrap();
        let request = AddRequest {
            exe: not_windows,
            name: None,
            dir: None,
            prefix: None,
            runtime: None,
        };
        assert!(add(&paths, &request).is_err());
    }

    #[test]
    fn a_steam_proton_is_chosen_by_name_with_a_named_prefix() {
        let home = tempfile::tempdir().unwrap();
        let paths = test_paths(home.path());
        let ge = home
            .path()
            .join(".local/share/Steam/compatibilitytools.d/GE-Proton10-25");
        std::fs::create_dir_all(&ge).unwrap();
        std::fs::write(ge.join("proton"), b"#!/usr/bin/env python3\n").unwrap();
        let exe = home.path().join("drive/Witcher 3/bin/x64/witcher3.exe");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"").unwrap();
        let game = home.path().join("drive/Witcher 3");
        let recipe = add(
            &paths,
            &AddRequest {
                exe,
                name: Some("The Witcher 3".into()),
                dir: Some(game.clone()),
                prefix: Some("witcher-prefix".into()),
                runtime: Some("GE-Proton10-25".into()),
            },
        )
        .unwrap();
        assert_eq!(recipe.title.product_id, "LOCAL-THE-WITCHER-3");
        assert_eq!(recipe.install.dir.as_deref(), game.to_str());
        assert_eq!(recipe.launch.executable, "bin/x64/witcher3.exe");
        assert_eq!(recipe.install.prefix.as_deref(), Some("witcher-prefix"));
        assert_eq!(
            runtime::selected(paths.config_dir(), "LOCAL-THE-WITCHER-3"),
            Some(ge)
        );
    }
}
