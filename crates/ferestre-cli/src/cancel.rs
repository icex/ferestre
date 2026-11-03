//! Cooperative cancellation from the window, without killing the launcher
//! before it has waited for its downloader and finished bookkeeping.
use std::io::{Read, Write};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Default)]
pub struct Cancellation {
    requested: Arc<AtomicBool>,
    managed: bool,
    paused: Arc<AtomicBool>,
}

impl Cancellation {
    pub fn from_gui() -> Self {
        let token = Self {
            managed: std::env::var_os("FERESTRE_GUI_CANCELLATION").is_some(),
            ..Self::default()
        };
        if token.managed {
            let reader = token.clone();
            std::thread::spawn(move || {
                let mut byte = [0];
                loop {
                    match std::io::stdin().read(&mut byte) {
                        Ok(1) if byte[0] == b'p' => reader.paused.store(true, Ordering::Relaxed),
                        Ok(1) if byte[0] == b'r' => reader.paused.store(false, Ordering::Relaxed),
                        _ => {
                            reader.requested.store(true, Ordering::Relaxed);
                            break;
                        }
                    }
                }
            });
        }
        token
    }

    pub fn requested(&self) -> bool {
        self.requested.load(Ordering::Relaxed)
    }

    pub fn check(&self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.requested(), "Download stopped");
        Ok(())
    }

    pub fn run(&self, mut command: Command) -> anyhow::Result<u8> {
        use std::os::unix::process::CommandExt;
        let streaming = command
            .get_args()
            .next()
            .is_some_and(|arg| arg == "streaming");
        if !self.managed && !streaming {
            return super::run_inherited(command);
        }
        let diagnostics = streaming
            .then(|| {
                ferestre_core::paths::Paths::from_env()
                    .ok()
                    .map(|p| p.state_dir().to_path_buf())
            })
            .flatten();
        self.check()?;
        // The download and its helpers belong to their own process group.
        // The launcher stays alive to wait and clean up after cancellation.
        command.process_group(0);
        if streaming && self.managed {
            command
                .env("FERESTRE_STREAMING_CONTROL", "1")
                .stdin(std::process::Stdio::piped());
        } else {
            command.stdin(std::process::Stdio::null());
        }
        if streaming {
            command.stderr(std::process::Stdio::piped());
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                if streaming {
                    let diagnostic =
                        ferestre_core::diagnostics::Diagnostic::new("spawn", 0).unwrap();
                    if let Some(state) = &diagnostics {
                        let _ = ferestre_core::diagnostics::record(state, &diagnostic);
                    }
                    eprintln!("{}", diagnostic.line());
                }
                return Err(error.into());
            }
        };
        let mut last_paused = false;
        let reader = child.stderr.take().map(|stderr| {
            std::thread::spawn(move || {
                use std::io::BufRead;
                let mut input = std::io::BufReader::new(stderr);
                let mut last = None;
                let mut discard = false;
                loop {
                    let mut bytes = Vec::new();
                    let count = std::io::Read::take(std::io::Read::by_ref(&mut input), 4097)
                        .read_until(b'\n', &mut bytes)
                        .unwrap_or(0);
                    if count == 0 {
                        break;
                    }
                    let end = bytes.last() == Some(&b'\n');
                    if !discard && bytes.len() <= 4096 {
                        if let Ok(line) = std::str::from_utf8(&bytes) {
                            if let Some(diagnostic) =
                                ferestre_core::diagnostics::Diagnostic::parse(line.trim_end())
                            {
                                if let Some(state) = &diagnostics {
                                    let _ = ferestre_core::diagnostics::record(state, &diagnostic);
                                }
                                if last.as_ref().is_none_or(
                                    |old: &ferestre_core::diagnostics::Diagnostic| {
                                        diagnostic.priority() >= old.priority()
                                    },
                                ) {
                                    last = Some(diagnostic);
                                }
                            }
                        }
                    }
                    // Terminal output remains useful, but is never written to the
                    // persistent diagnostic log. The GUI consumes allowlisted records.
                    let _ = std::io::stderr().write_all(&bytes);
                    discard = !end;
                }
                last
            })
        });
        let mut signalled = false;
        loop {
            if let Some(status) = child.try_wait()? {
                let diagnostic = reader.and_then(|reader| reader.join().ok()).flatten();
                if !status.success() && !self.requested() {
                    let diagnostic = diagnostic.unwrap_or_else(|| {
                        ferestre_core::diagnostics::Diagnostic::new("backend-exit", 0).unwrap()
                    });
                    eprintln!("{}", diagnostic.line());
                    eprintln!("{}", diagnostic.message());
                }
                return Ok(super::exit_code_from(
                    status.code(),
                    super::signal_of(&status),
                ));
            }
            let paused = self.paused.load(Ordering::Relaxed);
            if paused != last_paused {
                if let Some(stdin) = child.stdin.as_mut() {
                    let command = if paused { "pause" } else { "resume" };
                    let _ = writeln!(stdin, "{{\"schema\":1,\"command\":\"{command}\"}}");
                    let _ = stdin.flush();
                }
                last_paused = paused;
            }
            if self.requested() && !signalled {
                // Safety: this is the process group created just above.
                let result = unsafe { libc::kill(-(child.id() as i32), libc::SIGTERM) };
                if result != 0 {
                    let err = std::io::Error::last_os_error();
                    if err.raw_os_error() != Some(libc::ESRCH) {
                        // Keep ownership and wait rather than leaving a live downloader.
                        eprintln!("-- could not stop downloader: {err}");
                    }
                }
                signalled = true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}
