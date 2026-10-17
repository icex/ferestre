//! Cooperative cancellation from the window, without killing the launcher
//! before it has waited for its downloader and finished bookkeeping.
use std::io::Read;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Default)]
pub struct Cancellation {
    requested: Arc<AtomicBool>,
    managed: bool,
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
                // EOF also stops work if the window unexpectedly disappears.
                let _ = std::io::stdin().read(&mut byte);
                reader.requested.store(true, Ordering::Relaxed);
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
        if !self.managed {
            return super::run_inherited(command);
        }
        self.check()?;
        // The download and its helpers belong to their own process group.
        // The launcher stays alive to wait and clean up after cancellation.
        command.process_group(0).stdin(std::process::Stdio::null());
        let mut child = command.spawn()?;
        let mut signalled = false;
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok(super::exit_code_from(
                    status.code(),
                    super::signal_of(&status),
                ));
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
