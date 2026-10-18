//! Whether the Xbox client can start at all, and what to install if not.
//!
//! xodus-cli signs in through a WebKitGTK WebView: Microsoft's sign-in page,
//! with request headers of its own and a JavaScript bridge back to the client,
//! which a browser handed a URL cannot stand in for. So the binary links
//! libwebkit2gtk-4.1 and, through it, libjavascriptcoregtk-4.1, libsoup-3.0
//! and GTK 3. On a system without them the loader refuses the client for every
//! command, not only sign-in, and all anyone sees is exit status 127.
//!
//! The AppImage does not carry WebKit. It is large, it takes security updates
//! of its own, and every distribution packages it, so the fix is the
//! distribution's package. This finds what the loader would not find, with
//! `ldd`, and names that package for the distribution `/etc/os-release`
//! describes.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// What the WebKitGTK package brings with it. Any of these missing is fixed
/// by installing that one package: the rest are its own dependencies.
pub const WEBKIT_SONAMES: [&str; 4] = [
    "libwebkit2gtk-4.1.so.0",
    "libjavascriptcoregtk-4.1.so.0",
    "libsoup-3.0.so.0",
    "libgtk-3.so.0",
];

/// Read instead of `/etc/os-release` when set. For tests, and for seeing what
/// another distribution would be told.
pub const OS_RELEASE_ENV: &str = "FERESTRE_OS_RELEASE";

/// The distribution families whose WebKitGTK package name is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Debian,
    Fedora,
    Arch,
    Suse,
}

impl Family {
    /// The family `os-release` describes: its `ID` first, then each `ID_LIKE`
    /// entry in order, so a derivative that names its parent is covered
    /// without being listed here.
    pub fn detect(os_release: &str) -> Option<Family> {
        let field = |key: &str| {
            os_release.lines().find_map(|line| {
                let value = line.trim().strip_prefix(key)?.strip_prefix('=')?;
                Some(value.trim().trim_matches(['"', '\'']).to_ascii_lowercase())
            })
        };
        let id = field("ID").unwrap_or_default();
        let like = field("ID_LIKE").unwrap_or_default();
        std::iter::once(id.as_str())
            .chain(like.split_whitespace())
            .find_map(Family::from_id)
    }

    fn from_id(id: &str) -> Option<Family> {
        match id {
            "debian" | "ubuntu" => Some(Family::Debian),
            "fedora" | "rhel" | "centos" => Some(Family::Fedora),
            "arch" => Some(Family::Arch),
            "suse" | "sles" | "sled" => Some(Family::Suse),
            id if id.starts_with("opensuse") => Some(Family::Suse),
            _ => None,
        }
    }

    pub fn webkit_package(self) -> &'static str {
        match self {
            Family::Debian => "libwebkit2gtk-4.1-0",
            Family::Fedora => "webkit2gtk4.1",
            Family::Arch => "webkit2gtk-4.1",
            Family::Suse => "libwebkit2gtk-4_1-0",
        }
    }

    pub fn install_command(self, package: &str) -> String {
        let manager = match self {
            Family::Debian => "apt install",
            Family::Fedora => "dnf install",
            Family::Arch => "pacman -S",
            Family::Suse => "zypper install",
        };
        format!("sudo {manager} {package}")
    }
}

/// Libraries the client needs and the loader cannot find.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Missing {
    pub libraries: Vec<String>,
    pub family: Option<Family>,
}

impl Missing {
    pub fn needs_webkit(&self) -> bool {
        self.libraries
            .iter()
            .any(|lib| WEBKIT_SONAMES.contains(&lib.as_str()))
    }

    /// The package to install, when the libraries are WebKit's and the
    /// distribution is one whose name for it is known.
    pub fn package(&self) -> Option<&'static str> {
        self.family
            .filter(|_| self.needs_webkit())
            .map(Family::webkit_package)
    }

    pub fn command(&self) -> Option<String> {
        Some(self.family?.install_command(self.package()?))
    }

    /// Missing libraries that installing [`Missing::package`] does not cover.
    pub fn others(&self) -> Vec<&str> {
        self.libraries
            .iter()
            .map(String::as_str)
            .filter(|lib| !WEBKIT_SONAMES.contains(lib))
            .collect()
    }

    /// What to do, for a terminal or a dialog. A command stands on its own
    /// with the explanation after it, so copying it takes nothing extra.
    pub fn hint(&self) -> String {
        let others = self.others();
        let also = if self.needs_webkit() && !others.is_empty() {
            format!(", and install whatever provides {}", others.join(", "))
        } else {
            String::new()
        };
        match (self.command(), self.needs_webkit()) {
            (Some(command), _) => format!(
                "run: {command}   (WebKitGTK, for the sign-in window{also}; see docs/DEPENDENCIES.md)"
            ),
            (None, true) => format!(
                "install WebKitGTK 4.1, which provides libwebkit2gtk-4.1.so.0, with your distribution's package manager{also}; see docs/DEPENDENCIES.md"
            ),
            (None, false) => {
                "install the packages that provide these libraries; see docs/DEPENDENCIES.md".into()
            }
        }
    }
}

