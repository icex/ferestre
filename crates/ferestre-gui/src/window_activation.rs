//! Present our main window after an explicit controller Home press.
//! KWin's session API supplies activation for joystick input on Wayland,
//! where GTK cannot obtain a keyboard/pointer activation serial for it.
use super::*;
use std::os::unix::fs::DirBuilderExt;

fn script(pid: u32, app_id: &str, title: &str) -> String {
    let app = serde_json::to_string(app_id).unwrap();
    let title = serde_json::to_string(title).unwrap();
    format!(
        "const targetPid = {pid}; const app = {app}; const title = {title};\n\
        for (const w of workspace.windowList()) {{\n\
          if (w.pid === targetPid && String(w.resourceClass) === app && w.caption === title) {{\n\
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
async fn kwin(app: String, title: String) -> Result<(), glib::Error> {
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
    if std::fs::write(&path, script(std::process::id(), &app, &title)).is_err() {
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
            let title = ui.window.title().unwrap_or_default().to_string();
            glib::spawn_future_local(async move {
                let _ = kwin(app, title).await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn activation_matches_our_pid_app_and_title_and_escapes_data() {
        let source = script(42, "io.test.App", "a\"; throw 1; //\n");
        assert!(source.contains("const targetPid = 42;"));
        assert!(source.contains(
            "w.pid === targetPid && String(w.resourceClass) === app && w.caption === title"
        ));
        assert!(source.contains(r#"const title = "a\"; throw 1; //\n";"#));
        assert!(source.contains("workspace.activeWindow = w"));
    }
}
