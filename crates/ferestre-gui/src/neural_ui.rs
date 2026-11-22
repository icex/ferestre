//! Neural rendering (DLSSNR-AMD) for one game, and its Settings field.
use super::*;
use ferestre_core::neural::{self, apply::Gate, Network, Overview, Route};

/// What the dialog shows, decided from an [`Overview`] without any widget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct View {
    pub summary: String,
    pub can_enable: bool,
    pub blocked_because: Option<String>,
    pub acknowledge: Vec<&'static str>,
    /// The kernel-level ones among `acknowledge`: the strongest warning.
    pub kernel: Vec<&'static str>,
    pub warnings: Vec<String>,
    pub route: Option<Route>,
    pub enabled: bool,
}

fn api_text(api: neural::detect::Api) -> &'static str {
    use neural::detect::Api;
    match api {
        Api::Dx10To12 => "Direct3D 10–12",
        Api::Vulkan => "Vulkan",
        Api::Dx9 => "Direct3D 9",
        Api::OpenGl => "OpenGL",
        Api::Unknown => "an unknown graphics API",
    }
}

pub(crate) fn view(o: &Overview) -> View {
    let enabled = o.entry.as_ref().is_some_and(|e| e.enabled);
    let mut lines = Vec::new();
    match o.entry.as_ref().filter(|e| e.enabled) {
        Some(entry) => lines.push(format!("On: {}, {}", entry.route, entry.model)),
        None => lines.push("Off".to_string()),
    }
    if let Some(Err(error)) = &o.installed {
        lines.push(format!(
            "The install is incomplete ({error}); the next launch installs it again"
        ));
    }
    let d = &o.detected;
    let game = if d.bits == 0 {
        "The game’s executable could not be read".to_string()
    } else {
        format!(
            "Game: {}-bit, {}{}",
            d.bits,
            api_text(d.api),
            if d.assumed { " (assumed)" } else { "" }
        )
    };
    lines.push(match d.route {
        Some(route) => format!("{game}; route: {route}"),
        None => format!("{game}; no route detected, choose one"),
    });
    if !d.upscalers.is_empty() {
        lines.push(format!("Upscalers: {}", d.upscalers.join(", ")));
    }

    let mut warnings = o.hardware.warnings.clone();
    if d.assumed {
        warnings.push(
            "The executable is encrypted, so x86-64 Direct3D 12 is assumed (true of Xbox PC games)"
                .to_string(),
        );
    }
    if o.models.is_empty() {
        warnings.push(if o.dll_url_configured {
            "No model is cached yet: Enable sets it up first, from nvngx_dlssnr.dll found on this computer or the download address in Settings".to_string()
        } else {
            "No model is cached yet: Enable looks for nvngx_dlssnr.dll on this computer first; if none is found, set a download address in Settings".to_string()
        });
    }
    if !o.package_ready {
        warnings.push("The DLSSNR-AMD package is downloaded when you enable it".to_string());
    }

    let (can_enable, blocked_because, acknowledge) = match &o.gate {
        // Only an RX 9000 runs it; on Intel, NVIDIA or an older Radeon the
        // switch stays off whatever the game.
        _ if o.hardware.unsupported.is_some() => {
            (false, o.hardware.unsupported.clone(), Vec::new())
        }
        Gate::Allowed => (true, None, Vec::new()),
        Gate::NeedsAcknowledgement(names) => (true, None, names.clone()),
    };
    let kernel: Vec<&'static str> = neural::apply::kernel_risk(&o.anticheat)
        .into_iter()
        .filter(|name| acknowledge.contains(name))
        .collect();
    if !kernel.is_empty() {
        warnings.push(format!(
            "{} is a kernel-level anti-cheat: online, a game that loads a mod with it is likely \
             to get the account banned. Enable it only for a build that never goes online.",
            kernel.join(", ")
        ));
    }
    View {
        summary: lines.join("\n"),
        can_enable,
        blocked_because,
        acknowledge,
        kernel,
        warnings,
        route: d.route,
        enabled,
    }
}

