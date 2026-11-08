//! Searching across snapshots: `find` by pattern and the version history of one path.
//! Engine-agnostic: callers supply a function that opens a snapshot's sorted entries.

use std::collections::BTreeMap;

use rayon::prelude::*;
use serde::Serialize;

use crate::entry::{Entry, Kind};
use crate::snapshot::SnapshotMeta;

/// Opens the entries of a snapshot, sorted by path.
pub type OpenEntries<'a, E> = dyn Fn(&SnapshotMeta) -> Result<Box<dyn Iterator<Item = Result<Entry, E>> + Send + 'a>, E>
    + Sync
    + 'a;

/// One distinct version of a path, present in a run of consecutive snapshots.
#[derive(Debug, Clone, Serialize)]
pub struct Version {
    /// Oldest and newest snapshot holding this version.
    pub first: String,
    pub last: String,
    pub snapshots: u32,
    pub kind: Kind,
    pub size: u64,
    pub mtime: i64,
}

fn same_version(a: &Entry, b: &Entry) -> bool {
    if a.kind == Kind::File {
        a.same_file_as(b)
    } else {
        a.same_meta_as(b)
    }
}

fn lookup<E>(
    open: &OpenEntries<'_, E>,
    snap: &SnapshotMeta,
    path: &[u8],
) -> Result<Option<Entry>, E> {
    for e in open(snap)? {
        let e = e?;
        match e.path.as_slice().cmp(path) {
            std::cmp::Ordering::Less => continue,
            std::cmp::Ordering::Equal => return Ok(Some(e)),
            // Entries are sorted: once past the path, it isn't there.
            std::cmp::Ordering::Greater => return Ok(None),
        }
    }
    Ok(None)
}

/// Versions of `path` (relative, no leading `/`) across `snaps` (oldest first).
/// A version ends when the path changes or disappears from a snapshot.
pub fn history<E: Send>(
    open: &OpenEntries<'_, E>,
    snaps: &[SnapshotMeta],
    path: &[u8],
) -> Result<Vec<Version>, E> {
    let found: Vec<Option<Entry>> = snaps
        .par_iter()
        .map(|s| lookup(open, s, path))
        .collect::<Result<_, _>>()?;
    let mut out: Vec<Version> = Vec::new();
    let mut current: Option<Entry> = None;
    for (snap, entry) in snaps.iter().zip(found) {
        match entry {
            None => current = None,
            Some(e) => {
                if current.as_ref().is_some_and(|c| same_version(c, &e)) {
                    let v = out.last_mut().expect("a current version exists");
                    v.last = snap.id.clone();
                    v.snapshots += 1;
                } else {
                    out.push(Version {
                        first: snap.id.clone(),
                        last: snap.id.clone(),
                        snapshots: 1,
                        kind: e.kind,
                        size: e.size,
                        mtime: e.mtime,
                    });
                }
                current = Some(e);
            }
        }
    }
    Ok(out)
}

/// A path found by [`find`].
#[derive(Debug, Clone, Serialize)]
pub struct Found {
    pub path: String,
    pub kind: Kind,
    /// Distinct versions across the searched snapshots.
    pub versions: u32,
    pub snapshots: u32,
    /// Newest snapshot containing the path.
    pub latest: String,
    pub latest_size: u64,
    pub latest_mtime: i64,
}

/// Paths matching `matches` in any of `snaps` (oldest first), searched in parallel.
pub fn find<E: Send>(
    open: &OpenEntries<'_, E>,
    snaps: &[SnapshotMeta],
    matches: &(dyn Fn(&[u8]) -> bool + Sync),
) -> Result<Vec<Found>, E> {
    let per_snap: Vec<Vec<Entry>> = snaps
        .par_iter()
        .map(|s| {
            let mut hits = Vec::new();
            for e in open(s)? {
                let e = e?;
                if matches(&e.path) {
                    hits.push(e);
                }
            }
            Ok(hits)
        })
        .collect::<Result<_, E>>()?;

    let mut acc: BTreeMap<Vec<u8>, (Entry, usize, Found)> = BTreeMap::new();
    for (i, (snap, hits)) in snaps.iter().zip(per_snap).enumerate() {
        for e in hits {
            match acc.get_mut(&e.path) {
                Some((prev, prev_i, f)) => {
                    if *prev_i + 1 != i || !same_version(prev, &e) {
                        f.versions += 1;
                    }
                    f.snapshots += 1;
                    f.latest = snap.id.clone();
                    f.latest_size = e.size;
                    f.latest_mtime = e.mtime;
                    f.kind = e.kind;
                    *prev = e;
                    *prev_i = i;
                }
                None => {
                    let f = Found {
                        path: format!("/{}", String::from_utf8_lossy(&e.path)),
                        kind: e.kind,
                        versions: 1,
                        snapshots: 1,
                        latest: snap.id.clone(),
                        latest_size: e.size,
                        latest_mtime: e.mtime,
                    };
                    acc.insert(e.path.clone(), (e, i, f));
                }
            }
        }
    }
    Ok(acc.into_values().map(|(_, _, f)| f).collect())
}
