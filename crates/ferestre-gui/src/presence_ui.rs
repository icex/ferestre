//! The window's half of online status: the switch, the dot, and following the
//! runners its own launches report.
//!
//! The window starts no runner. `ferestre run` does, for every launch, so a
//! game keeps its status when the window closes and a Steam shortcut gets
//! one too (see the CLI's `presence` module). The window reads the runner's
//! events from the launch's stderr, which goes to the launch's output file
//! (see `state::follow_lines`), asks Xbox what it shows with `presence
//! get`, and changes a running launch's mind with SIGUSR1 (stop) and SIGUSR2
//! (start again).

use super::*;
use ferestre_core::presence::{self, Availability, Badge, Event, Runner, Server};
use std::time::{Duration, Instant};

/// How often Xbox is asked, from the window's minute timer.
const ASK_EVERY: Duration = Duration::from_secs(5 * 60);
/// How long a Player's sign-out waits for runners to tell Xbox goodbye.
const GOODBYE: Duration = Duration::from_secs(5);

pub(super) struct Launch {
    pid: u32,
    /// The Player the launch was started for.
    player: Option<String>,
    /// Whether it has relayed anything yet. It does only once its signal
    /// handlers are in place, so before that it is never signalled.
    heard: bool,
    runner: Runner,
}

#[derive(Default)]
pub(super) struct State {
    availability: Availability,
    player: Option<String>,
    sharing: bool,
    server: Server,
    asked: Option<Instant>,
    asking: bool,
    /// A refresh asked for while one was running, and whether it probes. The
    /// one running may have read Xbox before the change it was asked for.
    owed: Option<bool>,
    launches: BTreeMap<String, Launch>,
    /// Set by switching it on: the next failure gets one toast. Otherwise a
    /// network problem shows in the badge and nowhere else.
    report_failure: bool,
    /// The switch and caption of the account menu, while it is open.
    menu: Option<(gtk::Switch, gtk::Label)>,
}

impl State {
    fn mine(&self) -> impl Iterator<Item = &Launch> {
        self.launches
            .values()
            .filter(move |l| l.player.is_some() && l.player == self.player)
    }

    fn runner(&self) -> Option<&Runner> {
        presence::strongest(self.mine().map(|l| &l.runner))
    }

    pub(super) fn badge(&self) -> Badge {
        presence::badge(self.availability, self.sharing, self.runner(), &self.server)
    }
}

/// Ask Xbox what it shows, and with `probe` ask the client first whether it
/// can and for which Player. Never the busy flag: this must not hold up a
/// download or a sign-in, and it is skipped while one is already asking.
pub(super) fn refresh(ui: &Ui, probe: bool) {
    if *ui.closing.borrow() {
        return;
    }
    let (client, roles, config, signed_in) = {
        let model = ui.model.borrow();
        (
            model.xodus_cli(),
            model.roles.clone(),
            model.paths.as_ref().map(|p| p.config_dir().to_path_buf()),
            model.account.is_some(),
        )
    };
    let (Some(client), Some(config)) = (client, config) else {
        return;
    };
    {
        let mut state = ui.presence.borrow_mut();
        if !signed_in {
            // Nobody to show. Probed again once somebody signs in.
            state.availability = Availability::Checking;
            state.server = Server::Unknown;
            state.asked = None;
            drop(state);
            update(ui);
            return;
        }
        if state.asking {
            state.owed = Some(probe || state.owed.unwrap_or(false));
            return;
        }
        if !probe && state.availability != Availability::Ready {
            return;
        }
        state.asking = true;
    }
    let known = (!probe)
        .then(|| ui.presence.borrow().player.clone())
        .flatten();
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let expected = roles.clone();
        let answer = gio::spawn_blocking(move || {
            let (availability, player) = match known {
                Some(player) => (Availability::Ready, Some(player)),
                None => match accounts::fetch(&client) {
                    Ok(profiles) if presence::supported(&profiles) => {
                        match presence::player(&profiles, roles.as_ref()) {
                            Some(p) if p.signed_in => (Availability::Ready, Some(p.id.clone())),
                            Some(p) => (Availability::SignedOut, Some(p.id.clone())),
                            None => (Availability::SignedOut, None),
                        }
                    }
                    _ => (Availability::Unsupported, None),
                },
            };
            let server = match (&availability, &player) {
                (Availability::Ready, Some(player)) => presence::get_command(&client, player)
                    .stdin(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .output()
                    .ok()
                    .filter(|output| output.status.success())
                    .and_then(|output| presence::parse_status(&output.stdout).ok())
                    .map_or(Server::Failed, Server::Known),
                _ => Server::Unknown,
            };
            (availability, player, server)
        })
        .await;
        let owed = {
            let mut state = ui.presence.borrow_mut();
            state.asking = false;
            state.owed.take()
        };
        if *ui.closing.borrow() {
            return;
        }
        // An answer for accounts that are no longer selected says nothing
        // about the ones that are.
        if ui.model.borrow().roles != expected {
            refresh(&ui, true);
            return;
        }
        if ui.model.borrow().account.is_none() {
            return;
        }
        let Ok((availability, player, server)) = answer else {
            return;
        };
        {
            let mut state = ui.presence.borrow_mut();
            state.sharing = player
                .as_deref()
                .is_some_and(|player| presence::sharing(&config, player));
            state.availability = availability;
            state.player = player;
            state.server = server;
            state.asked = Some(Instant::now());
        }
        update(&ui);
        if let Some(probe) = owed {
            refresh(&ui, probe);
        }
    });
}

