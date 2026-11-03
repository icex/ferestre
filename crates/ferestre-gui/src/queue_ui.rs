//! Install queue state survives redraws and application restarts.
use super::*;
use ferestre_core::transfers::{Job, Status};

pub(super) fn save(ui: &Ui) -> bool {
    if let Some(error) = ui.queue_error.borrow().as_ref() {
        ui.toasts.add_toast(toast(error));
        return false;
    }
    let state = ui
        .model
        .borrow()
        .paths
        .as_ref()
        .map(|p| p.state_dir().to_path_buf());
    let result = state
        .ok_or_else(|| anyhow::anyhow!("State directory is unavailable"))
        .and_then(|state| ui.install_queue.borrow().save(&state));
    if let Err(error) = result {
        ui.toasts
            .add_toast(toast(format!("Could not save the install queue: {error}")));
        return false;
    }
    true
}
pub(super) fn enqueue(ui: &Ui, id: &str, name: &str, dir: Option<&Path>, what: &str) {
    if *ui.closing.borrow() {
        return;
    }
    let before = ui.install_queue.borrow().jobs.clone();
    match ui
        .install_queue
        .borrow_mut()
        .enqueue(Job::new(id, name, dir, what == "Updating"))
    {
        Ok(false) => {
            ui.toasts
                .add_toast(toast("This game is already in Downloads"));
            return;
        }
        Err(error) => {
            ui.toasts.add_toast(toast(error.to_string()));
            return;
        }
        _ => (),
    }
    if !save(ui) {
        ui.install_queue.borrow_mut().jobs = before;
        return;
    }
    ui.toasts
        .add_toast(toast(format!("{name} added to Downloads")));
    start_next(ui);
    render(ui);
}
/// Resume an existing transfer, including one loaded paused after a restart.
pub(super) fn resume(ui: &Ui, id: &str) -> bool {
    let key = id.to_ascii_uppercase();
    if !ui
        .install_queue
        .borrow()
        .jobs
        .iter()
        .any(|j| j.product_id == key)
    {
        return false;
    }
    if *ui.closing.borrow() {
        return true;
    }
    if ui
        .download
        .current
        .borrow()
        .as_ref()
        .is_some_and(|d| d.product_id == key)
    {
        pause(ui, false);
        return true;
    }
    let before = ui.install_queue.borrow().jobs.clone();
    ui.install_queue.borrow_mut().resume(&key);
    if save(ui) {
        start_next(ui);
        render(ui);
    } else {
        ui.install_queue.borrow_mut().jobs = before;
    }
    true
}
pub(super) fn start_next(ui: &Ui) {
    if ui.busy.borrow().is_some() || *ui.closing.borrow() {
        return;
    }
    let next = ui.install_queue.borrow().next();
    if let Some(job) = next {
        ui.install_queue
            .borrow_mut()
            .set_status(&job.product_id, Status::Running);
        if !save(ui) {
            ui.install_queue
                .borrow_mut()
                .set_status(&job.product_id, Status::Paused);
            return;
        }
        spawn_transfer(
            ui,
            &job.product_id,
            &job.name,
            job.destination.as_deref(),
            if job.updating {
                "Updating"
            } else {
                "Installing"
            },
            "install",
        );
    }
}
pub(super) fn completed(ui: &Ui, id: &str, success: bool, stopped: bool) {
    if success {
        ui.install_queue.borrow_mut().remove(id);
    } else {
        ui.install_queue.borrow_mut().set_status(
            id,
            if stopped {
                Status::Paused
            } else {
                Status::Failed
            },
        );
    }
    save(ui);
}
pub(super) fn pause(ui: &Ui, paused: bool) {
    if let Some(control) = ui.download_control.borrow().as_ref() {
        control.pause(paused);
    }
    let id = ui
        .download
        .current
        .borrow()
        .as_ref()
        .map(|d| d.product_id.clone());
    if let Some(id) = id {
        ui.install_queue.borrow_mut().set_status(
            &id,
            if paused {
                Status::Paused
            } else {
                Status::Running
            },
        );
        save(ui);
    }
    ui.download
        .pause
        .set_label(if paused { "Resume" } else { "Pause" });
    if ui.model.borrow().section == Section::Downloads {
        render(ui);
    }
}
pub(super) fn render_queue(ui: &Ui, page: &gtk::Box) {
    let heading = gtk::Label::builder()
        .label("Downloads")
        .xalign(0.0)
        .css_classes(["title-1"])
        .build();
    page.append(&heading);
    let subtitle = gtk::Label::builder()
        .label("Pause a download, queue another game, or continue after restarting Ferestre.")
        .xalign(0.0)
        .wrap(true)
        .css_classes(["dim-label"])
        .build();
    page.append(&subtitle);
    if let Some(error) = ui.queue_error.borrow().as_ref() {
        page.append(&plain_row("The saved queue could not be read", Some(error)));
        return;
    }
    let jobs = ui.install_queue.borrow().jobs.clone();
    let group = adw::PreferencesGroup::new();
    if jobs.is_empty() {
        group.add(&plain_row(
            "No downloads queued",
            Some("Choose Install or Update on a game to add it here."),
        ));
    }
    let current = ui
        .download
        .current
        .borrow()
        .as_ref()
        .map(|d| d.product_id.clone());
    for job in jobs {
        let row = plain_row(&job.name, Some(job.status.label()));
        row.set_title_lines(2);
        row.set_subtitle_lines(2);
        let active = current.as_deref() == Some(job.product_id.as_str());
        let action = gtk::Button::with_label(if active && job.status == Status::Running {
            "Pause"
        } else {
            "Resume"
        });
        action.set_valign(gtk::Align::Center);
        action.set_widget_name(&format!("queue-{}-primary", job.product_id));
        if active {
            action.set_sensitive(ui.download.pause.is_visible());
        }
        let (action_ui, id) = (ui.clone(), job.product_id.clone());
        action.connect_clicked(move |_| {
            if active && job.status == Status::Running {
                pause(&action_ui, true);
            } else {
                resume(&action_ui, &id);
            }
        });
        row.add_suffix(&action);
        let remove = gtk::Button::with_label(if active { "Stop" } else { "Remove" });
        remove.set_valign(gtk::Align::Center);
        remove.set_widget_name(&format!("queue-{}-stop", job.product_id));
        let (remove_ui, id) = (ui.clone(), job.product_id.clone());
        remove.connect_clicked(move |_| {
            if active {
                if let Some(control) = remove_ui.download_control.borrow().as_ref() {
                    control.stop();
                }
            } else {
                remove_ui.install_queue.borrow_mut().remove(&id);
                save(&remove_ui);
                render(&remove_ui);
            }
        });
        row.add_suffix(&remove);
        group.add(&row);
    }
    page.append(&group);
    let diagnostics = gtk::Button::with_label("View download diagnostics");
    diagnostics.set_halign(gtk::Align::Start);
    let ui = ui.clone();
    diagnostics.connect_clicked(move |_| show_diagnostics(&ui));
    page.append(&diagnostics);
}
pub(super) fn show_diagnostics(ui: &Ui) {
    let entries = ui
        .model
        .borrow()
        .paths
        .as_ref()
        .map(|p| ferestre_core::diagnostics::read(p.state_dir()));
    let text=match entries {
        Some(Ok(entries)) if !entries.is_empty()=>entries.iter().rev().map(|e|format!("{}  {}",e.unix_seconds,e.diagnostic.message())).collect::<Vec<_>>().join("\n\n"),
        Some(Err(error))=>format!("Could not read diagnostics: {error}"),
        _=>"No download failures have been recorded. Diagnostics contain local reason codes and HTTP status only.".into(),
    };
    let dialog = adw::AlertDialog::new(Some("Download diagnostics"), Some(&text));
    dialog.add_response("close", "Close");
    dialog.set_close_response("close");
    dialog.present(Some(&ui.window));
}
