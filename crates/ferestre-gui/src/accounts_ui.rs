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
    let window = adw::Window::builder()
        .title("Accounts")
        .transient_for(&ui.window)
        .modal(true)
        .default_width(600)
        .default_height(680)
        .build();
    let header = adw::HeaderBar::new();
    let add = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("Sign in another Microsoft account")
        .build();
    add.update_property(&[gtk::accessible::Property::Label("Add account")]);
    header.pack_start(&add);
    let page = adw::PreferencesPage::new();
    let content = adw::ToolbarView::new();
    content.add_top_bar(&header);
    content.set_content(Some(&page));
    window.set_content(Some(&content));

    let choices = Rc::new(profiles.profiles.clone());
    let labels: Vec<String> = choices
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
    let selection = ui.model.borrow().roles.clone();
    let default = profiles
        .default_profile
        .as_deref()
        .or_else(|| choices.first().map(|p| p.id.as_str()));
    let index = |id: Option<&str>| -> u32 {
        id.and_then(|id| choices.iter().position(|p| p.id == id))
            .map(|i| i as u32)
            .unwrap_or(gtk::INVALID_LIST_POSITION)
    };

    // Who owns and who plays. Dropdowns rather than combo rows so each keeps a
    // plain accessible combo box the GUI test can drive.
    let roles = adw::PreferencesGroup::builder()
        .title("Who plays")
        .description(
            "The Store account supplies purchases and Game Pass. The Player account is \
             the gamertag the game signs in with, and gets its own saves.",
        )
        .build();
    let picker = |title: &str, subtitle: &str, icon: &str, selected: u32| {
        let dropdown = gtk::DropDown::from_strings(&refs);
        dropdown.set_valign(gtk::Align::Center);
        dropdown.set_selected(selected);
        dropdown.update_property(&[gtk::accessible::Property::Label(title)]);
        let row = adw::ActionRow::builder()
            .title(title)
            .subtitle(subtitle)
            .activatable_widget(&dropdown)
            .build();
        row.add_prefix(&gtk::Image::from_icon_name(icon));
        row.add_suffix(&dropdown);
        roles.add(&row);
        dropdown
    };
    let store = picker(
        "Store account",
        "Owns the games, or has Game Pass",
        "emblem-shared-symbolic",
        index(selection.as_ref().map(|s| s.store.as_str()).or(default)),
    );
    let player = picker(
        "Player account",
        "Gamertag, achievements and saves",
        "avatar-default-symbolic",
        index(selection.as_ref().map(|s| s.player.as_str()).or(default)),
    );
    page.add(&roles);

    let summary = gtk::Label::builder()
        .wrap(true)
        .xalign(0.0)
        .css_classes(["dim-label"])
        .margin_top(12)
        .build();
    let apply = gtk::Button::builder()
        .label("Apply accounts")
        .halign(gtk::Align::Center)
        .margin_top(12)
        .css_classes(["pill", "suggested-action"])
        .build();
    let footer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    footer.append(&summary);
    footer.append(&apply);
    let footer_group = adw::PreferencesGroup::new();
    footer_group.add(&footer);
    page.add(&footer_group);

    let update = {
        let (store, player, apply, summary, choices) = (
            store.clone(),
            player.clone(),
            apply.clone(),
            summary.clone(),
            choices.clone(),
        );
        move || {
            let owner = choices.get(store.selected() as usize);
            let gamer = choices.get(player.selected() as usize);
            let ready = owner.zip(gamer).filter(|(o, g)| o.signed_in && g.signed_in);
            apply.set_sensitive(ready.is_some());
            summary.set_label(&match (owner, gamer) {
                (Some(o), Some(g)) if !o.signed_in || !g.signed_in => {
                    "Sign the selected accounts in again before applying.".to_string()
                }
                (Some(o), Some(g)) if o.id == g.id => format!(
                    "{} plays with its own purchases and Game Pass. Changes apply to the next \
                     launch; running games keep their accounts.",
                    o.label
                ),
                (Some(o), Some(g)) => format!(
                    "{} plays with {}'s purchases and Game Pass, with separate saves. Changes \
                     apply to the next launch; running games keep their accounts.",
                    g.label, o.label
                ),
                _ => "Choose a Store account and a Player account.".to_string(),
            });
        }
    };
    update();
    let update = Rc::new(update);
    for dropdown in [&store, &player] {
        let update = update.clone();
        dropdown.connect_selected_notify(move |_| update());
    }

    // Every account this computer knows, with what it is used for now.
    let known = adw::PreferencesGroup::builder()
        .title("Accounts on this computer")
        .description("Games still enforce Microsoft and publisher account-sharing rules.")
        .build();
    for profile in choices.iter() {
        let row = adw::ActionRow::builder()
            .title(&profile.label)
            .subtitle(if profile.signed_in {
                "Signed in"
            } else {
                "Signed out"
            })
            .build();
        row.set_use_markup(false);
        row.add_prefix(
            &adw::Avatar::builder()
                .size(36)
                .text(&profile.label)
                .show_initials(true)
                .build(),
        );
        if let Some(selected) = &selection {
            for (role, id, class) in [
                ("Store", &selected.store, "store"),
                ("Player", &selected.player, "player"),
            ] {
                if *id == profile.id {
                    row.add_suffix(
                        &gtk::Label::builder()
                            .label(role)
                            .valign(gtk::Align::Center)
                            .css_classes(["role-badge", class])
                            .build(),
                    );
                }
            }
        }
        let action = gtk::Button::builder()
            .label(if profile.signed_in {
                "Sign out"
            } else {
                "Sign in again"
            })
            .valign(gtk::Align::Center)
            .css_classes(if profile.signed_in {
                ["flat", "destructive-action"]
            } else {
                ["flat", "suggested-action"]
            })
            .build();
        let name = format!(
            "{} {}",
            if profile.signed_in {
                "Sign out"
            } else {
                "Sign in again as"
            },
            profile.label
        );
        // GTK before 4.16 names a labelled button after its label, which would
        // leave every row with the same "Sign out".
        action.reset_relation(gtk::AccessibleRelation::LabelledBy);
        action.update_property(&[gtk::accessible::Property::Label(&name)]);
        let (id, signed_in) = (profile.id.clone(), profile.signed_in);
        action.connect_clicked(glib::clone!(
            #[strong]
            ui,
            #[strong]
            window,
            move |_| {
                window.close();
                if signed_in {
                    logout(&ui, id.clone());
                } else {
                    // The client recognises a known account and reuses its
                    // profile and save namespace.
                    add_account(&ui);
                }
            }
        ));
        row.add_suffix(&action);
        known.add(&row);
    }
    page.add(&known);

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
                Err(error) => ui.toasts.add_toast(
                    adw::Toast::builder()
                        .title(format!("Accounts: {error}"))
                        .use_markup(false)
                        .build(),
                ),
            }
        }
    ));
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
            // Clear the in-memory Store account explicitly: `Model::reload`
            // keeps it while the Store selection is unchanged.
            let mut model = ui.model.borrow_mut();
            let store = model.roles.as_ref().map(|r| r.store.as_str()).or_else(|| {
                model
                    .profiles
                    .as_ref()
                    .and_then(|p| p.default_profile.as_deref())
            });
            let signs_out_store = store.is_some_and(|store| store == id);
            // With no role selection the default profile's caches are the
            // unscoped ones, which a restart would otherwise show again.
            let mut stale = Vec::new();
            if let Some(paths) = model.paths.as_ref() {
                stale.push(paths.state_dir().join("accounts").join(&id));
                if signs_out_store && model.roles.is_none() {
                    stale.push(paths.state_dir().to_path_buf());
                }
            }
            for dir in stale {
                for cache in [library::cache_path(&dir), account::cache_path(&dir)] {
                    if let Err(e) = std::fs::remove_file(&cache) {
                        if e.kind() != std::io::ErrorKind::NotFound {
                            eprintln!("!! {}: {e}", cache.display());
                        }
                    }
                }
            }
            if signs_out_store {
                model.account = None;
                model.avatar = None;
                model.ownership = Ownership::Unknown;
                model.subscriptions.clear();
                model.refused.clear();
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