/// `ferestre neural enable …` with only what the person chose.
pub(crate) fn enable_args(
    product_id: &str,
    route: Option<Route>,
    model: Option<Network>,
    acknowledged: bool,
) -> Vec<String> {
    let mut args = vec![
        "neural".to_string(),
        "enable".to_string(),
        product_id.to_string(),
    ];
    if let Some(route) = route {
        args.extend(["--route".to_string(), route.as_str().to_string()]);
    }
    if let Some(model) = model {
        args.extend(["--model".to_string(), model.as_str().to_string()]);
    }
    if acknowledged {
        args.push("--accept-anticheat-risk".to_string());
    }
    args
}

/// `--anticheat-shown` for each anti-cheat the dialog named, so accepting
/// covers only those: one detected since refuses instead of riding along.
pub(crate) fn shown_args(names: &[&str]) -> Vec<String> {
    names
        .iter()
        .flat_map(|name| ["--anticheat-shown".to_string(), name.to_string()])
        .collect()
}

/// The route dropdown's choice: index 0 is "Automatic", then [`Route::ALL`].
pub(crate) fn chosen_route(index: u32) -> Option<Route> {
    (index as usize)
        .checked_sub(1)
        .and_then(|i| Route::ALL.get(i).copied())
}

/// The network dropdown's choice: index 0 is "Automatic", then [`Network::ALL`].
pub(crate) fn chosen_network(index: u32) -> Option<Network> {
    (index as usize)
        .checked_sub(1)
        .and_then(|i| Network::ALL.get(i).copied())
}

/// Whether Enable can be pressed: never on a refusal, only with a route
/// (detected, or chosen in place of "Automatic (none detected)"), and with
/// the acknowledgement box ticked when there is one.
pub(crate) fn enable_sensitive(v: &View, route_index: u32, acknowledged: Option<bool>) -> bool {
    v.can_enable
        && (v.route.is_some() || chosen_route(route_index).is_some())
        && acknowledged.unwrap_or(true)
}

/// Reads the game's neural rendering state off the main loop, then shows
/// the dialog.
pub fn open(ui: &Ui, product_id: &str, name: &str) {
    let (paths, recipe) = {
        let model = ui.model.borrow();
        let (Some(paths), Some(recipe)) = (model.paths.clone(), model.recipe_for(product_id))
        else {
            ui.toasts
                .add_toast(toast(format!("{name} has no recipe to read")));
            return;
        };
        (paths, recipe.clone())
    };
    // The folder `ferestre neural` acts on, so the dialog and the CLI agree.
    let game_dir = match paths.install_dir(&recipe) {
        Ok(dir) => dir,
        Err(error) => {
            ui.toasts
                .add_toast(toast(format!("Cannot find {name}: {error:#}")));
            return;
        }
    };
    let key = recipe.title.product_id.to_ascii_uppercase();
    let ui = ui.clone();
    let name = name.to_string();
    glib::spawn_future_local(async move {
        let blocking_key = key.clone();
        let result = gio::spawn_blocking(move || {
            let ctx = neural::Context::from_paths(&paths);
            neural::overview(
                &ctx,
                &paths,
                &blocking_key,
                &game_dir,
                &recipe.launch.executable,
            )
        })
        .await;
        if *ui.closing.borrow() {
            return;
        }
        match result {
            Ok(o) => present(&ui, &key, &name, &view(&o)),
            Err(_) => ui
                .toasts
                .add_toast(toast("Could not read neural rendering for this game")),
        }
    });
}

