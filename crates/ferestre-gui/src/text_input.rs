//! An in-app keyboard for controller text entry. Text is committed only on Done.
use super::*;

fn edit(text: &str, key: &str) -> String {
    match key {
        "Backspace" => {
            let mut value = text.to_string();
            value.pop();
            value
        }
        "Clear" => String::new(),
        "Space" => format!("{text} "),
        "New line" => format!("{text}\n"),
        value => format!("{text}{value}"),
    }
}
pub fn open(ui: &Ui, target: &gtk::Editable, parent: &gtk::Window) {
    let secret = target.is::<gtk::PasswordEntry>()
        || (target.find_property("visibility").is_some() && !target.property::<bool>("visibility"));
    let initial = target.text().to_string();
    let target = target.clone();
    show(ui, initial, secret, false, parent, move |text| {
        target.set_text(text);
        target.set_position(-1);
    });
}
pub fn open_view(ui: &Ui, view: &gtk::TextView, parent: &gtk::Window) {
    let buffer = view.buffer();
    let initial = buffer
        .text(&buffer.start_iter(), &buffer.end_iter(), false)
        .to_string();
    show(ui, initial, false, true, parent, move |text| {
        buffer.set_text(text)
    });
}
fn show(
    ui: &Ui,
    initial: String,
    secret: bool,
    multiline: bool,
    parent: &gtk::Window,
    commit: impl Fn(&str) + 'static,
) {
    let dialog = adw::Dialog::builder()
        .title("On-screen keyboard")
        .content_width(680)
        .content_height(440)
        .build();
    dialog.set_widget_name("controller-keyboard");
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();
    let value = Rc::new(RefCell::new(initial));
    let preview = gtk::Label::builder()
        .label(value.borrow().as_str())
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Start)
        .height_request(44)
        .selectable(false)
        .css_classes(["title-3"])
        .build();
    // Password drafts remain masked in both pixels and accessibility.
    let update: Rc<dyn Fn()> = {
        let (value, preview) = (value.clone(), preview.clone());
        Rc::new(move || {
            preview.set_label(&if secret {
                "•".repeat(value.borrow().chars().count())
            } else {
                value.borrow().clone()
            })
        })
    };
    update();
    page.append(&preview);
    let (a, b, _, _, _, _) = ui.controller_family.get().controls();
    page.append(
        &gtk::Label::builder()
            .label(format!(
                "D-pad / stick: move   ·   {a}: select   ·   {b}: cancel"
            ))
            .wrap(true)
            .css_classes(["dim-label"])
            .build(),
    );
    let grid = gtk::Grid::builder()
        .column_homogeneous(true)
        .row_homogeneous(true)
        .column_spacing(6)
        .row_spacing(6)
        .vexpand(true)
        .build();
    let upper = Rc::new(Cell::new(false));
    let symbols = Rc::new(Cell::new(false));
    let letters = "qwertyuiopasdfghjklzxcvbnm";
    let punctuation = "1234567890/\\.:_-@+~!?(),=[]";
    let keys = Rc::new(RefCell::new(Vec::<gtk::Button>::new()));
    for index in 0..30 {
        let button = gtk::Button::builder().height_request(44).build();
        button.set_widget_name(&format!("keyboard-key-{index}"));
        let (value, update) = (value.clone(), update.clone());
        button.connect_clicked(move |button| {
            let next = edit(&value.borrow(), button.label().as_deref().unwrap_or(""));
            *value.borrow_mut() = next;
            update();
        });
        grid.attach(&button, index % 10, index / 10, 1, 1);
        keys.borrow_mut().push(button);
    }
    let redraw: Rc<dyn Fn()> = {
        let (keys, upper, symbols) = (keys.clone(), upper.clone(), symbols.clone());
        Rc::new(move || {
            let chars: Vec<char> = if symbols.get() {
                punctuation.chars().collect()
            } else {
                letters.chars().collect()
            };
            for (index, button) in keys.borrow().iter().enumerate() {
                if let Some(ch) = chars.get(index) {
                    let text = if upper.get() {
                        ch.to_uppercase().collect::<String>()
                    } else {
                        ch.to_string()
                    };
                    button.set_label(&text);
                    button.set_sensitive(true);
                } else {
                    button.set_label("");
                    button.set_sensitive(false);
                }
            }
        })
    };
    redraw();
    page.append(&grid);
    let tools = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    tools.set_homogeneous(true);
    let mut tools_keys = vec!["Shift", "Symbols", "Space", "Backspace", "Clear"];
    if multiline {
        tools_keys.push("New line");
    }
    for key in tools_keys {
        let button = gtk::Button::with_label(key);
        button.set_height_request(44);
        let (value, update, redraw, upper, symbols) = (
            value.clone(),
            update.clone(),
            redraw.clone(),
            upper.clone(),
            symbols.clone(),
        );
        button.connect_clicked(move |_| {
            match key {
                "Shift" => upper.set(!upper.get()),
                "Symbols" => symbols.set(!symbols.get()),
                _ => {
                    let next = edit(&value.borrow(), key);
                    *value.borrow_mut() = next;
                }
            }
            redraw();
            update();
        });
        tools.append(&button);
    }
    page.append(&tools);
    let responses = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    responses.set_homogeneous(true);
    let cancel = gtk::Button::with_label("Cancel");
    let done = gtk::Button::with_label("Done");
    done.add_css_class("suggested-action");
    let cancel_dialog = dialog.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(dialog) = cancel_dialog.upgrade() {
            dialog.close();
        }
    });
    let done_dialog = dialog.downgrade();
    done.connect_clicked(move |_| {
        commit(value.borrow().as_str());
        if let Some(dialog) = done_dialog.upgrade() {
            dialog.close();
        }
    });
    responses.append(&cancel);
    responses.append(&done);
    page.append(&responses);
    dialog.set_child(Some(&page));
    dialog.set_focus(Some(&keys.borrow()[0]));
    dialog.present(Some(parent));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editing_keeps_unicode_valid_and_changes_only_the_draft() {
        assert_eq!(edit("Hi 🎮", "Backspace"), "Hi ");
        assert_eq!(edit("", "Backspace"), "");
        assert_eq!(edit("~/Games", "Space"), "~/Games ");
        assert_eq!(edit("old", "Clear"), "");
        assert_eq!(edit("~/", "é"), "~/é");
        assert_eq!(edit("KEY=1", "New line"), "KEY=1\n");
    }
}
