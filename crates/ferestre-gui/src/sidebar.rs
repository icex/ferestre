//! The sidebar's lower half: every installed title with a status line, and a
//! downloads button while the install queue has work.
//!
//! The list is drawn from the same rows the game cards are ([`draw_list`]),
//! so the sidebar and the Installed tab never disagree about what is
//! installed. It is rebuilt only when what it shows changed, and the
//! download's percentage is updated in place from the download bar.
use super::*;
use std::hash::{Hash, Hasher};

/// A status line's words and its icon.
type Status = (String, &'static str);

/// What one installed title's line says, most urgent first.
fn status(ui: &Ui, row: &LibraryRow, downloading: Option<&str>) -> Option<Status> {
    let key = row.product_id.to_ascii_uppercase();
    if downloading.is_some_and(|d| d.eq_ignore_ascii_case(&key)) {
        let paused = ui
            .download_control
            .borrow()
            .as_ref()
            .is_some_and(|c| c.paused());
        return Some(if paused {
            ("Paused".into(), "media-playback-pause-symbolic")
        } else {
            (
                format!("Downloading {}%", percent(ui)),
                "folder-download-symbolic",
            )
        });
    }
    if ui
        .install_queue
        .borrow()
        .jobs
        .iter()
        .any(|j| j.product_id.eq_ignore_ascii_case(&key))
    {
        return Some(("Queued".into(), "content-loading-symbolic"));
    }
    if row.has_update() {
        return Some((
            // Not the cards' "Update available" badge: one name, one badge.
            "Update ready".into(),
            "software-update-available-symbolic",
        ));
    }
    if row.action == Action::Stop {
        return Some(("Running".into(), "media-playback-start-symbolic"));
    }
    if row.unavailable.is_some() {
        return Some((
            "Unavailable to this account".into(),
            "action-unavailable-symbolic",
        ));
    }
    match row.badge.map(|(label, _)| label) {
        Some("Issues") => Some(("Playable with issues".into(), "dialog-warning-symbolic")),
        Some("Menus only") => Some(("Menus only".into(), "dialog-warning-symbolic")),
        Some("Does not run") => Some(("Does not run".into(), "dialog-error-symbolic")),
        _ => None,
    }
}

fn percent(ui: &Ui) -> u32 {
    (ui.download.bar.fraction().clamp(0.0, 1.0) * 100.0).round() as u32
}

/// Redraw the Installed list, if anything it shows changed.
pub(crate) fn render_installed(ui: &Ui) {
    let downloading = ui
        .download
        .current
        .borrow()
        .as_ref()
        .map(|d| d.product_id.clone());
    // Every installed row, not one page of them, and never narrowed by the
    // search box.
    let drawn = draw_list(
        &ui.model.borrow(),
        &ui.running.borrow(),
        downloading.as_deref(),
        NameLookups {
            asked: &ui.asked_names.borrow(),
            fetching: ui.fetching_names.get(),
        },
        Section::Installed,
        |row| row.installed,
        Listing::All,
    );
    let mut titles: Vec<(Option<std::time::SystemTime>, LibraryRow)> = {
        let model = ui.model.borrow();
        drawn
            .rows
            .into_iter()
            .map(|row| (model.last_played(&row.product_id), row))
            .collect()
    };
    titles.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase()))
    });
    let lines: Vec<(LibraryRow, Option<Status>, bool)> = {
        let model = ui.model.borrow();
        titles
            .into_iter()
            .map(|(_, row)| {
                let line = status(ui, &row, downloading.as_deref());
                let icon = model
                    .icons
                    .contains_key(&row.product_id.to_ascii_uppercase());
                (row, line, icon)
            })
            .collect()
    };
    let compact = ui.compact.get();
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    compact.hash(&mut hash);
    for (row, line, icon) in &lines {
        row.product_id.hash(&mut hash);
        row.name.hash(&mut hash);
        icon.hash(&mut hash);
        // The percentage changes in place; only whether it is downloading
        // rebuilds the list.
        line.as_ref()
            .map(|(text, _)| {
                if text.starts_with("Downloading") {
                    "Downloading"
                } else {
                    text.as_str()
                }
            })
            .hash(&mut hash);
    }
    let fingerprint = hash.finish();
    if ui.recents_fingerprint.replace(fingerprint) == fingerprint
        && ui.recent_games.first_child().is_some()
    {
        return;
    }
    while let Some(child) = ui.recent_games.first_child() {
        ui.recent_games.remove(&child);
    }
    ui.sidebar_progress.replace(None);
    ui.recent_games.append(
        &gtk::Label::builder()
            .label("Installed")
            .xalign(0.0)
            .margin_top(10)
            .margin_bottom(4)
            .margin_start(6)
            .css_classes(["caption-heading", "dim-label", "sidebar-heading"])
            .visible(!compact)
            .build(),
    );
    if lines.is_empty() && !compact {
        ui.recent_games.append(
            &gtk::Label::builder()
                .label("Your installed games appear here")
                .wrap(true)
                .xalign(0.0)
                .margin_start(6)
                .css_classes(["caption", "dim-label", "sidebar-heading"])
                .build(),
        );
    }
    for (row, line, _) in lines {
        let id = row.product_id.to_ascii_uppercase();
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let image = gtk::Image::builder()
            .icon_name("applications-games-symbolic")
            .pixel_size(40)
            .width_request(40)
            .height_request(40)
            .css_classes(["sidebar-icon"])
            .build();
        if let Some(path) = ui.model.borrow().icons.get(&id) {
            textures::set_image(&image, path, textures::Size::Icon);
        }
        ui.art_slots
            .borrow_mut()
            .add_image(id.clone(), &image, textures::Size::Icon);
        content.append(&image);
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .valign(gtk::Align::Center)
            .css_classes(["sidebar-text"])
            .build();
        text.append(
            &gtk::Label::builder()
                .label(&row.name)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .max_width_chars(20)
                .css_classes(["sidebar-title"])
                .build(),
        );
        let mut description = row.name.clone();
        if let Some((words, icon)) = &line {
            let status = gtk::Box::new(gtk::Orientation::Horizontal, 5);
            let glyph = gtk::Image::from_icon_name(icon);
            glyph.set_pixel_size(12);
            status.append(&glyph);
            let label = gtk::Label::builder()
                .label(words)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .max_width_chars(22)
                .build();
            if words.starts_with("Downloading") {
                ui.sidebar_progress.replace(Some(label.downgrade()));
            }
            status.append(&label);
            status.add_css_class("sidebar-status");
            if words == "Update ready" || words == "Running" {
                status.add_css_class("accent");
            }
            text.append(&status);
            description = format!("{} · {words}", row.name);
        }
        if compact {
            text.set_visible(false);
            content.set_halign(gtk::Align::Center);
        }
        content.append(&text);
        let action = match row.action {
            Action::Stop => "Resume",
            _ => "Play",
        };
        let button = gtk::Button::builder()
            .child(&content)
            .tooltip_text(&description)
            .build();
        button.set_widget_name(&format!("recent-{id}"));
        button.update_property(&[
            gtk::accessible::Property::Label(&row.name),
            gtk::accessible::Property::Description(&format!("{action} · {description}")),
        ]);
        let (recent_ui, name) = (ui.clone(), row.name.clone());
        button.connect_clicked(move |_| spawn_title(&recent_ui, &id, &name));
        ui.recent_games.append(&button);
    }
}

