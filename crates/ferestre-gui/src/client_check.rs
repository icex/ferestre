//! Telling someone the Xbox client cannot start, and installing what it needs.
//!
//! Without WebKitGTK the loader refuses the client outright, and the window
//! used to report that as "Sign-in exited 127" with no sign-in window at all.
//! The check runs once the window is up, and again after an install; a banner
//! stays up while the client cannot start, and signing in explains instead of
//! failing. See ferestre_core::client_deps for what is checked.
use super::*;
use ferestre_core::client_deps::Missing;

/// The libraries the client cannot load, and whether PackageKit is there to
/// install the package that provides them.
pub(super) struct Blocked {
    missing: Missing,
    packagekit: bool,
}

impl Blocked {
    /// The package to install from the window, when there is a way to.
    fn installable(&self) -> Option<&'static str> {
        self.missing.package().filter(|_| self.packagekit)
    }
}

/// Check the client off the main thread, then update the banner. `then` runs
/// once the client is known to be able to start. `installed` is set for the
/// check after an install, which says so when the install was not enough.
pub(super) fn check(ui: &Ui, then: Option<fn(&Ui)>, installed: bool) {
    let Some(client) = ui.model.borrow().xodus_cli().filter(|p| p.is_file()) else {
        return;
    };
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let checked = gio::spawn_blocking(move || ferestre_core::client_deps::check(&client))
            .await
            .unwrap_or_else(|_| Err(anyhow::anyhow!("the check panicked")));
        let blocked = match checked {
            Ok(None) => None,
            Ok(Some(missing)) => Some(Blocked {
                packagekit: missing.package().is_some() && packagekit::available().await,
                missing,
            }),
            // Not evidence either way, so the window carries on as before.
            Err(error) => {
                eprintln!("!! could not check the Xbox client's libraries: {error:#}");
                None
            }
        };
        let still = blocked
            .as_ref()
            .map(|blocked| blocked.missing.libraries.join(", "));
        *ui.client_blocked.borrow_mut() = blocked;
        update_banner(&ui);
        match still {
            None => {
                if let Some(then) = then {
                    then(&ui);
                }
            }
            Some(libraries) if installed => ui.toasts.add_toast(
                adw::Toast::builder()
                    .title(format!(
                        "The Xbox client still cannot start. Missing: {libraries}"
                    ))
                    .use_markup(false)
                    .timeout(15)
                    .build(),
            ),
            Some(_) => {}
        }
    });
}

fn update_banner(ui: &Ui) {
    let blocked = ui.client_blocked.borrow();
    let Some(blocked) = blocked.as_ref() else {
        // Hidden as well as unrevealed, so assistive technology does not
        // keep reading a problem that has gone.
        ui.banner.set_revealed(false);
        ui.banner.set_visible(false);
        return;
    };
    let title = if blocked.missing.needs_webkit() {
        "The Xbox client cannot start: WebKitGTK is missing".to_string()
    } else {
        format!(
            "The Xbox client cannot start: {} is missing",
            blocked.missing.libraries.join(", ")
        )
    };
    ui.banner.set_title(&title);
    ui.banner
        .set_button_label(Some(if blocked.installable().is_some() {
            "Install…"
        } else {
            "Details…"
        }));
    ui.banner.set_visible(true);
    ui.banner.set_revealed(true);
}

/// The banner's button: install straight away when PackageKit can, since the
/// password prompt that follows is the confirmation; otherwise explain.
pub(super) fn banner_pressed(ui: &Ui) {
    let installable = ui
        .client_blocked
        .borrow()
        .as_ref()
        .and_then(Blocked::installable);
    match installable {
        Some(package) => install(ui, package, None),
        None => explain(ui, None),
    }
}

/// When the client cannot start, say so instead of running it, and offer the
/// fix. `then` is what was asked for, and runs after a successful install.
pub(super) fn blocks(ui: &Ui, then: fn(&Ui)) -> bool {
    if ui.client_blocked.borrow().is_none() {
        return false;
    }
    explain(ui, Some(then));
    true
}

fn explain(ui: &Ui, then: Option<fn(&Ui)>) {
    let (body, command, installable) = {
        let blocked = ui.client_blocked.borrow();
        let Some(blocked) = blocked.as_ref() else {
            return;
        };
        let missing = &blocked.missing;
        let libraries = missing.libraries.join(", ");
        let mut body = if missing.needs_webkit() {
            format!(
                "Ferestre's Xbox client shows Microsoft's sign-in page through WebKitGTK, \
                 and this system does not have it, so the client cannot start at all. \
                 Missing: {libraries}."
            )
        } else {
            format!("The Xbox client cannot start. Missing: {libraries}.")
        };
        match (missing.command(), blocked.installable()) {
            (Some(_), Some(package)) => body.push_str(&format!(
                "\n\nInstall the {package} package now? Your system asks for your password."
            )),
            (Some(command), None) => body.push_str(&format!(
                "\n\nTo install it, run in a terminal:\n\n{command}"
            )),
            (None, _) => body.push_str(&format!("\n\nTo fix it: {}", missing.hint())),
        }
        (body, missing.command(), blocked.installable())
    };

    let dialog = adw::AlertDialog::new(Some("The Xbox client cannot start"), Some(&body));
    dialog.add_response("cancel", "Cancel");
    if command.is_some() {
        dialog.add_response("copy", "Copy Command");
    }
    if installable.is_some() {
        dialog.add_response("install", "Install");
        dialog.set_response_appearance("install", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("install"));
    }
    dialog.set_close_response("cancel");
    let window = ui.window.clone();
    let ui = ui.clone();
    dialog.connect_response(None, move |_, response| match response {
        "copy" => {
            if let Some(command) = &command {
                ui.window.clipboard().set_text(command);
                ui.toasts.add_toast(adw::Toast::new("Command copied"));
            }
        }
        "install" => {
            if let Some(package) = installable {
                install(&ui, package, then);
            }
        }
        _ => {}
    });
    dialog.present(Some(&window));
}

fn install(ui: &Ui, package: &'static str, then: Option<fn(&Ui)>) {
    if !begin(ui, &format!("Installing {package}")) {
        report_busy(ui);
        return;
    }
    note_close(
        ui,
        "You can close the window. The system finishes the installation on its own.",
    );
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let result = packagekit::install(package).await;
        finish(&ui);
        match result {
            Ok(()) => {
                ui.toasts
                    .add_toast(adw::Toast::new(&format!("Installed {package}")));
                check(&ui, then, true);
            }
            Err(error) => ui.toasts.add_toast(
                adw::Toast::builder()
                    .title(format!("Could not install {package}: {error}"))
                    .use_markup(false)
                    .timeout(15)
                    .build(),
            ),
        }
    });
}
