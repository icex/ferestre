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
//! live prefix, so those processes are left out of the search. The view
//! asks them instead: every launch takes one end of a [`Channel`] with it,
//! and the view, which is already inside the namespaces, answers a close
//! request on it by running the same `taskkill` beside its title. The window
//! never enters those namespaces itself, and only ever talks to the launch
//! it started.

use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
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
/// How long the image view may take to answer a close request: its own
/// `taskkill` has [`ASK_LIMIT`], and finding what to ask takes a moment more.
const VIEW_ANSWER: Duration = Duration::from_secs(15);
/// The variable that names the launch's end of its [`Channel`], by
/// descriptor number.
pub const CHANNEL_FD: &str = "FERESTRE_STOP_FD";

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
/// [`Channel`] descriptor is the same kind of thing.
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

/// Whether the process at `dir` runs in the window's own user and mount
/// namespaces. One that does not -- a title in the real-path image view --
/// has a Wine server this window cannot reach: a `taskkill` started out here
/// for it would start a second server and Wine session on the live prefix.
/// Its view asks it through the launch's [`Channel`] instead.
fn in_own_namespaces(proc_root: &Path, dir: &Path) -> bool {
    let links = |at: &Path| ["ns/user", "ns/mnt"].map(|ns| std::fs::read_link(at.join(ns)).ok());
    links(&proc_root.join("self")) == links(dir)
}