/// The downloads button in the sidebar footer: shown while the queue has
/// work, with a thin bar under the icon that follows the download bar.
pub(crate) fn downloads_button(ui: &Ui) -> gtk::Button {
    let icon = gtk::Image::from_icon_name("folder-download-symbolic");
    let bar = gtk::ProgressBar::builder()
        .css_classes(["sidebar-progress"])
        .valign(gtk::Align::End)
        .build();
    ui.download
        .bar
        .bind_property("fraction", &bar, "fraction")
        .sync_create()
        .build();
    let column = gtk::Box::new(gtk::Orientation::Vertical, 3);
    column.append(&icon);
    column.append(&bar);
    column.set_valign(gtk::Align::Center);
    let label = gtk::Label::builder()
        .label("Downloads")
        .xalign(0.0)
        .hexpand(true)
        .css_classes(["sidebar-label"])
        .build();
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    content.append(&column);
    content.append(&label);
    let button = gtk::Button::builder()
        .child(&content)
        .halign(gtk::Align::Fill)
        .visible(false)
        .tooltip_text("Downloads")
        .build();
    button.set_widget_name("navigation-downloads");
    button.update_property(&[gtk::accessible::Property::Label("Downloads")]);
    let click_ui = ui.clone();
    button.connect_clicked(move |_| select_section(&click_ui, Section::Downloads));
    // The percentage, on the button and on the downloading title's line, in
    // place: a redraw four times a second would take the focus with it.
    let (weak, progress_ui) = (button.downgrade(), ui.clone());
    // The download bar's tick sets the fraction while it holds the download
    // state, so the labels follow from the main loop, not from inside it.
    ui.download.bar.connect_fraction_notify(move |_| {
        let (weak, progress_ui) = (weak.clone(), progress_ui.clone());
        glib::idle_add_local_once(move || follow_progress(&progress_ui, weak.upgrade()));
    });
    button
}

