//! One game's details: what the window knows, and what Microsoft's catalog says.
use super::*;
use ferestre_core::product_details;

/// The tabs after Overview, each with the catalog's groups it shows. A group
/// named in none of them is on Overview, under the description.
const TABS: [(&str, &[&str]); 5] = [
    ("Features", &["Features"]),
    ("Requirements", &["System requirements"]),
    ("Age ratings", &["Age ratings"]),
    ("Package", &["Package"]),
    ("Links and legal", &["Links and legal"]),
];

/// `known` is what the window knows already. A local game has no Store page,
/// and its id is not sent anywhere.
pub fn open(ui: &Ui, product_id: &str, name: &str, known: &str, local: bool) {
    let dialog = adw::Dialog::builder()
        .title(name)
        .content_width(720)
        .content_height(720)
        .build();
    let stack = gtk::Stack::builder().vexpand(true).build();
    let overview = page(&stack, "Overview");
    overview.append(&paragraph(known));
    let switcher = gtk::StackSwitcher::builder()
        .stack(&stack)
        .halign(gtk::Align::Center)
        .margin_start(12)
        .margin_end(12)
        .margin_bottom(6)
        .build();
    let tabs = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .child(&switcher)
        // One tab is nothing to choose from.
        .visible(false)
        .build();
    if !local {
        let store = gtk::Button::builder()
            .label("Open Store page")
            .halign(gtk::Align::Start)
            .build();
        let url = format!("{STORE_PAGE}{product_id}");
        store.connect_clicked(move |button| open_link(button, &url));
        overview.append(&store);
        load(ui, product_id, &stack, &tabs, &overview);
    }
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.add_top_bar(&tabs);
    view.set_content(Some(&stack));
    dialog.set_child(Some(&view));
    dialog.present(Some(&ui.window));
}

/// Add a tab and return the box its content goes in.
fn page(stack: &gtk::Stack, title: &str) -> gtk::Box {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&content)
        .build();
    stack.add_titled(&scroll, Some(title), title);
    content
}

fn paragraph(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .wrap(true)
        .xalign(0.0)
        .selectable(true)
        .focusable(false)
        .build()
}

fn group_rows(group: &product_details::Group) -> adw::PreferencesGroup {
    let rows = adw::PreferencesGroup::builder().title(group.title).build();
    for (label, value) in &group.rows {
        let row = plain_row(label, Some(value));
        row.add_css_class("property");
        if let Some(url) = product_details::link(value) {
            let open = gtk::Button::builder()
                .icon_name("web-browser-symbolic")
                .tooltip_text(url)
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            open.update_property(&[gtk::accessible::Property::Label(&format!("Open {label}"))]);
            let url = url.to_string();
            open.connect_clicked(move |button| open_link(button, &url));
            row.add_suffix(&open);
            row.set_activatable_widget(Some(&open));
        } else {
            row.set_subtitle_selectable(true);
        }
        rows.add(&row);
    }
    rows
}

