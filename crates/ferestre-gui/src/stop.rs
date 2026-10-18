//! Stopping a title the window started.
//!
//! Stop used to run `wineserver -k` straight away, which ends every Windows
//! process in the prefix the way TerminateProcess does: no chance to save, and
//! a save being written at that moment could be left torn. Now it asks first,
//! the way closing the game's window would: Wine's own `taskkill`, without
//! `/F`, posts WM_CLOSE to the title's windows. The window then waits up to
//! [`CLOSE_WAIT`] for the title to exit before ending the prefix, and pressing
//! Stop again during that wait ends it at once.
//!
//! Finding what to ask is done from outside the prefix. Every process Wine
//! runs for a title carries `WINEPREFIX` in its environment, and Wine rewrites
//! a process's argument list so the first argument is the Windows executable,
//! which is the image name `taskkill /IM` matches. The `taskkill` itself runs
//! with the environment of one of those processes, so it joins the running
//! wineserver with the same synchronisation settings and paths as the title.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long a title gets to close after being asked, before it is ended.
pub const CLOSE_WAIT: Duration = Duration::from_secs(15);
/// Presses this soon after Play are the second click of a double-click: the
/// Stop button is drawn where Play was, under the pointer.
pub const PLAY_GRACE: Duration = Duration::from_millis(1500);
/// A second press this soon after asking is the same double-click, not a
/// decision to end the title now.
pub const REPEAT_GRACE: Duration = Duration::from_secs(1);
/// How long `taskkill` itself may take.
const ASK_LIMIT: Duration = Duration::from_secs(10);

/// A title this window started and has not seen exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    /// The launcher's process group: it is a session leader, so its pid.
    pub pgid: u32,
    pub started: Instant,
    /// When it was asked to close, while the window waits for it to.
    pub asked: Option<Instant>,
}

impl Launch {
    pub fn new(pgid: u32, started: Instant) -> Self {
        Launch {
            pgid,
            started,
            asked: None,
        }
    }
}

/// What a press of Stop does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    /// Nothing: a double-click that started the title, or one that asked it
    /// to close a moment ago.
    Ignore,
    /// Ask the title to close, and end it if it has not after [`CLOSE_WAIT`].
    Ask,
    /// End it now. The second press, while the window waits.
    End,
}

pub fn press(launch: &Launch, now: Instant) -> Press {
    if now.saturating_duration_since(launch.started) < PLAY_GRACE {
        return Press::Ignore;
    }
    match launch.asked {
        None => Press::Ask,
        Some(asked) if now.saturating_duration_since(asked) < REPEAT_GRACE => Press::Ignore,
        Some(_) => Press::End,
    }
}

/// One Windows process in the title's prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WineProcess {
    pub pid: u32,
    /// The executable's file name, as `taskkill /IM` matches it.
    pub image: String,
    pub env: Vec<(OsString, OsString)>,
}

/// Wine's own processes, and Proton's launcher, which own no window a person
/// would close. Asking them is noise in the best case.
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
/// nothing, and that process cannot reach the server at all.
const LOADER_PRIVATE: &[&str] = &["WINESERVERSOCKET", "WINEPRELOADRESERVE", "WINELOADERNOEXEC"];

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

/// Every Windows process of `uid` running in `prefix`, read from `proc_root`
/// (`/proc`, or a fixture in a test).
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
            let env = environment(&std::fs::read(dir.join("environ")).ok()?);
            let in_prefix = env
                .iter()
                .any(|(key, value)| key == "WINEPREFIX" && same_dir(Path::new(value), prefix));
            if !in_prefix {
                return None;
            }
            let image = image_name(&std::fs::read(dir.join("cmdline")).ok()?)?;
            Some(WineProcess { pid, image, env })
        })
        .collect();
    found.sort_by_key(|p| p.pid);
    found
}

/// The `taskkill` that asks every one of `found` to close, run as the first of
/// them would be. `None` when there is nothing to ask.
pub fn close_command(wine: &Path, found: &[WineProcess]) -> Option<Command> {
    let first = found.first()?;
    let mut command = Command::new(wine);
    command.arg("taskkill");
    let mut named: Vec<String> = Vec::new();
    for process in found {
        if !named.iter().any(|n| n.eq_ignore_ascii_case(&process.image)) {
            named.push(process.image.clone());
        }
    }
    for image in &named {
        command.arg("/IM").arg(image);
    }
    command
        .env_clear()
        .envs(first.env.iter().map(|(k, v)| (k, v)))
        .current_dir("/")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Some(command)
}

/// The `wine` beside a runtime's `wineserver`.
fn wine_beside(server: &Path) -> Option<PathBuf> {
    ["wine", "wine64"]
        .iter()
        .map(|name| server.with_file_name(name))
        .find(|path| path.is_file())
}

/// Ask the title running in `prefix` to close. Blocking; returns whether
/// anything was asked.
pub fn ask_to_close(server: &Path, prefix: &Path) -> bool {
    let Some(wine) = wine_beside(server) else {
        return false;
    };
    // Safety: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    let found = processes_in(Path::new("/proc"), uid, prefix);
    let Some(mut command) = close_command(&wine, &found) else {
        return false;
    };
    // taskkill exits non-zero when one of the images has no window to close,
    // which is common and says nothing about the ones that did.
    crate::deadline::status(&mut command, ASK_LIMIT).is_ok()
}

