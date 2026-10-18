//! "Playing <game>" for as long as a launch runs.
//!
//! Every launch goes through `ferestre run`: the window's Play button, a Steam
//! shortcut and a terminal alike. So this is the one place a presence runner
//! is started, and it lives exactly as long as the game. Closing the window
//! does not end a game, so it does not end what friends see either.
//!
//! The runner is the client's `presence run`, a child holding the Player's
//! token. Its stdin is the switch: closed, it tells Xbox the game has stopped
//! and exits. Its JSON lines are passed on to this launch's stderr behind
//! `presence::RELAY_PREFIX`, which is how the window follows a game it
//! started without starting a second runner of its own.
//!
//! The window can change its mind while the game runs, with two signals to
//! this process (never to its group, which holds the game):
//! - SIGUSR1 stops the runner, for "Show me online" switched off or the
//!   Player signing out. A sign-out leaves the saved preference on, so this
//!   one does not read it.
//! - SIGUSR2 makes the runner follow the saved preference: one is started if
//!   it is on and none is up, and the one up is stopped if it is off. A
//!   runner still telling Xbox goodbye counts as none: the new one starts as
//!   soon as it has gone, so switching off and on quickly ends up on.
//!
//! Neither is decided from what the window last heard, which can be a moment
//! behind. Two signals close together leave the later one, which is the
//! later choice.
//!
//! The handlers are in place before the first relayed line, and the window
//! signals only a launch it has heard from, so neither signal can arrive to
//! the default action, which would end the launch.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use ferestre_core::accounts::{Profiles, Selection};
use ferestre_core::presence::{self, Disabled, Event};

/// How long the runner gets to say goodbye to Xbox once its stdin closes. The
/// client gives its own request 5 s, timed from after it has noticed; this
/// leaves it room to finish rather than be killed halfway.
const GOODBYE: Duration = Duration::from_secs(7);
const POLL: Duration = Duration::from_millis(200);

static REQUEST: AtomicI32 = AtomicI32::new(0);

extern "C" fn on_presence_signal(signal: libc::c_int) {
    REQUEST.store(signal, Ordering::SeqCst);
}

/// What a runner needs, once the client has said it can do this at all.
pub struct Setup {
    client: PathBuf,
    config_dir: PathBuf,
    player: String,
    signed_in: bool,
    game_dir: PathBuf,
}

/// Decide whether this launch can show presence. `None` is the quiet case:
/// the client does not have `presence-v1`, or it could not be asked. Nothing
/// is printed for it, so older clients launch exactly as before.
pub fn setup(
    client: &Path,
    config_dir: &Path,
    roles: Option<&Selection>,
    profiles: Option<&Profiles>,
    game_dir: &Path,
) -> Option<Setup> {
    let profiles = profiles.filter(|p| presence::supported(p))?;
    let player = presence::player(profiles, roles)?;
    Some(Setup {
        client: client.to_path_buf(),
        config_dir: config_dir.to_path_buf(),
        player: player.id.clone(),
        signed_in: player.signed_in,
        game_dir: game_dir.to_path_buf(),
    })
}

/// Pass one event on. Errors are ignored: the window that read this stderr may
/// have closed, and the game must not stop because of it.
fn relay(event: &Event) {
    let _ = writeln!(std::io::stderr(), "{}", presence::relay_line(event));
}

struct Running {
    child: Child,
    stdin: Option<ChildStdin>,
    /// Says when the stdout reader has passed on everything.
    drained: mpsc::Receiver<()>,
}

