//! A cancellation pipe survives a close request racing the worker's spawn.
use std::io::Write;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct DownloadControl(Arc<Mutex<State>>);

#[derive(Default)]
struct State {
    requested: bool,
    pipe: Option<ChildStdin>,
}

impl DownloadControl {
    pub fn requested(&self) -> bool {
        self.0.lock().unwrap().requested
    }

    pub fn stop(&self) {
        let mut state = self.0.lock().unwrap();
        state.requested = true;
        if let Some(mut pipe) = state.pipe.take() {
            let _ = pipe.write_all(b"x");
        }
    }

    pub fn spawn(&self, command: &mut Command) -> std::io::Result<Child> {
        let mut child = command
            .env("FERESTRE_GUI_CANCELLATION", "1")
            .stdin(Stdio::piped())
            .spawn()?;
        let mut state = self.0.lock().unwrap();
        state.pipe = child.stdin.take();
        if state.requested {
            if let Some(mut pipe) = state.pipe.take() {
                let _ = pipe.write_all(b"x");
            }
        }
        Ok(child)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_reaches_a_child_even_before_it_has_spawned() {
        let control = DownloadControl::default();
        control.stop();
        let mut command = Command::new("cat");
        command.stdout(Stdio::piped());
        let child = control.spawn(&mut command).unwrap();
        assert_eq!(child.wait_with_output().unwrap().stdout, b"x");
    }

    #[test]
    fn stop_closes_the_pipe_after_delivering_cancellation() {
        let control = DownloadControl::default();
        let mut command = Command::new("cat");
        command.stdout(Stdio::piped());
        let child = control.spawn(&mut command).unwrap();
        control.stop();
        control.stop();
        assert_eq!(child.wait_with_output().unwrap().stdout, b"x");
    }
}
