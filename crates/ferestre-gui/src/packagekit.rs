//! Installing a distribution package through PackageKit.
//!
//! PackageKit is the system service KDE Discover and GNOME Software install
//! through, so it is what the desktop already trusts with this. It asks
//! polkit, polkit shows the desktop's own password prompt, and the
//! distribution's package manager does the work. Ferestre runs nothing as root
//! itself.
//!
//! The calls are PackageKit 1.x's D-Bus API on the system bus, as described in
//! src/org.freedesktop.PackageKit.xml and
//! src/org.freedesktop.PackageKit.Transaction.xml, with the enums from
//! lib/packagekit-glib2/pk-enum.h (all checked at tag v1.3.6). Filters and
//! transaction flags are bitfields of those enums: pk-bitfield.h defines
//! `pk_bitfield_value(e)` as `1 << e`. GLib reads DBUS_SYSTEM_BUS_ADDRESS, so
//! a test can put a private bus where the system one would be.

use futures_channel::oneshot;
use gtk::{gio, glib, prelude::*};
use std::cell::RefCell;
use std::rc::Rc;

const NAME: &str = "org.freedesktop.PackageKit";
const PATH: &str = "/org/freedesktop/PackageKit";
const TRANSACTION: &str = "org.freedesktop.PackageKit.Transaction";

/// PK_FILTER_ENUM_NOT_INSTALLED is 3 (UNKNOWN 0, NONE 1, INSTALLED 2).
const FILTER_NOT_INSTALLED: u64 = 1 << 3;
/// PK_TRANSACTION_FLAG_ENUM_ONLY_TRUSTED is 1 (NONE 0): signed packages from
/// the configured repositories only, as Discover and Software install.
const FLAG_ONLY_TRUSTED: u64 = 1 << 1;
/// PK_EXIT_ENUM_SUCCESS is 1 (UNKNOWN 0, FAILED 2, CANCELLED 3).
const EXIT_SUCCESS: u32 = 1;
const EXIT_CANCELLED: u32 = 3;
/// PK_ERROR_ENUM_NOT_AUTHORIZED: the password prompt was dismissed or refused.
const ERROR_NOT_AUTHORIZED: u32 = 48;

/// No timeout. A transaction method returns at once, but a slow daemon is
/// still better waited for than reported as a failure it is not.
const NO_TIMEOUT: i32 = i32::MAX;

async fn system_bus() -> Result<gio::DBusConnection, String> {
    gio::bus_get_future(gio::BusType::System)
        .await
        .map_err(|e| format!("no system bus: {e}"))
}

/// Whether PackageKit is running or the bus can start it.
pub async fn available() -> bool {
    let Ok(bus) = system_bus().await else {
        return false;
    };
    let ask = |method: &str, args: Option<glib::Variant>| {
        bus.call_future(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            method,
            args.as_ref(),
            None,
            gio::DBusCallFlags::NONE,
            -1,
        )
    };
    let owned = ask("NameHasOwner", Some((NAME,).to_variant()))
        .await
        .ok()
        .and_then(|reply| reply.get::<(bool,)>());
    if owned == Some((true,)) {
        return true;
    }
    ask("ListActivatableNames", None)
        .await
        .ok()
        .and_then(|reply| reply.get::<(Vec<String>,)>())
        .is_some_and(|(names,)| names.iter().any(|name| name == NAME))
}

/// Install `package` by name: resolve it to a package id, then install that.
pub async fn install(package: &str) -> Result<(), String> {
    let bus = system_bus().await?;
    let found = run(
        &bus,
        "Resolve",
        (FILTER_NOT_INSTALLED, vec![package.to_string()]).to_variant(),
    )
    .await?;
    let Some(id) = ferestre_core::client_deps::pick_package_id(&found, package) else {
        return Err(format!(
            "the package manager has no {package} to install. Is it already installed, or are the package lists out of date?"
        ));
    };
    run(
        &bus,
        "InstallPackages",
        (FLAG_ONLY_TRUSTED, vec![id.to_string()]).to_variant(),
    )
    .await?;
    Ok(())
}

#[derive(Default)]
struct Progress {
    packages: Vec<String>,
    error: Option<(u32, String)>,
    exit: Option<u32>,
    done: Option<oneshot::Sender<()>>,
}

impl Progress {
    fn end(&mut self) {
        if let Some(done) = self.done.take() {
            let _ = done.send(());
        }
    }
}