fn present(ui: &Ui, product_id: &str, name: &str, v: &View) {
    let dialog = adw::Dialog::builder()
        .title(format!("Neural rendering for {name}"))
        .content_width(560)
        .content_height(520)
        .build();
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();
    let label = |text: &str| {
        gtk::Label::builder()
            .label(text)
            .wrap(true)
            .xalign(0.0)
            .selectable(true)
            .build()
    };
    content.append(&label(
        "Experimental: DLSSNR-AMD upscales with NVIDIA’s neural model on AMD RDNA4. \
         It goes into the game’s folder and comes out again, with your own files put back, when you disable it.",
    ));
    content.append(&label(&v.summary));
    for warning in &v.warnings {
        let warning = label(warning);
        warning.add_css_class("warning");
        content.append(&warning);
    }

    let detected = match v.route {
        Some(route) => format!("Automatic ({route})"),
        None => "Automatic (none detected)".to_string(),
    };
    let mut route_names = vec![detected.as_str()];
    route_names.extend(Route::ALL.map(Route::as_str));
    let routes = gtk::DropDown::from_strings(&route_names);
    let mut network_names = vec!["Automatic"];
    network_names.extend(Network::ALL.map(Network::as_str));
    let networks = gtk::DropDown::from_strings(&network_names);
    let choice = |title: &str, widget: &gtk::DropDown| {
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .build();
        row.append(
            &gtk::Label::builder()
                .label(title)
                .xalign(0.0)
                .hexpand(true)
                .build(),
        );
        row.append(widget);
        row
    };
    content.append(&choice("Route", &routes));
    content.append(&choice("Starting network", &networks));

    let acknowledged = (!v.acknowledge.is_empty()).then(|| {
        gtk::CheckButton::with_label(&if v.kernel.is_empty() {
            format!(
                "I understand {} may ban accounts that load mods, and this game is single-player for me",
                v.acknowledge.join(", ")
            )
        } else {
            format!(
                "I understand {} may ban accounts that load mods; this is an offline build and \
                 I take the risk",
                v.acknowledge.join(", ")
            )
        })
    });
    if let Some(check) = &acknowledged {
        content.append(check);
    }

    let enable = gtk::Button::builder()
        .label(if v.enabled { "Reinstall" } else { "Enable" })
        .build();
    enable.add_css_class("suggested-action");
    if let Some(reason) = &v.blocked_because {
        enable.set_tooltip_text(Some(reason));
        content.append(&label(reason));
    }
    // Enable needs a route (detected or chosen) and, for an anti-cheat the
    // person has not acknowledged, the box ticked. A refusal has no override.
    let sensitivity = {
        let (enable, routes, check) = (enable.clone(), routes.clone(), acknowledged.clone());
        let v = v.clone();
        move || {
            enable.set_sensitive(enable_sensitive(
                &v,
                routes.selected(),
                check.as_ref().map(|c| c.is_active()),
            ));
        }
    };
    sensitivity();
    {
        let sensitivity = sensitivity.clone();
        routes.connect_selected_notify(move |_| sensitivity());
    }
    if let Some(check) = &acknowledged {
        check.connect_toggled(move |_| sensitivity());
    }
    {
        let (ui, dialog, product_id) = (ui.clone(), dialog.clone(), product_id.to_string());
        let (routes, networks, acknowledged) =
            (routes.clone(), networks.clone(), acknowledged.clone());
        let shown = v.acknowledge.clone();
        enable.connect_clicked(move |_| {
            let route = chosen_route(routes.selected());
            let model = chosen_network(networks.selected());
            let ack = acknowledged.as_ref().is_some_and(|c| c.is_active());
            let mut args = enable_args(&product_id, route, model, ack);
            if ack {
                args.extend(shown_args(&shown));
            }
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            dialog.close();
            spawn_cli_tracked(&ui, &args, "Enabling neural rendering");
        });
    }
    content.append(&enable);
    if v.enabled {
        let disable = gtk::Button::with_label("Disable");
        let (ui, dialog, product_id) = (ui.clone(), dialog.clone(), product_id.to_string());
        disable.connect_clicked(move |_| {
            dialog.close();
            spawn_cli_tracked(
                &ui,
                &["neural", "disable", &product_id],
                "Disabling neural rendering",
            );
        });
        content.append(&disable);
    }
    let cancel = gtk::Button::with_label("Cancel");
    let cancel_dialog = dialog.clone();
    cancel.connect_clicked(move |_| {
        cancel_dialog.close();
    });
    content.append(&cancel);
    dialog.set_child(Some(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&content)
            .build(),
    ));
    dialog.present(Some(&ui.window));
    cancel.grab_focus();
}

