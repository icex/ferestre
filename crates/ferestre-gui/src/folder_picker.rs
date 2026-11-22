//! A folder browser inside Ferestre, reachable without a desktop file portal.
use super::*;

/// What the browser picks: a folder, or a file with one of these extensions.
#[derive(Clone, Copy)]
enum Pick {
    Folder,
    File(&'static [&'static str]),
}

impl Pick {
    fn title(self) -> &'static str {
        match self {
            Pick::Folder => "Choose library folder",
            Pick::File(_) => "Choose the game's program",
        }
    }

    /// Whether a file of this name is listed: its extension is one wanted,
    /// compared without case.
    fn lists_file(self, name: &str) -> bool {
        match self {
            Pick::Folder => false,
            Pick::File(extensions) => Path::new(name)
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| extensions.iter().any(|x| x.eq_ignore_ascii_case(e))),
        }
    }
}

pub fn open(ui: &Ui, target: &gtk::Editable) {
    browse_dialog(ui, target, Pick::Folder);
}

/// The same browser, choosing a file whose extension is one of `extensions`
/// (without case). Activating a file puts its path in `target`.
pub fn open_file(ui: &Ui, target: &gtk::Editable, extensions: &'static [&'static str]) {
    browse_dialog(ui, target, Pick::File(extensions));
}

fn browse_dialog(ui: &Ui, target: &gtk::Editable, pick: Pick) {
    let dialog = adw::Dialog::builder()
        .title(pick.title())
        .content_width(640)
        .content_height(520)
        .build();
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();
    page.append(
        &gtk::Label::builder()
            .label(pick.title())
            .xalign(0.0)
            .css_classes(["title-2"])
            .build(),
    );
    let home = home_path();
    let initial = current_path(&target.text(), &home);
    let current = Rc::new(RefCell::new(initial.clone()));
    let path = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Start)
        .selectable(false)
        .build();
    page.append(&path);
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let home_button = gtk::Button::with_label("Home folder");
    let up = gtk::Button::with_label("Parent folder");
    let hidden = gtk::CheckButton::with_label("Show hidden folders");
    bar.append(&home_button);
    bar.append(&up);
    bar.append(&hidden);
    page.append(&bar);
    let folders = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .min_content_height(200)
        .child(&folders)
        .build();
    page.append(&scroll);
    let generation = Rc::new(Cell::new(0u64));
    type Browse = Rc<dyn Fn(PathBuf)>;
    let browse_slot = Rc::new(RefCell::new(None::<Browse>));
    let browse: Browse = {
        let (dialog_ref, target) = (dialog.downgrade(), target.clone());
        let (current, path, folders, up, hidden, generation, slot) = (
            current.clone(),
            path.clone(),
            folders.clone(),
            up.downgrade(),
            hidden.downgrade(),
            generation.clone(),
            Rc::downgrade(&browse_slot),
        );
        Rc::new(move |dir| {
            let (Some(up), Some(hidden)) = (up.upgrade(), hidden.upgrade()) else {
                return;
            };
            *current.borrow_mut() = dir.clone();
            path.set_label(&dir.to_string_lossy());
            up.set_sensitive(dir.parent().is_some());
            generation.set(generation.get() + 1);
            let revision = generation.get();
            while let Some(child) = folders.first_child() {
                folders.remove(&child);
            }
            folders.append(&gtk::Label::new(Some("Loading folders…")));
            let (folders, generation, slot) = (folders.clone(), generation.clone(), slot.clone());
            let (dialog_ref, target) = (dialog_ref.clone(), target.clone());
            let show_hidden = hidden.is_active();
            glib::spawn_future_local(async move {
                let found = gio::spawn_blocking(move || -> std::io::Result<Vec<PathBuf>> {
                    let mut paths = Vec::new();
                    for entry in std::fs::read_dir(dir)?.flatten().take(2000) {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        if !show_hidden && name.starts_with('.') {
                            continue;
                        }
                        let path = entry.path();
                        if path.is_dir() || (pick.lists_file(&name) && path.is_file()) {
                            paths.push(path);
                        }
                    }
                    paths
                        .sort_by_key(|p| p.file_name().map(|s| s.to_string_lossy().to_lowercase()));
                    Ok(paths)
                })
                .await;
                if generation.get() != revision {
                    return;
                }
                while let Some(child) = folders.first_child() {
                    folders.remove(&child);
                }
                match found {
                    Ok(Ok(paths)) if !paths.is_empty() => {
                        for child in paths {
                            let label = child
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string();
                            let button = gtk::Button::builder()
                                .label(&label)
                                .height_request(44)
                                .tooltip_text(child.to_string_lossy())
                                .build();
                            let slot = slot.clone();
                            let (dialog_ref, target) = (dialog_ref.clone(), target.clone());
                            button.connect_clicked(move |_| {
                                if !child.is_dir() {
                                    target.set_text(&child.to_string_lossy());
                                    if let Some(dialog) = dialog_ref.upgrade() {
                                        dialog.close();
                                    }
                                } else if let Some(browse) =
                                    slot.upgrade().and_then(|s| s.borrow().clone())
                                {
                                    browse(child.clone());
                                }
                            });
                            folders.append(&button);
                        }
                    }
                    Ok(Ok(_)) => folders.append(&gtk::Label::new(Some(match pick {
                        Pick::Folder => "No subfolders",
                        Pick::File(_) => "No subfolders or programs here",
                    }))),
                    _ => folders.append(&gtk::Label::new(Some(
                        "Cannot read this folder. Choose a different location.",
                    ))),
                }
            });
        })
    };
    *browse_slot.borrow_mut() = Some(browse.clone());
    {
        let browse = browse.clone();
        home_button.connect_clicked(move |_| browse(home.clone()));
    }
    {
        let (browse, current) = (browse.clone(), current.clone());
        up.connect_clicked(move |_| {
            let parent = current.borrow().parent().map(Path::to_path_buf);
            if let Some(parent) = parent {
                browse(parent);
            }
        });
    }
    {
        let (browse, current) = (browse.clone(), current.clone());
        hidden.connect_toggled(move |_| {
            let dir = current.borrow().clone();
            browse(dir);
        });
    }
    let responses = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    responses.set_homogeneous(true);
    let cancel = gtk::Button::with_label("Cancel");
    let select = gtk::Button::with_label("Use This Folder");
    select.add_css_class("suggested-action");
    {
        let dialog = dialog.downgrade();
        cancel.connect_clicked(move |_| {
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
        });
    }
    {
        let (dialog, target) = (dialog.downgrade(), target.clone());
        select.connect_clicked(move |_| {
            target.set_text(&current.borrow().to_string_lossy());
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
        });
    }
    responses.append(&cancel);
    // A file is chosen by pressing it; there is no folder to use.
    if matches!(pick, Pick::Folder) {
        responses.append(&select);
    }
    page.append(&responses);
    // Retain the recursive browser only while this dialog is alive.
    dialog.connect_closed(move |_| {
        browse_slot.borrow_mut().take();
    });
    browse(initial);
    dialog.set_child(Some(&page));
    if matches!(pick, Pick::Folder) {
        dialog.set_focus(Some(&select));
    }
    dialog.present(Some(&ui.window));
}
fn home_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}
fn current_path(text: &str, home: &Path) -> PathBuf {
    let path = Path::new(text);
    // A selected drive's library may not exist yet. Start on that drive so
    // that Browse can choose a different folder there.
    path.ancestors()
        .find(|parent| parent.is_absolute() && parent.is_dir())
        .unwrap_or(home)
        .to_path_buf()
}
