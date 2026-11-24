//! Games Steam installed, played through Steam.
//!
//! Such a game is a local game whose recipe has `[launch] steam-app-id`.
//! Play asks Steam to start it (`steam steam://rungameid/<id>`), so Steam's
//! own Proton, launch options, DRM and cloud saves all apply as they do from
//! Steam's library. Its executable says only where neural rendering goes.
//!
//! Neural rendering's DLL overrides cannot ride in the environment of a game
//! Steam starts, so they go into the game's own prefix
//! (`steamapps/compatdata/<id>/pfx/user.reg`), for its program alone (see
//! [`crate::wine_reg`]). Nothing in Steam's own configuration is written. What
//! was written, and any value it replaced, is kept in
//! `steam-dll-overrides.toml` in the state directory, so turning neural
//! rendering off puts the prefix back as it was.

use crate::steam::{ArtworkFiles, Value};
use crate::wine_reg;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Steamworks Common Redistributables: installed beside games, never one.
const REDISTRIBUTABLES: u32 = 228_980;

/// `StateFlags` bit Steam sets once a game is fully installed.
const FULLY_INSTALLED: u64 = 4;

/// A game in one of Steam's libraries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    pub app_id: u32,
    pub name: String,
    /// `steamapps/common/<installdir>`.
    pub dir: PathBuf,
    /// `steamapps/compatdata/<id>` in the same library: the prefix Steam's
    /// Proton gives it. It exists once the game has been started from Steam.
    pub compatdata: PathBuf,
}

/// Every installed game of these Steam installs, each once, by app id. Steam's
/// own tools (Proton, the Linux runtimes) and redistributables are left out.
pub fn installed(roots: &[PathBuf]) -> Vec<App> {
    let mut found: BTreeMap<u32, App> = BTreeMap::new();
    for library in roots.iter().flat_map(|r| crate::steam::libraries(r)) {
        let steamapps = library.join("steamapps");
        let Ok(entries) = std::fs::read_dir(&steamapps) else {
            continue;
        };
        let mut manifests: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("appmanifest_") && n.ends_with(".acf"))
            })
            .collect();
        manifests.sort();
        for manifest in manifests {
            let Some(app) = read_manifest(&manifest, &steamapps) else {
                continue;
            };
            if app.app_id == REDISTRIBUTABLES || is_tool(&app.dir) || !app.dir.is_dir() {
                continue;
            }
            found.entry(app.app_id).or_insert(app);
        }
    }
    found.into_values().collect()
}

/// One `appmanifest_<id>.acf`, if it is a fully installed app.
fn read_manifest(path: &Path, steamapps: &Path) -> Option<App> {
    let text = std::fs::read_to_string(path).ok()?;
    let doc = crate::vdf::parse(&text).ok()?;
    let state = doc.get("AppState")?;
    let text = |key: &str| state.get(key).and_then(Value::as_str).map(str::to_owned);
    let app_id: u32 = text("appid")?.trim().parse().ok().filter(|id| *id != 0)?;
    let flags: u64 = text("StateFlags")?.trim().parse().ok()?;
    if flags & FULLY_INSTALLED == 0 {
        return None;
    }
    let install_dir = text("installdir")
        .filter(|d| !d.is_empty() && !d.contains('/') && d != "." && d != "..")?;
    let name = text("name")
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| install_dir.clone());
    Some(App {
        app_id,
        name,
        dir: steamapps.join("common").join(install_dir),
        compatdata: steamapps.join("compatdata").join(app_id.to_string()),
    })
}

/// Whether `dir` is a Steam tool (a Proton, a Linux runtime) rather than a
/// game: tools carry a `toolmanifest.vdf`.
fn is_tool(dir: &Path) -> bool {
    dir.join("toolmanifest.vdf").is_file()
}

/// Folders a game's own program is never in.
const SKIPPED_FOLDERS: &[&str] = &[
    "_commonredist",
    "__installer",
    "commonredist",
    "redist",
    "redistributables",
    "directx",
    "vcredist",
    "dotnet",
    "easyanticheat",
    "eaanticheat",
    "battleye",
    "support",
    "installers",
    "prerequisites",
];

/// Words in a program's name that say it is not the game.
const SKIPPED_PROGRAMS: &[&str] = &[
    "unins",
    "setup",
    "install",
    "redist",
    "crash",
    "report",
    "anticheat",
    "easyanticheat",
    "battleye",
    "launcher",
    "helper",
    "updater",
    "service",
    "cefprocess",
    "webhelper",
    "uploader",
    "dxsetup",
    "vc_redist",
    "prereq",
    "start_protected_game",
    "handler",
    "bugsplat",
    "overlay",
    "benchmark",
    "editor",
    "config",
    "settings",
];

