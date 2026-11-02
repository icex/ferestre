//! Application/runtime updates use their own background check and global banner.
use super::*;
use ferestre_core::updater;
use serde::Deserialize;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct State {
    original_image: Option<String>,
    checking: bool,
    check_again: bool,
    updating: bool,
    next_check: Option<Instant>,
    status: Option<updater::Status>,
    restart: Option<updater::AppliedAppImage>,
}

impl State {
    pub fn capture() -> Self {
        let original_image =
            updater::current_image().and_then(|p| updater::image_fingerprint(&p).ok());
        Self {
            original_image,
            ..Self::default()
        }
    }
}

/// A separate runtime install may finish while a release check is in flight.
/// Discard that stale snapshot and ask again after the installed state changed.
pub fn invalidate(ui: &Ui) {
    let mut state = ui.app_updates.borrow_mut();
    state.next_check = None;
    if state.checking {
        state.check_again = true;
        return;
    }
    drop(state);
    refresh(ui, false);
}

pub fn refresh(ui: &Ui, forced: bool) {
    {
        let mut state = ui.app_updates.borrow_mut();
        if state.checking || state.updating || state.restart.is_some() || *ui.closing.borrow() {
            return;
        }
        if !forced && state.next_check.is_some_and(|next| Instant::now() < next) {
            return;
        }
        state.checking = true;
    }
    let program = cli_binary();
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || -> Result<updater::Status, String> {
            let output = deadline::output(
                Command::new(program).args(["check-app-updates", "--json"]),
                deadline::CLIENT_LISTING,
            )
            .map_err(|e| e.to_string())?;
            if !output.status.success() {
                return Err(message(&output.stderr));
            }
            let status: updater::Status =
                serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
            if status.schema != 1 {
                return Err("Unsupported update response".into());
            }
            Ok(status)
        })
        .await;
        let mut state = ui.app_updates.borrow_mut();
        state.checking = false;
        if state.check_again {
            state.check_again = false;
            state.next_check = None;
            drop(state);
            refresh(&ui, false);
            return;
        }
        match result {
            Ok(Ok(status)) => {
                state.next_check = Some(Instant::now() + Duration::from_secs(6 * 60 * 60));
                if forced && !status.available() {
                    ui.toasts.add_toast(toast("Ferestre is up to date"));
                }
                state.status = Some(status);
            }
            _ => {
                state.next_check = Some(Instant::now() + Duration::from_secs(15 * 60));
                if forced {
                    ui.toasts.add_toast(toast(
                        "Could not check for Ferestre updates. Try again when online.",
                    ));
                }
            }
        }
        drop(state);
        render(&ui);
    });
}

pub fn render(ui: &Ui) {
    let state = ui.app_updates.borrow();
    if state.restart.is_some() {
        ui.update_banner
            .set_title("Ferestre updated. Restarting when current work finishes.");
        ui.update_banner.set_button_label(None);
        ui.update_banner.set_visible(true);
        ui.update_banner.set_revealed(true);
        return;
    }
    let available = state.status.as_ref().filter(|s| s.available());
    if let Some(status) = available {
        let title = if status.actionable() {
            status.title()
        } else {
            format!(
                "{}. Update Ferestre with your package manager.",
                status.title()
            )
        };
        ui.update_banner.set_title(&title);
        ui.update_banner
            .set_button_label(status.actionable().then_some("Update"));
        ui.update_banner.set_sensitive(
            !state.updating && ui.busy.borrow().is_none() && ui.running.borrow().is_empty(),
        );
        ui.update_banner.set_visible(true);
        ui.update_banner.set_revealed(true);
    } else {
        ui.update_banner.set_revealed(false);
        ui.update_banner.set_visible(false);
    }
}

#[derive(Deserialize)]
struct Outcome {
    schema: u32,
    updated: Vec<String>,
    appimage: Option<updater::AppliedAppImage>,
}

fn message(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("The update did not finish; the previous Ferestre was kept")
        .chars()
        .take(500)
        .collect()
}

