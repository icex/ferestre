//! Mounted drive shortcuts for the new-install dialog.
use super::*;
use ferestre_core::transfers::InstallMount;

pub(super) fn selector(
    default: &Path,
    folder: &adw::EntryRow,
) -> (adw::ComboRow, Rc<RefCell<Option<InstallMount>>>) {
    let mut choices = vec![("Default library".to_string(), default.to_path_buf(), None)];
    choices.extend(
        mounted_libraries()
            .into_iter()
            .filter(|(_, path, _)| path != default)
            .map(|(label, path, mount)| (label, path, Some(mount))),
    );
    let custom = choices.len() as u32;
    let entries = gtk::StringList::new(&[]);
    for (label, _, _) in &choices {
        entries.append(label);
    }
    entries.append("Custom folder");
    let row = adw::ComboRow::builder()
        .title("Destination drive")
        .model(&entries)
        .build();

    let choices = Rc::new(choices);
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
            if let Some((_, path, mount)) = choices.get(row.selected() as usize) {
                *selected_mount.borrow_mut() = mount.clone();
                syncing.set(true);
                folder.set_text(&path.to_string_lossy());
                syncing.set(false);
            } else {
                folder.grab_focus();
            }
        });
    }
    let selector = row.downgrade();
    let follow_mount = selected_mount.clone();
    folder.connect_changed(move |folder| {
        if syncing.get() {
            return;
        }
        let Some(row) = selector.upgrade() else {
            return;
        };
        let text = folder.text();
        let path = Path::new(text.trim());
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
    });
    (row, selected_mount)
}

fn mounted_libraries() -> Vec<(String, PathBuf, InstallMount)> {
    // Ask GIO for the desktop's mounted volumes, including external drives.
    // Nonlocal paths are still available through the editable folder field
    // when the desktop has mounted them onto the local filesystem.
    let mut seen = std::collections::BTreeSet::new();
    let mut choices = Vec::new();
    for mount in gio::VolumeMonitor::get().mounts() {
        if mount.is_shadowed() {
            continue;
        }
        let Some(root) = mount.root().path() else {
            continue;
        };
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
            choices.push((mount.name().to_string(), library, identity));
        }
    }
    choices.sort_by(|(a, a_path, _), (b, b_path, _)| a.cmp(b).then_with(|| a_path.cmp(b_path)));
    choices
}
