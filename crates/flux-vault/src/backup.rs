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
use flux_core::progress::{Event, Phase, Progress, Reporter, Stage};
use flux_fs::entry::{Entry, Kind};
use flux_fs::outcome::{BackupOutcome, PlanStats};
use flux_fs::snapshot::{FileError, SnapshotMeta, SnapshotStats, SnapshotStatus};

use crate::VaultError;
use crate::crypto::{BlobId, hex_id, random};
use crate::format::{BlobKind, Content, Hole, IndexFile, Node, SnapshotRecord, Tree};
use crate::pack::{Packer, encode_blob, write_index};
use crate::repo::{Owner, VaultRepo};
use flux_core::sync::Locked;

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
    /// Sources whose drive isn't connected (left out of `profile`): their files are kept
    /// from the previous snapshot instead of looking deleted.
    pub keep: Vec<PathBuf>,
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
        !self.index.contains(id) && self.new[id[0] as usize % SEEN_SHARDS].locked().insert(*id)
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
    if !req.keep.is_empty()
        && let Some(p) = &parent
    {
        let kept = flux_fs::scan::carry_over(
            &mut entries,
            parent_flat.iter().map(|(e, _)| e.clone()),
            &req.keep,
        );
        let names: Vec<String> = req.keep.iter().map(|p| p.display().to_string()).collect();
        report(Event::Info {
            message: format!(
                "kept {kept} items of {} from snapshot {}: the drive isn't connected",
                names.join(" and "),
                p.meta.id
            ),
        });
    }
    let index = repo.index()?;
    let tree_format = repo.tree_format();
    // Which files can reuse the parent's content without being read? (Format 1 can't keep
    // holes; its snapshots never have any, so this is only a guard.)
    let reusable: HashMap<&[u8], &(Entry, Content)> = parent_flat
        .iter()
        .filter(|(e, c)| {
            e.kind == Kind::File
                && c.blobs.iter().all(|b| index.contains(b))
                && (tree_format >= 2 || c.holes.is_empty())
        })
        .map(|pe| (pe.0.path.as_slice(), pe))
        .collect();
    let mut content: Vec<Option<Content>> = vec![None; entries.len()];
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
    let data_packer =
        Packer::padded(repo.backend(), &repo.crypto, repo.pad_packs).timed(&progress.timings);
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
    let opener = flux_fs::fsops::SourceOpener::new(
        &flux_fs::scan::normalize_sources(&profile.sources.paths),
        profile.options.symlinks != flux_core::profile::SymlinkMode::Preserve,
    );
    let failures: Mutex<Vec<(usize, FileError)>> = Mutex::new(Vec::new());
    let results: Vec<(usize, Content, u64)> = pool.install(|| {
        to_read
            .par_iter()
            .filter_map(|&i| {
                if progress.is_cancelled() {
                    return None;
                }
                let e = &entries[i];
                let r = {
                    let _turn = gate.enter(e.size);
                    store_file(
                        repo,
                        &seen,
                        (&data_packer, &opener),
                        e,
                        level,
                        tree_format >= 2,
                        progress,
                    )
                };
                progress.entries_done.fetch_add(1, Relaxed);
                match r {
                    Ok((c, data, n)) => {
                        read_bytes.fetch_add(data, Relaxed);
                        if n != e.size {
                            report(Event::Warning {
                                path: e.abs_path().display().to_string(),
                                message: format!(
                                    "file changed while being backed up ({} → {n} bytes)",
                                    e.size
                                ),
                            });
                        }
                        Some((i, c, n))
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
                        failures.locked().push((i, fe));
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
    // A pack that couldn't be written took the data of files that seemed to succeed.
    data_packer.check()?;
    for (i, c, n) in results {
        content[i] = Some(c);
        entries[i].size = n; // what was actually stored
    }
    let failures = failures.into_value();
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
    let tree_packer =
        Packer::padded(repo.backend(), &repo.crypto, repo.pad_packs).timed(&progress.timings);
    let root = build_trees(
        repo,
        &seen,
        &tree_packer,
        &entries,
        &content,
        &failed,
        tree_format,
    )?;

    let mut packs = data_packer.finish()?;
    let tree_bytes = tree_packer.bytes_written.load(Relaxed);
    let data_bytes: u64 = packs
        .iter()
        .flat_map(|p| p.blobs.iter())
        .map(|b| u64::from(b.length))
        .sum();
    packs.extend(tree_packer.finish()?);
    undisturbed(repo, &index, &packs)?;
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
        format: tree_format,
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
        sources: {
            let mut all: Vec<PathBuf> = profile
                .sources
                .paths
                .iter()
                .chain(&req.keep)
                .cloned()
                .collect();
            all.sort();
            all.dedup();
            all
        },
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
        sources_missing: req
            .keep
            .iter()
            .map(|p| flux_fs::snapshot::SourceMissing {
                path: p.clone(),
                kept_from: parent.as_ref().map(|r| r.meta.id.clone()),
            })
            .collect(),
        note: None,
        tags: Vec::new(),
    };
    // The new snapshot's file list for the next run, exactly as `flatten` would return it.
    if let Some(f) = &flat_cache {
        let flat: Vec<(Entry, Content)> = entries
            .iter()
            .enumerate()
            .filter(|(i, _)| !failed.contains(i))
            .map(|(i, e)| {
                let entry = Node::from_entry(e, Vec::new()).to_entry(e.path.clone());
                let c = if e.kind == Kind::File {
                    content[i].clone().unwrap_or_default()
                } else {
                    Content::default()
                };
                (entry, c)
            })
            .collect();
        save_flat(f, repo, &root_hex, &flat);
    }
    repo.write_record(&SnapshotRecord {
        meta: meta.clone(),
        tree: root_hex,
        parent: parent.map(|p| p.meta.id),
        tree_format,
    })?;
    progress.set_phase(Phase::Done);
    report(Event::Phase { phase: Phase::Done });
    Ok(BackupOutcome::Created {
        snapshot: Box::new(meta),
        pruned: Vec::new(),
    })
}

/// Chunk one file and store new chunks. Returns (its content, data bytes read, the file's
/// length as stored: the data plus the holes).
///
/// `sparse` (format 2): a file with holes has only its data read and stored, and the holes
/// recorded. Format 1 reads every file densely (holes as zeros), as Flux before 1.0 did.
///
/// Files up to the maximum chunk size are read whole and chunked in memory. Most files are
/// small, and fastcdc's streaming chunker allocates a max-size (8 MiB) buffer per file,
/// which would dominate the run. Both chunkers cut at the same points.
#[allow(clippy::too_many_arguments)]
fn store_file(
    repo: &VaultRepo,
    seen: &Seen<'_>,
    (packer, opener): (&Packer<'_>, &flux_fs::fsops::SourceOpener),
    e: &Entry,
    level: u8,
    sparse: bool,
    progress: &Progress,
) -> Result<(Content, u64, u64), VaultError> {
    let path = e.abs_path();
    let io_err = |err: std::io::Error| VaultError::io(&path.display().to_string(), err);
    let f = opener.open(&path).map_err(io_err)?;
    let runs = if sparse {
        flux_fs::fsops::data_segments(&f, e.size).map_err(io_err)?
    } else {
        None
    };
    let holes: Vec<Hole> = runs
        .as_deref()
        .map(|r| Content::holes_between(r, e.size))
        .unwrap_or_default();
    let hole_bytes: u64 = holes.iter().map(|h| h.len).sum();
    // Holes count as done at once, as Mirror's sparse copy counts them.
    progress.bytes_done.fetch_add(hole_bytes, Relaxed);
    let data_len = e.size - hole_bytes;
    let reader = || -> Result<Box<dyn std::io::Read + Send>, VaultError> {
        let f = f.try_clone().map_err(io_err)?;
        Ok(match &runs {
            Some(r) => Box::new(Segments::new(f, r.clone())),
            None => {
                use std::io::Seek;
                let mut f = f;
                f.rewind().map_err(io_err)?;
                Box::new(f)
            }
        })
    };
    let c = &repo.config.chunker;
    let (min, avg, max) = (c.min as usize, c.avg as usize, c.max as usize);
    let seed = repo.crypto.chunk_seed();
    let level1 = fastcdc::v2020::Normalization::Level1;
    let mut blobs = Vec::new();
    let mut total = 0u64;
    let timings = &progress.timings;
    let mut store = |data: &[u8]| -> Result<(), VaultError> {
        if progress.is_cancelled() {
            return Err(VaultError::Cancelled);
        }
        let id = timings.time(Stage::Dedup, || repo.crypto.blob_id(data));
        if seen.claim(&id) {
            // Claimed: other files now count on this blob being stored.
            let (_, sealed) = timings
                .time(Stage::Compress, || encode_blob(&repo.crypto, data, level))
                .inspect_err(|e| packer.fail(e))?;
            packer.add(BlobKind::Data, id, data.len(), &sealed)?;
        }
        blobs.push(id);
        total += data.len() as u64;
        progress.bytes_done.fetch_add(data.len() as u64, Relaxed);
        Ok(())
    };
    let done = |blobs: Vec<BlobId>, total: u64, holes: Vec<Hole>| {
        // A sparse file that changed while being read: its holes were found before the
        // read and no longer describe what was read (one could end past the new size,
        // which no reader of the snapshot accepts). It isn't stored this time.
        if !holes.is_empty() && total != data_len {
            return Err(io_err(std::io::Error::other(
                "a sparse file that changed while being backed up; it is backed up next time",
            )));
        }
        let n = total + hole_bytes;
        Ok((Content { blobs, holes }, total, n))
    };

    if data_len as usize <= max {
        use std::io::Read;
        let mut data = Vec::with_capacity(data_len as usize + 1);
        // Read at most max+1 bytes: if the file grew past the limit, fall through to streaming.
        timings.time(Stage::Read, || {
            reader()?
                .take(max as u64 + 1)
                .read_to_end(&mut data)
                .map_err(io_err)
        })?;
        if data.len() <= max {
            if data.is_empty() {
                return done(Vec::new(), 0, holes);
            }
            let cuts: Vec<_> = timings.time(Stage::Chunk, || {
                fastcdc::v2020::FastCDC::with_level_and_seed(&data, min, avg, max, level1, seed)
                    .collect()
            });
            for chunk in cuts {
                store(&data[chunk.offset..chunk.offset + chunk.length])?;
            }
            return done(blobs, total, holes);
        }
    }
    // Larger files: this thread reads and cuts the next batch of chunks while the pool's
    // other threads hash, compress and encrypt the batch before it, so one big file (a
    // disk image, a video) no longer runs on a single core. Chunks keep their order.
    let mut chunks = fastcdc::v2020::StreamCDC::with_level_and_seed(
        BufReader::with_capacity(1 << 20, reader()?),
        min,
        avg,
        max,
        level1,
        seed,
    );
    let mut read_batch = || -> Result<Vec<Vec<u8>>, VaultError> {
        let t = std::time::Instant::now();
        let mut batch = Vec::new();
        let mut bytes = 0;
        while bytes < STREAM_BATCH {
            let Some(chunk) = chunks.next() else { break };
            let chunk = chunk.map_err(|err| io_err(std::io::Error::other(err.to_string())))?;
            bytes += chunk.data.len();
            batch.push(chunk.data);
        }
        // Reading a big file and cutting it happen together here.
        timings.add(Stage::Read, t.elapsed());
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
                    let id = timings.time(Stage::Dedup, || repo.crypto.blob_id(data));
                    let sealed = if seen.claim(&id) {
                        Some(
                            timings
                                .time(Stage::Compress, || encode_blob(&repo.crypto, data, level))
                                .inspect_err(|e| packer.fail(e))?
                                .1,
                        )
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
    done(blobs, total, holes)
}

/// Before a backup commits: is what it stored, and what it counted on, still there?
///
/// A backup holds a lock while it runs, but another computer can take that lock for stale
/// (this one slept or lost its connection for longer than a lock lives, or the clocks
/// disagree) and prune the Vault meanwhile: the packs this run wrote aren't in any index
/// yet, so they'd be deleted as left over, and data it deduplicated against may be gone.
/// A snapshot written after that would name data that doesn't exist. So: every index file
/// this run started from must still be there (a prune that removed or moved data replaces
/// them), and so must every pack it wrote.
fn undisturbed(
    repo: &VaultRepo,
    index: &crate::pack::Index,
    packs: &[crate::format::IndexPack],
) -> Result<(), VaultError> {
    let gone = |what: &str| {
        VaultError::Store(format!(
            "{what} was removed from the Vault while this backup ran (another computer pruned it, taking this backup's lock for abandoned)"
        ))
    };
    let backend = repo.backend();
    let now: HashSet<String> = backend
        .list("index")
        .map_err(|e| VaultError::io("index", e))?
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    if index.files.iter().any(|f| !now.contains(f)) {
        return Err(gone("data this backup counted on"));
    }
    // A few packs: ask for each. Many: one listing is cheaper.
    let present: Option<HashSet<String>> = if packs.len() > 32 {
        Some(
            backend
                .list("data")
                .map_err(|e| VaultError::io("data", e))?
                .into_iter()
                .map(|(n, _)| n)
                .collect(),
        )
    } else {
        None
    };
    for p in packs {
        let hex = hex_id(&p.pack);
        let there = match &present {
            Some(names) => names.contains(&hex),
            None => {
                let path = crate::backend::pack_path(&hex);
                backend
                    .exists(&path)
                    .map_err(|e| VaultError::io(&path, e))?
            }
        };
        if !there {
            return Err(gone("data this backup had stored"));
        }
    }
    Ok(())
}

/// The data of a sparse file, its runs read one after the other (the holes left out).
struct Segments {
    file: File,
    runs: std::vec::IntoIter<(u64, u64)>,
    /// (file offset, bytes left) of the run being read.
    current: Option<(u64, u64)>,
}

impl Segments {
    fn new(file: File, runs: Vec<(u64, u64)>) -> Self {
        Self {
            file,
            runs: runs.into_iter(),
            current: None,
        }
    }
}

impl std::io::Read for Segments {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use std::os::unix::fs::FileExt;
        loop {
            let (off, left) = match self.current {
                Some((_, 0)) | None => match self.runs.next() {
                    Some(r) => {
                        self.current = Some(r);
                        continue;
                    }
                    None => return Ok(0),
                },
                Some(c) => c,
            };
            let want = buf.len().min(left as usize);
            let n = self.file.read_at(&mut buf[..want], off)?;
            // The file shrank while being read: this run ends here.
            self.current = Some(if n == 0 {
                (off, 0)
            } else {
                (off + n as u64, left - n as u64)
            });
            if n > 0 {
                return Ok(n);
            }
        }
    }
}

/// The cached file list of the latest snapshot, one file per repository and owner.
fn flat_cache_path(dir: &Path, repo: &VaultRepo, owner: &Owner) -> PathBuf {
    let key = format!(
        "{}\0{}\0{}\0{}",
        repo.config.id, owner.profile, owner.host, owner.user
    );
    // "flat2": the list holds holes since format 2; older lists are left alone (and unused).
    dir.join(format!(
        "flat2-{}",
        &blake3::hash(key.as_bytes()).to_hex()[..24]
    ))
}

type Flat = Vec<(Entry, Content)>;

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
    content: &[Option<Content>],
    failed: &HashSet<usize>,
    tree_format: u32,
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
                    let c = content[i].clone().unwrap_or_default();
                    n.content = c.blobs;
                    n.holes = c.holes;
                }
                if e.kind == Kind::Dir {
                    n.subtree = subtree.get(e.path.as_slice()).copied();
                }
                n
            })
            .collect();
        nodes.sort_by(|a, b| a.name.cmp(&b.name));
        let bytes = Tree { nodes }
            .encode(tree_format)
            .map_err(|e| VaultError::Corrupt(format!("tree format {tree_format}: {e}")))?;
        let id = repo.crypto.blob_id(&bytes);
        if seen.claim(&id) {
            let (_, sealed) = encode_blob(&repo.crypto, &bytes, 3)?;
            packer.add(BlobKind::Tree, id, bytes.len(), &sealed)?;
        }
        subtree.insert(dir, id);
    }
    Ok(subtree[&b""[..]])
}