/// Every Windows process of `uid` running in `prefix` in the window's own
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
            let env = environment(&std::fs::read(dir.join("environ")).ok()?);
            let in_prefix = env
                .iter()
                .any(|(key, value)| key == "WINEPREFIX" && same_dir(Path::new(value), prefix));
            if !in_prefix || !in_own_namespaces(proc_root, &dir) {
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

/// The window's end of a launch's stop channel: a socket pair, the other end
/// of which the launch inherits, named by [`CHANNEL_FD`].
///
/// Only the real-path image view reads it. Once it has started its title it
/// says `ready`; a close request is `close <wine>`, and it answers `asked`
/// when it ran `taskkill` for the title, inside its namespaces, or `none`
/// when there was nothing to ask. Every other launch leaves it unread, and
/// the window then asks nothing through it.
pub struct Channel {
    ours: UnixStream,
    ready: AtomicBool,
}

impl Channel {
    /// Make a channel for the launch `command` starts. The launch's end goes
    /// with the command, and closes when the command is dropped.
    pub fn attach(command: &mut Command) -> std::io::Result<Channel> {
        // Both ends close on exec; the launch's is opened up in the child
        // alone, so nothing else this window starts holds it.
        let (channel, theirs) = Channel::pair()?;
        command.env(CHANNEL_FD, theirs.as_raw_fd().to_string());
        // Safety: fcntl is async-signal-safe, and changes only the child's
        // copy of a descriptor the closure owns.
        unsafe {
            command.pre_exec(move || {
                let fd = theirs.as_raw_fd();
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(channel)
    }

    /// A channel, and the other end of it.
    fn pair() -> std::io::Result<(Channel, UnixStream)> {
        let (ours, theirs) = UnixStream::pair()?;
        let channel = Channel {
            ours,
            ready: AtomicBool::new(false),
        };
        Ok((channel, theirs))
    }

    /// Whether an image view at the other end has said it is listening. Does
    /// not wait for it.
    pub fn view_listening(&self) -> bool {
        if self.ready.load(Ordering::Relaxed) {
            return true;
        }
        let mut pending = Vec::new();
        if self.ours.set_nonblocking(true).is_ok() {
            let mut chunk = [0u8; 64];
            while let Ok(n @ 1..) = (&self.ours).read(&mut chunk) {
                pending.extend_from_slice(&chunk[..n]);
            }
            let _ = self.ours.set_nonblocking(false);
        }
        let ready = pending.split(|b| *b == b'\n').any(|line| line == b"ready");
        if ready {
            self.ready.store(true, Ordering::Relaxed);
        }
        ready
    }

    /// Ask the image view to close its title with `wine`, and wait up to
    /// `limit` for its answer. `None` when no view is listening, or it did
    /// not answer; otherwise whether it asked anything.
    pub fn ask(&self, wine: &Path, limit: Duration) -> Option<bool> {
        let wine = wine.as_os_str().as_bytes();
        if wine.contains(&b'\n') || !self.view_listening() {
            return None;
        }
        let mut request = b"close ".to_vec();
        request.extend_from_slice(wine);
        request.push(b'\n');
        (&self.ours).write_all(&request).ok()?;
        let answer = self.line(Instant::now() + limit)?;
        Some(answer == b"asked")
    }

    /// The next line from the view, without its newline, if it comes by
    /// `deadline`.
    fn line(&self, deadline: Instant) -> Option<Vec<u8>> {
        let mut line = Vec::new();
        let mut byte = [0u8];
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            self.ours.set_read_timeout(Some(left)).ok()?;
            match (&self.ours).read(&mut byte) {
                Ok(0) => return None,
                Ok(_) if byte[0] == b'\n' => return Some(line),
                Ok(_) => line.push(byte[0]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return None,
            }
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

/// Ask the title running in `prefix` to close: a `taskkill` from here for its
/// processes in the window's own namespaces, and a request through the
/// launch's `channel` to the image view, if one is listening, for those in
/// its namespaces. Blocking; returns whether anything was asked.
pub fn ask_to_close(server: &Path, prefix: &Path, channel: Option<&Channel>) -> bool {
    let Some(wine) = wine_beside(server) else {
        return false;
    };
    // Safety: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    let found = processes_in(Path::new("/proc"), uid, prefix);
    let mut asked = false;
    if let Some(mut command) = close_command(&wine, &found) {
        // taskkill exits non-zero when one of the images has no window to
        // close, which is common and says nothing about the ones that did.
        asked |= crate::deadline::status(&mut command, ASK_LIMIT).is_ok();
    }
    if let Some(channel) = channel {
        asked |= channel.ask(&wine, VIEW_ANSWER) == Some(true);
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
    use std::os::fd::FromRawFd;

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

    /// A title in the real-path view's namespaces has a Wine server this
    /// window cannot reach: a `taskkill` from out here would start a second
    /// Wine session on its prefix. It is left out of the search, for its view
    /// to ask.
    #[test]
    fn a_title_in_another_namespace_is_left_to_its_view() {
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
            (301, "user:[4026531837]", "mnt:[4026533001]"),
            (400, "user:[4026531837]", "mnt:[4026531841]"),
        ] {
            fake_process(&root, pid, 1000, b"C:\\Game\\HaloInfinite.exe\0", &env);
            let ns = root.join(pid.to_string()).join("ns");
            std::fs::create_dir_all(&ns).unwrap();
            std::os::unix::fs::symlink(user, ns.join("user")).unwrap();
            std::os::unix::fs::symlink(mnt, ns.join("mnt")).unwrap();
        }
        let found = processes_in(&root, 1000, &prefix);
        assert_eq!(found.iter().map(|p| p.pid).collect::<Vec<_>>(), [400]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// What an image view does with its end: say it is listening, then
    /// answer one close request with `answer`, after checking it.
    fn fake_view(theirs: UnixStream, answer: &'static [u8]) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            (&theirs).write_all(b"ready\n").unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8];
            while (&theirs).read(&mut byte).unwrap() == 1 && byte[0] != b'\n' {
                request.push(byte[0]);
            }
            (&theirs).write_all(answer).unwrap();
            request
        })
    }

    #[test]
    fn a_view_that_is_listening_is_asked_and_its_answer_counts() {
        for (answer, expected) in [(&b"asked\n"[..], true), (b"none\n", false)] {
            let (channel, theirs) = Channel::pair().unwrap();
            let view = fake_view(theirs, answer);
            let deadline = Instant::now() + Duration::from_secs(5);
            while !channel.view_listening() {
                assert!(Instant::now() < deadline, "the view never said ready");
                std::thread::sleep(Duration::from_millis(10));
            }
            let wine = Path::new("/opt/proton/files/bin/wine");
            assert_eq!(channel.ask(wine, Duration::from_secs(5)), Some(expected));
            assert_eq!(view.join().unwrap(), b"close /opt/proton/files/bin/wine");
        }
    }

    /// A launch with no image view never reads its end. Stop must not wait on
    /// it: the request is not even sent, and the title is ended at once when
    /// nothing else was asked.
    #[test]
    fn no_view_listening_means_nothing_is_asked_and_nothing_is_waited_for() {
        let (channel, theirs) = Channel::pair().unwrap();
        let started = Instant::now();
        assert_eq!(
            channel.ask(Path::new("/bin/wine"), Duration::from_secs(30)),
            None
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        theirs.set_nonblocking(true).unwrap();
        let mut sent = [0u8; 16];
        assert!(
            (&theirs).read(&mut sent).is_err(),
            "a request was sent to nobody"
        );
    }

    /// A view that says it is listening and then never answers, or goes
    /// away, has asked nothing.
    #[test]
    fn a_view_that_does_not_answer_has_asked_nothing() {
        let (channel, theirs) = Channel::pair().unwrap();
        (&theirs).write_all(b"ready\n").unwrap();
        let started = Instant::now();
        assert_eq!(
            channel.ask(Path::new("/bin/wine"), Duration::from_millis(300)),
            None
        );
        assert!(started.elapsed() >= Duration::from_millis(300));

        let (channel, theirs) = Channel::pair().unwrap();
        (&theirs).write_all(b"ready\n").unwrap();
        drop(theirs);
        assert_eq!(
            channel.ask(Path::new("/bin/wine"), Duration::from_secs(5)),
            None
        );
        // A path the line protocol cannot carry is not sent at all.
        let (channel, theirs) = Channel::pair().unwrap();
        (&theirs).write_all(b"ready\n").unwrap();
        assert_eq!(
            channel.ask(Path::new("/bin/wi\nne"), Duration::from_secs(5)),
            None
        );
    }

    /// The launch inherits its end, under the number in [`CHANNEL_FD`], and
    /// nothing else the window starts does.
    #[test]
    fn the_launch_inherits_its_end_of_the_channel_and_nothing_else_does() {
        let mut launch = Command::new("python3");
        launch.args([
            "-c",
            "import os, socket\n\
             s = socket.socket(fileno=int(os.environ['FERESTRE_STOP_FD']))\n\
             s.sendall(b'ready\\n')\n\
             line = s.makefile('rb').readline()\n\
             s.sendall(b'asked\\n' if line == b'close /usr/bin/wine\\n' else b'none\\n')\n",
        ]);
        let channel = Channel::attach(&mut launch).unwrap();
        let fd: i32 = launch
            .get_envs()
            .find(|(key, _)| *key == CHANNEL_FD)
            .and_then(|(_, value)| value?.to_str()?.parse().ok())
            .unwrap();
        let mut child = launch.spawn().unwrap();

        // Started while the launch's end is still open here.
        let other = Command::new("python3")
            .args(["-c", &format!("import os, sys\ntry:\n    os.fstat({fd})\nexcept OSError:\n    sys.exit(0)\nsys.exit(1)")])
            .status()
            .unwrap();
        assert!(other.success(), "another child inherited the launch's end");
        drop(launch);

        let deadline = Instant::now() + Duration::from_secs(10);
        while !channel.view_listening() {
            assert!(Instant::now() < deadline, "the launch never said ready");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            channel.ask(Path::new("/usr/bin/wine"), Duration::from_secs(10)),
            Some(true)
        );
        assert!(child.wait().unwrap().success());
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
    /// with a private Wine server folder over /tmp/.wine-<uid>. Stop finds
    /// nothing to ask from out here, and asks the view through the launch's
    /// channel; the view runs the request as the title's own user id beside
    /// the title's own server folder, which from out here is not there at
    /// all. A fake `wine` stands in for Wine's taskkill and writes down what
    /// it saw.
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
        let plaintext = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
        let mut licensed = b"MZ".to_vec();
        licensed.extend(b"licensed fixture image".repeat(50));
        std::io::Write::write_all(
            &mut std::fs::File::from(plaintext.try_clone().unwrap()),
            &licensed,
        )
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
        let mut launch = Command::new("python3");
        launch
            .arg(&view)
            .args(["--", "bash", "-c", &title])
            .current_dir(&root)
            .env("WINE_DLL_FILE_MAP", &mapping)
            .env("FERESTRE_IMAGE_VIEW", "real-path")
            .env("WINEPREFIX", &prefix)
            .env_remove("STEAM_COMPAT_DATA_PATH")
            .stdin(Stdio::null());
        let channel = Channel::attach(&mut launch).unwrap();
        let mut helper = launch.spawn().unwrap();
        drop(launch);
        drop(plaintext);
        let cleanup = |helper: &mut std::process::Child| {
            // Safety: a signal to the helper this test started.
            unsafe { libc::kill(helper.id() as i32, libc::SIGTERM) };
            let _ = helper.wait();
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        while !ready.exists() || !channel.view_listening() {
            if let Ok(Some(status)) = helper.try_wait() {
                panic!("the image view exited {status} before the title started");
            }
            if Instant::now() > deadline {
                cleanup(&mut helper);
                panic!("the title never started, or the view never said it listens");
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        let result = std::panic::catch_unwind(|| {
            // Safety: getuid cannot fail.
            let uid = unsafe { libc::getuid() };
            assert_eq!(
                processes_in(Path::new("/proc"), uid, &prefix),
                [],
                "nothing in the view is asked from out here"
            );
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
                     echo \"$@\"; echo \"${{{CHANNEL_FD}:-unset}}\"; }} > {}\n",
                    seen.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&wine, std::os::unix::fs::PermissionsExt::from_mode(0o755))
                .unwrap();
            std::fs::write(root.join("bin/wineserver"), "").unwrap();

            assert!(ask_to_close(
                &root.join("bin/wineserver"),
                &prefix,
                Some(&channel)
            ));
            let seen = std::fs::read_to_string(&seen).unwrap();
            assert_eq!(
                seen,
                format!("{uid}\ninside\ntaskkill /IM HaloInfinite.exe\nunset\n"),
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
        assert!(ask_to_close(&server, &prefix, None), "nothing was asked");
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