impl Running {
    fn spawn(setup: &Setup) -> Option<Running> {
        let mut command = presence::run_command(&setup.client, &setup.player, &setup.game_dir);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(e) => {
                eprintln!("-- online status: could not start the client: {e}");
                return None;
            }
        };
        relay(&Event::Starting {
            player: Some(setup.player.clone()),
        });
        let stdin = child.stdin.take();
        let stdout = child.stdout.take()?;
        let (done, drained) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Some(event) = presence::parse_event(&line) {
                    relay(&event);
                }
            }
            let _ = done.send(());
        });
        Some(Running {
            child,
            stdin,
            drained,
        })
    }

    /// Close stdin: the runner says goodbye to Xbox and exits by itself.
    fn ask_to_stop(&mut self) {
        self.stdin = None;
    }

    /// Whether it has been asked to stop and is on its way out.
    fn stopping(&self) -> bool {
        self.stdin.is_none()
    }

    fn exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }

    /// Wait for the last lines, then say the runner is gone.
    fn finish(self) {
        let _ = self.drained.recv_timeout(Duration::from_secs(1));
        relay(&Event::Ended);
    }

    fn stop(mut self) {
        self.ask_to_stop();
        crate::wait_at_most(&mut self.child, GOODBYE);
        self.finish();
    }
}

