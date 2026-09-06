//! The logic behind the launcher, with no I/O policy of its own.
//!
//! Split out from the binary so a GUI can use exactly the same code paths as the
//! CLI rather than reimplementing them, and so the parts that decide things --
//! which recipe applies, what environment a title needs, whether the runtime can
//! satisfy it -- can be tested without a Microsoft account, a GPU, or a 2 GB
//! download.
//!
//! One boundary matters and is deliberate: this crate never links the Xodus
//! client. Xodus is GPL-3.0-only and it is what signs in, fetches licences and
//! decrypts packages; running it as a child process is aggregation and leaves
//! this crate under MIT. It is also the only shape that works, because the
//! decrypted executable is passed to Wine as an inherited file descriptor and
//! has to stay a child of the process that opened it.

pub mod account;
pub mod capability;
pub mod catalog;
pub mod gamepass;
pub mod http;
pub mod install;
pub mod launch;
pub mod library;
pub mod paths;
pub mod recipe;
pub mod runtime;
pub mod steam;

pub use recipe::{Recipe, Status, TitleState};

/// A byte count in the units people actually say.
///
/// Powers of ten, like a store page: nobody comparing a download against their
/// free space means gibibytes, and the number on the Store listing is decimal.
/// One place for it because it turns up in three -- a catalog size, a download
/// in progress, and a directory about to be deleted -- and three of them would
/// eventually disagree.
pub fn human_bytes(n: u64) -> String {
    match n {
        n if n >= 1_000_000_000 => format!("{:.1} GB", n as f64 / 1e9),
        n if n >= 1_000_000 => format!("{} MB", n / 1_000_000),
        n if n >= 1_000 => format!("{} kB", n / 1_000),
        n => format!("{n} B"),
    }
}
