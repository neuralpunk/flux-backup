//! Taking a Vault snapshot.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use jiff::Timestamp;
use rayon::prelude::*;

use flux_core::Profile;
use flux_core::progress::{Event, Phase, Progress, Reporter};
use flux_fs::entry::{Entry, Kind};
use flux_fs::outcome::{BackupOutcome, PlanStats};
use flux_fs::snapshot::{FileError, SnapshotMeta, SnapshotStats, SnapshotStatus};

use crate::VaultError;
use crate::crypto::{BlobId, hex_id, random};
use crate::format::{BlobKind, IndexFile, Node, SnapshotRecord, Tree};
use crate::pack::{Packer, encode_blob, write_index};
use crate::repo::{Owner, VaultRepo};

const MAX_STORED_ERRORS: usize = 1000;

pub struct BackupRequest<'a> {
    pub owner: &'a Owner,
    pub profile: &'a Profile,
    pub name: Option<String>,
    pub dry_run: bool,
    /// Paths never scanned (the repository itself).
    pub never: Vec<PathBuf>,
    /// Where to keep the latest snapshot's file list between runs (Flux's cache), so the
    /// next backup needn't rebuild it from the repository's trees. `None`: always rebuild.
    pub cache_dir: Option<PathBuf>,
}

/// Blobs this run has stored or decided to store (dedup within the run). Split into
/// shards by the id's first byte (ids are uniformly random), so threads claiming chunks
/// rarely wait on each other.
struct Seen<'a> {
    index: &'a crate::pack::Index,
    new: [Mutex<HashSet<BlobId>>; SEEN_SHARDS],
}

const SEEN_SHARDS: usize = 64;

impl<'a> Seen<'a> {
    fn new(index: &'a crate::pack::Index) -> Self {
        Self {
            index,
            new: std::array::from_fn(|_| Mutex::new(HashSet::new())),
        }
    }

    /// True if the blob must be stored now (first time anyone sees it).
    fn claim(&self, id: &BlobId) -> bool {
        !self.index.contains(id)
            && self.new[id[0] as usize % SEEN_SHARDS]
                .lock()
                .expect("seen lock")
                .insert(*id)
    }
}

