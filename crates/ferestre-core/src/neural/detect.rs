//! Which DLSSNR-AMD route suits a game, from its files.
//!
//! The executable's bitness and graphics API come from its imports, plus
//! those of the DLLs beside it that it imports (an engine DLL such as
//! `UnityPlayer.dll` is where the D3D import often is). Whether the game
//! ships an upscaler OptiScaler can take over comes from file names under the
//! game folder. Nothing is guessed silently: an executable that cannot be read
//! gives no route, except in an installed Xbox package, whose main executable
//! is kept encrypted on disk and whose PC titles are all x86-64 D3D12, where
//! that is assumed and said so.

use super::{pe, Route};
use std::path::{Path, PathBuf};

/// The graphics API a game draws with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Api {
    Dx10To12,
    Vulkan,
    Dx9,
    OpenGl,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    /// 32 or 64; 0 when the executable could not be read and nothing was
    /// assumed.
    pub bits: u8,
    pub api: Api,
    /// Upscaler files found under the game folder, lowercase, sorted.
    pub upscalers: Vec<String>,
    /// Whether `bits` and `api` are assumed rather than read (an Xbox
    /// package's encrypted executable).
    pub assumed: bool,
    pub route: Option<Route>,
}

/// Upscaler files, matched by exact lowercase name.
pub const UPSCALER_FILES: &[&str] = &[
    "nvngx_dlss.dll",
    "sl.interposer.dll",
    "sl.dlss.dll",
    "libxess.dll",
    "libxess_dx11.dll",
    "amd_fidelityfx_dx12.dll",
    "amd_fidelityfx_vk.dll",
];

/// Upscaler DLLs matched by the start of their lowercase name
/// (`ffx_fsr2_api_x64.dll`, `ffx_fsr2_api_dx12_x64.dll`, …).
pub const UPSCALER_PREFIXES: &[&str] = &["ffx_fsr2_api", "ffx_fsr3", "amd_fidelityfx_upscaler"];

const MAX_DEPTH: usize = 8;
const MAX_ENTRIES: usize = 50_000;

/// The route for a game: OptiScaler when it ships an upscaler and is 64-bit,
/// else the ReShade route for its API; none for OpenGL or an unknown API.
pub fn route_for(bits: u8, api: Api, has_upscaler: bool) -> Option<Route> {
    if has_upscaler && bits == 64 {
        return Some(Route::Optiscaler);
    }
    match api {
        Api::Dx10To12 => Some(Route::ReshadeFeeder),
        Api::Vulkan => Some(Route::ReshadeFeederVulkan),
        Api::Dx9 => Some(Route::ReshadeFeederDx9),
        Api::OpenGl | Api::Unknown => None,
    }
}

fn api_of(imports: &[String]) -> Api {
    let has = |names: &[&str]| {
        imports.iter().any(|import| {
            let stem = import.strip_suffix(".dll").unwrap_or(import);
            names.contains(&stem)
        })
    };
    if has(&["d3d12", "d3d11", "d3d10", "d3d10_1", "dxgi"]) {
        Api::Dx10To12
    } else if has(&["d3d9"]) {
        Api::Dx9
    } else if has(&["vulkan-1"]) {
        Api::Vulkan
    } else if has(&["opengl32"]) {
        Api::OpenGl
    } else {
        Api::Unknown
    }
}

fn is_upscaler(lowercase: &str) -> bool {
    UPSCALER_FILES.contains(&lowercase)
        || (lowercase.ends_with(".dll")
            && UPSCALER_PREFIXES
                .iter()
                .any(|prefix| lowercase.starts_with(prefix)))
}

