//! Filesystem layer shared by Flux's engines: the metadata model ([`entry::Entry`]), the
//! parallel source scanner, metadata-preserving file operations, snapshot metadata, and
//! engine-agnostic diff, search and restore.

pub mod diff;
pub mod entry;
pub mod fsops;
pub mod history;
pub mod outcome;
pub mod restore;
pub mod safe;
pub mod scan;
pub mod snapshot;