/// The banner and screenshots, one at a time. Each is fetched in turn while
/// the page is open; one that cannot be fetched is left out.
fn carousel(ui: &Ui, urls: Vec<String>) -> Option<gtk::Box> {
    let paths = ui.model.borrow().paths.clone()?;
    let dir = paths.cache_dir().join("catalog").join("pictures");
    if urls.is_empty() {
        return None;
    }
    let carousel = adw::Carousel::builder()
        .height_request(340)
        .hexpand(true)
        // The wheel scrolls the page, which is longer than its pictures.
        .allow_scroll_wheel(false)
        .build();
    let turn = |icon: &str, label: &str, align: gtk::Align, step: i32| {
        let button = gtk::Button::builder()
            .icon_name(icon)
            .tooltip_text(label)
            .halign(align)
            .valign(gtk::Align::Center)
            .margin_start(8)
            .margin_end(8)
            .css_classes(["osd", "circular"])
            .build();
        button.update_property(&[gtk::accessible::Property::Label(label)]);
        let carousel = carousel.downgrade();
        button.connect_clicked(move |_| {
            let Some(carousel) = carousel.upgrade() else {
                return;
            };
            let count = carousel.n_pages() as i32;
            if count > 0 {
                let next = (carousel.position().round() as i32 + step).rem_euclid(count);
                carousel.scroll_to(&carousel.nth_page(next as u32), true);
            }
        });
        button
    };
    let stage = gtk::Overlay::builder().child(&carousel).build();
    stage.add_overlay(&turn(
        "go-previous-symbolic",
        "Previous picture",
        gtk::Align::Start,
        -1,
    ));
    stage.add_overlay(&turn(
        "go-next-symbolic",
        "Next picture",
        gtk::Align::End,
        1,
    ));
    let dots = adw::CarouselIndicatorDots::new();
    dots.set_carousel(Some(&carousel));
    let all = gtk::Box::new(gtk::Orientation::Vertical, 6);
    all.append(&stage);
    all.append(&dots);
    all.set_visible(false);

    let (weak_all, weak) = (all.downgrade(), carousel.downgrade());
    glib::spawn_future_local(async move {
        for url in urls {
            let dir = dir.clone();
            let got = gio::spawn_blocking(move || product_details::picture(&dir, &url)).await;
            let (Some(all), Some(carousel)) = (weak_all.upgrade(), weak.upgrade()) else {
                return;
            };
            let Ok(Ok(path)) = got else { continue };
            let picture = gtk::Picture::builder()
                .can_shrink(true)
                .content_fit(gtk::ContentFit::Contain)
                .hexpand(true)
                .build();
            picture.set_alternative_text(Some("Picture from the game’s Store page"));
            textures::set_picture(&picture, &path, textures::Size::Hero);
            carousel.append(&picture);
            all.set_visible(true);
        }
    });
    Some(all)
}

fn load(
    ui: &Ui,
    product_id: &str,
    stack: &gtk::Stack,
    tabs: &gtk::ScrolledWindow,
    overview: &gtk::Box,
) {
    let (market, language) = ui.model.borrow().market();
    let waiting = paragraph("Asking Microsoft’s catalog…");
    overview.append(&waiting);
    let ui = ui.clone();
    let (stack, tabs, overview) = (stack.clone(), tabs.clone(), overview.clone());
    let product_id = product_id.to_string();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || {
            product_details::fetch(&product_id, &market, &language).map_err(|e| format!("{e:#}"))
        })
        .await
        .unwrap_or_else(|_| Err("the request stopped".into()));
        let details = match result {
            Ok(details) => details,
            Err(error) => {
                waiting.set_label(&format!("Microsoft’s catalog could not be read: {error}"));
                return;
            }
        };
        overview.remove(&waiting);
        if *ui.closing.borrow() {
            return;
        }
        if let Some(pictures) = carousel(&ui, details.pictures) {
            overview.prepend(&pictures);
        }
        if !details.description.is_empty() {
            overview.append(&paragraph(&details.description));
        }
        let on_tab = |title: &str| TABS.iter().any(|(_, groups)| groups.contains(&title));
        for group in details.groups.iter().filter(|g| !on_tab(g.title)) {
            overview.append(&group_rows(group));
        }
        for (title, wanted) in TABS {
            let groups: Vec<_> = details
                .groups
                .iter()
                .filter(|g| wanted.contains(&g.title))
                .collect();
            if groups.is_empty() {
                continue;
            }
            let content = page(&stack, title);
            for group in groups {
                content.append(&group_rows(group));
            }
            tabs.set_visible(true);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tabs_name_groups_the_catalog_gives() {
        let source = include_str!("../../ferestre-core/src/product_details.rs");
        for group in TABS.iter().flat_map(|(_, groups)| groups.iter()) {
            assert!(
                source.contains(&format!("into_group(\"{group}\"")),
                "{group}"
            );
        }
    }
}