const SCAN_DEPTH: usize = 4;
const SCAN_ENTRIES: usize = 20_000;

/// The program a Steam game most likely renders from, relative to its folder
/// with `/`: an Unreal `*-Win64-Shipping.exe` if there is one, else the
/// largest `.exe` that is not an installer, launcher, crash reporter or
/// anti-cheat. `None` for a game without one (a Linux build).
pub fn main_executable(dir: &Path) -> Option<String> {
    let mut found: Vec<(bool, u64, usize, String)> = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    let mut seen = 0usize;
    while let Some((folder, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > SCAN_ENTRIES {
                break;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if kind.is_dir() {
                if depth < SCAN_DEPTH && !SKIPPED_FOLDERS.contains(&name.as_str()) {
                    stack.push((entry.path(), depth + 1));
                }
                continue;
            }
            if !kind.is_file()
                || !name.ends_with(".exe")
                || SKIPPED_PROGRAMS.iter().any(|w| name.contains(w))
            {
                continue;
            }
            let Ok(inside) = entry.path().strip_prefix(dir).map(Path::to_path_buf) else {
                continue;
            };
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            found.push((
                name.ends_with("-win64-shipping.exe"),
                size,
                usize::MAX - depth,
                inside.to_string_lossy().into_owned(),
            ));
        }
    }
    found.sort();
    found.pop().map(|(_, _, _, path)| path)
}

/// Steam's cached library art for an app, from each install's
/// `appcache/librarycache`: the current `<id>/<hash>/<name>` layout and the
/// older `<id>_<name>` one.
pub fn library_artwork(roots: &[PathBuf], app_id: u32) -> ArtworkFiles {
    let mut art = ArtworkFiles::default();
    for root in roots {
        let cache = root.join("appcache/librarycache");
        let mut files: Vec<PathBuf> = Vec::new();
        let own = cache.join(app_id.to_string());
        if let Ok(entries) = std::fs::read_dir(&own) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if let Ok(inner) = std::fs::read_dir(&path) {
                        files.extend(inner.flatten().map(|e| e.path()));
                    }
                } else {
                    files.push(path);
                }
            }
        }
        files.sort();
        let named = |names: &[&str]| {
            files
                .iter()
                .find(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| names.contains(&n))
                })
                .cloned()
                .or_else(|| {
                    names
                        .iter()
                        .map(|n| cache.join(format!("{app_id}_{n}")))
                        .find(|p| p.is_file())
                })
        };
        let pick = |slot: &mut Option<PathBuf>, found: Option<PathBuf>| {
            if slot.is_none() {
                *slot = found;
            }
        };
        pick(
            &mut art.portrait,
            named(&["library_600x900.jpg", "library_capsule.jpg"]),
        );
        pick(
            &mut art.landscape,
            named(&["header.jpg", "library_header.jpg"]),
        );
        pick(&mut art.hero, named(&["library_hero.jpg"]));
        pick(&mut art.logo, named(&["logo.png"]));
        // The icon is the one picture straight in `<id>/`, named by its hash.
        pick(
            &mut art.icon,
            files
                .iter()
                .find(|p| {
                    p.parent() == Some(own.as_path())
                        && p.extension().and_then(|e| e.to_str()) == Some("jpg")
                        && !p
                            .file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with("library_") || n == "header.jpg")
                })
                .cloned(),
        );
    }
    art
}

/// The registry file of a Steam game's prefix.
pub fn user_reg(compatdata: &Path) -> PathBuf {
    compatdata.join("pfx").join("user.reg")
}

/// What Wine is told for each of neural rendering's DLLs.
pub const NATIVE_FIRST: &str = "native,builtin";

/// What was written into one game's prefix.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct Written {
    user_reg: PathBuf,
    program: String,
    /// The DLLs whose override Ferestre set.
    dlls: Vec<String>,
    /// The value a DLL had before, when it had one.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    previous: BTreeMap<String, String>,
}

const RECORD: &str = "steam-dll-overrides.toml";

