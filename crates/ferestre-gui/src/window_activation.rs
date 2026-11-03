//! Present our main window after an explicit controller Home press.
//! KWin's session API supplies activation for joystick input on Wayland,
//! where GTK cannot obtain a keyboard/pointer activation serial for it.
use super::*;
use std::os::unix::fs::DirBuilderExt;

fn script(pid: u32, app_id: &str) -> String {
    let app = serde_json::to_string(app_id).unwrap();
    format!(
        "const targetPid = {pid}; const app = {app};\n\
        for (const w of workspace.windowList()) {{\n\
          if (w.pid === targetPid && String(w.resourceClass) === app && w.normalWindow && !w.transient) {{\n\
            w.minimized = false;\n\
            if (w.desktops.length) workspace.currentDesktop = w.desktops[0];\n\
            workspace.activeWindow = w; break;\n\
          }}\n\
        }}\n"
    )
}

struct ScriptFile(PathBuf);
impl Drop for ScriptFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
async fn kwin(source: String) -> Result<(), glib::Error> {
    let bus = gio::bus_get_future(gio::BusType::Session).await?;
    let call = |path: &str, interface: &str, method: &str, args: Option<glib::Variant>| {
        bus.call_future(
            Some("org.kde.KWin"),
            path,
            interface,
            method,
            args.as_ref(),
            None,
            gio::DBusCallFlags::NO_AUTO_START,
            1000,
        )
    };
    let name = format!("ferestre-home-{}", launch_state::tag());
    let dir = std::env::temp_dir().join(&name);
    if std::fs::DirBuilder::new().mode(0o700).create(&dir).is_err() {
        return Ok(());
    }
    let file = ScriptFile(dir);
    let path = file.0.join("activate.js");
    if std::fs::write(&path, source).is_err() {
        return Ok(());
    }
    let reply = call(
        "/Scripting",
        "org.kde.kwin.Scripting",
        "loadScript",
        Some((path.to_string_lossy().as_ref(), name.as_str()).to_variant()),
    )
    .await?;
    let Some((id,)) = reply.get::<(i32,)>().filter(|(id,)| *id >= 0) else {
        return Ok(());
    };
    let result = call(
        &format!("/Scripting/Script{id}"),
        "org.kde.kwin.Script",
        "run",
        None,
    )
    .await;
    // Even on a failed run, unload this one-shot script; no desktop setting is changed.
    let _ = call(
        "/Scripting",
        "org.kde.kwin.Scripting",
        "unloadScript",
        Some((name,).to_variant()),
    )
    .await;
    result.map(|_| ())
}

pub fn present(ui: &Ui) {
    ui.window.unminimize();
    ui.window.present();
    controller::focus_content(ui);
    if gdk::Display::default().is_some_and(|d| d.type_().name().contains("Wayland")) {
        let app = ui
            .window
            .application()
            .and_then(|a| a.application_id())
            .map(|s| s.to_string());
        if let Some(app) = app {
            glib::spawn_future_local(async move {
                let _ = kwin(script(std::process::id(), &app)).await;
            });
        }
    }
}

