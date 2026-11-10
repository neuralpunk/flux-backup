//! Flux Backup core: the profile model, configuration storage and shared services used by
//! the CLI, the GUI, the engines and the privileged helper.

#![deny(clippy::unwrap_used)]

pub mod filter;
pub mod hooks;
pub mod network;
pub mod paths;
pub mod power;
pub mod profile;
pub mod progress;
pub mod retention;
pub mod source_check;
pub mod store;
pub mod sync;
pub mod sys;
pub mod target_check;
pub mod units;
pub mod webhook;

pub use paths::{Paths, Scope};
pub use profile::{Engine, Issue, Profile, ProfileId, Severity, Target};
pub use store::{ProfileStore, StoreError};
pub use units::{ByteSize, HumanDuration, Percent};

/// Flux version string, shared by every binary.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
