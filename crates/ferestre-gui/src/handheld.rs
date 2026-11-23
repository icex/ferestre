//! Fullscreen browsing keeps the games in reach on 720p and 800p handhelds.
use super::*;

pub(super) const SECTIONS: [(&str, Section); 6] = [
    ("Home", Section::Home),
    ("Game Pass", Section::GamePass),
    ("My Library", Section::Library),
    ("Installed", Section::Installed),
    ("Updates", Section::Updates),
    ("Install Queue", Section::Downloads),
];

/// A saved window preference (`window.json`), such as `fullscreen` or
/// `compact`.
pub(super) fn read_preference(ui: &Ui, key: &str) -> Option<bool> {
    read_value(ui, key).and_then(|v| v.as_bool())
}

pub(super) fn read_value(ui: &Ui, key: &str) -> Option<serde_json::Value> {
    preference(ui)
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.get(key).cloned())
}

/// Save one window preference, keeping the others.
pub(super) fn save_preference(ui: &Ui, key: &str, value: bool) -> anyhow::Result<()> {
    save_value(ui, key, serde_json::Value::Bool(value))
}

pub(super) fn save_value(ui: &Ui, key: &str, value: serde_json::Value) -> anyhow::Result<()> {
    let Some(path) = preference(ui) else {
        return Ok(());
    };
    let mut saved = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .filter(|v| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    saved[key] = value;
    ferestre_core::cache::write(&path, &serde_json::to_vec(&saved)?)?;
    Ok(())
}

fn preference(ui: &Ui) -> Option<PathBuf> {
    ui.model
        .borrow()
        .paths
        .as_ref()
        .map(|p| p.config_dir().join("window.json"))
}

pub(super) fn install(
    ui: &Ui,
    view: &adw::ToolbarView,
    header: &adw::HeaderBar,
    back: &gtk::Button,
) {
    let button = gtk::Button::from_icon_name("view-fullscreen-symbolic");
    button.set_tooltip_text(Some("Enter fullscreen (F11)"));
    button.update_property(&[gtk::accessible::Property::Label("Enter fullscreen")]);
    header.pack_end(&button);
    let window = ui.window.clone();
    button.connect_clicked(move |_| {
        if window.is_fullscreen() {
            window.unfullscreen();
        } else {
            window.fullscreen();
        }
    });
    for (label, section) in SECTIONS {
        let tab = gtk::Button::with_label(label);
        tab.set_widget_name(&format!("fullscreen-nav-{}", section.title()));
        let tab_ui = ui.clone();
        tab.connect_clicked(move |_| select_section(&tab_ui, section));
        ui.fullscreen_nav.append(&tab);
    }
    let navigation = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .child(&ui.fullscreen_nav)
        .visible(false)
        .build();
    view.add_top_bar(&navigation);
    let (update_ui, header, back) = (ui.clone(), header.clone(), back.clone());
    ui.window.connect_fullscreened_notify(move |window| {
        let ui = &update_ui;
        let fullscreen = window.is_fullscreen();
        if fullscreen {
            window.add_css_class("fullscreen-browser");
        } else {
            window.remove_css_class("fullscreen-browser");
        }
        header.set_show_back_button(!fullscreen);
        back.set_visible(!fullscreen);
        navigation.set_visible(fullscreen);
        ui.split.set_collapsed(fullscreen || window.width() <= 860);
        ui.split.set_show_content(true);
        button.set_icon_name(if fullscreen {
            "view-restore-symbolic"
        } else {
            "view-fullscreen-symbolic"
        });
        let label = if fullscreen {
            "Exit fullscreen"
        } else {
            "Enter fullscreen"
        };
        button.set_tooltip_text(Some(&format!("{label} (F11)")));
        button.update_property(&[gtk::accessible::Property::Label(label)]);
        if let Err(error) = save_preference(ui, "fullscreen", fullscreen) {
            ui.toasts.add_toast(toast(format!(
                "Could not save fullscreen preference: {error}"
            )));
        }
        render(ui);
        if fullscreen {
            let ui = ui.clone();
            glib::idle_add_local_once(move || controller::focus_content(&ui));
        }
    });
    let window = ui.window.clone();
    ui.split.connect_collapsed_notify(move |split| {
        // A width breakpoint restoring the desktop sidebar must not shrink
        // fullscreen content when the display rotates or its scale changes.
        if window.is_fullscreen() && !split.is_collapsed() {
            split.set_collapsed(true);
        }
    });
    if read_preference(ui, "fullscreen") == Some(true) {
        ui.window.fullscreen();
    }
}

pub(super) fn render_navigation(ui: &Ui) {
    let active = format!("fullscreen-nav-{}", ui.model.borrow().section.title());
    let mut child = ui.fullscreen_nav.first_child();
    while let Some(tab) = child {
        if tab.widget_name() == active {
            tab.add_css_class("active");
        } else {
            tab.remove_css_class("active");
        }
        child = tab.next_sibling();
    }
}