pub fn backup(
    repo: &VaultRepo,
    req: &BackupRequest<'_>,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<BackupOutcome, VaultError> {
    let started = Timestamp::now();
    let profile = req.profile;
    let _lock = repo.lock(false)?;
    let pool = flux_fs::scan::thread_pool(profile)?;

    progress.set_phase(Phase::Scanning);
    report(Event::Phase {
        phase: Phase::Scanning,
    });
    let scanned =
        pool.install(|| flux_fs::scan::scan_profile(profile, &req.never, progress, report))?;
    let mut entries = scanned.entries;
    let mut errors = scanned.errors;

    progress.set_phase(Phase::Planning);
    report(Event::Phase {
        phase: Phase::Planning,
    });
    let parent = repo
        .records()?
        .into_iter()
        .rfind(|r| req.owner.matches(&r.meta));
    let flat_cache = req
        .cache_dir
        .as_ref()
        .map(|d| flat_cache_path(d, repo, req.owner));
    let parent_flat = match &parent {
        Some(p) => match flat_cache
            .as_ref()
            .and_then(|f| load_flat(f, repo, &p.tree))
        {
            Some(flat) => flat,
            None => repo.flatten(p)?,
        },
        None => Vec::new(),
    };
    let index = repo.index()?;
    // Which files can reuse the parent's content without being read?
    let reusable: HashMap<&[u8], &(Entry, Vec<BlobId>)> = parent_flat
        .iter()
        .filter(|(e, c)| e.kind == Kind::File && c.iter().all(|b| index.contains(b)))
        .map(|pe| (pe.0.path.as_slice(), pe))
        .collect();
    let mut content: Vec<Option<Vec<BlobId>>> = vec![None; entries.len()];
    let mut to_read = Vec::new();
    for (i, e) in entries.iter_mut().enumerate() {
        if e.kind != Kind::File {
            continue;
        }
        match reusable.get(e.path.as_slice()) {
            Some((pe, c)) if e.same_file_as(pe) => {
                if e.hash.is_none() {
                    e.hash = pe.hash;
                }
                content[i] = Some(c.clone());
            }
            _ => to_read.push(i),
        }
    }
    // Counts only: no entry is copied (both lists are sorted by path).
    let summary = flux_fs::diff::summarize(parent_flat.iter().map(|(e, _)| e), entries.iter());
    let mut stats = PlanStats {
        entries: entries.len() as u64,
        new: summary.added,
        changed: summary.modified + summary.type_changed,
        deleted: summary.removed,
        copy_files: to_read.len() as u64,
        copy_bytes: to_read.iter().map(|&i| entries[i].size).sum(),
        link_files: entries.iter().filter(|e| e.kind == Kind::File).count() as u64
            - to_read.len() as u64,
        ..Default::default()
    };
    for e in &entries {
        match e.kind {
            Kind::Dir => stats.dirs += 1,
            Kind::File => {
                stats.files += 1;
                stats.total_bytes += e.size;
            }
            Kind::Symlink => stats.symlinks += 1,
            _ => stats.specials += 1,
        }
    }
    stats.unchanged = stats.entries.saturating_sub(stats.new + stats.changed);
    if req.dry_run {
        progress.set_phase(Phase::Done);
        return Ok(BackupOutcome::DryRun { stats, errors });
    }
    // Nothing changed (directory mtimes aside, as in Mirror): skip reading anything.
    if let Some(p) = &parent
        && stats.new + stats.changed + stats.deleted == 0
        && to_read.is_empty()
        && errors.is_empty()
        && !profile.options.snapshot_if_unchanged
        && req.name.is_none()
    {
        progress.set_phase(Phase::Done);
        return Ok(BackupOutcome::Unchanged {
            previous: p.meta.id.clone(),
            pruned: Vec::new(),
        });
    }

    // Read, chunk, dedup, compress, encrypt, pack.
    progress.set_phase(Phase::Transferring);
    report(Event::Phase {
        phase: Phase::Transferring,
    });
    progress.entries_total.store(to_read.len() as u64, Relaxed);
    progress.bytes_total.store(stats.copy_bytes, Relaxed);
    let level = profile.vault.as_ref().map_or(3, |v| v.compression_level);
    let seen = Seen::new(&index);
    let data_packer = Packer::new(repo.backend(), &repo.crypto);
    let read_bytes = AtomicU64::new(0);
    // Sources on a spinning disk: read large files two at a time (no seeking between many).
    let gate = flux_fs::disk::IoGate::for_paths(profile.sources.paths.iter().map(|p| p.as_path()));
    if gate.is_limited() {
        report(Event::Info {
            message: format!(
                "a spinning disk is involved: reading large files {} at a time",
                flux_fs::disk::ROTATIONAL_STREAMS
            ),
        });
    }
    let failures: Mutex<Vec<(usize, FileError)>> = Mutex::new(Vec::new());
    let results: Vec<(usize, Vec<BlobId>, u64)> = pool.install(|| {
        to_read
            .par_iter()
            .filter_map(|&i| {
                if progress.is_cancelled() {
                    return None;
                }
                let e = &entries[i];
                let r = {
                    let _turn = gate.enter(e.size);
                    store_file(repo, &seen, &data_packer, e, level, progress)
                };
                progress.entries_done.fetch_add(1, Relaxed);
                match r {
                    Ok((blobs, n)) => {
                        read_bytes.fetch_add(n, Relaxed);
                        if n != e.size {
                            report(Event::Warning {
                                path: e.abs_path().display().to_string(),
                                message: format!(
                                    "file changed while being backed up ({} → {n} bytes)",
                                    e.size
                                ),
                            });
                        }
                        Some((i, blobs, n))
                    }
                    Err(err) => {
                        let fe = FileError {
                            path: e.abs_path().display().to_string(),
                            message: err.to_string(),
                        };
                        report(Event::Warning {
                            path: fe.path.clone(),
                            message: fe.message.clone(),
                        });
                        progress.warnings.fetch_add(1, Relaxed);
                        failures.lock().expect("failures lock").push((i, fe));
                        None
                    }
                }
            })
            .collect()
    });
    if progress.is_cancelled() {
        // Packs written so far have no index and are removed by the next prune.
        return Err(VaultError::Cancelled);
    }
    for (i, blobs, n) in results {
        content[i] = Some(blobs);
        entries[i].size = n; // what was actually stored
    }
    let failures = failures.into_inner().expect("failures lock");
    if !failures.is_empty() && !profile.options.continue_on_errors {
        return Err(VaultError::Aborted(failures[0].1.message.clone()));
    }
    let failed: HashSet<usize> = failures.iter().map(|(i, _)| *i).collect();
    errors.extend(failures.into_iter().map(|(_, fe)| fe));

    // Trees, bottom-up.
    progress.set_phase(Phase::Finalizing);
    report(Event::Phase {
        phase: Phase::Finalizing,
    });
    let tree_packer = Packer::new(repo.backend(), &repo.crypto);
    let root = build_trees(repo, &seen, &tree_packer, &entries, &content, &failed)?;

    let mut packs = data_packer.finish()?;
    let tree_bytes = tree_packer.bytes_written.load(Relaxed);
    let data_bytes: u64 = packs
        .iter()
        .flat_map(|p| p.blobs.iter())
        .map(|b| u64::from(b.length))
        .sum();
    packs.extend(tree_packer.finish()?);
    if !packs.is_empty() {
        // Every pack on disk before the index that names them (one sync, not two per pack).
        repo.backend()
            .flush_packs()
            .map_err(|e| VaultError::io("data", e))?;
        write_index(
            repo.backend(),
            &repo.crypto,
            &IndexFile {
                packs,
                supersedes: Vec::new(),
            },
        )?;
        repo.invalidate_index();
    }

    let root_hex = hex_id(&root);
    let errors_total = errors.len() as u64;
    errors.truncate(MAX_STORED_ERRORS);
    let copied_files = (to_read.len() - failed.len()) as u64;
    let meta = SnapshotMeta {
        format: crate::format::FORMAT,
        id: hex::encode(random::<8>()?),
        profile: req.owner.profile.clone(),
        host: req.owner.host.clone(),
        user: req.owner.user.clone(),
        started,
        finished: Timestamp::now(),
        status: if errors_total == 0 {
            SnapshotStatus::Complete
        } else {
            SnapshotStatus::CompleteWithErrors
        },
        name: req.name.clone(),
        pinned: false,
        sources: profile.sources.paths.clone(),
        stats: SnapshotStats {
            entries: stats.entries - failed.len() as u64,
            files: stats.files - failed.len() as u64,
            dirs: stats.dirs,
            symlinks: stats.symlinks,
            specials: stats.specials,
            total_bytes: entries
                .iter()
                .enumerate()
                .filter(|(i, e)| e.kind == Kind::File && !failed.contains(i))
                .map(|(_, e)| e.size)
                .sum(),
            new: stats.new,
            changed: stats.changed,
            unchanged: stats.unchanged,
            deleted: stats.deleted,
            copied_bytes: read_bytes.into_inner(),
            copied_files,
            linked_files: stats.link_files,
            stored_bytes: data_bytes + tree_bytes,
        },
        errors,
        errors_total,
        flux_version: flux_core::VERSION.to_owned(),
    };
    // The new snapshot's file list for the next run, exactly as `flatten` would return it.
    if let Some(f) = &flat_cache {
        let flat: Vec<(Entry, Vec<BlobId>)> = entries
            .iter()
            .enumerate()
            .filter(|(i, _)| !failed.contains(i))
            .map(|(i, e)| {
                let entry = Node::from_entry(e, Vec::new()).to_entry(e.path.clone());
                let blobs = if e.kind == Kind::File {
                    content[i].clone().unwrap_or_default()
                } else {
                    Vec::new()
                };
                (entry, blobs)
            })
            .collect();
        save_flat(f, repo, &root_hex, &flat);
    }
    repo.write_record(&SnapshotRecord {
        meta: meta.clone(),
        tree: root_hex,
        parent: parent.map(|p| p.meta.id),
    })?;
    progress.set_phase(Phase::Done);
    report(Event::Phase { phase: Phase::Done });
    Ok(BackupOutcome::Created {
        snapshot: Box::new(meta),
        pruned: Vec::new(),
    })
}