/// For the minute timer: a check never made, or one five minutes old.
pub(super) fn due(ui: &Ui) -> Option<bool> {
    let state = ui.presence.borrow();
    match state.availability {
        Availability::Checking if ui.model.borrow().account.is_some() => Some(true),
        Availability::Ready if state.asked.is_none_or(|at| at.elapsed() >= ASK_EVERY) => {
            Some(false)
        }
        _ => None,
    }
}

/// Redraw what shows the status: the account button, and the menu if open.
pub(super) fn update(ui: &Ui) {
    render_account_button(ui);
    let (menu, ready, sharing, caption) = {
        let state = ui.presence.borrow();
        (
            state.menu.clone(),
            state.availability == Availability::Ready,
            state.sharing,
            presence::caption(state.availability, state.sharing, state.runner()),
        )
    };
    if let Some((switch, label)) = menu {
        switch.set_sensitive(ready);
        // The handler reads this back as no change: it only acts when what
        // the switch says differs from what is saved.
        if switch.is_active() != (ready && sharing) {
            switch.set_active(ready && sharing);
        }
        label.set_label(&caption);
    }
}

/// The person flipped the switch.
fn set_sharing(ui: &Ui, on: bool) {
    let config = ui
        .model
        .borrow()
        .paths
        .as_ref()
        .map(|p| p.config_dir().to_path_buf());
    let Some(player) = ui.presence.borrow().player.clone() else {
        return;
    };
    let saved = config
        .ok_or_else(|| anyhow::anyhow!("Cannot resolve settings"))
        .and_then(|config| presence::set_sharing(&config, &player, on));
    if let Err(error) = saved {
        ui.toasts.add_toast(
            adw::Toast::builder()
                .title(format!("Online status: {error}"))
                .use_markup(false)
                .build(),
        );
        update(ui);
        return;
    }
    {
        let mut state = ui.presence.borrow_mut();
        state.sharing = on;
        state.report_failure = on;
        for launch in state.mine().filter(|l| l.heard) {
            let signal = match (on, launch.runner.live()) {
                (true, false) => libc::SIGUSR2,
                (false, true) => libc::SIGUSR1,
                _ => continue,
            };
            // Safety: the launcher this window started, by its own pid and not
            // its group, which also holds the game.
            unsafe { libc::kill(launch.pid as i32, signal) };
        }
    }
    update(ui);
}

pub(super) fn launch_started(ui: &Ui, key: &str, pid: u32) {
    let roles_player = ui.model.borrow().roles.as_ref().map(|r| r.player.clone());
    let mut state = ui.presence.borrow_mut();
    let player = roles_player.or_else(|| state.player.clone());
    state.launches.insert(
        key.to_string(),
        Launch {
            pid,
            player,
            heard: false,
            runner: Runner::Idle,
        },
    );
}