/// One transaction: create it, call `method` on it, and wait for Finished.
/// Returns the package ids it reported.
async fn run(
    bus: &gio::DBusConnection,
    method: &str,
    args: glib::Variant,
) -> Result<Vec<String>, String> {
    let reply = bus
        .call_future(
            Some(NAME),
            PATH,
            NAME,
            "CreateTransaction",
            None,
            Some(glib::VariantTy::new("(o)").expect("a valid type string")),
            gio::DBusCallFlags::NONE,
            -1,
        )
        .await
        .map_err(|e| format!("PackageKit: {e}"))?;
    let path = reply
        .child_value(0)
        .str()
        .map(String::from)
        .ok_or("PackageKit returned no transaction")?;

    // The daemon's unique name, so only signals PackageKit itself sends are
    // heard. Any client on the system bus may emit a signal with this path
    // on it; a forged Package or Finished must not choose what is installed.
    let owner = bus
        .call_future(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "GetNameOwner",
            Some(&(NAME,).to_variant()),
            Some(glib::VariantTy::new("(s)").expect("a valid type string")),
            gio::DBusCallFlags::NONE,
            -1,
        )
        .await
        .map_err(|e| format!("PackageKit: {e}"))?
        .child_value(0)
        .str()
        .map(String::from)
        .ok_or("PackageKit has no owner on the bus")?;

    let (sender, finished) = oneshot::channel();
    let progress = Rc::new(RefCell::new(Progress {
        done: Some(sender),
        ..Default::default()
    }));
    // Before the method is called, so no signal can arrive unheard.
    let signals = bus.signal_subscribe(
        Some(&owner),
        Some(TRANSACTION),
        None,
        Some(&path),
        None,
        gio::DBusSignalFlags::NONE,
        glib::clone!(
            #[strong]
            progress,
            #[strong]
            owner,
            move |_, sender, _, _, signal, parameters| {
                if sender != owner {
                    return;
                }
                let mut progress = progress.borrow_mut();
                match signal {
                    // Package(u info, s package_id, s summary)
                    "Package" => {
                        if let Some((_, id, _)) = parameters.get::<(u32, String, String)>() {
                            if !progress.packages.contains(&id) {
                                progress.packages.push(id);
                            }
                        }
                    }
                    // ErrorCode(u code, s details)
                    "ErrorCode" => progress.error = parameters.get::<(u32, String)>(),
                    // Finished(u exit, u runtime)
                    "Finished" => {
                        progress.exit = parameters.get::<(u32, u32)>().map(|(exit, _)| exit);
                        progress.end();
                    }
                    // The transaction is gone without saying how it ended.
                    "Destroy" => progress.end(),
                    _ => {}
                }
            }
        ),
    );
    // A daemon that exits or crashes mid-transaction sends nothing more.
    let vanished = bus.signal_subscribe(
        Some("org.freedesktop.DBus"),
        Some("org.freedesktop.DBus"),
        Some("NameOwnerChanged"),
        Some("/org/freedesktop/DBus"),
        Some(NAME),
        gio::DBusSignalFlags::NONE,
        glib::clone!(
            #[strong]
            progress,
            #[strong]
            owner,
            move |_, _, _, _, _, parameters| {
                if parameters
                    .get::<(String, String, String)>()
                    .is_some_and(|(_, old, new)| old == owner && new != owner)
                {
                    progress.borrow_mut().end();
                }
            }
        ),
    );

    let called = async {
        let call = |method: &str, args: glib::Variant| {
            // To the same daemon the signals are taken from.
            bus.call_future(
                Some(&owner),
                &path,
                TRANSACTION,
                method,
                Some(&args),
                None,
                gio::DBusCallFlags::NONE,
                NO_TIMEOUT,
            )
        };
        // interactive: the daemon may ask polkit, which prompts for a password.
        let mut hints = vec!["interactive=true".to_string()];
        if let Some(lang) = std::env::var("LANG").ok().filter(|l| !l.is_empty()) {
            hints.push(format!("locale={lang}"));
        }
        call("SetHints", (hints,).to_variant())
            .await
            .map_err(|e| format!("PackageKit: {e}"))?;
        call(method, args)
            .await
            .map_err(|e| format!("PackageKit: {e}"))?;
        let _ = finished.await;
        Ok::<_, String>(())
    }
    .await;
    bus.signal_unsubscribe(signals);
    bus.signal_unsubscribe(vanished);
    called?;

    let progress = progress.take();
    match (progress.exit, progress.error) {
        (Some(EXIT_SUCCESS), _) => Ok(progress.packages),
        (_, Some((ERROR_NOT_AUTHORIZED, _))) => {
            Err("the installation was not authorised".to_string())
        }
        (Some(EXIT_CANCELLED), _) => Err("the installation was cancelled".to_string()),
        (_, Some((_, details))) => Err(details),
        (None, None) => Err("PackageKit stopped before the transaction finished".to_string()),
        (Some(exit), None) => Err(format!(
            "PackageKit ended the transaction with status {exit}"
        )),
    }
}