/// Chunk one file and store new chunks. Returns (blob ids, bytes read).
///
/// Files up to the maximum chunk size are read whole and chunked in memory. Most files are
/// small, and fastcdc's streaming chunker allocates a max-size (8 MiB) buffer per file,
/// which would dominate the run. Both chunkers cut at the same points.
fn store_file(
    repo: &VaultRepo,
    seen: &Seen<'_>,
    packer: &Packer<'_>,
    e: &Entry,
    level: u8,
    progress: &Progress,
) -> Result<(Vec<BlobId>, u64), VaultError> {
    let path = e.abs_path();
    let io_err = |err: std::io::Error| VaultError::io(&path.display().to_string(), err);
    let mut f = File::open(&path).map_err(io_err)?;
    let c = &repo.config.chunker;
    let (min, avg, max) = (c.min as usize, c.avg as usize, c.max as usize);
    let seed = repo.crypto.chunk_seed();
    let level1 = fastcdc::v2020::Normalization::Level1;
    let mut blobs = Vec::new();
    let mut total = 0u64;
    let mut store = |data: &[u8]| -> Result<(), VaultError> {
        if progress.is_cancelled() {
            return Err(VaultError::Cancelled);
        }
        let id = repo.crypto.blob_id(data);
        if seen.claim(&id) {
            let (_, sealed) = encode_blob(&repo.crypto, data, level)?;
            packer.add(BlobKind::Data, id, data.len(), &sealed)?;
        }
        blobs.push(id);
        total += data.len() as u64;
        progress.bytes_done.fetch_add(data.len() as u64, Relaxed);
        Ok(())
    };

    if e.size as usize <= max {
        use std::io::Read;
        let mut data = Vec::with_capacity(e.size as usize + 1);
        // Read at most max+1 bytes: if the file grew past the limit, fall through to streaming.
        (&mut f)
            .take(max as u64 + 1)
            .read_to_end(&mut data)
            .map_err(io_err)?;
        if data.len() <= max {
            if data.is_empty() {
                return Ok((Vec::new(), 0));
            }
            for chunk in
                fastcdc::v2020::FastCDC::with_level_and_seed(&data, min, avg, max, level1, seed)
            {
                store(&data[chunk.offset..chunk.offset + chunk.length])?;
            }
            return Ok((blobs, total));
        }
        use std::io::Seek;
        f.rewind().map_err(io_err)?;
    }
    // Larger files: this thread reads and cuts the next batch of chunks while the pool's
    // other threads hash, compress and encrypt the batch before it, so one big file (a
    // disk image, a video) no longer runs on a single core. Chunks keep their order.
    let mut chunks = fastcdc::v2020::StreamCDC::with_level_and_seed(
        BufReader::with_capacity(1 << 20, f),
        min,
        avg,
        max,
        level1,
        seed,
    );
    let mut read_batch = || -> Result<Vec<Vec<u8>>, VaultError> {
        let mut batch = Vec::new();
        let mut bytes = 0;
        while bytes < STREAM_BATCH {
            let Some(chunk) = chunks.next() else { break };
            let chunk = chunk.map_err(|err| io_err(std::io::Error::other(err.to_string())))?;
            bytes += chunk.data.len();
            batch.push(chunk.data);
        }
        Ok(batch)
    };
    let mut current = read_batch()?;
    while !current.is_empty() {
        if progress.is_cancelled() {
            return Err(VaultError::Cancelled);
        }
        let (next, sealed) = rayon::join(&mut read_batch, || {
            current
                .par_iter()
                .map(|data| -> Result<(BlobId, Option<Vec<u8>>), VaultError> {
                    let id = repo.crypto.blob_id(data);
                    let sealed = if seen.claim(&id) {
                        Some(encode_blob(&repo.crypto, data, level)?.1)
                    } else {
                        None
                    };
                    Ok((id, sealed))
                })
                .collect::<Result<Vec<_>, _>>()
        });
        for (data, (id, sealed)) in current.iter().zip(sealed?) {
            if let Some(sealed) = sealed {
                packer.add(BlobKind::Data, id, data.len(), &sealed)?;
            }
            blobs.push(id);
            total += data.len() as u64;
            progress.bytes_done.fetch_add(data.len() as u64, Relaxed);
        }
        current = next?;
    }
    Ok((blobs, total))
}