/// Return only processes carrying this launch's marker, including detached Wine
/// children. Ordinary unrelated applications' environments are not inspected.
fn owned_pids(root: &Path, uid: u32, pgid: u32, tag: &str) -> Vec<u32> {
    if tag.is_empty() {
        return Vec::new();
    }
    let expected = format!("{}={tag}", launch_state::TAG);
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
            let dir = entry.path();
            let stat = std::fs::read_to_string(dir.join("stat")).ok()?;
            let fields: Vec<_> = stat.rsplit_once(") ")?.1.split_whitespace().collect();
            let group = fields.get(2)?.parse::<u32>().ok()?;
            let cmd = std::fs::read(dir.join("cmdline")).ok()?;
            let image = String::from_utf8_lossy(cmd.split(|b| *b == 0).next()?);
            if group != pgid && !image.to_ascii_lowercase().ends_with(".exe") {
                return None;
            }
            let status = std::fs::read_to_string(dir.join("status")).ok()?;
            let owner = status.lines().find_map(|s| {
                s.strip_prefix("Uid:")?
                    .split_whitespace()
                    .next()?
                    .parse::<u32>()
                    .ok()
            });
            if owner != Some(uid) {
                return None;
            }
            let env = std::fs::read(dir.join("environ")).ok()?;
            env.split(|b| *b == 0)
                .any(|v| v == expected.as_bytes())
                .then_some(pid)
        })
        .collect()
}
fn game_script(pids: &[u32]) -> String {
    let pids = serde_json::to_string(pids).unwrap();
    format!("const pids = {pids};\nfor (const w of Array.from(workspace.stackingOrder).reverse()) {{\n  if (pids.includes(w.pid) && (w.normalWindow || w.dialog)) {{\n    w.minimized = false;\n    if (w.desktops.length) workspace.currentDesktop = w.desktops[0];\n    workspace.activeWindow = w; break;\n  }}\n}}\n")
}
pub fn resume(ui: &Ui, id: &str, name: &str) {
    let key = id.to_ascii_uppercase();
    let Some(launch) = ui.running.borrow().get(&key).filter(|l| l.ready).cloned() else {
        return;
    };
    let ui = ui.clone();
    let name = name.to_string();
    glib::spawn_future_local(async move {
        let (tag, pgid) = (launch.tag.clone(), launch.pgid);
        let pids = gio::spawn_blocking(move || {
            owned_pids(Path::new("/proc"), unsafe { libc::getuid() }, pgid, &tag)
        })
        .await
        .unwrap_or_default();
        if ui.running.borrow().get(&key) != Some(&launch) {
            return;
        }
        if pids.is_empty() {
            ui.toasts
                .add_toast(toast(format!("{name}'s window is not available yet")));
            return;
        }
        if kwin(game_script(&pids)).await.is_err() {
            // Isolate Xlib errors (for a window closing mid-query) from GTK.
            let sent = gio::spawn_blocking(move || {
                let Ok(exe) = std::env::current_exe() else {
                    return false;
                };
                deadline::status(
                    std::process::Command::new(exe)
                        .arg("--resume-x11")
                        .args(pids.iter().map(u32::to_string)),
                    std::time::Duration::from_secs(2),
                )
                .is_ok_and(|s| s.success())
            })
            .await
            .unwrap_or(false);
            if !sent && ui.running.borrow().get(&key) == Some(&launch) {
                ui.toasts.add_toast(toast(format!("Your desktop could not activate {name}. Use the desktop task switcher to return to it.")));
                return;
            }
        }
        glib::timeout_add_local_once(std::time::Duration::from_millis(750), move || {
            if ui.window.is_active() && ui.running.borrow().get(&key) == Some(&launch) {
                ui.toasts.add_toast(toast(format!(
                    "Could not focus {name}. Its window may still be loading."
                )));
            }
        });
    });
}