/// What stops the client binary from loading, or `None` when nothing does.
/// An unavailable diagnostic -- no `ldd`, or one that fails -- is an error,
/// never evidence either way.
pub fn check(client: &Path) -> Result<Option<Missing>> {
    let libraries = missing_libraries(client)?;
    if libraries.is_empty() {
        return Ok(None);
    }
    let family = std::fs::read_to_string(os_release_path())
        .ok()
        .and_then(|text| Family::detect(&text));
    Ok(Some(Missing { libraries, family }))
}

/// The client's libraries that the loader cannot find, by soname. Run with
/// this process's environment, which is the one the client inherits.
pub fn missing_libraries(client: &Path) -> Result<Vec<String>> {
    let output = Command::new("ldd")
        .arg(client)
        .env("LC_ALL", "C")
        .output()
        .context("checking the Xbox client's libraries with ldd")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() {
        // A script or a static binary: the loader is not involved, so it
        // cannot be what stops it. A development checkout may use either.
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stdout.contains("not a dynamic executable")
            || stderr.contains("not a dynamic executable")
        {
            return Ok(Vec::new());
        }
        bail!("ldd could not inspect {}", client.display());
    }
    Ok(crate::media::missing_from_ldd(&stdout))
}

pub fn os_release_path() -> PathBuf {
    if let Some(path) = std::env::var_os(OS_RELEASE_ENV).filter(|p| !p.is_empty()) {
        return path.into();
    }
    // os-release(5): /etc first, /usr/lib when /etc has none.
    let etc = Path::new("/etc/os-release");
    if etc.exists() {
        etc.into()
    } else {
        "/usr/lib/os-release".into()
    }
}

/// The PackageKit id to install for `package`, from what a Resolve reported.
///
/// An id is `name;version;arch;data`. A backend can report the same id twice,
/// and on a multiarch Debian, where Steam has turned i386 on, the 32-bit
/// package as well, which is no use to a 64-bit client.
pub fn pick_package_id<'a>(ids: &'a [String], package: &str) -> Option<&'a str> {
    let native: &[&str] = match std::env::consts::ARCH {
        "x86_64" => &["x86_64", "amd64"],
        "aarch64" => &["aarch64", "arm64"],
        other => return pick(ids, package, &[other]),
    };
    pick(ids, package, native)
}

