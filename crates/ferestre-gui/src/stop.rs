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
//! The search is `ferestre_core::running`'s, which `ferestre run` uses too, to
//! refuse a second copy.
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

use ferestre_core::running::own_uid;
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

/// A title this window started and has not seen exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    /// The launcher's process group: it is a session leader, so its pid.
    pub pgid: u32,
    /// Private marker inherited only by processes from this launch.
    pub tag: String,
    pub started: Instant,
    /// A game executable from this launch has been observed, beyond setup.
    pub ready: bool,
    /// When it was asked to close, while the window waits for it to.
    pub asked: Option<Instant>,
}

impl Launch {
    pub fn new(pgid: u32, started: Instant) -> Self {
        Launch {
            pgid,
            tag: String::new(),
            started,
            ready: false,
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

/// The answer applies only to the launch and kind of stop shown in the dialog.
pub fn confirmed(current: Option<&Launch>, shown: &Launch) -> Option<Press> {
    let current = current
        .filter(|l| l.pgid == shown.pgid && l.tag == shown.tag && l.started == shown.started)?;
    match (shown.asked, current.asked) {
        (None, None) => Some(Press::Ask),
        (Some(before), Some(now)) if before == now => Some(Press::End),
        _ => None,
    }
}

/// One Windows process in the title's prefix, and how the prefix is
/// searched: shared with `ferestre run`, which refuses a second copy.
pub use ferestre_core::running::{processes_in, running_in, WineProcess, CHANNEL_FD};

/// How long processes get to go after `wineserver -k`, or a signal, before
/// the next step.
const AFTER_KILL: Duration = Duration::from_secs(3);

/// End everything in `prefix` now, without a launch of this window's to
/// signal: `wineserver -k`. Blocking, bounded by [`ASK_LIMIT`].
fn kill_prefix(server: &Path, prefix: &Path) {
    let mut command = Command::new(server);
    command
        .env("WINEPREFIX", prefix)
        .arg("-k")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let _ = crate::deadline::status(&mut command, ASK_LIMIT);
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
    let found = processes_in(Path::new("/proc"), own_uid(), prefix);
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

/// Whether nothing of a title is left in `prefix` by the time `limit` is up.
fn gone_within(prefix: &Path, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        if running_in(prefix).is_empty() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Whether a title in a prefix of `server`'s runtime can be asked to close,
/// rather than only ended: there is a `wine` beside the server to ask it with.
pub fn can_ask(server: &Path) -> bool {
    wine_beside(server).is_some()
}

/// Close a title running in `prefix` that this window did not start, so has
/// no launch of to signal. Blocking; returns whether nothing of it is left.
///
/// First it is asked, the way closing its window would, and given
/// [`CLOSE_WAIT`] to go. It gets the wait whenever there was a `wine` to ask
/// with, even if `taskkill` did not finish in time: the close may already be
/// on its way, and a save being written. With no `wine` beside `server` there
/// is nothing to ask, and it is not waited for ([`can_ask`] tells the window
/// to say so). Then `wineserver -k` ends the prefix: it signals whichever
/// server holds the prefix's lock, from any Wine build, and that server ends
/// its processes. What is left after that is a title whose server is already
/// gone, which nothing else will end: it is sent SIGKILL. Only the processes
/// that were in the prefix when the close began are signalled, so a copy
/// started meanwhile is never the target, and only this user's, in this
/// prefix and these namespaces.
pub fn close_running(server: &Path, prefix: &Path) -> bool {
    close_running_within(server, prefix, CLOSE_WAIT)
}

fn close_running_within(server: &Path, prefix: &Path, wait: Duration) -> bool {
    let first: Vec<u32> = processes_in(Path::new("/proc"), own_uid(), prefix)
        .iter()
        .map(|process| process.pid)
        .collect();
    if can_ask(server) {
        ask_to_close(server, prefix, None);
        if gone_within(prefix, wait) {
            return true;
        }
    }
    kill_prefix(server, prefix);
    if gone_within(prefix, AFTER_KILL) {
        return true;
    }
    for process in processes_in(Path::new("/proc"), own_uid(), prefix) {
        if first.contains(&process.pid) {
            // Safety: a signal to a process found in this title's prefix,
            // owned by this user, both when the close began and a moment ago.
            unsafe { libc::kill(process.pid as i32, libc::SIGKILL) };
        }
    }
    gone_within(prefix, AFTER_KILL)
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
    use std::ffi::OsStr;
    use std::os::fd::FromRawFd;

    #[test]
    fn confirmation_cannot_stop_a_new_launch_or_silently_escalate() {
        let shown = Launch {
            tag: "launch-one".into(),
            ..Launch::new(42, Instant::now())
        };
        assert_eq!(confirmed(Some(&shown), &shown), Some(Press::Ask));
        assert_eq!(confirmed(None, &shown), None);
        let mut next = shown.clone();
        next.tag = "launch-two".into();
        assert_eq!(confirmed(Some(&next), &shown), None);
        next = shown.clone();
        next.pgid = 43;
        assert_eq!(confirmed(Some(&next), &shown), None);
        next = shown.clone();
        next.asked = Some(Instant::now());
        assert_eq!(confirmed(Some(&next), &shown), None);
        assert_eq!(confirmed(Some(&next), &next), Some(Press::End));
    }

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

    /// The `taskkill` asks, never forces, every image found, and runs with
    /// the environment of the first process (its prefix included).
    #[test]
    fn the_close_command_asks_every_image_in_the_titles_own_environment() {
        let prefix = "/var/tmp/ferestre-unit/fh5-proton/pfx/";
        let found = [WineProcess {
            pid: 300,
            image: "ForzaHorizon5.exe".into(),
            env: vec![
                ("WINEPREFIX".into(), prefix.into()),
                ("WINEFSYNC".into(), "1".into()),
            ],
        }];
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
        assert!(env.contains(&(OsStr::new("WINEPREFIX"), Some(OsStr::new(prefix)))));
        assert!(env.contains(&(OsStr::new("WINEFSYNC"), Some(OsStr::new("1")))));
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
            let uid = own_uid();
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

    /// A fake game, `sleep` under the image name Wine gives a Windows
    /// executable, in a prefix under the test's own folder in /var/tmp.
    fn fake_game(prefix: &Path) -> std::process::Child {
        let child = Command::new("bash")
            .args(["-c", "exec -a 'C:\\Game\\FakeGame.exe' sleep 120"])
            .env("WINEPREFIX", format!("{}/", prefix.display()))
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !processes_in(Path::new("/proc"), own_uid(), prefix)
            .iter()
            .any(|p| p.pid == child.id())
        {
            assert!(Instant::now() < deadline, "the fake game never started");
            std::thread::sleep(Duration::from_millis(20));
        }
        child
    }

    /// The signal a fake game was ended with, reaping it.
    fn ended_by(child: &mut std::process::Child) -> Option<i32> {
        use std::os::unix::process::ExitStatusExt;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return status.signal();
            }
            if Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A folder of the test's own, removed when it is dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct Fixture {
        // Dropped after the children: fields drop in order, and `Drop for
        // Fixture` runs first.
        root: Scratch,
        prefix: PathBuf,
        server: PathBuf,
        children: Vec<std::process::Child>,
    }

    impl Fixture {
        /// A throwaway runtime and prefix under /var/tmp, where no real
        /// prefix is. `kill` is what the fake `wineserver -k` does to the
        /// pids in `game-pids` besides writing down that it ran.
        fn new(kill: &str) -> Self {
            static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let root = Scratch(PathBuf::from(format!(
                "/var/tmp/ferestre-close-running-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )));
            let prefix = root.path().join("fake-proton/pfx");
            std::fs::create_dir_all(&prefix).unwrap();
            let server = root.path().join("runtime/files/bin/wineserver");
            std::fs::create_dir_all(server.parent().unwrap()).unwrap();
            let pids = root.path().join("game-pids");
            let calls = root.path().join("calls");
            std::fs::write(
                &server,
                format!(
                    "#!/bin/sh\necho \"$WINEPREFIX $*\" >> {calls}\n\
                     {kill}\nexit 0\n",
                    calls = calls.display(),
                    kill = kill.replace("PIDS", &pids.display().to_string())
                ),
            )
            .unwrap();
            std::fs::set_permissions(&server, std::os::unix::fs::PermissionsExt::from_mode(0o755))
                .unwrap();
            Fixture {
                root,
                prefix,
                server,
                children: Vec::new(),
            }
        }

        fn game_in(&mut self, prefix: &Path) -> usize {
            let child = fake_game(prefix);
            let mut pids =
                std::fs::read_to_string(self.root.path().join("game-pids")).unwrap_or_default();
            if prefix == self.prefix {
                pids.push_str(&format!("{}\n", child.id()));
            }
            std::fs::write(self.root.path().join("game-pids"), pids).unwrap();
            self.children.push(child);
            self.children.len() - 1
        }

        fn calls(&self) -> String {
            std::fs::read_to_string(self.root.path().join("calls")).unwrap_or_default()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            for child in &mut self.children {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    /// `wineserver -k` ends the title, as it does for whatever server holds
    /// the prefix: nothing is sent SIGKILL after it, and with no `wine` to ask
    /// with there is no wait first. A title in another prefix is untouched.
    #[test]
    fn close_running_leaves_it_to_wineserver_when_that_works() {
        let mut fixture = Fixture::new("xargs -r kill -TERM < PIDS");
        let game = fixture.game_in(&fixture.prefix.clone());
        let sibling = fixture.root.path().join("other-proton/pfx");
        let other = fixture.game_in(&sibling);
        let started = Instant::now();
        assert!(close_running_within(
            &fixture.server,
            &fixture.prefix,
            Duration::from_secs(30)
        ));
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "it waited for a title nothing asked to close"
        );
        assert_eq!(ended_by(&mut fixture.children[game]), Some(libc::SIGTERM));
        assert_eq!(
            fixture.calls(),
            format!("{} -k\n", fixture.prefix.display())
        );
        assert!(
            fixture.children[other].try_wait().unwrap().is_none(),
            "a title in another prefix was ended"
        );
    }

    /// When the prefix's server is gone and its processes are not, `-k` ends
    /// nothing: what was in the prefix is sent SIGKILL, and only that.
    #[test]
    fn close_running_ends_what_wineserver_left_and_nothing_else() {
        let mut fixture = Fixture::new(":");
        let game = fixture.game_in(&fixture.prefix.clone());
        let sibling = fixture.root.path().join("other-proton/pfx");
        let other = fixture.game_in(&sibling);
        assert!(close_running_within(
            &fixture.server,
            &fixture.prefix,
            Duration::from_secs(30)
        ));
        assert_eq!(ended_by(&mut fixture.children[game]), Some(libc::SIGKILL));
        assert!(
            fixture.children[other].try_wait().unwrap().is_none(),
            "a title in another prefix was ended"
        );
    }

    /// With a `wine` to ask with, the title gets its full wait even when the
    /// asking did not end it, before `-k`.
    #[test]
    fn close_running_waits_for_a_title_it_asked() {
        let mut fixture = Fixture::new("xargs -r kill -TERM < PIDS");
        let wine = fixture.server.with_file_name("wine");
        std::fs::write(&wine, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&wine, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let game = fixture.game_in(&fixture.prefix.clone());
        let started = Instant::now();
        assert!(close_running_within(
            &fixture.server,
            &fixture.prefix,
            Duration::from_secs(2)
        ));
        assert!(started.elapsed() >= Duration::from_secs(2));
        assert_eq!(ended_by(&mut fixture.children[game]), Some(libc::SIGTERM));
    }

    /// Against a real Wine, opt-in: `FERESTRE_STOP_WINESERVER` names a
    /// runtime's wineserver and `FERESTRE_STOP_PREFIX` a prefix in which a
    /// windowed program that closes on WM_CLOSE (notepad) is running.
    #[test]
    #[ignore]
    fn a_running_wine_program_closes_when_asked() {
        let var = |name: &str| PathBuf::from(std::env::var_os(name).expect(name));
        let (server, prefix) = (var("FERESTRE_STOP_WINESERVER"), var("FERESTRE_STOP_PREFIX"));
        let uid = own_uid();
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
