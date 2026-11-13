//! Verify a Vault: structure always, every stored byte on request, and (on request)
//! repair what is damaged from its repair data ([`crate::protect`]). What a repair can't
//! rebuild it takes out of the index ([`crate::heal`]), so that the next backup stores it
//! again from the files that still have it.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::Ordering::Relaxed;

use rayon::prelude::*;
use serde::Serialize;

use flux_core::progress::{Event, Phase, Progress, Reporter};

use crate::VaultError;
use crate::backend::pack_path;
use crate::crypto::{BlobId, hex_id, parse_id};
use crate::pack::{decode_blob, split_pack};
use crate::protect::{self, Fix, ParityState};
use crate::repo::VaultRepo;
use flux_core::sync::Locked;

#[derive(Debug, Default, Serialize)]
pub struct CheckReport {
    pub snapshots: u64,
    pub trees: u64,
    pub packs: u64,
    pub blobs: u64,
    /// Packs fully read and verified (`--read-data`).
    pub packs_read: u64,
    pub bytes_read: u64,
    /// Problems that make data unrestorable.
    pub errors: Vec<String>,
    /// Harmless findings (e.g. unreferenced packs that prune will remove).
    pub warnings: Vec<String>,
    /// Damage that was repaired from repair data (with [`CheckOptions::repair`]).
    pub repaired: Vec<String>,
    /// Of `errors`, how many are damage that the repair data can rebuild.
    pub repairable: u64,
    /// Files given repair data they lacked.
    pub parity_added: u64,
}

/// What a check does beyond the structure.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CheckOptions {
    /// Read every stored byte and verify it.
    pub read_data: bool,
    /// Rebuild what is damaged from its repair data, write damaged repair data again,
    /// and (if the Vault is written with repair data) add it where it's missing. Nothing
    /// is taken out of the Vault: what can't be rebuilt is reported.
    pub repair: bool,
    /// With `repair`, for what repair data doesn't put right: an index that can't be read
    /// is written again from the packs, and packs that are missing or can't be rebuilt
    /// leave the index (and the Vault), with what could still be read of them moved to a
    /// new pack (see [`crate::heal`]). Only when asked for: it removes stored data, if
    /// data that can't be read any more.
    pub heal: bool,
}

