use super::*;

pub(super) fn open(ui: &Ui) {
    let Some(client) = ui.model.borrow().xodus_cli() else {
        return;
    };
    if !begin(ui, "Loading accounts") {
        report_busy(ui);
        return;
    }
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || -> anyhow::Result<accounts::Profiles> {
            let status = std::process::Command::new(&client)
                .arg("migrate-accounts")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()?;
            anyhow::ensure!(
                status.success(),
                "Cannot load accounts. Update the client and unlock your keychain."
            );
            accounts::fetch(&client)
        })
        .await;
        finish(&ui);
        if *ui.closing.borrow() {
            return;
        }
        match result {
            Ok(Ok(profiles)) => show(&ui, profiles),
            _ => ui.toasts.add_toast(adw::Toast::new(
                "Could not load accounts. Update the client and unlock your keychain.",
            )),
        }
    });
}

fn show(ui: &Ui, profiles: accounts::Profiles) {
    ui.model.borrow_mut().profiles = Some(profiles.clone());
    let window = gtk::Window::builder()
        .title("Accounts")
        .transient_for(&ui.window)
        .modal(true)
        .default_width(540)
        .build();
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();
    let help=gtk::Label::builder().label("The Store account supplies purchases and subscriptions. The Player account supplies Xbox identity, achievements and saves.")
        .wrap(true).xalign(0.0).build();
    content.append(&help);
    let labels: Vec<String> = profiles
        .profiles
        .iter()
        .map(|p| {
            format!(
                "{}{}",
                p.label,
                if p.signed_in { "" } else { " (signed out)" }
            )
        })
        .collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let store = gtk::DropDown::from_strings(&refs);
    store.update_property(&[gtk::accessible::Property::Label("Store account")]);
    let player = gtk::DropDown::from_strings(&refs);
    player.update_property(&[gtk::accessible::Property::Label("Player account")]);
    let selection = ui.model.borrow().roles.clone();
    let default = profiles
        .default_profile
        .as_deref()
        .or_else(|| profiles.profiles.first().map(|p| p.id.as_str()));
    let index = |id: Option<&str>| -> u32 {
        id.and_then(|id| profiles.profiles.iter().position(|p| p.id == id))
            .map(|i| i as u32)
            .unwrap_or(gtk::INVALID_LIST_POSITION)
    };
    store.set_selected(index(
        selection.as_ref().map(|s| s.store.as_str()).or(default),
    ));
    player.set_selected(index(
        selection.as_ref().map(|s| s.player.as_str()).or(default),
    ));
    for (label, dropdown) in [("Store account", &store), ("Player account", &player)] {
        content.append(&gtk::Label::builder().label(label).xalign(0.0).build());
        content.append(dropdown);
    }
    let note=gtk::Label::builder().label("Changes apply to the next launch. Running games keep their accounts. A different player gets separate local saves; existing saves are kept.")
        .wrap(true).xalign(0.0).css_classes(["dim-label"]).build();
    content.append(&note);
    let apply = gtk::Button::with_label("Apply accounts");
    apply.add_css_class("suggested-action");
    let choices = Rc::new(profiles.profiles.clone());
    let update = {
        let store = store.clone();
        let player = player.clone();
        let apply = apply.clone();
        let choices = choices.clone();
        move || {
            apply.set_sensitive(
                choices
                    .get(store.selected() as usize)
                    .is_some_and(|p| p.signed_in)
                    && choices
                        .get(player.selected() as usize)
                        .is_some_and(|p| p.signed_in),
            );
        }
    };
    update();
    let update = Rc::new(update);
    store.connect_selected_notify({
        let update = update.clone();
        move |_| update()
    });
    player.connect_selected_notify({
        let update = update.clone();
        move |_| update()
    });
    apply.connect_clicked(glib::clone!(
        #[strong]
        ui,
        #[strong]
        window,
        #[strong]
        store,
        #[strong]
        player,
        #[strong]
        choices,
        move |_| {
            if ui.busy.borrow().is_some() {
                report_busy(&ui);
                return;
            }
            let (Some(owner), Some(gamer)) = (
                choices.get(store.selected() as usize),
                choices.get(player.selected() as usize),
            ) else {
                return;
            };
            if !owner.signed_in || !gamer.signed_in {
                return;
            }
            let result = {
                let model = ui.model.borrow();
                model
                    .paths
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Cannot resolve account settings"))
                    .and_then(|paths| accounts::save(paths.config_dir(), &owner.id, &gamer.id))
            };
            match result {
                Ok(_) => {
                    ui.model.borrow_mut().reload();
                    window.close();
                    render(&ui);
                    refresh_account(&ui);
                    ui.toasts
                        .add_toast(adw::Toast::new("Accounts saved for future launches"));
                }
                Err(error) => ui
                    .toasts
                    .add_toast(adw::Toast::new(&format!("Accounts: {error}"))),
            }
        }
    ));
    content.append(&apply);
    let add = gtk::Button::with_label("Add account");
    add.connect_clicked(glib::clone!(
        #[strong]
        ui,
        #[strong]
        window,
        move |_| {
            window.close();
            add_account(&ui);
        }
    ));
    content.append(&add);
    let signouts = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    for (label, selector) in [
        ("Sign out Store account", store.clone()),
        ("Sign out Player account", player.clone()),
    ] {
        let button = gtk::Button::with_label(label);
        button.add_css_class("destructive-action");
        button.connect_clicked(glib::clone!(
            #[strong]
            ui,
            #[strong]
            window,
            #[strong]
            choices,
            move |_| {
                let Some(profile) = choices.get(selector.selected() as usize) else {
                    return;
                };
                window.close();
                logout(&ui, profile.id.clone());
            }
        ));
        signouts.append(&button);
    }
    content.append(&signouts);
    let warning=gtk::Label::builder().label("Signing out a profile used by both roles signs out both. Games still enforce Microsoft and publisher account-sharing rules.")
        .wrap(true).xalign(0.0).css_classes(["dim-label"]).build();
    content.append(&warning);
    let close = gtk::Button::with_label("Close");
    close.connect_clicked(glib::clone!(
        #[strong]
        window,
        move |_| window.close()
    ));
    content.append(&close);
    window.set_child(Some(&content));
    window.present();
}

