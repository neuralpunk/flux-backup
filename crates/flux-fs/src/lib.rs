//! Filesystem layer shared by Flux's engines: the metadata model ([`entry::Entry`]), the
//! parallel source scanner, metadata-preserving file operations, snapshot metadata, and
//! engine-agnostic diff, search and restore.

#![deny(clippy::unwrap_used)]

pub mod coverage;
pub mod diff;
pub mod disk;
pub mod entry;
pub mod fsops;
pub mod history;
pub mod outcome;
pub mod replicate;
pub mod restore;
pub mod safe;
pub mod scan;
pub mod snapshot;