fn follow_progress(progress_ui: &Ui, button: Option<gtk::Button>) {
    {
        if let Some(button) = button {
            label_downloads(progress_ui, &button);
        }
        let percent = percent(progress_ui);
        if let Some(label) = progress_ui
            .sidebar_progress
            .borrow()
            .as_ref()
            .and_then(|w| w.upgrade())
        {
            if label.label().starts_with("Downloading") {
                label.set_label(&format!("Downloading {percent}%"));
            }
        }
    }
}

fn label_downloads(ui: &Ui, button: &gtk::Button) {
    let active = ui
        .download
        .current
        .try_borrow()
        .map_or(true, |c| c.is_some());
    let name = if active {
        format!("Downloads, {}%", percent(ui))
    } else {
        "Downloads".to_string()
    };
    button.update_property(&[gtk::accessible::Property::Label(&name)]);
    button.set_tooltip_text(Some(&name));
}

/// Show the downloads button while the queue has anything in it.
pub(crate) fn render_downloads(ui: &Ui) {
    let Some(button) = ui.downloads_button.borrow().clone() else {
        return;
    };
    let busy = !ui.install_queue.borrow().jobs.is_empty()
        || ui
            .download
            .current
            .try_borrow()
            .map_or(true, |c| c.is_some());
    button.set_visible(busy);
    if busy {
        label_downloads(ui, &button);
    }
}

/// Compact mode: the sidebar becomes a rail of icons. Every control keeps
/// its name (set outright, not taken from a label), so the controller's
/// order and assistive technology see the same sidebar; the names show as
/// tooltips.
pub(crate) fn set_compact(ui: &Ui, compact: bool) {
    ui.compact.set(compact);
    let (min, max) = if compact {
        (72.0, 72.0)
    } else {
        (250.0, 270.0)
    };
    ui.split.set_min_sidebar_width(min);
    ui.split.set_max_sidebar_width(max);
    if compact {
        ui.window.add_css_class("compact-sidebar");
    } else {
        ui.window.remove_css_class("compact-sidebar");
    }
    let mut row = ui.sidebar.first_child();
    while let Some(widget) = row {
        if let Some(label) = widget
            .first_child()
            .and_then(|content| content.last_child())
            .filter(|w| w.has_css_class("nav-label"))
        {
            label.set_visible(!compact);
        }
        if let Some(content) = widget.first_child() {
            content.set_halign(if compact {
                gtk::Align::Center
            } else {
                gtk::Align::Fill
            });
        }
        row = widget.next_sibling();
    }
    if let Some(top) = named_widget(ui.window.upcast_ref(), "sidebar-top")
        .and_then(|w| w.downcast::<gtk::Box>().ok())
    {
        top.set_orientation(if compact {
            gtk::Orientation::Vertical
        } else {
            gtk::Orientation::Horizontal
        });
        top.set_margin_end(if compact { 0 } else { 8 });
    }
    for name in ["navigation-settings", "navigation-downloads"] {
        if let Some(button) = named_widget(ui.window.upcast_ref(), name) {
            hide_labels(&button, compact);
        }
    }
    ui.navigation_hint.set_visible(false);
    ui.recents_fingerprint.set(0);
    render(ui);
}

/// Hide (or show) the text inside a footer button, keeping its icon.
fn hide_labels(root: &gtk::Widget, hide: bool) {
    let mut child = root.first_child();
    while let Some(widget) = child {
        if widget.is::<gtk::Label>() {
            widget.set_visible(!hide);
        } else {
            hide_labels(&widget, hide);
        }
        child = widget.next_sibling();
    }
}