fn add_account(ui: &Ui) {
    let Some(client) = ui.model.borrow().xodus_cli() else {
        return;
    };
    if !begin(ui, "Signing in another account") {
        report_busy(ui);
        return;
    }
    *ui.wait_on_close.borrow_mut() = true;
    show_activity(ui);
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || {
            std::process::Command::new(&client)
                .arg("add-account")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .status()
        })
        .await;
        finish(&ui);
        if *ui.closing.borrow() {
            return;
        }
        if !matches!(result,Ok(Ok(status)) if status.success()) {
            ui.toasts.add_toast(adw::Toast::new(
                "Sign-in was cancelled or failed. Your selected accounts were kept.",
            ));
        }
        open(&ui);
    });
}

fn logout(ui: &Ui, id: String) {
    let Some(client) = ui.model.borrow().xodus_cli() else {
        return;
    };
    if !begin(ui, "Signing out selected account") {
        report_busy(ui);
        return;
    }
    *ui.wait_on_close.borrow_mut() = true;
    show_activity(ui);
    let worker_id = id.clone();
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || {
            std::process::Command::new(&client)
                .args(["--profile", &worker_id, "logout"])
                .stdout(std::process::Stdio::null())
                .status()
        })
        .await;
        finish(&ui);
        if matches!(result,Ok(Ok(status)) if status.success()) {
            if let Some(paths) = ui.model.borrow().paths.as_ref() {
                let private = paths.state_dir().join("accounts").join(&id);
                let _ = std::fs::remove_file(library::cache_path(&private));
                let _ = std::fs::remove_file(account::cache_path(&private));
            }
            // Read the scoped caches again; reload normally preserves a live account.
            let mut model = ui.model.borrow_mut();
            let store = model.roles.as_ref().map(|r| r.store.as_str()).or_else(|| {
                model
                    .profiles
                    .as_ref()
                    .and_then(|p| p.default_profile.as_deref())
            });
            let signs_out_store = store.is_some_and(|store| store == id);
            if signs_out_store {
                model.account = None;
                model.avatar = None;
                model.ownership = Ownership::Unknown;
                model.subscriptions.clear();
                model.entitlements_checked = false;
            }
            drop(model);
            refresh_account(&ui);
        } else {
            ui.toasts.add_toast(adw::Toast::new(
                "Could not sign out this account. Check your keychain.",
            ));
        }
        render(&ui);
    });
}
