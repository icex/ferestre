//! Fullscreen browsing keeps the games in reach on 720p and 800p handhelds. A
//! larger screen has room for the sidebar, fullscreen or not.
use super::*;

pub(super) const SECTIONS: [Section; 7] = [
    Section::Home,
    Section::GamePass,
    Section::Installed,
    Section::Library,
    Section::Elsewhere,
    Section::Updates,
    Section::Downloads,
];

/// The window's class where a top bar of tabs stands in for the sidebar:
/// fullscreen, on a small screen.
const TOP_BAR: &str = "fullscreen-browser";

/// Whether a top bar of tabs stands in for the sidebar.
pub(super) fn top_bar(ui: &Ui) -> bool {
    ui.window.has_css_class(TOP_BAR)
}

/// The window's class on a small screen, where lists are compact.
pub(super) const CLASS: &str = "handheld";

/// Whether a view this large, on a screen this wide, is a handheld's. The
/// screen's width is in millimetres, 0 when it is not known, and then the view
/// decides: a small window on a desktop's screen is not a handheld.
fn is_handheld(width: i32, height: i32, screen: i32) -> bool {
    if width <= 0 || height <= 0 {
        return false;
    }
    match screen {
        // Implausible: some compositors make a size up.
        ..=50 => height <= 820 && width <= 1366,
        screen => screen <= 230,
    }
}

/// Follow the window's size and screen; true when it changed.
pub(super) fn update(ui: &Ui) -> bool {
    let screen = ui
        .window
        .surface()
        .and_then(|surface| surface.display().monitor_at_surface(&surface))
        .map_or(0, |monitor| monitor.width_mm());
    let handheld = is_handheld(ui.window.width(), ui.window.height(), screen);
    if handheld == ui.window.has_css_class(CLASS) {
        return false;
    }
    if handheld {
        ui.window.add_css_class(CLASS);
    } else {
        ui.window.remove_css_class(CLASS);
    }
    true
}

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
    for section in SECTIONS {
        let tab = section_tab(section);
        tab.set_widget_name(&format!("fullscreen-nav-{}", section.title()));
        let tab_ui = ui.clone();
        tab.connect_clicked(move |_| select_section(&tab_ui, section));
        ui.fullscreen_nav.append(&tab);
    }
    let menu = library_menu(ui);
    menu.set_widget_name("fullscreen-library-menu");
    let bar = gtk::Box::builder()
        .spacing(8)
        .halign(gtk::Align::Center)
        .css_classes(["fullscreen-navigation"])
        .build();
    bar.append(&ui.fullscreen_nav);
    bar.append(&menu);
    let navigation = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .child(&bar)
        .visible(false)
        .build();
    view.add_top_bar(&navigation);
    let lay_out = {
        let (ui, header, back) = (ui.clone(), header.clone(), back.clone());
        move || {
            let top = ui.window.is_fullscreen() && ui.window.has_css_class(CLASS);
            if top == top_bar(&ui) {
                return;
            }
            if top {
                ui.window.add_css_class(TOP_BAR);
            } else {
                ui.window.remove_css_class(TOP_BAR);
            }
            header.set_show_back_button(!top);
            back.set_visible(!top);
            navigation.set_visible(top);
            ui.split.set_collapsed(top || ui.window.width() <= 860);
            ui.split.set_show_content(true);
            render_soon(&ui);
        }
    };
    let lay_out = Rc::new(lay_out);
    let sized = lay_out.clone();
    // The screen's size may be known only once the window is on it.
    ui.window.connect_css_classes_notify(move |_| sized());
    let update_ui = ui.clone();
    ui.window.connect_fullscreened_notify(move |window| {
        let ui = &update_ui;
        let fullscreen = window.is_fullscreen();
        update(ui);
        lay_out();
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
        // the content when the display rotates or its scale changes.
        if window.has_css_class(TOP_BAR) && !split.is_collapsed() {
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

#[cfg(test)]
mod tests {
    use super::is_handheld;

    #[test]
    fn small_screens_are_handhelds() {
        // 7 to 8 inch screens, scaled, rotated and not.
        assert!(is_handheld(1280, 720, 155));
        assert!(is_handheld(1280, 800, 0));
        assert!(is_handheld(1920, 1080, 155));
        assert!(!is_handheld(1920, 1080, 340));
        // A small window on a large screen.
        assert!(!is_handheld(1280, 800, 600));
        assert!(!is_handheld(2461, 1173, 800));
        assert!(!is_handheld(0, 0, 155));
    }
}