pub fn update(ui: &Ui) {
    if !ui.running.borrow().is_empty() {
        ui.toasts
            .add_toast(toast("Close running games before updating Ferestre"));
        return;
    }
    if ui.app_updates.borrow().restart.is_some() {
        maybe_restart(ui);
        return;
    }
    let (minimum_launcher, minimum_runtime, expected_image) = {
        let state = ui.app_updates.borrow();
        let Some(status) = &state.status else {
            return;
        };
        let app = status
            .launcher
            .as_ref()
            .filter(|_| status.appimage.is_some());
        if app.is_some() && state.original_image.is_none() {
            ui.toasts
                .add_toast(toast("Restart Ferestre before updating this AppImage"));
            return;
        }
        (
            app.map(|r| r.version.clone()),
            status.runtime.as_ref().map(|r| r.version.clone()),
            app.and(state.original_image.clone()),
        )
    };
    if !begin(ui, "Updating Ferestre and runtime") {
        report_busy(ui);
        return;
    }
    ui.app_updates.borrow_mut().updating = true;
    *ui.wait_on_close.borrow_mut() = true;
    let control = cancel::DownloadControl::default();
    *ui.download_control.borrow_mut() = Some(control.clone());
    ui.download
        .start("application-update", "Ferestre and runtime", "Updating");
    super::render(ui);
    let ticker = ui.clone();
    let tick = glib::timeout_add_local(Duration::from_millis(250), move || {
        ticker.download.bar.pulse();
        ticker
            .download
            .detail
            .set_label("Downloading and verifying signed updates");
        glib::ControlFlow::Continue
    });
    let program = cli_binary();
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || -> Result<Outcome, String> {
            let mut command = Command::new(program);
            command
                .args(["self-update", "--json"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if let Some(version) = minimum_launcher {
                command.args(["--minimum-launcher", &version]);
            }
            if let Some(version) = minimum_runtime {
                command.args(["--minimum-runtime", &version]);
            }
            if let Some(digest) = expected_image {
                command.args(["--expected-appimage-sha256", &digest]);
            }
            let child = control.spawn(&mut command).map_err(|e| e.to_string())?;
            let output = child.wait_with_output().map_err(|e| e.to_string())?;
            if !output.status.success() {
                return Err(message(&output.stderr));
            }
            let result: Outcome =
                serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
            if result.schema != 1 {
                return Err("Unsupported update response".into());
            }
            Ok(result)
        })
        .await;
        tick.remove();
        ui.download.stop();
        let stopped = ui
            .download_control
            .borrow()
            .as_ref()
            .is_some_and(|c| c.requested());
        finish(&ui);
        ui.app_updates.borrow_mut().updating = false;
        match result {
            Ok(Ok(outcome)) => {
                ui.model.borrow_mut().reload();
                ui.app_updates.borrow_mut().status = None;
                ui.app_updates.borrow_mut().restart = outcome.appimage;
                if !outcome.updated.is_empty() {
                    ui.toasts
                        .add_toast(toast(format!("Updated {}", outcome.updated.join(" and "))));
                }
                if ui.app_updates.borrow().restart.is_some() {
                    let restarting = ui.clone();
                    glib::timeout_add_seconds_local(1, move || {
                        maybe_restart(&restarting);
                        if restarting.app_updates.borrow().restart.is_some() {
                            glib::ControlFlow::Continue
                        } else {
                            glib::ControlFlow::Break
                        }
                    });
                } else {
                    refresh(&ui, true);
                }
            }
            _ if stopped => {
                ui.toasts
                    .add_toast(toast("Update stopped. The previous Ferestre was kept."));
            }
            Ok(Err(error)) => {
                ui.toasts
                    .add_toast(toast(format!("Update failed: {error}")));
            }
            Err(_) => {
                ui.toasts
                    .add_toast(toast("The update could not be started"));
            }
        }
        super::render(&ui);
    });
}

fn maybe_restart(ui: &Ui) {
    if *ui.closing.borrow() || ui.busy.borrow().is_some() || !ui.running.borrow().is_empty() {
        return;
    }
    let Some(applied) = ui.app_updates.borrow_mut().restart.take() else {
        return;
    };
    // Recheck the installed bytes before exec. A later independent update
    // must not be executed or rolled back using this operation's receipt.
    if let Err(error) = updater::verify_checksum(&applied.target, &applied.installed_sha256) {
        ui.toasts
            .add_toast(toast(format!("Ferestre changed before restart: {error}")));
        render(ui);
        return;
    }
    let error = updater::image_command(&applied.target).exec();
    let restored = updater::rollback(&applied);
    ui.toasts.add_toast(toast(match restored {
        Ok(()) => {
            format!("Could not restart Ferestre ({error}). The previous version was restored.")
        }
        Err(rollback) => format!(
            "Could not restart Ferestre ({error}). Previous copy: {}. {rollback}",
            applied.backup.display()
        ),
    }));
    render(ui);
}
