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
//!
//! A title launched through the real-path image view
//! (`scripts/real-path-image-view.py`, Halo Infinite) runs in user and mount
//! namespaces of its own, with a private Wine server folder mounted over
//! `/tmp/.wine-<uid>`. A `taskkill` started out here would not find that
//! server and would start a second one, and a second Wine session, on the
//! live prefix. So the `taskkill` for such a title first joins the title's
//! namespaces ([`Join`]): the window's user owns them, so no privilege is
//! needed, and inside them it sees the title's server folder and the same
//! user id the title has.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
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
    /// The user and mount namespaces it runs in, when they are not the
    /// window's own: a `taskkill` for it has to [`Join`] them.
    pub namespaces: Option<String>,
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

/// The user and mount namespaces of the process at `dir`, when they are not
/// the window's own. A title launched through the real-path image view runs
/// Wine in namespaces of its own, with a private Wine server folder: a
/// `taskkill` started out here cannot reach its server, and would start a
/// second server and Wine session on the live prefix instead. It has to
/// [`Join`] them first.
fn foreign_namespaces(proc_root: &Path, dir: &Path) -> Option<String> {
    let links = |at: &Path| {
        ["ns/user", "ns/mnt"].map(|ns| {
            std::fs::read_link(at.join(ns))
                .map(|link| link.to_string_lossy().into_owned())
                .ok()
        })
    };
    let (own, theirs) = (links(&proc_root.join("self")), links(dir));
    (own != theirs).then(|| {
        theirs
            .map(|link| link.unwrap_or_else(|| "?".into()))
            .join(" ")
    })
}

/// Every Windows process of `uid` running in `prefix`, read from `proc_root`
/// (`/proc`, or a fixture in a test). Those in namespaces of their own say
/// so, in [`WineProcess::namespaces`].
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
            let namespaces = foreign_namespaces(proc_root, &dir);
            Some(WineProcess {
                pid,
                image,
                env,
                namespaces,
            })
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

/// The namespaces to join, in order, to run a command where a process in
/// namespaces of its own runs: every user namespace from the window's own
/// down to the process's, each entered from its parent, and the process's
/// mount namespace right after the user namespace that owns it.
///
/// No privilege is needed when the window's user made them, as the image
/// view does with `unshare`: the creator of a user namespace has every
/// capability in it and in the namespaces it owns. Entered that way, without
/// changing ids, the command keeps the window's user, which inside the
/// namespaces is the same user id the title has, and so finds the same Wine
/// server folder. A process whose namespaces cannot be entered like that --
/// somebody else's, or a mount namespace of the window's own user namespace
/// -- has no `Join`.
pub struct Join {
    steps: Vec<(File, libc::c_int)>,
}

/// A namespace's identity, as `stat` gives it on its nsfs file.
fn namespace_id(file: &File) -> std::io::Result<(u64, u64)> {
    let meta = file.metadata()?;
    Ok((meta.dev(), meta.ino()))
}

