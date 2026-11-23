//! The window keeps the size it was given, across starts. It is made smaller
//! only when no display has room for it any more, and takes its own size back
//! once one has: a size it was only squeezed into is never saved.
//!
//! Its position is the compositor's: Wayland gives an application no way to
//! ask for one, and GTK none to set it.
use super::*;

pub(crate) const DEFAULT: (i32, i32) = (1280, 800);
/// A drag reports every size on the way; the last one is the one to keep.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(400);

/// A compositor may squeeze the window itself when a display gets smaller:
/// sizes this soon after that are not the person's.
const SQUEEZE: std::time::Duration = std::time::Duration::from_secs(3);

/// `size` as large as the displays allow. A display that fits it wins.
fn fitted(size: (i32, i32), displays: &[(i32, i32)]) -> (i32, i32) {
    if displays.is_empty() || displays.iter().any(|d| size.0 <= d.0 && size.1 <= d.1) {
        return size;
    }
    displays
        .iter()
        .map(|d| (size.0.min(d.0), size.1.min(d.1)))
        .max_by_key(|s| i64::from(s.0) * i64::from(s.1))
        .unwrap_or(size)
}

fn displays() -> Vec<(i32, i32)> {
    let Some(display) = gdk::Display::default() else {
        return Vec::new();
    };
    let monitors = display.monitors();
    (0..monitors.n_items())
        .filter_map(|i| monitors.item(i).and_downcast::<gdk::Monitor>())
        .map(|m| (m.geometry().width(), m.geometry().height()))
        .filter(|d| d.0 > 0 && d.1 > 0)
        .collect()
}

fn read(ui: &Ui) -> Option<(i32, i32)> {
    let value = handheld::read_value(ui, "size")?;
    let side = |i: usize| {
        value
            .get(i)?
            .as_i64()
            .and_then(|v| i32::try_from(v).ok())
            .filter(|v| (200..=32_000).contains(v))
    };
    Some((side(0)?, side(1)?))
}

pub(crate) fn install(ui: &Ui) {
    let window = &ui.window;
    let wanted = Rc::new(Cell::new(read(ui).unwrap_or(DEFAULT)));
    // The size last set here, not by the person.
    let set = Rc::new(Cell::new(wanted.get()));
    let squeezed: Rc<Cell<Option<std::time::Instant>>> = Rc::default();
    let fit: Rc<dyn Fn(&adw::ApplicationWindow)> = {
        let (wanted, set, squeezed) = (wanted.clone(), set.clone(), squeezed.clone());
        Rc::new(move |window| {
            let size = fitted(wanted.get(), &displays());
            squeezed.set((size != wanted.get()).then(std::time::Instant::now));
            if size != window.default_size() {
                set.set(size);
                window.set_default_size(size.0, size.1);
            }
        })
    };
    fit(window);
    if handheld::read_preference(ui, "maximized") == Some(true) {
        window.maximize();
    }

    let pending: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
    let changed: Rc<dyn Fn()> = {
        let squeezed = squeezed.clone();
        let (ui, wanted, set) = (ui.clone(), wanted.clone(), set.clone());
        Rc::new(move || {
            if let Some(old) = pending.take() {
                old.remove();
            }
            let (ui, wanted, set) = (ui.clone(), wanted.clone(), set.clone());
            let (done, squeezed) = (pending.clone(), squeezed.clone());
            let id = glib::timeout_add_local_once(SETTLE, move || {
                done.take();
                if squeezed.get().is_some_and(|at| at.elapsed() < SQUEEZE) {
                    set.set(ui.window.default_size());
                    return;
                }
                let size = ui.window.default_size();
                // GTK keeps the unmaximized size here while the window is
                // maximized or fullscreen.
                if size != set.get() && size.0 > 0 && size.1 > 0 {
                    set.set(size);
                    wanted.set(size);
                    save(&ui, "size", serde_json::json!([size.0, size.1]));
                }
            });
            pending.replace(Some(id));
        })
    };
    for property in ["default-width", "default-height"] {
        let changed = changed.clone();
        window.connect_notify_local(Some(property), move |_, _| changed());
    }
    let max_ui = ui.clone();
    window.connect_maximized_notify(move |window| {
        if !window.is_fullscreen() {
            save(
                &max_ui,
                "maximized",
                serde_json::Value::Bool(window.is_maximized()),
            );
        }
    });

    // A display that changed its mode, or came or went.
    if let Some(display) = gdk::Display::default() {
        let monitors = display.monitors();
        let watch: Rc<dyn Fn(&gdk::Monitor)> = {
            let (window, fit) = (window.downgrade(), fit.clone());
            Rc::new(move |monitor| {
                let (window, fit) = (window.clone(), fit.clone());
                monitor.connect_geometry_notify(move |_| {
                    if let Some(window) = window.upgrade() {
                        fit(&window);
                    }
                });
            })
        };
        for i in 0..monitors.n_items() {
            if let Some(monitor) = monitors.item(i).and_downcast::<gdk::Monitor>() {
                watch(&monitor);
            }
        }
        let window = window.downgrade();
        monitors.connect_items_changed(move |monitors, at, _, added| {
            for i in at..at + added {
                if let Some(monitor) = monitors.item(i).and_downcast::<gdk::Monitor>() {
                    watch(&monitor);
                }
            }
            if let Some(window) = window.upgrade() {
                fit(&window);
            }
        });
    }
}

fn save(ui: &Ui, key: &str, value: serde_json::Value) {
    if let Err(error) = handheld::save_value(ui, key, value) {
        ui.toasts
            .add_toast(toast(format!("Could not save the window's size: {error}")));
    }
}

#[cfg(test)]
mod tests {
    use super::fitted;

    #[test]
    fn a_size_is_kept_while_any_display_has_room() {
        assert_eq!(
            fitted((2000, 1200), &[(1280, 720), (2560, 1440)]),
            (2000, 1200)
        );
        assert_eq!(fitted((2000, 1200), &[]), (2000, 1200));
    }

    #[test]
    fn a_size_no_display_has_room_for_takes_the_most_of_one() {
        assert_eq!(
            fitted((2000, 1200), &[(1280, 720), (1920, 1080)]),
            (1920, 1080)
        );
        assert_eq!(fitted((1600, 1200), &[(1920, 1080)]), (1600, 1080));
    }
}