thread_local! {
    /// The download address as typed but not yet applied, kept across the
    /// redraws that rebuild the Settings group.
    static DLL_URL_DRAFT: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The Settings group: the DLL's download address (from the settings file,
/// never the environment) and which models are cached.
pub(crate) fn settings_group(
    ui: &Ui,
    paths: &ferestre_core::paths::Paths,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("Neural rendering (experimental)")
        .description(
            "DLSSNR-AMD takes its model from your own nvngx_dlssnr.dll. Ferestre looks for it \
             on this computer first; the address below is used only when none is found.",
        )
        .build();
    let config_dir = paths.config_dir().to_path_buf();
    let entry = adw::EntryRow::builder()
        .title("nvngx_dlssnr.dll download address")
        .show_apply_button(true)
        .build();
    // This group is rebuilt on every redraw, which follows every finished
    // background job: an unreadable file is said in place rather than as a
    // toast per redraw, and an address typed but not yet applied survives.
    let saved = match ferestre_core::settings::file_dlssnr_dll_url(&config_dir) {
        Ok(url) => url.unwrap_or_default(),
        Err(error) => {
            let row = plain_row("Could not read settings", Some(&format!("{error:#}")));
            row.add_css_class("error");
            group.add(&row);
            String::new()
        }
    };
    let draft = DLL_URL_DRAFT.with_borrow(Clone::clone);
    entry.set_text(draft.as_deref().unwrap_or(&saved));
    entry.connect_changed(move |entry| {
        let text = entry.text().to_string();
        DLL_URL_DRAFT.set((text != saved).then_some(text));
    });
    {
        let ui = ui.clone();
        entry.connect_apply(move |entry| {
            let text = entry.text();
            let text = text.trim();
            let url = (!text.is_empty()).then_some(text);
            match ferestre_core::settings::set_dlssnr_dll_url(&config_dir, url) {
                Ok(()) => {
                    DLL_URL_DRAFT.set(None);
                    ui.toasts.add_toast(toast(if url.is_some() {
                        "Download address saved"
                    } else {
                        "Download address cleared"
                    }))
                }
                Err(error) => ui
                    .toasts
                    .add_toast(toast(format!("Could not save the address: {error:#}"))),
            }
        });
    }
    group.add(&entry);

    // Sizes only, no hashing: cheap enough for a redraw.
    let cached =
        neural::models::cached(&neural::root(paths.state_dir()), &neural::Expect::pinned());
    let subtitle = if cached.is_empty() {
        "None cached".to_string()
    } else {
        cached
            .iter()
            .map(|m| m.file.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let models = plain_row("Models", Some(&subtitle));
    let setup = gtk::Button::builder()
        .label("Set up")
        .valign(gtk::Align::Center)
        .tooltip_text("Download the DLSSNR-AMD package and extract the model from nvngx_dlssnr.dll")
        .build();
    {
        let ui = ui.clone();
        setup.connect_clicked(move |_| {
            spawn_cli_tracked(&ui, &["neural", "setup"], "Setting up neural rendering")
        });
    }
    models.add_suffix(&setup);
    group.add(&models);
    group
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferestre_core::anticheat::Detection;
    use ferestre_core::neural::detect::{Api, Detected};

    fn det(name: &'static str, kernel: bool, proton_runtime: bool) -> Detection {
        Detection {
            name,
            kernel,
            proton_runtime,
            evidence: PathBuf::new(),
        }
    }

    fn overview(anticheat: Vec<Detection>, gate: Gate, route: Option<Route>) -> Overview {
        Overview {
            entry: None,
            detected: Detected {
                bits: 64,
                api: Api::Dx10To12,
                upscalers: Vec::new(),
                assumed: false,
                route,
            },
            anticheat,
            gate,
            installed: None,
            models: Vec::new(),
            package_ready: false,
            hardware: neural::hardware::Hardware::default(),
            dll_url_configured: false,
        }
    }

    #[test]
    fn a_kernel_anti_cheat_asks_for_the_strongest_acknowledgement() {
        let v = view(&overview(
            vec![det("Ricochet", true, false), det("BattlEye", true, true)],
            Gate::NeedsAcknowledgement(vec!["Ricochet", "BattlEye"]),
            Some(Route::ReshadeFeeder),
        ));
        assert!(v.can_enable);
        assert_eq!(v.blocked_because, None);
        assert_eq!(v.acknowledge, vec!["Ricochet", "BattlEye"]);
        assert_eq!(v.kernel, vec!["Ricochet"]);
        assert!(
            v.warnings
                .iter()
                .any(|w| w.contains("Ricochet is a kernel-level")),
            "{:?}",
            v.warnings
        );
        assert!(
            !enable_sensitive(&v, 0, Some(false)),
            "not without the tick"
        );
        assert!(enable_sensitive(&v, 0, Some(true)));
    }

    #[test]
    fn an_unsupported_gpu_disables_enable_for_every_game() {
        let reason = "Neural rendering needs an AMD Radeon RX 9000 (RDNA4) GPU; \
                      this machine has an NVIDIA GPU.";
        for (anticheat, gate) in [
            (Vec::new(), Gate::Allowed),
            (
                vec![det("BattlEye", true, true)],
                Gate::NeedsAcknowledgement(vec!["BattlEye"]),
            ),
        ] {
            let mut o = overview(anticheat, gate, Some(Route::Optiscaler));
            o.hardware.unsupported = Some(reason.to_string());
            let v = view(&o);
            assert!(!v.can_enable);
            assert_eq!(v.blocked_because.as_deref(), Some(reason));
            assert!(v.acknowledge.is_empty());
            for route in 0..=Route::ALL.len() as u32 {
                assert!(!enable_sensitive(&v, route, Some(true)));
            }
        }
    }

    #[test]
    fn another_anti_cheat_asks_for_an_acknowledgement() {
        let v = view(&overview(
            vec![det("Easy Anti-Cheat", true, true)],
            Gate::NeedsAcknowledgement(vec!["Easy Anti-Cheat"]),
            Some(Route::Optiscaler),
        ));
        assert_eq!(v.acknowledge, vec!["Easy Anti-Cheat"]);
        assert!(v.can_enable);
        assert_eq!(v.blocked_because, None);
        assert_eq!(v.route, Some(Route::Optiscaler));
    }

    #[test]
    fn an_undetectable_route_still_lets_the_person_pick_one() {
        let v = view(&overview(Vec::new(), Gate::Allowed, None));
        assert_eq!(v.route, None);
        assert!(v.can_enable);
        assert!(!v.enabled);
    }

    #[test]
    fn dropdown_indexes_map_to_automatic_then_each_choice() {
        assert_eq!(chosen_route(0), None);
        for (i, route) in Route::ALL.iter().enumerate() {
            assert_eq!(chosen_route(i as u32 + 1), Some(*route));
        }
        assert_eq!(chosen_route(Route::ALL.len() as u32 + 1), None);
        assert_eq!(chosen_network(0), None);
        for (i, network) in Network::ALL.iter().enumerate() {
            assert_eq!(chosen_network(i as u32 + 1), Some(*network));
        }
        assert_eq!(chosen_network(Network::ALL.len() as u32 + 1), None);
    }

    #[test]
    fn enable_is_pressable_only_with_a_route_a_tick_and_no_refusal() {
        let ask = view(&overview(
            vec![det("BattlEye", true, true)],
            Gate::NeedsAcknowledgement(vec!["BattlEye"]),
            Some(Route::Optiscaler),
        ));
        assert!(!enable_sensitive(&ask, 0, Some(false)));
        assert!(enable_sensitive(&ask, 0, Some(true)));

        let undetected = view(&overview(Vec::new(), Gate::Allowed, None));
        assert!(
            !enable_sensitive(&undetected, 0, None),
            "Automatic (none detected)"
        );
        assert!(enable_sensitive(&undetected, 2, None));
    }

    #[test]
    fn accepting_names_each_anti_cheat_that_was_shown() {
        assert!(shown_args(&[]).is_empty());
        assert_eq!(
            shown_args(&["Easy Anti-Cheat", "BattlEye"]),
            [
                "--anticheat-shown",
                "Easy Anti-Cheat",
                "--anticheat-shown",
                "BattlEye"
            ]
        );
    }

    #[test]
    fn enable_args_carry_only_what_was_chosen() {
        assert_eq!(
            enable_args("9NBLGGH2JHXJ", None, None, false),
            ["neural", "enable", "9NBLGGH2JHXJ"]
        );
        assert_eq!(
            enable_args("9NBLGGH2JHXJ", None, None, true),
            [
                "neural",
                "enable",
                "9NBLGGH2JHXJ",
                "--accept-anticheat-risk"
            ]
        );
        assert_eq!(
            enable_args(
                "9NBLGGH2JHXJ",
                Some(Route::ReshadeFeederVulkan),
                Some(Network::SmallFp8),
                false
            ),
            [
                "neural",
                "enable",
                "9NBLGGH2JHXJ",
                "--route",
                "reshade-feeder-vulkan",
                "--model",
                "CC_SMALL_FP8"
            ]
        );
    }
}