/// The cached file list of the latest snapshot, one file per repository and owner.
fn flat_cache_path(dir: &Path, repo: &VaultRepo, owner: &Owner) -> PathBuf {
    let key = format!(
        "{}\0{}\0{}\0{}",
        repo.config.id, owner.profile, owner.host, owner.user
    );
    dir.join(format!(
        "flat-{}",
        &blake3::hash(key.as_bytes()).to_hex()[..24]
    ))
}

type Flat = Vec<(Entry, Vec<BlobId>)>;

/// The file list cached in `cache_dir` for `owner`'s snapshot with root tree `tree`, if any
/// (what the next backup will use instead of [`VaultRepo::flatten`]).
pub fn cached_file_list(
    cache_dir: &Path,
    repo: &VaultRepo,
    owner: &Owner,
    tree: &str,
) -> Option<Flat> {
    load_flat(&flat_cache_path(cache_dir, repo, owner), repo, tree)
}

/// The cached list, if it belongs to the snapshot whose root tree is `tree` (encrypted with
/// the repository's own key; anything unreadable is ignored and rebuilt).
fn load_flat(path: &Path, repo: &VaultRepo, tree: &str) -> Option<Flat> {
    let sealed = std::fs::read(path).ok()?;
    let plain = repo.crypto.open(&sealed).ok()?;
    let raw = zstd::bulk::decompress(&plain, 1 << 30).ok()?;
    let (cached_tree, flat): (String, Flat) = postcard::from_bytes(&raw).ok()?;
    (cached_tree == tree).then_some(flat)
}

