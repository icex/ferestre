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
async fn kwin(app: String) -> Result<(), glib::Error> {
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
    if std::fs::write(&path, script(std::process::id(), &app)).is_err() {
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
                let _ = kwin(app).await;
            });
        }
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
}