/// An event relayed by the launch with this pid. A late line from an earlier
/// launch of the same title is dropped.
pub(super) fn launch_event(ui: &Ui, key: &str, pid: u32, event: Event) {
    let failure = {
        let mut state = ui.presence.borrow_mut();
        let current = state.player.clone();
        let Some(launch) = state.launches.get_mut(key).filter(|l| l.pid == pid) else {
            return;
        };
        launch.heard = true;
        launch.runner.apply(&event);
        let mine = launch.player.is_some() && launch.player == current;
        let failure = match &event {
            Event::Retrying { reason, .. } => Some(presence::describe_retry(reason)),
            Event::Error { code, .. } => Some(code.message()),
            _ => None,
        };
        if matches!(event, Event::Online { .. }) && mine {
            state.report_failure = false;
        }
        failure.filter(|_| mine && std::mem::take(&mut state.report_failure))
    };
    if let Some(failure) = failure {
        ui.toasts.add_toast(
            adw::Toast::builder()
                .title(format!("Online status: {failure}"))
                .use_markup(false)
                .build(),
        );
    }
    update(ui);
    // The runner has told Xbox goodbye; see what Xbox shows now.
    if matches!(event, Event::Ended) {
        refresh(ui, false);
    }
}

pub(super) fn launch_ended(ui: &Ui, key: &str) {
    ui.presence.borrow_mut().launches.remove(key);
    update(ui);
    // Its last events may not have been read yet; ask Xbox either way.
    refresh(ui, false);
}

/// Before a Player signs out: stop their runners and give each a few seconds
/// to tell Xbox goodbye, which needs the token the sign-out removes.
pub(super) async fn stop_for(ui: &Ui, player: &str) {
    let stopping: Vec<String> = {
        let state = ui.presence.borrow();
        state
            .launches
            .iter()
            .filter(|(_, l)| l.heard && l.runner.live() && l.player.as_deref() == Some(player))
            .map(|(key, l)| {
                // Safety: as in `set_sharing`, the launcher's own pid.
                unsafe { libc::kill(l.pid as i32, libc::SIGUSR1) };
                key.clone()
            })
            .collect()
    };
    let deadline = Instant::now() + GOODBYE;
    while Instant::now() < deadline
        && stopping.iter().any(|key| {
            ui.presence
                .borrow()
                .launches
                .get(key)
                .is_some_and(|l| l.runner.live())
        })
    {
        glib::timeout_future(Duration::from_millis(100)).await;
    }
}

/// The Player whose status the window shows, once the client has said.
pub(super) fn player(ui: &Ui) -> Option<String> {
    ui.presence.borrow().player.clone()
}

/// The top of the account menu: the switch, and what it means right now.
pub(super) fn menu_section(ui: &Ui, items: &gtk::Box, popover: &gtk::Popover) {
    let heading = gtk::Label::builder()
        .label("Status")
        .xalign(0.0)
        .margin_start(6)
        .margin_top(4)
        .css_classes(["heading"])
        .build();
    let title = gtk::Label::builder()
        .label("Show me online")
        .xalign(0.0)
        .hexpand(true)
        .build();
    let switch = gtk::Switch::builder().valign(gtk::Align::Center).build();
    switch.update_property(&[gtk::accessible::Property::Label("Show me online")]);
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .margin_start(6)
        .margin_end(6)
        .margin_top(4)
        .build();
    row.append(&title);
    row.append(&switch);
    let caption = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .max_width_chars(34)
        .margin_start(6)
        .margin_end(6)
        .margin_bottom(6)
        .css_classes(["caption", "dim-label"])
        .build();
    items.append(&heading);
    items.append(&row);
    items.append(&caption);
    items.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    ui.presence.borrow_mut().menu = Some((switch.clone(), caption));
    update(ui);
    switch.connect_active_notify(glib::clone!(
        #[strong]
        ui,
        move |switch| {
            let (ready, saved) = {
                let state = ui.presence.borrow();
                (state.availability == Availability::Ready, state.sharing)
            };
            if ready && switch.is_active() != saved {
                set_sharing(&ui, switch.is_active());
            }
        }
    ));
    popover.connect_closed(glib::clone!(
        #[strong]
        ui,
        move |_| ui.presence.borrow_mut().menu = None
    ));
    // Opening the menu is when somebody looks: ask again.
    refresh(ui, true);
}

/// The dot for a badge, or nothing when there is nothing to show.
pub(super) fn dot(badge: &Badge) -> Option<gtk::Box> {
    let label = badge.label()?;
    let dot = gtk::Box::builder()
        .accessible_role(gtk::AccessibleRole::Img)
        .css_classes(["presence-dot"])
        .halign(gtk::Align::End)
        .valign(gtk::Align::End)
        .build();
    if let Some(class) = badge.css_class() {
        dot.add_css_class(class);
    }
    let name = format!("Status: {label}");
    dot.update_property(&[gtk::accessible::Property::Label(&name)]);
    dot.set_tooltip_text(badge.tooltip().as_deref());
    Some(dot)
}