fn save_flat(path: &Path, repo: &VaultRepo, tree: &str, flat: &Flat) {
    let write = || -> Option<()> {
        let raw = postcard::to_allocvec(&(tree, flat)).ok()?;
        let packed = zstd::bulk::compress(&raw, 1).ok()?;
        let sealed = repo.crypto.seal(&packed).ok()?;
        std::fs::create_dir_all(path.parent()?).ok()?;
        flux_core::store::write_atomic(path, &sealed, 0o600).ok()
    };
    if write().is_none() {
        let _ = std::fs::remove_file(path);
    }
}

/// How much of a large file is read ahead while the batch before is processed.
const STREAM_BATCH: usize = 16 << 20;

/// Build tree blobs from sorted entries, deepest directories first. Returns the root id.
fn build_trees(
    repo: &VaultRepo,
    seen: &Seen<'_>,
    packer: &Packer<'_>,
    entries: &[Entry],
    content: &[Option<Vec<BlobId>>],
    failed: &HashSet<usize>,
) -> Result<BlobId, VaultError> {
    fn parent_of(p: &[u8]) -> &[u8] {
        p.iter()
            .rposition(|&b| b == b'/')
            .map_or(&[][..], |i| &p[..i])
    }
    let mut children: HashMap<&[u8], Vec<usize>> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        if !failed.contains(&i) {
            children.entry(parent_of(&e.path)).or_default().push(i);
        }
    }
    let mut dirs: Vec<&[u8]> = entries
        .iter()
        .filter(|e| e.kind == Kind::Dir)
        .map(|e| e.path.as_slice())
        .collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.iter().filter(|&&b| b == b'/').count()));
    dirs.push(b""); // the root, last
    let mut subtree: HashMap<&[u8], BlobId> = HashMap::new();
    for dir in dirs {
        let mut nodes: Vec<Node> = children
            .get(dir)
            .map(|v| v.as_slice())
            .unwrap_or_default()
            .iter()
            .map(|&i| {
                let e = &entries[i];
                let name = e.path
                    [parent_of(&e.path).len() + usize::from(!parent_of(&e.path).is_empty())..]
                    .to_vec();
                let mut n = Node::from_entry(e, name);
                if e.kind == Kind::File {
                    n.content = content[i].clone().unwrap_or_default();
                }
                if e.kind == Kind::Dir {
                    n.subtree = subtree.get(e.path.as_slice()).copied();
                }
                n
            })
            .collect();
        nodes.sort_by(|a, b| a.name.cmp(&b.name));
        let bytes = postcard::to_allocvec(&Tree { nodes })
            .map_err(|e| VaultError::Corrupt(e.to_string()))?;
        let id = repo.crypto.blob_id(&bytes);
        if seen.claim(&id) {
            let (_, sealed) = encode_blob(&repo.crypto, &bytes, 3)?;
            packer.add(BlobKind::Tree, id, bytes.len(), &sealed)?;
        }
        subtree.insert(dir, id);
    }
    Ok(subtree[&b""[..]])
}
