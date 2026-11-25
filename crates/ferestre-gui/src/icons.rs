use gtk::{gdk, gio};

pub(super) const RESOURCE_PATH: &str = "/io/github/icex/ferestre/icons";

pub(super) fn install(display: &gdk::Display) {
    gio::resources_register_include!("ferestre-icons.gresource")
        .expect("register Ferestre's embedded icons");
    let theme = gtk::IconTheme::for_display(display);
    // Use the same artwork under every desktop theme. Keep GTK and Adwaita's
    // embedded resources available for their own secondary widgets.
    let fallback = theme.resource_path();
    let paths: Vec<_> = std::iter::once(RESOURCE_PATH)
        .chain(fallback.iter().map(|path| path.as_str()))
        .collect();
    theme.set_resource_path(&paths);
    theme.set_search_path(&[]);
    gtk::Settings::for_display(display).set_gtk_icon_theme_name(Some("hicolor"));
}
