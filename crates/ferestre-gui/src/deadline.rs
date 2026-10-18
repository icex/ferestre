//! Running a child process with a time limit.
//!
//! The window asks the Xbox client things in the background: who is signed in,
//! what the account owns, whether an update is waiting. The client builds its
//! HTTP client with no timeout and may wait on a keyring prompt, so a stalled
//! request -- a half-open connection after a suspend, a captive portal, an
//! unlock prompt nobody answers -- used to keep a task waiting until the window
//! was restarted, and every task that shares the window's busy flag with it
//! refused to start. Each of those children now gets a deadline, after which it
//! is killed and the call reports that it timed out.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a background question to the client may take. Generous, because a
/// slow answer is still an answer, and short enough that a stuck one ends.
pub const CLIENT_QUESTION: Duration = Duration::from_secs(60);
/// A whole-library listing pages through collections and can take longer.
pub const CLIENT_LISTING: Duration = Duration::from_secs(120);

/// How long to wait for a pipe to reach end of file once the child has exited.
/// Something it started can keep the pipe open for good; what was read by then
/// is what the caller gets.
const DRAIN: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(50);

fn timed_out(limit: Duration) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        format!("no answer within {} seconds", limit.as_secs()),
    )
}

/// Spawn in a process group of its own, so a timeout ends whatever the child
/// started as well as the child.
fn spawn(command: &mut Command) -> std::io::Result<Child> {
    command.stdin(Stdio::null()).process_group(0).spawn()
}

/// Wait for `child` until `deadline`, killing its whole group when it passes.
fn wait_until(
    child: &mut Child,
    deadline: Instant,
    limit: Duration,
) -> std::io::Result<ExitStatus> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            // Safety: a signal to the process group this call created.
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
            let _ = child.kill();
            let _ = child.wait();
            return Err(timed_out(limit));
        }
        std::thread::sleep(POLL);
    }
}

/// What a pipe has delivered so far, and a signal for when it reaches the end.
struct Pipe {
    bytes: Arc<Mutex<Vec<u8>>>,
    done: mpsc::Receiver<()>,
}

impl Pipe {
    fn read(mut pipe: impl Read + Send + 'static) -> Self {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let (sender, done) = mpsc::channel();
        let sink = Arc::clone(&bytes);
        std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            while let Ok(n) = pipe.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                sink.lock().unwrap().extend_from_slice(&chunk[..n]);
            }
            let _ = sender.send(());
        });
        Pipe { bytes, done }
    }

    /// Everything read, once the pipe ends or `by` passes.
    fn collect(self, by: Instant) -> Vec<u8> {
        let _ = self
            .done
            .recv_timeout(by.saturating_duration_since(Instant::now()));
        let bytes = std::mem::take(&mut *self.bytes.lock().unwrap());
        bytes
    }
}

/// Like [`Command::output`], with a deadline. Standard input is closed.
pub fn output(command: &mut Command, limit: Duration) -> std::io::Result<Output> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = spawn(command)?;
    let stdout = child.stdout.take().map(Pipe::read);
    let stderr = child.stderr.take().map(Pipe::read);
    let status = wait_until(&mut child, Instant::now() + limit, limit)?;
    let drained = Instant::now() + DRAIN;
    Ok(Output {
        status,
        stdout: stdout.map(|p| p.collect(drained)).unwrap_or_default(),
        stderr: stderr.map(|p| p.collect(drained)).unwrap_or_default(),
    })
}

/// Like [`Command::status`], with a deadline. Standard input is closed; the
/// caller decides where output goes.
pub fn status(command: &mut Command, limit: Duration) -> std::io::Result<ExitStatus> {
    let mut child = spawn(command)?;
    wait_until(&mut child, Instant::now() + limit, limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn an_answer_in_time_is_returned_whole() {
        let output = output(
            &mut sh("echo out; echo err >&2; exit 3"),
            Duration::from_secs(10),
        )
        .expect("runs");
        assert_eq!(output.stdout, b"out\n");
        assert_eq!(output.stderr, b"err\n");
        assert_eq!(output.status.code(), Some(3));
    }

    #[test]
    fn a_child_that_never_answers_is_killed_at_the_deadline() {
        let started = Instant::now();
        let error = output(&mut sh("sleep 30"), Duration::from_millis(300)).expect_err("times out");
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );

        let error = status(&mut sh("sleep 30"), Duration::from_millis(300)).expect_err("times out");
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    }

    #[test]
    fn a_grandchild_holding_the_pipe_does_not_hold_the_caller() {
        let started = Instant::now();
        let output =
            output(&mut sh("sleep 30 & echo done"), Duration::from_secs(10)).expect("runs");
        assert_eq!(output.stdout, b"done\n");
        assert!(output.status.success());
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_timeout_ends_what_the_child_started_too() {
        let marker =
            std::env::temp_dir().join(format!("ferestre-gui-deadline-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let script = format!("(sleep 1; touch {}) & sleep 30", marker.display());
        let _ = status(&mut sh(&script), Duration::from_millis(200));
        std::thread::sleep(Duration::from_millis(1500));
        assert!(!marker.exists(), "the grandchild outlived the timeout");
    }
}
