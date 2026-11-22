//! Mounted drive shortcuts for the new-install dialog.
use super::*;
use ferestre_core::transfers::InstallMount;

/// The destination drive row. It opens with the default library and
/// "Custom folder" only: which mounted drives can take an install is asked on a
/// worker (each needs a mount lookup and an access check, and a sleeping
/// external disk can take seconds to answer), and they join the list when it
/// answers, so the dialog never waits for the disks.
pub(super) fn selector(
    default: &Path,
    folder: &adw::EntryRow,
) -> (adw::ComboRow, Rc<RefCell<Option<InstallMount>>>) {
    type Choices = Rc<RefCell<Vec<(String, PathBuf, Option<InstallMount>)>>>;
    let choices: Choices = Rc::new(RefCell::new(vec![(
        "Default library".to_string(),
        default.to_path_buf(),
        None,
    )]));
    let entries = gtk::StringList::new(&["Default library", "Custom folder"]);
    let row = adw::ComboRow::builder()
        .title("Destination drive")
        .model(&entries)
        .build();

    let selected_mount = Rc::new(RefCell::new(None::<InstallMount>));
    let syncing = Rc::new(Cell::new(false));
    {
        let folder = folder.downgrade();
        let (choices, syncing) = (choices.clone(), syncing.clone());
        let selected_mount = selected_mount.clone();
        row.connect_selected_notify(move |row| {
            if syncing.get() {
                return;
            }
            let Some(folder) = folder.upgrade() else {
                return;
            };
            let chosen = choices.borrow().get(row.selected() as usize).cloned();
            if let Some((_, path, mount)) = chosen {
                *selected_mount.borrow_mut() = mount;
                syncing.set(true);
                folder.set_text(&path.to_string_lossy());
                syncing.set(false);
            } else {
                folder.grab_focus();
            }
        });
    }
    // Which row matches the folder field, "Custom folder" being the last.
    type Sync = Rc<dyn Fn(&adw::ComboRow, &adw::EntryRow)>;
    let sync_to_folder: Sync = {
        let (choices, syncing) = (choices.clone(), syncing.clone());
        let follow_mount = selected_mount.clone();
        Rc::new(move |row, folder| {
            let text = folder.text();
            let path = Path::new(text.trim());
            let choices = choices.borrow();
            let custom = choices.len() as u32;
            let selected = choices
                .iter()
                .position(|(_, choice, _)| choice == path)
                .map_or(custom, |index| index as u32);
            if selected != custom {
                *follow_mount.borrow_mut() = choices[selected as usize].2.clone();
            } else if follow_mount.borrow().as_ref().is_some_and(|mount| {
                (path.is_absolute() && !path.starts_with(mount.path())) || path.starts_with("~")
            }) {
                follow_mount.borrow_mut().take();
            }
            syncing.set(true);
            row.set_selected(selected);
            syncing.set(false);
        })
    };
    {
        let selector = row.downgrade();
        let sync = sync_to_folder.clone();
        let syncing = syncing.clone();
        folder.connect_changed(move |folder| {
            if syncing.get() {
                return;
            }
            if let Some(row) = selector.upgrade() {
                sync(&row, folder);
            }
        });
    }
    let roots = mount_roots();
    let default = default.to_path_buf();
    let (selector, folder) = (row.downgrade(), folder.downgrade());
    glib::spawn_future_local(async move {
        let found = gio::spawn_blocking(move || writable_libraries(roots))
            .await
            .unwrap_or_default();
        let (Some(row), Some(folder)) = (selector.upgrade(), folder.upgrade()) else {
            return;
        };
        let found: Vec<_> = found
            .into_iter()
            .filter(|(_, path, _)| *path != default)
            .collect();
        if found.is_empty() {
            return;
        }
        let labels: Vec<&str> = found.iter().map(|(label, _, _)| label.as_str()).collect();
        // Before "Custom folder", which stays last.
        syncing.set(true);
        entries.splice(1, 0, &labels);
        syncing.set(false);
        choices.borrow_mut().extend(
            found
                .into_iter()
                .map(|(label, path, mount)| (label, path, Some(mount))),
        );
        sync_to_folder(&row, &folder);
    });
    (row, selected_mount)
}

/// The desktop's mounted volumes, as GIO knows them: names and root folders,
/// read on the GTK thread, which is where the volume monitor lives. Asks the
/// disks nothing.
fn mount_roots() -> Vec<(String, PathBuf)> {
    // Including external drives. Nonlocal paths are still available through
    // the editable folder field when the desktop has mounted them onto the
    // local filesystem.
    gio::VolumeMonitor::get()
        .mounts()
        .into_iter()
        .filter(|mount| !mount.is_shadowed())
        .filter_map(|mount| Some((mount.name().to_string(), mount.root().path()?)))
        .collect()
}

/// Which of those can take a library, on a worker: each is a mount lookup,
/// a path check and an access call against the disk.
fn writable_libraries(roots: Vec<(String, PathBuf)>) -> Vec<(String, PathBuf, InstallMount)> {
    let mut seen = std::collections::BTreeSet::new();
    let mut choices = Vec::new();
    for (name, root) in roots {
        let Ok(identity) = InstallMount::capture(&root) else {
            continue;
        };
        let library = root.join("xbox-games");
        if library.to_str().is_none() || !seen.insert(library.clone()) {
            continue;
        }
        if identity.check_path(&library).is_err() {
            continue;
        }
        let writable_at = if library.is_dir() { &library } else { &root };
        let Ok(path) = std::ffi::CString::new(writable_at.as_os_str().as_encoded_bytes()) else {
            continue;
        };
        // Safety: path is a valid C string for the duration of the call.
        if unsafe { libc::access(path.as_ptr(), libc::W_OK | libc::X_OK) } == 0 {
            choices.push((name, library, identity));
        }
    }
    choices.sort_by(|(a, a_path, _), (b, b_path, _)| a.cmp(b).then_with(|| a_path.cmp(b_path)));
    choices
}