/// The namespace an `NS_GET_*` request on `file` names.
fn namespace_of(file: &File, request: libc::Ioctl) -> std::io::Result<File> {
    // Safety: an ioctl on a descriptor owned here; on success it returns a
    // new descriptor, owned by the caller from then on.
    let fd = unsafe { libc::ioctl(file.as_raw_fd(), request) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // Safety: just returned by the kernel, and nothing else owns it.
    Ok(File::from(unsafe { OwnedFd::from_raw_fd(fd) }))
}

/// Whether `uid`, as the window sees it, has an id inside the user namespace
/// whose `uid_map` is `map` -- read from outside, so each line's second
/// column is in the window's terms.
fn maps_uid(map: &str, uid: u32) -> bool {
    map.lines().any(|line| {
        let numbers: Vec<u64> = line
            .split_whitespace()
            .filter_map(|n| n.parse().ok())
            .collect();
        matches!(numbers[..], [_, outside, count] if (outside..outside + count).contains(&u64::from(uid)))
    })
}

impl Join {
    /// How to join the namespaces of process `pid`, read from `proc_root`
    /// (`/proc`). `None` when they cannot be joined from here.
    pub fn to(proc_root: &Path, pid: u32, uid: u32) -> std::io::Result<Option<Join>> {
        let process = proc_root.join(pid.to_string());
        if !maps_uid(&std::fs::read_to_string(process.join("uid_map"))?, uid) {
            return Ok(None);
        }
        let own_user = namespace_id(&File::open(proc_root.join("self/ns/user"))?)?;
        let own_mount = namespace_id(&File::open(proc_root.join("self/ns/mnt"))?)?;
        let mount = File::open(process.join("ns/mnt"))?;

        // Up from the process's user namespace to the window's. Asking past
        // the window's own fails (EPERM), and that is a namespace this
        // window's user did not make.
        let mut users = Vec::new();
        let mut user = File::open(process.join("ns/user"))?;
        while namespace_id(&user)? != own_user {
            if users.len() >= 32 {
                return Ok(None);
            }
            let Ok(parent) = namespace_of(&user, libc::NS_GET_PARENT) else {
                return Ok(None);
            };
            users.push(std::mem::replace(&mut user, parent));
        }
        users.reverse();

        // Joined right after the user namespace that owns it, which has to
        // be one of those being entered: one the window's own user namespace
        // owns, or one outside it, would need privileges the window lacks.
        let mount_after = if namespace_id(&mount)? == own_mount {
            None
        } else {
            let owner = namespace_id(&namespace_of(&mount, libc::NS_GET_USERNS)?)?;
            let mut position = None;
            for (i, user) in users.iter().enumerate() {
                if namespace_id(user)? == owner {
                    position = Some(i);
                }
            }
            if position.is_none() {
                return Ok(None);
            }
            position
        };
        let mut mount = Some(mount);
        let mut steps = Vec::new();
        for (i, user) in users.into_iter().enumerate() {
            steps.push((user, libc::CLONE_NEWUSER));
            if mount_after == Some(i) {
                if let Some(mount) = mount.take() {
                    steps.push((mount, libc::CLONE_NEWNS));
                }
            }
        }
        Ok(Some(Join { steps }))
    }

    /// Make `command` join the namespaces before it runs. The descriptors go
    /// with the command, and close when it is dropped.
    pub fn apply(self, command: &mut Command) {
        let steps = self.steps;
        // Safety: setns is async-signal-safe, and runs in the forked child,
        // which has a single thread, as entering a user namespace requires.
        unsafe {
            command.pre_exec(move || {
                for (namespace, kind) in &steps {
                    if libc::setns(namespace.as_raw_fd(), *kind) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
    }
}

/// The `wine` beside a runtime's `wineserver`.
fn wine_beside(server: &Path) -> Option<PathBuf> {
    ["wine", "wine64"]
        .iter()
        .map(|name| server.with_file_name(name))
        .find(|path| path.is_file())
}

/// The `taskkill`s that ask every one of `found` to close: one for the
/// processes in the window's own namespaces, and one for each set of
/// namespaces of their own, run inside them. A set that cannot be joined is
/// left out: asking it from out here would start a second Wine session on
/// its prefix.
pub fn close_commands(
    proc_root: &Path,
    uid: u32,
    wine: &Path,
    found: &[WineProcess],
) -> Vec<Command> {
    let mut groups: Vec<(Option<&str>, Vec<WineProcess>)> = Vec::new();
    for process in found {
        let key = process.namespaces.as_deref();
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, group)) => group.push(process.clone()),
            None => groups.push((key, vec![process.clone()])),
        }
    }
    groups
        .into_iter()
        .filter_map(|(key, group)| {
            let mut command = close_command(wine, &group)?;
            if key.is_some() {
                Join::to(proc_root, group[0].pid, uid)
                    .ok()
                    .flatten()?
                    .apply(&mut command);
            }
            Some(command)
        })
        .collect()
}

/// Ask the title running in `prefix` to close. Blocking; returns whether
/// anything was asked.
pub fn ask_to_close(server: &Path, prefix: &Path) -> bool {
    let Some(wine) = wine_beside(server) else {
        return false;
    };
    // Safety: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    let proc_root = Path::new("/proc");
    let found = processes_in(proc_root, uid, prefix);
    let mut asked = false;
    for mut command in close_commands(proc_root, uid, &wine, &found) {
        // taskkill exits non-zero when one of the images has no window to
        // close, which is common and says nothing about the ones that did.
        asked |= crate::deadline::status(&mut command, ASK_LIMIT).is_ok();
    }
    asked
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

    /// A title in the real-path view's namespaces is found from out here,
    /// and marked: its `taskkill` has to run inside them. One whose
    /// namespaces cannot be joined is not asked at all -- from out here the
    /// request would start a second Wine session on its prefix -- so Stop
    /// ends it.
    #[test]
    fn a_title_in_another_namespace_is_asked_only_from_inside_it() {
        let root =
            std::env::temp_dir().join(format!("ferestre-gui-proc-ns-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let prefix = root.join("games/halo-proton/pfx");
        std::fs::create_dir_all(&prefix).unwrap();
        std::fs::create_dir_all(root.join("self/ns")).unwrap();
        std::os::unix::fs::symlink("user:[4026531837]", root.join("self/ns/user")).unwrap();
        std::os::unix::fs::symlink("mnt:[4026531841]", root.join("self/ns/mnt")).unwrap();
        let env = format!("WINEPREFIX={}\0", prefix.display()).into_bytes();
        for (pid, user, mnt) in [
            (300, "user:[4026533000]", "mnt:[4026533001]"),
            (400, "user:[4026531837]", "mnt:[4026531841]"),
        ] {
            fake_process(&root, pid, 1000, b"C:\\Game\\HaloInfinite.exe\0", &env);
            let ns = root.join(pid.to_string()).join("ns");
            std::fs::create_dir_all(&ns).unwrap();
            std::os::unix::fs::symlink(user, ns.join("user")).unwrap();
            std::os::unix::fs::symlink(mnt, ns.join("mnt")).unwrap();
        }
        let found = processes_in(&root, 1000, &prefix);
        assert_eq!(
            found
                .iter()
                .map(|p| (p.pid, p.namespaces.as_deref()))
                .collect::<Vec<_>>(),
            [
                (300, Some("user:[4026533000] mnt:[4026533001]")),
                (400, None)
            ]
        );
        // The fixture's namespaces are only names: there is nothing to join,
        // so only the process in the window's own namespaces is asked.
        let commands = close_commands(&root, 1000, Path::new("wine"), &found);
        assert_eq!(commands.len(), 1);
        assert_eq!(
            commands[0].get_args().collect::<Vec<_>>(),
            ["taskkill", "/IM", "HaloInfinite.exe"]
        );
        assert!(close_commands(&root, 1000, Path::new("wine"), &found[..1]).is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_uid_map_read_from_outside_says_whether_the_window_user_is_in_it() {
        assert!(maps_uid("         0       1000          1\n", 1000));
        assert!(maps_uid("      1000       1000          1\n", 1000));
        assert!(maps_uid("         0     100000      65536\n", 100_500));
        assert!(!maps_uid("         0       1001          1\n", 1000));
        assert!(!maps_uid("", 1000));
    }

    /// Whether this machine lets an unprivileged user make user and mount
    /// namespaces, which the real-path image view needs. Where it is meant
    /// to (FERESTRE_REQUIRE_USERNS=1), not being able to is a failure.
    fn user_namespaces() -> bool {
        let works = Command::new("unshare")
            .args(["--user", "--map-root-user", "--mount", "true"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !works && std::env::var_os("FERESTRE_REQUIRE_USERNS").is_some_and(|v| v == "1") {
            panic!("unprivileged user namespaces are required (FERESTRE_REQUIRE_USERNS=1)");
        }
        works
    }

    /// A title started through the real image-view script, as Halo Infinite
    /// is: Wine runs as the caller in a user namespace nested in the view's,
    /// with a private Wine server folder over /tmp/.wine-<uid>. The close
    /// request joins those namespaces and runs as the title's own user id
    /// beside the title's own server folder; from out here that folder is not
    /// there at all. A fake `wine` stands in for Wine's taskkill and writes
    /// down what it saw.
    #[test]
    fn the_close_request_reaches_a_title_in_the_image_views_namespaces() {
        if !user_namespaces() {
            eprintln!("skipped: unprivileged user namespaces are not available");
            return;
        }
        let root =
            std::env::temp_dir().join(format!("ferestre-gui-stop-view-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let prefix = root.join("halo-proton/pfx");
        std::fs::create_dir_all(&prefix).unwrap();
        // The encrypted image on disk, and its licensed plaintext, which the
        // view binds over it inside its namespace.
        let image = root.join("HaloInfinite.exe");
        std::fs::write(&image, b"encrypted package bytes").unwrap();
        // Safety: a plain memfd_create; the descriptor is owned below.
        let fd = unsafe { libc::memfd_create(c"HaloInfinite.exe".as_ptr(), 0) };
        assert!(fd >= 0, "{}", std::io::Error::last_os_error());
        // Safety: just created, owned only here. Not close-on-exec: the
        // view reads it through the launch, as it does the client's.
        let plaintext = unsafe { OwnedFd::from_raw_fd(fd) };
        let mut licensed = b"MZ".to_vec();
        licensed.extend(b"licensed fixture image".repeat(50));
        std::io::Write::write_all(&mut File::from(plaintext.try_clone().unwrap()), &licensed)
            .unwrap();
        let mapping = format!(
            "{}:\\??\\Z:{}",
            plaintext.as_raw_fd(),
            image.display().to_string().replace('/', "\\")
        );

        let server_folder = format!("server-stop-test-{}", std::process::id());
        let ready = root.join("ready");
        // What the title does: leave a mark in its private server folder,
        // say it is up, and run under its Windows image name, as Wine does.
        let title = format!(
            "mkdir -p /tmp/.wine-$(id -u)/{server_folder} && \
             echo inside > /tmp/.wine-$(id -u)/{server_folder}/marker && \
             touch {ready} && exec -a 'C:\\Games\\Halo\\HaloInfinite.exe' sleep 60",
            ready = ready.display()
        );
        let view =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/real-path-image-view.py");
        let mut helper = Command::new("python3")
            .arg(&view)
            .args(["--", "bash", "-c", &title])
            .current_dir(&root)
            .env("WINE_DLL_FILE_MAP", &mapping)
            .env("FERESTRE_IMAGE_VIEW", "real-path")
            .env("WINEPREFIX", &prefix)
            .env_remove("STEAM_COMPAT_DATA_PATH")
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        drop(plaintext);
        let cleanup = |helper: &mut std::process::Child| {
            // Safety: a signal to the helper this test started.
            unsafe { libc::kill(helper.id() as i32, libc::SIGTERM) };
            let _ = helper.wait();
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        while !ready.exists() {
            if let Ok(Some(status)) = helper.try_wait() {
                panic!("the image view exited {status} before the title started");
            }
            if Instant::now() > deadline {
                cleanup(&mut helper);
                panic!("the title never started");
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        let result = std::panic::catch_unwind(|| {
            // Safety: getuid cannot fail.
            let uid = unsafe { libc::getuid() };
            let found = processes_in(Path::new("/proc"), uid, &prefix);
            assert_eq!(found.len(), 1, "{found:?}");
            assert_eq!(found[0].image, "HaloInfinite.exe");
            assert!(found[0].namespaces.is_some(), "{found:?}");
            assert!(
                !Path::new(&format!("/tmp/.wine-{uid}/{server_folder}/marker")).exists(),
                "the title's server folder is private to its namespaces"
            );

            // A `wine` that writes down where it ran and what it was asked.
            let seen = root.join("seen");
            let wine = root.join("bin/wine");
            std::fs::write(
                &wine,
                format!(
                    "#!/bin/sh\n{{ id -u; cat /tmp/.wine-$(id -u)/{server_folder}/marker; \
                     echo \"$@\"; }} > {}\n",
                    seen.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&wine, std::os::unix::fs::PermissionsExt::from_mode(0o755))
                .unwrap();
            std::fs::write(root.join("bin/wineserver"), "").unwrap();

            assert!(ask_to_close(&root.join("bin/wineserver"), &prefix));
            let seen = std::fs::read_to_string(&seen).unwrap();
            assert_eq!(
                seen,
                format!("{uid}\ninside\ntaskkill /IM HaloInfinite.exe\n"),
                "taskkill ran as the title's user, beside its server folder"
            );
        });
        cleanup(&mut helper);
        let _ = std::fs::remove_dir_all(&root);
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn several_instances_of_one_image_are_named_once() {
        let process = |pid, image: &str| WineProcess {
            pid,
            image: image.into(),
            env: vec![],
            namespaces: None,
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