/// End the title now: the launcher's process group, then every process in
/// the prefix. Returns the `wineserver -k` child, to be reaped.
pub fn end(pgid: u32, server: Option<&(PathBuf, PathBuf)>) -> Option<std::process::Child> {
    // Safety: a signal to a process group this window created.
    unsafe { libc::kill(-(pgid as i32), libc::SIGTERM) };
    // The group signal does not reach the game: wineserver detaches, so the
    // title is reparented out of it. `wineserver -k` ends the prefix.
    let (server, prefix) = server?;
    Command::new(server)
        .env("WINEPREFIX", prefix)
        .arg("-k")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_press_right_after_play_is_the_rest_of_a_double_click() {
        let start = Instant::now();
        let launch = Launch::new(10, start);
        assert_eq!(
            press(&launch, start + Duration::from_millis(200)),
            Press::Ignore
        );
        assert_eq!(press(&launch, start + PLAY_GRACE), Press::Ask);
    }

    #[test]
    fn the_first_press_asks_and_a_later_one_ends_it() {
        let start = Instant::now();
        let asked = start + Duration::from_secs(60);
        let launch = Launch {
            asked: Some(asked),
            ..Launch::new(10, start)
        };
        assert_eq!(
            press(&launch, asked + Duration::from_millis(300)),
            Press::Ignore
        );
        assert_eq!(press(&launch, asked + REPEAT_GRACE), Press::End);
        assert_eq!(press(&launch, asked + CLOSE_WAIT), Press::End);
    }

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
        let root = std::env::temp_dir().join(format!("ferestre-gui-proc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let prefix = root.join("games/fh5-proton/pfx");
        std::fs::create_dir_all(&prefix).unwrap();
        let env = |pfx: &str| {
            format!(
                "HOME=/home/u\0WINEPREFIX={pfx}\0WINEFSYNC=1\0WINESERVERSOCKET=16\0\
                 WINEPRELOADRESERVE=140000000-140043000\0WINELOADERNOEXEC=1\0"
            )
            .into_bytes()
        };
        let ours = format!("{}/", prefix.display());
        fake_process(
            &root,
            300,
            1000,
            b"C:\\Games\\FH5\\ForzaHorizon5.exe\0-x\0",
            &env(&ours),
        );
        fake_process(
            &root,
            200,
            1000,
            b"C:\\windows\\system32\\services.exe\0",
            &env(&ours),
        );
        fake_process(
            &root,
            201,
            1000,
            b"C:\\windows\\system32\\steam.exe\0",
            &env(&ours),
        );
        fake_process(
            &root,
            250,
            1000,
            b"/opt/proton/files/bin/wineserver\0",
            &env(&ours),
        );
        // Another title, another user, and a process that is not Wine.
        fake_process(
            &root,
            400,
            1000,
            b"Z:\\other\\Other.exe\0",
            &env("/elsewhere/pfx"),
        );
        fake_process(
            &root,
            500,
            1001,
            b"C:\\Games\\FH5\\ForzaHorizon5.exe\0",
            &env(&ours),
        );
        fake_process(&root, 600, 1000, b"/usr/bin/bash\0", b"PATH=/usr/bin\0");
        std::fs::create_dir_all(root.join("self")).unwrap();

        let found = processes_in(&root, 1000, &prefix);
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
        // The loader's own per-process variables are not passed on: with the
        // title's server socket number in it, taskkill could not reach the
        // server at all.
        for private in LOADER_PRIVATE {
            assert!(
                !found[0].env.iter().any(|(key, _)| key == private),
                "{private} was copied"
            );
        }

        let command = close_command(Path::new("/opt/proton/files/bin/wine"), &found).unwrap();
        let args: Vec<_> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["taskkill", "/IM", "ForzaHorizon5.exe"]);
        assert!(
            !args.iter().any(|a| a.eq_ignore_ascii_case("/F")),
            "asking must not force"
        );
        let env: Vec<_> = command.get_envs().collect();
        assert!(env.contains(&(OsStr::new("WINEPREFIX"), Some(OsStr::new(&ours)))));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn several_instances_of_one_image_are_named_once() {
        let process = |pid, image: &str| WineProcess {
            pid,
            image: image.into(),
            env: vec![],
        };
        let found = [
            process(1, "Game.exe"),
            process(2, "game.EXE"),
            process(3, "Helper.exe"),
        ];
        let command = close_command(Path::new("wine"), &found).unwrap();
        let args: Vec<_> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["taskkill", "/IM", "Game.exe", "/IM", "Helper.exe"]);
        assert!(close_command(Path::new("wine"), &[]).is_none());
    }

    /// Against a real Wine, opt-in: `FERESTRE_STOP_WINESERVER` names a
    /// runtime's wineserver and `FERESTRE_STOP_PREFIX` a prefix in which a
    /// windowed program that closes on WM_CLOSE (notepad) is running.
    #[test]
    #[ignore]
    fn a_running_wine_program_closes_when_asked() {
        let var = |name: &str| PathBuf::from(std::env::var_os(name).expect(name));
        let (server, prefix) = (var("FERESTRE_STOP_WINESERVER"), var("FERESTRE_STOP_PREFIX"));
        // Safety: getuid cannot fail.
        let uid = unsafe { libc::getuid() };
        let before = processes_in(Path::new("/proc"), uid, &prefix);
        assert!(
            !before.is_empty(),
            "nothing is running in {}",
            prefix.display()
        );
        assert!(ask_to_close(&server, &prefix), "nothing was asked");
        let deadline = Instant::now() + CLOSE_WAIT;
        while !processes_in(Path::new("/proc"), uid, &prefix).is_empty() {
            let left: Vec<_> = processes_in(Path::new("/proc"), uid, &prefix)
                .into_iter()
                .map(|p| (p.pid, p.image))
                .collect();
            assert!(
                Instant::now() < deadline,
                "still running after being asked: {left:?}"
            );
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}