fn pick<'a>(ids: &'a [String], package: &str, native: &[&str]) -> Option<&'a str> {
    let named: Vec<&str> = ids
        .iter()
        .map(String::as_str)
        .filter(|id| id.split(';').next() == Some(package))
        .collect();
    named
        .iter()
        .find(|id| {
            id.split(';')
                .nth(2)
                .is_some_and(|arch| native.contains(&arch))
        })
        .or_else(|| named.first())
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn missing(libraries: &[&str], family: Option<Family>) -> Missing {
        Missing {
            libraries: libraries.iter().map(|l| l.to_string()).collect(),
            family,
        }
    }

    #[test]
    fn derivatives_are_found_through_id_like() {
        let cases = [
            ("ID=debian\n", Some(Family::Debian)),
            (
                "NAME=\"Debian GNU/Linux\"\nID=debian\nVERSION_ID=\"13\"\n",
                Some(Family::Debian),
            ),
            (
                "ID=ubuntu\nID_LIKE=debian\nVARIANT=\"Kubuntu\"\n",
                Some(Family::Debian),
            ),
            (
                "ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n",
                Some(Family::Debian),
            ),
            ("ID=pop\nID_LIKE=\"ubuntu debian\"\n", Some(Family::Debian)),
            ("ID=fedora\n", Some(Family::Fedora)),
            (
                "ID=nobara\nID_LIKE=\"rhel centos fedora\"\n",
                Some(Family::Fedora),
            ),
            (
                "ID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\n",
                Some(Family::Fedora),
            ),
            ("ID=arch\n", Some(Family::Arch)),
            ("ID=cachyos\nID_LIKE=arch\n", Some(Family::Arch)),
            ("ID=endeavouros\nID_LIKE=arch\n", Some(Family::Arch)),
            ("ID=manjaro\nID_LIKE=arch\n", Some(Family::Arch)),
            (
                "ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n",
                Some(Family::Suse),
            ),
            ("ID='sles'\n", Some(Family::Suse)),
            ("ID=gentoo\n", None),
            ("ID=nixos\n", None),
            ("", None),
        ];
        for (text, family) in cases {
            assert_eq!(Family::detect(text), family, "{text:?}");
        }
    }

    #[test]
    fn a_neighbouring_key_is_not_read_as_id() {
        // VERSION_ID and ID_LIKE both start with the letters of ID.
        assert_eq!(
            Family::detect("ID_LIKE=debian\nVERSION_ID=arch\n"),
            Some(Family::Debian)
        );
        assert_eq!(Family::detect("IDENTITY=arch\n"), None);
    }

    #[test]
    fn each_family_gets_its_package_and_command() {
        let cases = [
            (Family::Debian, "sudo apt install libwebkit2gtk-4.1-0"),
            (Family::Fedora, "sudo dnf install webkit2gtk4.1"),
            (Family::Arch, "sudo pacman -S webkit2gtk-4.1"),
            (Family::Suse, "sudo zypper install libwebkit2gtk-4_1-0"),
        ];
        for (family, command) in cases {
            let report = missing(&["libsoup-3.0.so.0"], Some(family));
            assert_eq!(report.command().as_deref(), Some(command));
            assert!(report.hint().contains(command));
        }
    }

    #[test]
    fn webkit_on_an_unknown_distribution_still_names_the_library() {
        let report = missing(&["libwebkit2gtk-4.1.so.0"], None);
        assert_eq!(report.package(), None);
        assert_eq!(report.command(), None);
        assert!(report.hint().contains("libwebkit2gtk-4.1.so.0"));
        assert!(!report.hint().contains("sudo"));
    }

    #[test]
    fn other_libraries_are_named_and_get_no_package() {
        let report = missing(&["libferestre_fixture.so.1"], Some(Family::Debian));
        assert_eq!(report.package(), None);
        assert_eq!(report.others(), ["libferestre_fixture.so.1"]);

        let both = missing(
            &["libferestre_fixture.so.1", "libwebkit2gtk-4.1.so.0"],
            Some(Family::Debian),
        );
        assert_eq!(both.package(), Some("libwebkit2gtk-4.1-0"));
        assert!(both.hint().contains("libferestre_fixture.so.1"));
    }

    #[test]
    fn what_the_loader_cannot_find_is_parsed_from_ldd() {
        // The three Debian 13 reported, with the client's other libraries found.
        let ldd = "\tlinux-vdso.so.1 (0x00007ffd)\n\
            \tlibwebkit2gtk-4.1.so.0 => not found\n\
            \tlibgtk-3.so.0 => /lib/x86_64-linux-gnu/libgtk-3.so.0 (0x00007f00)\n\
            \tlibsoup-3.0.so.0 => not found\n\
            \tlibjavascriptcoregtk-4.1.so.0 => not found\n\
            \tlibc.so.6 => /lib/x86_64-linux-gnu/libc.so.6 (0x00007f01)\n";
        assert_eq!(
            crate::media::missing_from_ldd(ldd),
            [
                "libjavascriptcoregtk-4.1.so.0",
                "libsoup-3.0.so.0",
                "libwebkit2gtk-4.1.so.0"
            ]
        );
    }

    #[test]
    fn a_script_is_not_reported_and_a_missing_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("xodus-cli");
        std::fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
        // Only where ldd itself is there to ask.
        if Command::new("ldd").arg("--version").output().is_ok() {
            assert_eq!(missing_libraries(&script).unwrap(), Vec::<String>::new());
            assert!(missing_libraries(&dir.path().join("absent")).is_err());
        }
    }

    #[test]
    fn the_native_package_is_picked_once() {
        let ids: Vec<String> = [
            "libwebkit2gtk-4.1-0;2.54.0-1;i386;debian-stable-main",
            "libwebkit2gtk-4.1-0;2.54.0-1;amd64;debian-stable-main",
            "libwebkit2gtk-4.1-0;2.54.0-1;amd64;debian-stable-main",
            "libwebkit2gtk-4.1-0-dbg;2.54.0-1;amd64;debian-stable-main",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            pick(&ids, "libwebkit2gtk-4.1-0", &["x86_64", "amd64"]),
            Some("libwebkit2gtk-4.1-0;2.54.0-1;amd64;debian-stable-main")
        );
        // A backend that names the architecture some other way still installs.
        let other = vec!["libwebkit2gtk-4.1-0;2.54.0-1;x64;repo".to_string()];
        assert_eq!(
            pick(&other, "libwebkit2gtk-4.1-0", &["x86_64"]),
            Some(other[0].as_str())
        );
        assert_eq!(pick(&ids, "webkit2gtk4.1", &["x86_64"]), None);
    }
}
