//! What is running in a title's Wine prefix, started by anyone.
//!
//! A second copy of a title started in a prefix where the first is still
//! running exits at once, with a code that says nothing about why (Hogwarts
//! Legacy exits 5). The window asks what to do about the first; `ferestre
//! run`, which a Steam shortcut starts with nobody to ask, refuses. Both find
//! it the same way, here.
//!
//! The search is done from outside the prefix. Every process Wine runs for a
//! title carries `WINEPREFIX` in its environment, and Wine rewrites a
//! process's argument list so the first argument is the Windows executable,
//! which is the image name `taskkill /IM` matches.
//!
//! It is cheap: one small read of `/proc/<pid>/status` per process, then the
//! command line of the caller's own, and the environment only of those whose
//! command line names a Windows executable -- a handful at most.
//!
//! A title launched through the real-path image view
//! (`scripts/real-path-image-view.py`, Halo Infinite) runs in user and mount
//! namespaces of its own, and is left out of the search on purpose: its Wine
//! server cannot be reached from out here, so nothing could be asked of it.
//! That also means such a title does not count as running in its prefix. The
//! window knows about the ones it started through their launch's stop
//! channel; one started elsewhere is not seen, and a second copy is not
//! refused.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// The variable that names a launch's end of the window's stop channel, by
/// descriptor number. Defined here because it is one of the variables a
/// process's environment must not pass on (see [`LOADER_PRIVATE`]).
pub const CHANNEL_FD: &str = "FERESTRE_STOP_FD";

/// The exit status of `ferestre run` when it refused to start a title that
/// is already running in its prefix. Codes 1 and 2 are a failure and a usage
/// error, 3 a refused runtime downgrade.
pub const EXIT_ALREADY_RUNNING: u8 = 4;

/// One Windows process in the title's prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WineProcess {
    pub pid: u32,
    /// The executable's file name, as `taskkill /IM` matches it.
    pub image: String,
    pub env: Vec<(OsString, OsString)>,
}

/// Wine's own processes, and Proton's launcher, which own no window a person
/// would close. Asking them is noise in the best case. The image view keeps
/// the same list (`scripts/real-path-image-view.py`).
const SYSTEM_IMAGES: &[&str] = &[
    "conhost.exe",
    "explorer.exe",
    "plugplay.exe",
    "rpcss.exe",
    "rundll32.exe",
    "services.exe",
    "start.exe",
    "steam.exe",
    "svchost.exe",
    "tabtip.exe",
    "taskkill.exe",
    "wineboot.exe",
    "winedevice.exe",
    "winemenubuilder.exe",
];

/// The image name in a Wine process's command line: the last component of its
/// first argument, which Wine sets to the Windows path of the executable.
fn image_name(cmdline: &[u8]) -> Option<String> {
    let first = cmdline.split(|b| *b == 0).next()?;
    let first = String::from_utf8_lossy(first);
    let name = first.rsplit(['\\', '/']).next()?.trim();
    let lower = name.to_ascii_lowercase();
    (lower.ends_with(".exe") && !SYSTEM_IMAGES.contains(&lower.as_str())).then(|| name.to_string())
}

/// What Wine's loader hands one process of its own when it starts it: the
/// file descriptor of that process's connection to the wineserver, and where
/// its preloader reserved memory. Copied into another process they point at
/// nothing, and that process cannot reach the server at all. The launch's
/// stop channel descriptor is the same kind of thing.
const LOADER_PRIVATE: &[&str] = &[
    "WINESERVERSOCKET",
    "WINEPRELOADRESERVE",
    "WINELOADERNOEXEC",
    CHANNEL_FD,
];

fn environment(bytes: &[u8]) -> Vec<(OsString, OsString)> {
    bytes
        .split(|b| *b == 0)
        .filter_map(|pair| {
            let at = pair.iter().position(|b| *b == b'=')?;
            Some((
                OsStr::from_bytes(&pair[..at]).to_os_string(),
                OsStr::from_bytes(&pair[at + 1..]).to_os_string(),
            ))
        })
        .filter(|(key, _)| !LOADER_PRIVATE.iter().any(|private| key == private))
        .collect()
}

