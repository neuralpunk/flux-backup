//! Flux Backup transport: the system `ssh` client with connection sharing, a pipelined
//! SFTP client, and lock files for remote repositories.

#![deny(clippy::unwrap_used)]

pub mod lock;
pub mod sftp;
pub mod ssh;
#[doc(hidden)]
pub mod testing;

pub use ssh::{SMALL_FILE, SftpPool, SmallFiles, SshError, SshTarget, shell_quote};