/// A short-lived helper: EWMH activation works with X11 window managers and
/// XWayland games. It never sends keystrokes or modifies the game's state.
pub fn x11_resume(pids: &[u32]) -> bool {
    use std::ptr;
    use x11_dl::xlib;
    let Ok(x) = xlib::Xlib::open() else {
        return false;
    };
    unsafe extern "C" fn ignore_error(_: *mut xlib::Display, _: *mut xlib::XErrorEvent) -> i32 {
        0
    }
    unsafe {
        // This helper is its own process, so its handler cannot affect GTK.
        (x.XSetErrorHandler)(Some(ignore_error));
        let display = (x.XOpenDisplay)(ptr::null());
        if display.is_null() {
            return false;
        }
        let root = (x.XDefaultRootWindow)(display);
        let atom = |name: &std::ffi::CStr| (x.XInternAtom)(display, name.as_ptr(), 0);
        let property = |window, name, expected_type| {
            let (mut kind, mut format, mut count, mut left, mut data) =
                (0, 0, 0, 0, ptr::null_mut());
            let result = (x.XGetWindowProperty)(
                display,
                window,
                name,
                0,
                4096,
                0,
                expected_type,
                &mut kind,
                &mut format,
                &mut count,
                &mut left,
                &mut data,
            );
            let values = if result == 0
                && !data.is_null()
                && kind == expected_type
                && format == 32
                && count <= 4096
            {
                std::slice::from_raw_parts(data.cast::<libc::c_ulong>(), count as usize).to_vec()
            } else {
                Vec::new()
            };
            if !data.is_null() {
                (x.XFree)(data.cast());
            }
            values
        };
        let windows = property(root, atom(c"_NET_CLIENT_LIST_STACKING"), xlib::XA_WINDOW);
        let pid_atom = atom(c"_NET_WM_PID");
        let selected = windows.into_iter().rev().find(|w| {
            property(*w, pid_atom, xlib::XA_CARDINAL)
                .first()
                .is_some_and(|pid| pids.contains(&(*pid as u32)))
        });
        if let Some(window) = selected {
            let mut message: xlib::XEvent = std::mem::zeroed();
            message.client_message = xlib::XClientMessageEvent {
                type_: xlib::ClientMessage,
                serial: 0,
                send_event: 1,
                display,
                window,
                message_type: atom(c"_NET_ACTIVE_WINDOW"),
                format: 32,
                data: xlib::ClientMessageData::new(),
            };
            message.client_message.data.as_longs_mut()[0] = 2;
            (x.XSendEvent)(
                display,
                root,
                0,
                xlib::SubstructureRedirectMask | xlib::SubstructureNotifyMask,
                &mut message,
            );
            (x.XFlush)(display);
        }
        (x.XCloseDisplay)(display);
        selected.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn activation_matches_only_our_main_window_without_relying_on_its_caption() {
        let source = script(42, "io.test.App");
        assert!(source.contains("const targetPid = 42;"));
        assert!(source.contains("w.pid === targetPid && String(w.resourceClass) === app && w.normalWindow && !w.transient"));
        assert!(
            !source.contains("w.caption"),
            "KWin may add captions, hostnames or duplicate-title suffixes"
        );
        assert!(source.contains("workspace.activeWindow = w"));
        let escaped = script(42, "a\"; throw 1; //\n");
        assert!(escaped.contains(r#"const app = "a\"; throw 1; //\n";"#));
    }
    #[test]
    fn resume_selects_only_this_launch_including_detached_game_processes() {
        let root = std::env::temp_dir().join(format!("ferestre-resume-{}", launch_state::tag()));
        std::fs::create_dir_all(&root).unwrap();
        for (pid, group, image, uid, tag) in [
            (42, 42, "launcher", 1000, "ours"),
            (43, 42, "Game.exe", 1000, "ours"),
            (44, 99, "Child.exe", 1000, "ours"),
            (45, 99, "Other.exe", 1000, "other"),
            (46, 42, "Game.exe", 1001, "ours"),
            (47, 99, "unrelated", 1000, "ours"),
        ] {
            let dir = root.join(pid.to_string());
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("stat"),
                format!("{pid} (process with spaces) S 1 {group} 0 0"),
            )
            .unwrap();
            std::fs::write(
                dir.join("status"),
                format!("Uid:\t{uid}\t{uid}\t{uid}\t{uid}\n"),
            )
            .unwrap();
            std::fs::write(dir.join("cmdline"), format!("{image}\0")).unwrap();
            std::fs::write(
                dir.join("environ"),
                format!("{}={tag}\0", launch_state::TAG),
            )
            .unwrap();
        }
        let mut pids = owned_pids(&root, 1000, 42, "ours");
        pids.sort();
        assert_eq!(pids, vec![42, 43, 44]);
        assert!(owned_pids(&root, 1000, 42, "").is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn resume_script_uses_scoped_pids_and_topmost_owned_windows() {
        let source = game_script(&[42, 43]);
        assert!(source.contains("const pids = [42,43]"));
        assert!(source.contains("pids.includes(w.pid)"));
        assert!(source
            .contains("workspace.stackingOrder")
            .then_some(())
            .is_some());
        assert!(!source.contains("caption"));
    }
}