impl CheckReport {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

pub fn check(
    repo: &VaultRepo,
    read_data: bool,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<CheckReport, VaultError> {
    check_with(
        repo,
        CheckOptions {
            read_data,
            repair: false,
            heal: false,
        },
        progress,
        report,
    )
}

/// Index files and snapshot records that don't open, and have repair data: rebuild them
/// (or say that they could be). Before anything else: nothing can be checked without
/// them. With `read_data`, the parity files of the sound ones are checked too.
fn small_objects(repo: &VaultRepo, opts: CheckOptions, r: &mut CheckReport) {
    let backend = repo.backend();
    for dir in ["index", "snapshots"] {
        let Ok(par) = backend.list(&protect::path_for(dir)) else {
            continue;
        };
        if par.is_empty() {
            continue;
        }
        let have: HashSet<String> = par.into_iter().map(|(n, _)| n).collect();
        for (name, size) in backend.list(dir).unwrap_or_default() {
            if !have.contains(&name) {
                continue;
            }
            let path = format!("{dir}/{name}");
            let sound = |b: &[u8]| protect::opens(&repo.crypto, b);
            match backend.read(&path) {
                Ok(bytes) if sound(&bytes) => {
                    if !opts.read_data {
                        continue;
                    }
                    let state = backend
                        .read(&protect::path_for(&path))
                        .map_or(ParityState::Damaged, |p| protect::parity_state(&bytes, &p));
                    if state == ParityState::Sound {
                        continue;
                    }
                    // Stale (rewritten by a Flux that keeps no repair data) or damaged.
                    if !opts.repair {
                        r.warnings.push(format!(
                            "the repair data of {path} is out of date or damaged (a repair writes it again)"
                        ));
                    } else if protect::write_for(backend, &path, &bytes, repo.parity.max(1), false)
                        .is_ok()
                    {
                        r.repaired
                            .push(format!("{path}: its repair data was written again"));
                    }
                }
                _ => match protect::repair(backend, &path, size, &sound, opts.repair) {
                    Ok(Fix::Repaired { blocks }) => r
                        .repaired
                        .push(format!("{path}: damaged, repaired ({blocks} blocks)")),
                    Ok(Fix::Repairable { .. }) => {
                        r.repairable += 1;
                        r.errors.push(format!(
                            "{path} is damaged (its repair data can rebuild it)"
                        ));
                    }
                    // Beyond repair: the checks below say what that costs.
                    _ => {}
                },
            }
        }
    }
}

pub fn check_with(
    repo: &VaultRepo,
    opts: CheckOptions,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<CheckReport, VaultError> {
    let read_data = opts.read_data;
    // Shared, as a backup's; exclusive (a prune's) once the index has to be rewritten.
    let mut lock = Some(repo.lock(false)?);
    let mut exclusive = false;
    let mut r = CheckReport::default();
    progress.set_phase(Phase::Scanning);
    report(Event::Phase {
        phase: Phase::Scanning,
    });
    small_objects(repo, opts, &mut r);

    // 1. Index vs. packs on disk.
    repo.invalidate_index();
    let index = match repo.index() {
        Ok(i) => i,
        // An index file that doesn't open and that no repair data rebuilt: the packs
        // say what they hold.
        Err(e @ (VaultError::Corrupt(_) | VaultError::Crypto(_))) if opts.repair && opts.heal => {
            take_exclusive(repo, &mut lock, &mut exclusive)?;
            let rebuilt = crate::heal::rebuild_index(repo)?;
            r.repaired.push(format!(
                "the index couldn't be read ({e}); it was written again from the {} packs' own lists of what they hold",
                rebuilt.packs
            ));
            for hex in rebuilt.unreadable {
                r.errors.push(format!(
                    "pack {hex}: its list of what it holds can't be read, so it isn't in the rebuilt index"
                ));
            }
            repo.index()?
        }
        // An index file that doesn't open, and can be rebuilt: say that, not only that
        // the check couldn't run.
        Err(e) if r.repairable > 0 => {
            r.errors.push(format!("the index can't be read: {e}"));
            return Ok(r);
        }
        Err(e) => return Err(e),
    };
    let mut layout = Layout::of(repo, index)?;
    // Repairing: read (and repair) the packs first, so that what follows checks the
    // Vault as repaired.
    if read_data && opts.repair {
        let mut lost = read_packs(
            repo,
            opts,
            &layout.on_disk,
            &layout.indexed,
            progress,
            report,
            &mut r,
        )?;
        if !opts.heal {
            // Not asked to take anything out: said as found (the missing packs below).
            for (name, why) in lost.drain(..) {
                r.errors.push(format!("pack {name}: {why}"));
            }
        } else {
            // Packs the index names that aren't there at all.
            for (pack, blobs) in &layout.by_pack {
                let hex = hex_id(pack);
                if !layout.on_disk.contains_key(&hex) {
                    lost.push((hex, format!("it is missing ({} blobs)", blobs.len())));
                }
            }
        }
        if !lost.is_empty() {
            match take_exclusive(repo, &mut lock, &mut exclusive) {
                Ok(()) => {
                    let names: HashSet<String> = lost.iter().map(|(n, _)| n.clone()).collect();
                    let why: HashMap<&str, &str> =
                        lost.iter().map(|(n, w)| (n.as_str(), w.as_str())).collect();
                    for d in crate::heal::drop_lost(repo, &names)? {
                        r.repaired.push(format!(
                            "pack {}: {}. {} of its {} pieces still read and were moved to a new pack; the other {} were taken out of the index, and the next backup stores them again from the files that still have them",
                            d.pack,
                            why.get(d.pack.as_str()).copied().unwrap_or("damaged"),
                            d.saved,
                            d.saved + d.lost,
                            d.lost
                        ));
                    }
                    layout = Layout::of(repo, repo.index()?)?;
                }
                // In use by another job: said as it was found, to be tried again.
                Err(e) => {
                    for (name, why) in &lost {
                        r.errors.push(format!(
                            "pack {name}: {why}; it couldn't be taken out of the index now ({e})"
                        ));
                    }
                }
            }
        }
        progress.set_phase(Phase::Scanning);
    }
    let Layout {
        index,
        on_disk,
        by_pack,
        indexed,
    } = layout;
    r.packs = by_pack.len() as u64;
    r.blobs = index.map.len() as u64;
    let errors = Mutex::new(Vec::new());
    // The headers a few hundred packs at a time, each batch in a handful of reads: one
    // by one this is two reads a pack, an hour and more for a big Vault on a server.
    let listed: Vec<(&BlobId, &Vec<BlobId>)> = by_pack.iter().collect();
    for group in listed.chunks(256) {
        let mut present: Vec<(String, u64)> = Vec::with_capacity(group.len());
        let mut theirs: Vec<&Vec<BlobId>> = Vec::with_capacity(group.len());
        for (pack, blobs) in group {
            let hex = hex_id(*pack);
            match on_disk.get(&hex) {
                Some(&size) => {
                    present.push((hex, size));
                    theirs.push(blobs);
                }
                None => errors.locked().push(format!(
                    "pack {hex} is missing ({} blobs lost)",
                    blobs.len()
                )),
            }
        }
        let headers = crate::pack::read_headers(repo.backend(), &repo.crypto, &present);
        present
            .par_iter()
            .zip(headers)
            .zip(theirs)
            .for_each(|(((hex, _), header), blobs)| match header {
                Ok(header) => {
                    let ids: HashSet<BlobId> = header.iter().map(|e| e.id).collect();
                    if let Some(b) = blobs.iter().find(|b| !ids.contains(*b)) {
                        errors.locked().push(format!(
                            "pack {hex} lacks blob {} listed in the index",
                            hex_id(b)
                        ));
                    }
                }
                Err(e) => errors.locked().push(format!("pack {hex}: {e}")),
            });
        if progress.is_cancelled() {
            break;
        }
    }
    r.errors.extend(errors.into_inner().expect("errors"));
    for name in on_disk.keys().filter(|n| !indexed.contains(*n)) {
        r.warnings.push(format!(
            "pack {name} is not referenced by the index (an interrupted backup; prune removes it)"
        ));
    }

    // 2. Every snapshot's trees and data blobs are present.
    let (records, unreadable) = repo.record_listing()?;
    for (name, e) in unreadable {
        r.errors.push(format!("snapshot {name} can't be read: {e}"));
    }
    r.snapshots = records.len() as u64;
    let mut seen_trees: HashSet<BlobId> = HashSet::new();
    let mut used_data: HashSet<BlobId> = HashSet::new();
    for rec in &records {
        let Some(root) = parse_id(&rec.tree) else {
            r.errors
                .push(format!("snapshot {}: bad root tree id", rec.meta.id));
            continue;
        };
        let mut stack = vec![root];
        while let Some(t) = stack.pop() {
            if !seen_trees.insert(t) {
                continue;
            }
            match repo.load_tree(&t, rec.tree_format) {
                Ok(tree) => {
                    for n in tree.nodes {
                        stack.extend(n.subtree);
                        if !holes_fit(&n.holes, n.size) {
                            r.errors.push(format!(
                                "snapshot {}: the holes recorded for /{} don't fit the file",
                                rec.meta.id,
                                String::from_utf8_lossy(&n.name),
                            ));
                        }
                        used_data.extend(n.content.iter().copied());
                        for b in &n.content {
                            if !index.contains(b) {
                                r.errors.push(format!(
                                    "snapshot {}: data of /{} is missing (blob {})",
                                    rec.meta.id,
                                    String::from_utf8_lossy(&n.name),
                                    hex_id(b)
                                ));
                            }
                        }
                    }
                }
                Err(e) => r.errors.push(format!("snapshot {}: {e}", rec.meta.id)),
            }
        }
    }
    r.trees = seen_trees.len() as u64;

    // Data no snapshot uses: left by deletions before space was freed, or by interrupted
    // backups. Harmless, but it takes space.
    let (unused_blobs, unused_bytes) = index
        .map
        .iter()
        .filter(|(id, _)| !seen_trees.contains(*id) && !used_data.contains(*id))
        .fold((0u64, 0u64), |(n, b), (_, loc)| {
            (n + 1, b + u64::from(loc.entry.length))
        });
    if unused_blobs > 0 {
        r.warnings.push(format!(
            "{} ({unused_blobs} blobs) belongs to no snapshot; pruning (\"Free up space\") removes it",
            flux_core::ByteSize(unused_bytes).display_approx()
        ));
    }

    // 3. Optionally read and verify every pack (when repairing, this came first).
    if read_data && !opts.repair {
        for (name, why) in read_packs(repo, opts, &on_disk, &indexed, progress, report, &mut r)? {
            r.errors.push(format!("pack {name}: {why}"));
        }
    }
    if opts.repair {
        r.parity_added += protect::tidy(repo)?.added;
    }
    r.errors.sort();
    r.repaired.sort();
    drop(lock);
    progress.set_phase(Phase::Done);
    Ok(r)
}

/// Give up the shared lock for the exclusive one (once): the index is about to be
/// rewritten, which no backup may be in the middle of. If another job holds the Vault,
/// the shared lock is taken again and the error says who.
fn take_exclusive(
    repo: &VaultRepo,
    lock: &mut Option<crate::backend::LockGuard>,
    exclusive: &mut bool,
) -> Result<(), VaultError> {
    if *exclusive {
        return Ok(());
    }
    *lock = None;
    match repo.lock(true) {
        Ok(l) => {
            *lock = Some(l);
            *exclusive = true;
            Ok(())
        }
        Err(e) => {
            *lock = repo.lock(false).ok();
            Err(e)
        }
    }
}

/// The index, and the packs on disk it is checked against.
struct Layout {
    index: std::sync::Arc<crate::pack::Index>,
    /// Pack files there: name and size.
    on_disk: HashMap<String, u64>,
    /// The blobs the index places in each pack.
    by_pack: HashMap<BlobId, Vec<BlobId>>,
    /// The packs the index names.
    indexed: HashSet<String>,
}

impl Layout {
    fn of(repo: &VaultRepo, index: std::sync::Arc<crate::pack::Index>) -> Result<Self, VaultError> {
        let on_disk: HashMap<String, u64> = repo
            .backend()
            .list("data")
            .map_err(|e| VaultError::io("data", e))?
            .into_iter()
            .collect();
        let mut by_pack: HashMap<BlobId, Vec<BlobId>> = HashMap::new();
        for (id, loc) in &index.map {
            by_pack.entry(loc.pack).or_default().push(*id);
        }
        let indexed = by_pack.keys().map(|p| hex_id(p)).collect();
        Ok(Self {
            index,
            on_disk,
            by_pack,
            indexed,
        })
    }
}

/// Read every pack the index names and verify it: its bytes are its name, and every blob
/// in it decodes. A pack that fails and has repair data is rebuilt from it (`opts.repair`)
/// or reported as repairable; the repair data of the sound ones is checked as well, and
/// (repairing, in a Vault written with repair data) added where it's missing.
///
/// Returns the packs that fail and can't be rebuilt (name, and what is wrong in words):
/// errors for a check, and for a repair what [`crate::heal::drop_lost`] takes out.
fn read_packs(
    repo: &VaultRepo,
    opts: CheckOptions,
    on_disk: &HashMap<String, u64>,
    indexed: &HashSet<String>,
    progress: &Progress,
    report: Reporter<'_>,
    r: &mut CheckReport,
) -> Result<Vec<(String, String)>, VaultError> {
    progress.set_phase(Phase::Transferring);
    report(Event::Phase {
        phase: Phase::Transferring,
    });
    let backend = repo.backend();
    let with_parity: HashSet<String> = backend
        .list(&protect::path_for("data"))
        .map_err(|e| VaultError::io(protect::DIR, e))?
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    let packs: Vec<(&String, &u64)> = on_disk
        .iter()
        .filter(|(n, _)| indexed.contains(*n))
        .collect();
    progress
        .bytes_total
        .store(packs.iter().map(|(_, s)| **s).sum(), Relaxed);
    progress.entries_total.store(packs.len() as u64, Relaxed);
    #[derive(Default)]
    struct Found {
        errors: Vec<String>,
        warnings: Vec<String>,
        repaired: Vec<String>,
        repairable: u64,
        added: u64,
        /// Packs that fail and can't be rebuilt: name and what is wrong, in words.
        lost: Vec<(String, String)>,
    }
    let found = Mutex::new(Found::default());
    packs.par_iter().for_each(|(name, size)| {
        if progress.is_cancelled() {
            return;
        }
        let path = pack_path(name);
        let verify = |bytes: &[u8]| -> Result<(), String> {
            if !protect::pack_is_sound(name, bytes) {
                return Err("contents do not match the pack id".into());
            }
            for e in split_pack(&repo.crypto, name, bytes).map_err(|e| e.to_string())? {
                let sealed = &bytes[e.offset as usize..e.offset as usize + e.length as usize];
                decode_blob(&repo.crypto, sealed, &e).map_err(|e| e.to_string())?;
            }
            Ok(())
        };
        let result = backend
            .read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| verify(&bytes).map(|()| bytes));
        match result {
            Ok(bytes) if with_parity.contains(*name) => {
                let state = backend
                    .read(&protect::path_for(&path))
                    .map_or(ParityState::Damaged, |p| protect::parity_state(&bytes, &p));
                if state != ParityState::Sound {
                    if !opts.repair {
                        found.locked().warnings.push(format!(
                            "the repair data of pack {name} is damaged (a repair writes it again)"
                        ));
                    } else {
                        match protect::write_for(backend, &path, &bytes, repo.parity.max(1), false)
                        {
                            Ok(()) => found.locked().repaired.push(format!(
                                "pack {name}: its repair data was damaged, and was written again"
                            )),
                            Err(e) => found.locked().errors.push(format!(
                                "pack {name}: its repair data is damaged and couldn't be written again: {e}"
                            )),
                        }
                    }
                }
            }
            Ok(bytes) => {
                if opts.repair && repo.parity > 0 {
                    match protect::write_for(backend, &path, &bytes, repo.parity, false) {
                        Ok(()) => found.locked().added += 1,
                        Err(e) => found
                            .locked()
                            .warnings
                            .push(format!("pack {name}: no repair data could be written: {e}")),
                    }
                }
            }
            Err(why) => {
                let sound = |b: &[u8]| verify(b).is_ok();
                let fix = protect::repair(backend, &path, **size, &sound, opts.repair)
                    .unwrap_or_else(|e| Fix::Beyond(e.to_string()));
                let mut f = found.locked();
                match fix {
                    Fix::Repaired { blocks } => f
                        .repaired
                        .push(format!("pack {name}: {why}: repaired ({blocks} blocks)")),
                    Fix::Repairable { .. } => {
                        f.repairable += 1;
                        f.errors
                            .push(format!("pack {name}: {why} (its repair data can rebuild it)"));
                    }
                    Fix::NoParity => f.lost.push(((*name).clone(), why)),
                    Fix::Beyond(beyond) => f.lost.push((
                        (*name).clone(),
                        format!("{why}; it can't be repaired: {beyond}"),
                    )),
                }
            }
        }
        progress.bytes_done.fetch_add(**size, Relaxed);
        progress.entries_done.fetch_add(1, Relaxed);
    });
    let found = found.into_inner().expect("found");
    r.errors.extend(found.errors);
    r.warnings.extend(found.warnings);
    r.repaired.extend(found.repaired);
    r.repairable += found.repairable;
    r.parity_added += found.added;
    r.packs_read = packs.len() as u64;
    r.bytes_read = packs.iter().map(|(_, s)| **s).sum();
    if progress.is_cancelled() {
        return Err(VaultError::Cancelled);
    }
    Ok(found.lost)
}

/// Holes in order, not overlapping, not empty, and inside the file.
fn holes_fit(holes: &[crate::format::Hole], size: u64) -> bool {
    let mut pos = 0u64;
    holes.iter().all(|h| {
        let ok =
            h.len > 0 && h.offset >= pos && h.offset.checked_add(h.len).is_some_and(|e| e <= size);
        pos = h.offset.saturating_add(h.len);
        ok
    })
}

#[cfg(test)]
mod tests {
    use super::holes_fit;
    use crate::format::Hole;

    #[test]
    fn hole_lists_are_checked() {
        let h = |offset, len| Hole { offset, len };
        assert!(holes_fit(&[], 0));
        assert!(holes_fit(&[h(0, 4), h(8, 2)], 10));
        assert!(!holes_fit(&[h(8, 2), h(0, 4)], 10), "out of order");
        assert!(!holes_fit(&[h(0, 4), h(2, 4)], 10), "overlapping");
        assert!(!holes_fit(&[h(8, 4)], 10), "past the end");
        assert!(!holes_fit(&[h(3, 0)], 10), "empty");
        assert!(!holes_fit(&[h(u64::MAX, 2)], 10), "overflow");
    }
}