fn read_record(state_dir: &Path) -> Result<BTreeMap<String, Written>> {
    let path = state_dir.join(RECORD);
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).map_err(|e| {
            anyhow!(
                "{}: {e}\nfix the file; it records the DLL overrides Ferestre wrote into Steam \
                 games' prefixes",
                path.display()
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => bail!("{}: {e}", path.display()),
    }
}

fn write_record(state_dir: &Path, record: &BTreeMap<String, Written>) -> Result<()> {
    crate::cache::write(
        &state_dir.join(RECORD),
        toml::to_string_pretty(record)?.as_bytes(),
    )
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn rewrite(path: &Path, change: impl FnOnce(String) -> String) -> Result<()> {
    let text = std::fs::read_to_string(path).with_context(|| format!("{}", path.display()))?;
    let changed = change(text.clone());
    if changed != text {
        crate::cache::write_mode(path, changed.as_bytes(), 0o644)
            .with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

/// Put back what `written` replaced in its prefix.
fn undo(written: &Written, keep: &[String]) -> Result<()> {
    if !written.user_reg.is_file() {
        // The prefix is gone (the game uninstalled): nothing to put back.
        return Ok(());
    }
    rewrite(&written.user_reg, |mut text| {
        for dll in written.dlls.iter().filter(|d| !keep.contains(d)) {
            text = wine_reg::set(
                &text,
                &written.program,
                dll,
                written.previous.get(dll).map(String::as_str),
                now(),
            );
        }
        text
    })
}

/// Make `program`'s overrides in the prefix at `user_reg` load exactly
/// `dlls` native first, for the game `id`: what an earlier call set and is no
/// longer wanted goes back to what it was. Wine must not be running in the
/// prefix.
pub fn sync_overrides(
    state_dir: &Path,
    id: &str,
    user_reg: &Path,
    program: &str,
    dlls: &[String],
) -> Result<()> {
    let key = id.to_ascii_uppercase();
    let mut record = read_record(state_dir)?;
    let mut written = record.remove(&key).unwrap_or_default();
    if !written.dlls.is_empty()
        && (written.user_reg != user_reg || !written.program.eq_ignore_ascii_case(program))
    {
        undo(&written, &[])?;
        written = Written::default();
    }
    if !written.dlls.is_empty() {
        undo(&written, dlls)?;
        written.dlls.retain(|d| dlls.contains(d));
        let dlls_now = written.dlls.clone();
        written.previous.retain(|d, _| dlls_now.contains(d));
    }
    if !dlls.is_empty() {
        wine_reg::check_program(program)?;
        if !user_reg.is_file() {
            bail!(
                "{} is not there: start the game once from Steam, so Steam makes its prefix",
                user_reg.display()
            );
        }
        written.user_reg = user_reg.to_path_buf();
        written.program = program.to_owned();
        rewrite(user_reg, |mut text| {
            for dll in dlls {
                if !written.dlls.contains(dll) {
                    if let Some(before) = wine_reg::get(&text, program, dll) {
                        written.previous.insert(dll.clone(), before);
                    }
                    written.dlls.push(dll.clone());
                }
                if wine_reg::get(&text, program, dll).as_deref() != Some(NATIVE_FIRST) {
                    text = wine_reg::set(&text, program, dll, Some(NATIVE_FIRST), now());
                }
            }
            text
        })?;
        record.insert(key, written);
    }
    write_record(state_dir, &record)
}

/// Take out every override written for the game `id` and put back what they
/// replaced. Nothing recorded, nothing done.
pub fn clear_overrides(state_dir: &Path, id: &str) -> Result<()> {
    let key = id.to_ascii_uppercase();
    let mut record = read_record(state_dir)?;
    let Some(written) = record.remove(&key) else {
        return Ok(());
    };
    undo(&written, &[])?;
    write_record(state_dir, &record)
}

/// Whether `app_id` is a non-Steam shortcut's: Steam makes those with the
/// top bit set, which no app of its own has.
pub fn is_shortcut(app_id: u32) -> bool {
    app_id & 0x8000_0000 != 0
}

/// The game id `steam://rungameid/` takes: an app's own id, or for a
/// shortcut its id in the upper half with the shortcut type below.
pub fn run_game_id(app_id: u32) -> u64 {
    if is_shortcut(app_id) {
        (u64::from(app_id) << 32) | 0x0200_0000
    } else {
        u64::from(app_id)
    }
}

/// The program and arguments that ask Steam to start an app or shortcut:
/// `steam` when it is on `PATH`, else the Flatpak's, else whatever opens
/// `steam://` links.
pub fn launch_command(app_id: u32, path: Option<&std::ffi::OsStr>) -> (String, Vec<String>) {
    let url = format!("steam://rungameid/{}", run_game_id(app_id));
    let on_path = |name: &str| {
        path.map(std::env::split_paths)
            .into_iter()
            .flatten()
            .any(|dir| dir.join(name).is_file())
    };
    if on_path("steam") {
        ("steam".into(), vec![url])
    } else if on_path("flatpak")
        && Path::new("/var/lib/flatpak/app/com.valvesoftware.Steam").is_dir()
    {
        (
            "flatpak".into(),
            vec!["run".into(), "com.valvesoftware.Steam".into(), url],
        )
    } else {
        ("xdg-open".into(), vec![url])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn manifest(steamapps: &Path, id: u32, name: &str, dir: &str, flags: u32) {
        write(
            &steamapps.join(format!("appmanifest_{id}.acf")),
            format!(
                "\"AppState\"\n{{\n\t\"appid\"\t\t\"{id}\"\n\t\"name\"\t\t\"{name}\"\n\t\
                 \"StateFlags\"\t\t\"{flags}\"\n\t\"installdir\"\t\t\"{dir}\"\n}}\n"
            )
            .as_bytes(),
        );
    }

    #[test]
    fn installed_games_are_listed_and_tools_and_unfinished_ones_are_not() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("Steam");
        let other = home.path().join("Library");
        let apps = root.join("steamapps");
        write(
            &apps.join("libraryfolders.vdf"),
            format!(
                "\"libraryfolders\"\n{{\n\"0\" {{ \"path\" \"{}\" }}\n\"1\" {{ \"path\" \"{}\" }}\n}}\n",
                root.display(),
                other.display()
            )
            .as_bytes(),
        );
        manifest(
            &apps,
            1493710,
            "Proton Experimental",
            "Proton - Experimental",
            4,
        );
        write(
            &apps.join("common/Proton - Experimental/toolmanifest.vdf"),
            b"",
        );
        manifest(
            &apps,
            228980,
            "Steamworks Common Redistributables",
            "Steamworks Shared",
            4,
        );
        write(&apps.join("common/Steamworks Shared/x.exe"), b"");
        manifest(&apps, 10, "Half-done", "Half", 1026);
        write(&apps.join("common/Half/h.exe"), b"");
        manifest(&apps, 20, "Gone", "Gone", 4);
        let lib = other.join("steamapps");
        manifest(&lib, 2807960, "Battlefield™ 6", "Battlefield 6", 6);
        write(&lib.join("common/Battlefield 6/bf6.exe"), b"");

        let found = installed(std::slice::from_ref(&root));
        assert_eq!(
            found,
            vec![App {
                app_id: 2807960,
                name: "Battlefield™ 6".into(),
                dir: lib.join("common/Battlefield 6"),
                compatdata: lib.join("compatdata/2807960"),
            }]
        );
    }

    #[test]
    fn the_main_program_is_the_largest_that_is_not_a_helper() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        write(&d.join("bf6.exe"), &[0; 30]);
        write(&d.join("SP/bf6.exe"), &[0; 20]);
        write(&d.join("EAAntiCheat.Installer.exe"), &[0; 90]);
        write(&d.join("EAAntiCheat/x.exe"), &[0; 90]);
        write(&d.join("__Installer/Touchup.exe"), &[0; 90]);
        write(&d.join("GameLauncher.exe"), &[0; 90]);
        assert_eq!(main_executable(d).as_deref(), Some("bf6.exe"));

        // Unreal's shipping build wins over a bigger stub.
        write(
            &d.join("Game/Binaries/Win64/Game-Win64-Shipping.exe"),
            &[0; 5],
        );
        assert_eq!(
            main_executable(d).as_deref(),
            Some("Game/Binaries/Win64/Game-Win64-Shipping.exe")
        );
        let linux = tempfile::tempdir().unwrap();
        write(&linux.path().join("game.x86_64"), &[0; 5]);
        assert_eq!(main_executable(linux.path()), None);
    }

    #[test]
    fn library_art_is_found_in_both_cache_layouts() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().to_path_buf();
        let cache = root.join("appcache/librarycache");
        write(&cache.join("7/abc.jpg"), b"i");
        write(&cache.join("7/h1/library_capsule.jpg"), b"p");
        write(&cache.join("7/h2/library_hero.jpg"), b"h");
        write(&cache.join("7/h3/logo.png"), b"l");
        write(&cache.join("7/h4/library_header.jpg"), b"w");
        write(&cache.join("8_library_600x900.jpg"), b"p");
        let art = library_artwork(std::slice::from_ref(&root), 7);
        assert_eq!(art.portrait, Some(cache.join("7/h1/library_capsule.jpg")));
        assert_eq!(art.hero, Some(cache.join("7/h2/library_hero.jpg")));
        assert_eq!(art.logo, Some(cache.join("7/h3/logo.png")));
        assert_eq!(art.landscape, Some(cache.join("7/h4/library_header.jpg")));
        assert_eq!(art.icon, Some(cache.join("7/abc.jpg")));
        let old = library_artwork(&[root], 8);
        assert_eq!(old.portrait, Some(cache.join("8_library_600x900.jpg")));
        assert_eq!(old.hero, None);
    }

    const REG: &str = "WINE REGISTRY Version 2\n\n[Software\\\\Wine\\\\AppDefaults\\\\bf6.exe\\\\DllOverrides] 1\n\"winmm\"=\"builtin\"\n";

    #[test]
    fn overrides_go_into_the_prefix_and_come_out_putting_back_what_was_there() {
        let state = tempfile::tempdir().unwrap();
        let prefix = tempfile::tempdir().unwrap();
        let reg = prefix.path().join("pfx/user.reg");
        write(&reg, REG.as_bytes());
        let s = state.path();
        let id = "LOCAL-STEAMAPP-2807960";

        sync_overrides(s, id, &reg, "bf6.exe", &["dxgi".into()]).unwrap();
        let text = std::fs::read_to_string(&reg).unwrap();
        assert_eq!(
            wine_reg::get(&text, "bf6.exe", "dxgi").as_deref(),
            Some(NATIVE_FIRST)
        );
        assert_eq!(
            wine_reg::get(&text, "bf6.exe", "winmm").as_deref(),
            Some("builtin")
        );

        // A proxy change: dxgi comes out, winmm's own value is kept aside.
        sync_overrides(s, id, &reg, "bf6.exe", &["winmm".into()]).unwrap();
        let text = std::fs::read_to_string(&reg).unwrap();
        assert_eq!(wine_reg::get(&text, "bf6.exe", "dxgi"), None);
        assert_eq!(
            wine_reg::get(&text, "bf6.exe", "winmm").as_deref(),
            Some(NATIVE_FIRST)
        );
        // Again changes nothing.
        sync_overrides(s, id, &reg, "bf6.exe", &["winmm".into()]).unwrap();
        assert_eq!(std::fs::read_to_string(&reg).unwrap(), text);

        clear_overrides(s, id).unwrap();
        assert_eq!(std::fs::read_to_string(&reg).unwrap(), REG);
        assert!(read_record(s).unwrap().is_empty());
        clear_overrides(s, id).unwrap();
    }

    #[test]
    fn a_prefix_steam_has_not_made_yet_is_an_error_saying_so() {
        let state = tempfile::tempdir().unwrap();
        let reg = state.path().join("compatdata/1/pfx/user.reg");
        let err = sync_overrides(
            state.path(),
            "LOCAL-STEAMAPP-1",
            &reg,
            "g.exe",
            &["dxgi".into()],
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("start the game once from Steam"), "{err}");
        // Nothing wanted, nothing needed.
        sync_overrides(state.path(), "LOCAL-STEAMAPP-1", &reg, "g.exe", &[]).unwrap();
    }

    #[test]
    fn steam_is_asked_through_its_own_program_when_there_is_one() {
        let bin = tempfile::tempdir().unwrap();
        write(&bin.path().join("steam"), b"");
        let (program, args) = launch_command(42, Some(bin.path().as_os_str()));
        assert_eq!(
            (program.as_str(), args),
            ("steam", vec!["steam://rungameid/42".to_string()])
        );
        let empty = tempfile::tempdir().unwrap();
        let (program, _) = launch_command(42, Some(empty.path().as_os_str()));
        // A shortcut's id goes in the upper half, with the shortcut type.
        let (_, args) = launch_command(0x9f3a_1c2e, Some(bin.path().as_os_str()));
        assert_eq!(
            args,
            vec![format!("steam://rungameid/{}", 0x9f3a_1c2e_0200_0000u64)]
        );
        assert!(!is_shortcut(2_807_960) && run_game_id(2_807_960) == 2_807_960);
        assert!(program == "xdg-open" || program == "flatpak");
    }
}
