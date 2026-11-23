//! One game's details: what the window knows, and what Microsoft's catalog says.
use super::*;
use ferestre_core::product_details;

/// `known` is what the window knows already. A local game has no Store page,
/// and its id is not sent anywhere.
pub fn open(ui: &Ui, product_id: &str, name: &str, known: &str, local: bool) {
    let dialog = adw::Dialog::builder()
        .title(name)
        .content_width(640)
        .content_height(720)
        .build();
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();
    content.append(&paragraph(known));
    if !local {
        let store = gtk::Button::builder()
            .label("Open Store page")
            .halign(gtk::Align::Start)
            .build();
        let url = format!("{STORE_PAGE}{product_id}");
        store.connect_clicked(move |button| open_link(button, &url));
        content.append(&store);
        load(ui, product_id, &content);
    }
    let close = gtk::Button::with_label("Close");
    let header = adw::HeaderBar::new();
    header.pack_start(&close);
    {
        let dialog = dialog.clone();
        close.connect_clicked(move |_| {
            dialog.close();
        });
    }
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&content)
            .build(),
    ));
    dialog.set_child(Some(&view));
    dialog.present(Some(&ui.window));
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

fn load(ui: &Ui, product_id: &str, content: &gtk::Box) {
    let (market, language) = ui.model.borrow().market();
    let waiting = paragraph("Asking Microsoft’s catalog…");
    content.append(&waiting);
    let content = content.clone();
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
        content.remove(&waiting);
        if !details.description.is_empty() {
            content.append(&paragraph(&details.description));
        }
        for group in details.groups {
            let rows = adw::PreferencesGroup::builder().title(group.title).build();
            for (label, value) in group.rows {
                let row = plain_row(&label, Some(&value));
                row.add_css_class("property");
                row.set_subtitle_selectable(true);
                rows.add(&row);
            }
            content.append(&rows);
        }
    });
}
