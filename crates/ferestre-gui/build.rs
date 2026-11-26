use std::{env, fs, path::PathBuf, process::Command};

const APP_ICON: &str = "../../packaging/icons/hicolor/scalable/apps/io.github.icex.ferestre.svg";

fn main() {
    println!("cargo:rerun-if-changed=icons");
    println!("cargo:rerun-if-changed={APP_ICON}");
    let mut icons: Vec<_> = fs::read_dir("icons")
        .expect("read icon directory")
        .map(|entry| entry.expect("read icon entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "svg"))
        .collect();
    icons.sort_unstable();

    let mut manifest =
        String::from("<gresources><gresource prefix=\"/io/github/icex/ferestre/icons\">\n");
    for path in icons {
        let path = path.to_str().expect("UTF-8 icon path");
        let name = path.strip_prefix("icons/").expect("icon filename");
        manifest.push_str(&format!(
            "  <file alias=\"scalable/actions/{name}\">{path}</file>\n"
        ));
    }
    manifest.push_str(&format!(
        "  <file alias=\"scalable/apps/io.github.icex.ferestre.svg\">{APP_ICON}</file>\n"
    ));
    manifest.push_str("</gresource></gresources>\n");

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo build output"));
    let manifest_path = out.join("ferestre-icons.xml");
    fs::write(&manifest_path, manifest).expect("write icon resource manifest");
    let status = Command::new("glib-compile-resources")
        .arg("--sourcedir=.")
        .arg("--target")
        .arg(out.join("ferestre-icons.gresource"))
        .arg(manifest_path)
        .status()
        .expect("run glib-compile-resources from the GLib development tools");
    assert!(status.success(), "compile Ferestre's icon resources");
}