/// The same directory, however the two spellings differ: a trailing slash
/// (Proton writes one), or a symlink on the way.
fn same_dir(a: &Path, b: &Path) -> bool {
    let trim = |p: &Path| PathBuf::from(p.as_os_str().to_string_lossy().trim_end_matches('/'));
    let (a, b) = (trim(a), trim(b));
    a == b
        || matches!(
            (a.canonicalize(), b.canonicalize()),
            (Ok(a), Ok(b)) if a == b
        )
}

fn owner(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Whether the process at `dir` runs in the caller's own user and mount
/// namespaces. One that does not -- a title in the real-path image view --
/// has a Wine server the caller cannot reach: a `taskkill` started out here
/// for it would start a second server and Wine session on the live prefix.
fn in_own_namespaces(proc_root: &Path, dir: &Path) -> bool {
    let links = |at: &Path| ["ns/user", "ns/mnt"].map(|ns| std::fs::read_link(at.join(ns)).ok());
    links(&proc_root.join("self")) == links(dir)
}

/// Every Windows process of `uid` running in `prefix` in the caller's own
/// namespaces, read from `proc_root` (`/proc`, or a fixture in a test).
pub fn processes_in(proc_root: &Path, uid: u32, prefix: &Path) -> Vec<WineProcess> {
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return Vec::new();
    };
    let mut found: Vec<WineProcess> = entries
        .flatten()
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            let dir = entry.path();
            if owner(&std::fs::read_to_string(dir.join("status")).ok()?)? != uid {
                return None;
            }
            // The command line first: it is short, and it rules out nearly
            // every process before its environment, which can be long, is read.
            let image = image_name(&std::fs::read(dir.join("cmdline")).ok()?)?;
            let env = environment(&std::fs::read(dir.join("environ")).ok()?);
            let in_prefix = env
                .iter()
                .any(|(key, value)| key == "WINEPREFIX" && same_dir(Path::new(value), prefix));
            if !in_prefix || !in_own_namespaces(proc_root, &dir) {
                return None;
            }
            Some(WineProcess { pid, image, env })
        })
        .collect();
    found.sort_by_key(|p| p.pid);
    found
}

/// Wine's own processes, which start with any title and can outlive it by a
/// few seconds; none of them means a title is running.
const WINE_HELPERS: &[&str] = &[
    "conhost.exe",
    "explorer.exe",
    "plugplay.exe",
    "rpcss.exe",
    "rundll32.exe",
    "services.exe",
    "start.exe",
    "steam.exe",
    "svchost.exe",
    "tabtip.exe",
    "winedevice.exe",
    "wineboot.exe",
    "xalia.exe",
];

/// The image names of a title's own processes among `found`: everything that
/// is not one of Wine's helpers, each name once.
pub fn title_images(found: &[WineProcess]) -> Vec<String> {
    let mut images: Vec<String> = Vec::new();
    for process in found {
        let helper = WINE_HELPERS
            .iter()
            .any(|h| h.eq_ignore_ascii_case(&process.image));
        if !helper
            && !images
                .iter()
                .any(|i| i.eq_ignore_ascii_case(&process.image))
        {
            images.push(process.image.clone());
        }
    }
    images
}

/// The caller's user id.
pub fn own_uid() -> u32 {
    // Safety: getuid cannot fail.
    unsafe { libc::getuid() }
}

/// What is still running of a title in `prefix` (the `WINEPREFIX`, the `pfx`
/// folder of a Proton prefix), started by anyone: an earlier window, a Steam
/// shortcut, a terminal. Empty when nothing is. A title in the real-path
/// image view's namespaces is not seen (see the module's notes).
pub fn running_in(prefix: &Path) -> Vec<String> {
    title_images(&processes_in(Path::new("/proc"), own_uid(), prefix))
}

