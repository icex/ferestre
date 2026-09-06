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
pub mod http;
pub mod install;
pub mod launch;
pub mod library;
pub mod paths;
pub mod recipe;
pub mod runtime;
pub mod steam;

pub use recipe::{Recipe, Status, TitleState};