/// Upscaler files under `game_dir`: at most `MAX_DEPTH` folders down and
/// `MAX_ENTRIES` entries looked at, symlinked folders not followed.
fn upscalers(game_dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut seen = 0usize;
    let mut pending: Vec<(PathBuf, usize)> = vec![(game_dir.to_path_buf(), 0)];
    while let Some((dir, depth)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > MAX_ENTRIES {
                found.sort();
                found.dedup();
                return found;
            }
            // Not followed: file_type() describes a link as a link.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if depth < MAX_DEPTH {
                    pending.push((entry.path(), depth + 1));
                }
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if is_upscaler(&name) {
                found.push(name);
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

/// The file in `dir` whose name is `lowercase` ignoring case: Windows
/// imports name DLLs in any case.
fn beside(names: &[(String, PathBuf)], lowercase: &str) -> Option<PathBuf> {
    names
        .iter()
        .find(|(name, _)| name == lowercase)
        .map(|(_, path)| path.clone())
}

/// The route for the game in `game_dir` whose executable is `exe` (relative
/// to `game_dir`, or absolute).
pub fn detect(game_dir: &Path, exe: &Path) -> Detected {
    let exe = game_dir.join(exe);
    let upscalers = upscalers(game_dir);
    let has_upscaler = !upscalers.is_empty();
    match pe::read(&exe) {
        Ok(info) => {
            let mut imports = info.imports.clone();
            let dir = exe.parent().unwrap_or(game_dir);
            let files: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
                .map(|entries| {
                    entries
                        .flatten()
                        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                        .map(|e| {
                            (
                                e.file_name().to_string_lossy().to_ascii_lowercase(),
                                e.path(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            // One level: the DLLs beside the executable that it imports.
            for import in &info.imports {
                if let Some(path) = beside(&files, import) {
                    if let Ok(engine) = pe::read(&path) {
                        imports.extend(engine.imports);
                    }
                }
            }
            let api = api_of(&imports);
            Detected {
                bits: info.bits,
                api,
                route: route_for(info.bits, api, has_upscaler),
                upscalers,
                assumed: false,
            }
        }
        Err(_) if crate::install::looks_installed(game_dir) => Detected {
            bits: 64,
            api: Api::Dx10To12,
            route: route_for(64, Api::Dx10To12, has_upscaler),
            upscalers,
            assumed: true,
        },
        Err(_) => Detected {
            bits: 0,
            api: Api::Unknown,
            route: None,
            upscalers,
            assumed: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::neural::pe::tiny_pe;

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn routes_follow_the_spec_table() {
        assert_eq!(route_for(64, Api::Dx10To12, true), Some(Route::Optiscaler));
        assert_eq!(
            route_for(32, Api::Dx10To12, true),
            Some(Route::ReshadeFeeder)
        );
        assert_eq!(route_for(64, Api::Vulkan, true), Some(Route::Optiscaler));
        assert_eq!(
            route_for(64, Api::Vulkan, false),
            Some(Route::ReshadeFeederVulkan)
        );
        assert_eq!(
            route_for(32, Api::Dx9, false),
            Some(Route::ReshadeFeederDx9)
        );
        assert_eq!(route_for(64, Api::OpenGl, false), None);
        assert_eq!(route_for(64, Api::Unknown, false), None);
    }

    #[test]
    fn imports_name_the_api_in_order_of_preference() {
        let api = |names: &[&str]| api_of(&names.iter().map(|n| n.to_string()).collect::<Vec<_>>());
        assert_eq!(
            api(&["kernel32.dll", "dxgi.dll", "d3d9.dll"]),
            Api::Dx10To12
        );
        assert_eq!(api(&["d3d9.dll", "vulkan-1.dll"]), Api::Dx9);
        assert_eq!(api(&["vulkan-1.dll", "opengl32.dll"]), Api::Vulkan);
        assert_eq!(api(&["opengl32.dll"]), Api::OpenGl);
        assert_eq!(api(&["kernel32.dll", "d3d12core.dll"]), Api::Unknown);
    }

    #[test]
    fn an_engine_dll_beside_the_exe_names_the_api() {
        let game = tempfile::tempdir().unwrap();
        write(
            &game.path().join("Game.exe"),
            &tiny_pe(0x8664, 0x20b, &["KERNEL32.dll", "UnityPlayer.dll"]),
        );
        write(
            &game.path().join("UnityPlayer.dll"),
            &tiny_pe(0x8664, 0x20b, &["d3d11.dll"]),
        );
        let found = detect(game.path(), Path::new("Game.exe"));
        assert_eq!(found.bits, 64);
        assert_eq!(found.api, Api::Dx10To12);
        assert_eq!(found.route, Some(Route::ReshadeFeeder));
        assert!(!found.assumed);

        // An absolute executable is honoured too.
        let found = detect(game.path(), &game.path().join("Game.exe"));
        assert_eq!(found.api, Api::Dx10To12);
    }

    #[test]
    fn an_unreal_upscaler_deep_in_the_tree_is_found() {
        let game = tempfile::tempdir().unwrap();
        write(
            &game
                .path()
                .join("Game/Binaries/Win64/Game-Win64-Shipping.exe"),
            &tiny_pe(0x8664, 0x20b, &["d3d12.dll"]),
        );
        write(
            &game.path().join(
                "Engine/Plugins/Runtime/Nvidia/DLSS/Binaries/ThirdParty/Win64/nvngx_dlss.dll",
            ),
            b"dlss",
        );
        let found = detect(
            game.path(),
            Path::new("Game/Binaries/Win64/Game-Win64-Shipping.exe"),
        );
        assert_eq!(found.upscalers, vec!["nvngx_dlss.dll"]);
        assert_eq!(found.route, Some(Route::Optiscaler));
    }

    #[test]
    fn upscaler_names_match_ignoring_case_and_by_prefix() {
        let game = tempfile::tempdir().unwrap();
        write(&game.path().join("bin/FFX_FSR2_API_X64.DLL"), b"x");
        write(&game.path().join("bin/libxess.dll"), b"x");
        write(&game.path().join("bin/ffx_fsr2_api_x64.pdb"), b"x");
        write(
            &game.path().join("a/b/c/d/e/f/g/h/i/nvngx_dlss.dll"),
            b"too deep",
        );
        assert_eq!(
            upscalers(game.path()),
            vec!["ffx_fsr2_api_x64.dll", "libxess.dll"]
        );
    }

    #[test]
    fn a_symlinked_folder_is_not_followed() {
        let game = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        write(&elsewhere.path().join("nvngx_dlss.dll"), b"x");
        std::os::unix::fs::symlink(elsewhere.path(), game.path().join("link")).unwrap();
        assert!(upscalers(game.path()).is_empty());
    }

    #[test]
    fn an_unreadable_executable_is_assumed_only_for_an_xbox_install() {
        let game = tempfile::tempdir().unwrap();
        write(
            &game.path().join("Game.exe"),
            b"encrypted, or not a PE at all",
        );
        let found = detect(game.path(), Path::new("Game.exe"));
        assert_eq!(found.route, None);
        assert!(!found.assumed);

        write(&game.path().join(".xodus-streaming.msixvc"), b"prefix");
        let found = detect(game.path(), Path::new("Game.exe"));
        assert_eq!(found.route, Some(Route::ReshadeFeeder));
        assert!(found.assumed);
        assert_eq!((found.bits, found.api), (64, Api::Dx10To12));
    }
}