/// What `ferestre run` says when it will not start a second copy.
pub fn refusal(name: &str, images: &[String]) -> String {
    format!(
        "{name} is already running in its prefix ({}). Close it, or stop it from Ferestre, \
         then start it again.",
        images.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_process(root: &Path, pid: u32, uid: u32, cmdline: &[u8], env: &[u8]) {
        let dir = root.join(pid.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("status"),
            format!("Name:\tx\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n"),
        )
        .unwrap();
        std::fs::write(dir.join("cmdline"), cmdline).unwrap();
        std::fs::write(dir.join("environ"), env).unwrap();
    }

    #[test]
    fn the_title_is_found_by_its_prefix_and_named_by_its_image() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let prefix = root.join("games/fh5-proton/pfx");
        std::fs::create_dir_all(&prefix).unwrap();
        let env = |pfx: &str| {
            format!(
                "HOME=/home/u\0WINEPREFIX={pfx}\0WINEFSYNC=1\0WINESERVERSOCKET=16\0\
                 WINEPRELOADRESERVE=140000000-140043000\0WINELOADERNOEXEC=1\0\
                 FERESTRE_STOP_FD=9\0"
            )
            .into_bytes()
        };
        let ours = format!("{}/", prefix.display());
        fake_process(
            root,
            300,
            1000,
            b"C:\\Games\\FH5\\ForzaHorizon5.exe\0-x\0",
            &env(&ours),
        );
        fake_process(
            root,
            200,
            1000,
            b"C:\\windows\\system32\\services.exe\0",
            &env(&ours),
        );
        fake_process(
            root,
            201,
            1000,
            b"C:\\windows\\system32\\steam.exe\0",
            &env(&ours),
        );
        fake_process(
            root,
            250,
            1000,
            b"/opt/proton/files/bin/wineserver\0",
            &env(&ours),
        );
        // Another title, another user, and a process that is not Wine.
        fake_process(
            root,
            400,
            1000,
            b"Z:\\other\\Other.exe\0",
            &env("/elsewhere/pfx"),
        );
        fake_process(
            root,
            500,
            1001,
            b"C:\\Games\\FH5\\ForzaHorizon5.exe\0",
            &env(&ours),
        );
        fake_process(root, 600, 1000, b"/usr/bin/bash\0", b"PATH=/usr/bin\0");
        std::fs::create_dir_all(root.join("self")).unwrap();

        let found = processes_in(root, 1000, &prefix);
        assert_eq!(
            found
                .iter()
                .map(|p| (p.pid, p.image.as_str()))
                .collect::<Vec<_>>(),
            vec![(300, "ForzaHorizon5.exe")]
        );
        assert!(found[0]
            .env
            .contains(&(OsString::from("WINEFSYNC"), OsString::from("1"))));
        assert!(found[0]
            .env
            .contains(&(OsString::from("WINEPREFIX"), OsString::from(&ours))));
        // The loader's own per-process variables are not passed on: with the
        // title's server socket number in it, taskkill could not reach the
        // server at all.
        for private in LOADER_PRIVATE {
            assert!(
                !found[0].env.iter().any(|(key, _)| key == private),
                "{private} was copied"
            );
        }
        assert_eq!(title_images(&found), ["ForzaHorizon5.exe"]);
    }

    /// A title in the real-path view's namespaces has a Wine server that
    /// cannot be reached from out here: a `taskkill` from out here would
    /// start a second Wine session on its prefix. It is left out of the
    /// search.
    #[test]
    fn a_title_in_another_namespace_is_left_out() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let prefix = root.join("games/halo-proton/pfx");
        std::fs::create_dir_all(&prefix).unwrap();
        std::fs::create_dir_all(root.join("self/ns")).unwrap();
        std::os::unix::fs::symlink("user:[4026531837]", root.join("self/ns/user")).unwrap();
        std::os::unix::fs::symlink("mnt:[4026531841]", root.join("self/ns/mnt")).unwrap();
        let env = format!("WINEPREFIX={}\0", prefix.display()).into_bytes();
        for (pid, user, mnt) in [
            (300, "user:[4026533000]", "mnt:[4026533001]"),
            (301, "user:[4026531837]", "mnt:[4026533001]"),
            (400, "user:[4026531837]", "mnt:[4026531841]"),
        ] {
            fake_process(root, pid, 1000, b"C:\\Game\\HaloInfinite.exe\0", &env);
            let ns = root.join(pid.to_string()).join("ns");
            std::fs::create_dir_all(&ns).unwrap();
            std::os::unix::fs::symlink(user, ns.join("user")).unwrap();
            std::os::unix::fs::symlink(mnt, ns.join("mnt")).unwrap();
        }
        let found = processes_in(root, 1000, &prefix);
        assert_eq!(found.iter().map(|p| p.pid).collect::<Vec<_>>(), [400]);
    }

    /// Wine's helpers linger after a title and must not read as the title
    /// still running; the title's own executables do, each named once.
    #[test]
    fn only_a_titles_own_processes_count_as_running() {
        let process = |pid, image: &str| WineProcess {
            pid,
            image: image.into(),
            env: Vec::new(),
        };
        let helpers = [
            process(1, "services.exe"),
            process(2, "Explorer.exe"),
            process(3, "steam.exe"),
        ];
        assert!(title_images(&helpers).is_empty());
        let game = [
            process(1, "services.exe"),
            process(4, "HogwartsLegacy.exe"),
            process(5, "HogwartsLegacy.exe"),
            process(6, "CrashReportClient.exe"),
        ];
        assert_eq!(
            title_images(&game),
            vec!["HogwartsLegacy.exe", "CrashReportClient.exe"]
        );
    }

    /// A real process, in a throwaway prefix, named the way Wine names one.
    #[test]
    fn a_live_process_in_a_prefix_is_seen_and_gone_once_it_exits() {
        // Under /var/tmp, like every fake prefix: nowhere a real one is.
        let tmp = tempfile::Builder::new()
            .prefix("ferestre-running-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let prefix = tmp.path().join("fake-proton/pfx");
        std::fs::create_dir_all(&prefix).unwrap();
        assert!(running_in(&prefix).is_empty());
        let mut child = std::process::Command::new("bash")
            .args(["-c", "exec -a 'C:\\Game\\FakeGame.exe' sleep 30"])
            .env("WINEPREFIX", format!("{}/", prefix.display()))
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut seen = Vec::new();
        while seen.is_empty() && std::time::Instant::now() < deadline {
            seen = running_in(&prefix);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(seen, ["FakeGame.exe"]);
        assert!(running_in(&prefix).is_empty());
        assert!(running_in(&tmp.path().join("other/pfx")).is_empty());
    }

    #[test]
    fn the_refusal_names_what_runs_and_what_to_do() {
        assert_eq!(
            refusal(
                "Hogwarts Legacy",
                &["HogwartsLegacy.exe".into(), "CrashReportClient.exe".into()]
            ),
            "Hogwarts Legacy is already running in its prefix (HogwartsLegacy.exe, \
             CrashReportClient.exe). Close it, or stop it from Ferestre, then start it again."
        );
    }

    /// The image view keeps its own copy of what is asked and what is left
    /// alone; the two must not drift apart.
    #[test]
    fn the_image_view_skips_the_same_images_and_variables() {
        let script = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/real-path-image-view.py"),
        )
        .unwrap();
        let listed = |name: &str| -> Vec<String> {
            let start = script
                .find(&format!("\n{name} = ("))
                .unwrap_or_else(|| panic!("{name} is not in the script"));
            let block = &script[start..];
            let block = &block[..block.find(')').unwrap()];
            let mut names: Vec<String> = block
                .split('"')
                .skip(1)
                .step_by(2)
                .map(str::to_string)
                .collect();
            names.sort();
            names
        };
        let sorted = |list: &[&str]| {
            let mut list: Vec<String> = list.iter().map(|s| s.to_string()).collect();
            list.sort();
            list
        };
        assert_eq!(listed("SYSTEM_IMAGES"), sorted(SYSTEM_IMAGES));
        assert_eq!(listed("LOADER_PRIVATE"), sorted(LOADER_PRIVATE));
        assert!(script.contains(&format!("CHANNEL_FD = \"{CHANNEL_FD}\"")));
    }
}