/// The runner for one launch, stopped when this is dropped.
pub struct Presence {
    done: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Presence {
    pub fn inert() -> Self {
        Self {
            done: Arc::new(AtomicBool::new(true)),
            worker: None,
        }
    }

    /// Start the runner if the Player wants one. Called once the game is about
    /// to start, so a launch refused before then never shows anybody online.
    pub fn start(setup: Option<Setup>) -> Self {
        let Some(setup) = setup else {
            return Self::inert();
        };
        for signal in [libc::SIGUSR1, libc::SIGUSR2] {
            // Safety: a handler that only stores to an atomic, installed with a
            // fully initialised sigaction.
            unsafe {
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = on_presence_signal as extern "C" fn(libc::c_int) as usize;
                action.sa_flags = libc::SA_RESTART;
                libc::sigemptyset(&mut action.sa_mask);
                if libc::sigaction(signal, &action, std::ptr::null_mut()) != 0 {
                    // Without the handler a signal from the window would end
                    // the launch, so go without presence rather than risk it.
                    return Self::inert();
                }
            }
        }
        REQUEST.store(0, Ordering::SeqCst);
        let mut running = begin(&setup);
        let done = Arc::new(AtomicBool::new(false));
        let finished = Arc::clone(&done);
        let worker = std::thread::spawn(move || {
            // Asked for a runner once the current one has gone. A runner that
            // ends by itself, on a fatal error or `busy`, is not restarted.
            let mut start = false;
            while !finished.load(Ordering::SeqCst) {
                match REQUEST.swap(0, Ordering::SeqCst) {
                    libc::SIGUSR1 => {
                        start = false;
                        if let Some(runner) = running.as_mut() {
                            runner.ask_to_stop();
                        }
                    }
                    libc::SIGUSR2 => match running.as_mut() {
                        Some(runner) if !runner.stopping() => {
                            if !wanted(&setup) {
                                runner.ask_to_stop();
                            }
                        }
                        // `begin` reads the preference again when it is
                        // time, and says why if there is still nothing to do.
                        _ => start = true,
                    },
                    _ => {}
                }
                if running.as_mut().is_some_and(Running::exited) {
                    if let Some(runner) = running.take() {
                        runner.finish();
                    }
                }
                if start && running.is_none() {
                    start = false;
                    running = begin(&setup);
                }
                std::thread::sleep(POLL);
            }
            if let Some(runner) = running.take() {
                runner.stop();
            }
        });
        Self {
            done,
            worker: Some(worker),
        }
    }
}

/// Whether the Player wants a runner now.
fn wanted(setup: &Setup) -> bool {
    setup.signed_in && presence::sharing(&setup.config_dir, &setup.player)
}

/// Start a runner, or say why there is none.
fn begin(setup: &Setup) -> Option<Running> {
    let reason = if !setup.signed_in {
        Disabled::SignedOut
    } else if !presence::sharing(&setup.config_dir, &setup.player) {
        Disabled::Off
    } else {
        return Running::spawn(setup);
    };
    relay(&Event::Disabled {
        reason,
        player: Some(setup.player.clone()),
    });
    None
}

impl Drop for Presence {
    fn drop(&mut self) {
        self.done.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: &str = "11111111111111111111111111111111";
    const PLAYER: &str = "22222222222222222222222222222222";

    fn profiles(capabilities: &[&str]) -> Profiles {
        let json = serde_json::json!({"schema":1,"capabilities":capabilities,"default_profile":OWNER,
            "profiles":[{"id":OWNER,"label":"Owner","signed_in":true,"legacy_saves":true,"save_namespace":"a".repeat(64)},
                        {"id":PLAYER,"label":"Player","signed_in":true,"legacy_saves":false,"save_namespace":"b".repeat(64)}]});
        ferestre_core::accounts::parse_profiles(&serde_json::to_vec(&json).unwrap()).unwrap()
    }

    #[test]
    fn only_a_client_with_presence_gets_a_runner_and_it_is_the_players() {
        let (client, config, game) = (Path::new("/c"), Path::new("/cfg"), Path::new("/g"));
        let old = profiles(&["store-player-accounts"]);
        assert!(setup(client, config, None, Some(&old), game).is_none());
        assert!(setup(client, config, None, None, game).is_none());
        let new = profiles(&["store-player-accounts", presence::CLIENT_CAPABILITY]);
        assert_eq!(
            setup(client, config, None, Some(&new), game)
                .unwrap()
                .player,
            OWNER
        );
        let roles = Selection {
            store: OWNER.into(),
            player: PLAYER.into(),
            generation: 1,
        };
        assert_eq!(
            setup(client, config, Some(&roles), Some(&new), game)
                .unwrap()
                .player,
            PLAYER
        );
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("ferestre-presence-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The signals go to the whole test process, and the request they leave
    /// is one static, so the tests that send them take turns.
    static SIGNALS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !ready() {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for {what}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn signal(which: libc::c_int) {
        // Safety: this process, with a handler `Presence::start` installed.
        unsafe { libc::kill(libc::getpid(), which) };
    }

    /// A client whose runner logs its argv, says it is online, and on stdin
    /// EOF logs "closed", takes `goodbye` seconds telling Xbox, then logs
    /// "gone" and exits.
    struct Fixture {
        root: Scratch,
        log: PathBuf,
        config: PathBuf,
    }

    impl Fixture {
        fn new(name: &str, goodbye: &str) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let root = Scratch::new(name);
            let log = root.path().join("runner.log");
            let client = root.path().join("xodus-cli");
            std::fs::write(
                &client,
                format!(
                    "#!/bin/sh\necho \"$*\" >> '{log}'\n\
                     echo '{{\"schema\":1,\"event\":\"online\",\"title\":\"Fixture\",\"heartbeat_after\":60}}'\n\
                     cat > /dev/null\necho closed >> '{log}'\nsleep {goodbye}\n\
                     echo '{{\"event\":\"offline\"}}'\necho gone >> '{log}'\n",
                    log = log.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&client, std::fs::Permissions::from_mode(0o755)).unwrap();
            let config = root.path().join("config");
            Self { root, log, config }
        }

        fn setup(&self) -> Setup {
            Setup {
                client: self.root.path().join("xodus-cli"),
                config_dir: self.config.clone(),
                player: PLAYER.into(),
                signed_in: true,
                game_dir: self.root.path().join("game"),
            }
        }

        fn lines(&self) -> Vec<String> {
            std::fs::read_to_string(&self.log)
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        }

        fn count(&self, what: &str) -> usize {
            self.lines().iter().filter(|l| l.contains(what)).count()
        }

        fn share(&self, on: bool) {
            presence::set_sharing(&self.config, PLAYER, on).unwrap();
        }
    }

    #[test]
    fn the_runner_follows_the_launch_and_the_windows_signals() {
        let _turn = SIGNALS.lock().unwrap_or_else(|e| e.into_inner());
        let fixture = Fixture::new("follow", "0");
        let launch = Presence::start(Some(fixture.setup()));
        wait_until("the first runner", || fixture.count("presence run") == 1);
        assert!(fixture.lines()[0].starts_with(&format!(
            "--profile {PLAYER} presence run --json --game-dir "
        )));
        // Switched off in the window: the runner's stdin closes.
        fixture.share(false);
        signal(libc::SIGUSR1);
        wait_until("the runner to be stopped", || fixture.count("gone") == 1);
        // Asked to follow a preference that is off: nothing.
        signal(libc::SIGUSR2);
        std::thread::sleep(POLL * 3);
        assert_eq!(fixture.count("presence run"), 1);
        fixture.share(true);
        signal(libc::SIGUSR2);
        wait_until("a second runner", || fixture.count("presence run") == 2);
        // Off again, told only to follow the preference: it stops.
        fixture.share(false);
        signal(libc::SIGUSR2);
        wait_until("the second runner to stop", || fixture.count("gone") == 2);
        // On and straight back off: the later choice holds. A runner may have
        // started in between, but none is left up.
        fixture.share(true);
        signal(libc::SIGUSR2);
        fixture.share(false);
        signal(libc::SIGUSR1);
        std::thread::sleep(POLL * 3);
        wait_until("no runner left up", || {
            fixture.count("presence run") == fixture.count("gone")
        });
        // The game exits with a runner up: dropping the launch closes its
        // stdin and waits for it.
        let runs = fixture.count("presence run");
        fixture.share(true);
        signal(libc::SIGUSR2);
        wait_until("another runner", || {
            fixture.count("presence run") == runs + 1
        });
        drop(launch);
        assert_eq!(fixture.count("gone"), runs + 1);
    }

    /// A runner that never finishes its goodbye does not keep the launch
    /// open: it gets `GOODBYE`, and is then killed.
    #[test]
    fn a_runner_that_ignores_its_stdin_closing_is_not_waited_for_forever() {
        use std::os::unix::fs::PermissionsExt;
        let _turn = SIGNALS.lock().unwrap_or_else(|e| e.into_inner());
        let fixture = Fixture::new("stubborn", "0");
        let pid = fixture.root.path().join("runner.pid");
        // `exec`, so the process killed is the one holding the pipes.
        std::fs::write(
            fixture.root.path().join("xodus-cli"),
            format!(
                "#!/bin/sh\necho \"$*\" >> '{log}'\necho $$ > '{pid}'\n\
                 echo '{{\"schema\":1,\"event\":\"online\",\"title\":\"Fixture\",\"heartbeat_after\":60}}'\n\
                 exec sleep 60\n",
                log = fixture.log.display(),
                pid = pid.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(
            fixture.root.path().join("xodus-cli"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let launch = Presence::start(Some(fixture.setup()));
        wait_until("the runner", || {
            pid.exists() && fixture.count("presence run") == 1
        });
        let runner: i32 = std::fs::read_to_string(&pid)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let started = std::time::Instant::now();
        drop(launch);
        let elapsed = started.elapsed();
        // Safety: signal 0 only asks whether the runner is still there.
        let gone = unsafe { libc::kill(runner, 0) } != 0;
        if !gone {
            unsafe { libc::kill(runner, libc::SIGKILL) };
        }
        assert!(gone, "the runner was left running");
        assert!(elapsed >= GOODBYE, "{elapsed:?}");
        assert!(elapsed < GOODBYE + Duration::from_secs(3), "{elapsed:?}");
    }

    #[test]
    fn off_and_on_again_while_the_runner_says_goodbye_ends_up_on() {
        let _turn = SIGNALS.lock().unwrap_or_else(|e| e.into_inner());
        let fixture = Fixture::new("goodbye", "1");
        let launch = Presence::start(Some(fixture.setup()));
        wait_until("the first runner", || fixture.count("presence run") == 1);
        fixture.share(false);
        signal(libc::SIGUSR1);
        wait_until("the runner to be told", || fixture.count("closed") == 1);
        // Switched on again while it is still telling Xbox goodbye.
        std::thread::sleep(Duration::from_millis(300));
        fixture.share(true);
        signal(libc::SIGUSR2);
        wait_until("a runner after the first has gone", || {
            fixture.count("presence run") == 2
        });
        let lines = fixture.lines();
        let gone = lines.iter().position(|l| l == "gone").unwrap();
        let second = lines
            .iter()
            .rposition(|l| l.contains("presence run"))
            .unwrap();
        assert!(gone < second, "two runners at once: {lines:?}");
        drop(launch);
        assert_eq!(fixture.count("gone"), 2);
    }
}
