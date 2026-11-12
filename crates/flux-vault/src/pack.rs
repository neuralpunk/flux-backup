//! Blob encoding and pack files.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::VaultError;
use crate::backend::{Backend, pack_path};
use crate::crypto::{BlobId, Crypto, hex_id};
use crate::format::{BlobKind, IndexFile, IndexPack, PackEntry};
use flux_core::sync::Locked;

/// Close a pack once it reaches this size.
pub const PACK_TARGET: usize = 16 << 20;
/// Room for the largest pack: the target, the blob that crossed it (up to the largest
/// chunk, 8 MiB, sealed), then padding (at most 1/32 at this size) and the header. A
/// buffer that had to grow past this would be copied, and too big to reuse.
const PACK_ROOM: usize = PACK_TARGET + (10 << 20);
/// Written packs' buffers a [`Packer`] keeps for its next packs: as many as can be away
/// at once (queued for a writer, or being written, see [`Packer::write_behind`]), so a
/// backup doesn't allocate new ones as it goes; more would be memory held for nothing.
const SPARE_PACKS: usize = 8;
const RAW: u8 = 0;
const ZSTD: u8 = 1;

/// Encode plaintext as a stored blob: compress if it helps, then seal.
/// Returns (id, sealed bytes).
pub fn encode_blob(
    crypto: &Crypto,
    plaintext: &[u8],
    level: u8,
) -> Result<(BlobId, Vec<u8>), VaultError> {
    let mut sealed = Vec::new();
    seal_blob(crypto, plaintext, level, &mut sealed)?;
    Ok((crypto.blob_id(plaintext), sealed))
}

/// [`encode_blob`] into `out` (replacing what it held), without the id: a backup has the
/// id already (it decided by it whether to store the blob), and keeps `out` for the next
/// blob. The blob is compressed and sealed in place, with this thread's zstd compressor:
/// a new buffer per step, and a new compressor per blob, were most of what storing a
/// blob cost (a megabyte of incompressible data took three times as long as it does now).
pub fn seal_blob(
    crypto: &Crypto,
    plaintext: &[u8],
    level: u8,
    out: &mut Vec<u8>,
) -> Result<(), VaultError> {
    let prefix = crypto.seal_prefix_len();
    out.clear();
    out.reserve(prefix + 1 + plaintext.len() + 16);
    out.resize(prefix, 0);
    if !(level > 0 && compress_into(plaintext, level, out)) {
        out.push(RAW);
        out.extend_from_slice(plaintext);
    }
    Ok(crypto.seal_in_place(out)?)
}

/// This thread's zstd compressors: one at level 1 (samples, and data that barely
/// compresses), one at the level asked for; and the buffer they write to.
struct Zstd {
    fast: zstd::bulk::Compressor<'static>,
    asked: Option<(u8, zstd::bulk::Compressor<'static>)>,
    buf: Vec<u8>,
}

thread_local! {
    static COMPRESSOR: std::cell::Cell<Option<Zstd>> = const { std::cell::Cell::new(None) };
}

/// Blobs at least this long are sampled before they are compressed (see [`compress_into`]).
const SAMPLE_FROM: usize = 128 << 10;
/// The sample: this much from the middle of the blob.
const SAMPLE: usize = 64 << 10;

/// How hard to compress `plaintext`, asked to at `level` (more than 0): a sample from its
/// middle (past a file's header, which often compresses better than the rest), compressed
/// at level 1, tells. `None`: it would save under 3% (already compressed: photos, video,
/// archives); store it as it is. Under 10%: level 1, which saves nearly as much on such
/// data as higher levels, two to seven times faster (a JPEG at level 3 compresses at
/// 550 MiB/s on one core, at level 1 at 1350, both to 95%). Otherwise `level`.
fn level_for(z: &mut Zstd, plaintext: &[u8], level: u8) -> Option<u8> {
    if plaintext.len() < SAMPLE_FROM || level == 1 {
        return Some(level);
    }
    let from = (plaintext.len() - SAMPLE) / 2;
    z.buf.clear();
    z.buf.reserve(zstd::zstd_safe::compress_bound(SAMPLE));
    let n = z
        .fast
        .compress_to_buffer(&plaintext[from..from + SAMPLE], &mut z.buf)
        .ok()?;
    match n * 100 / SAMPLE {
        97.. => None,
        90.. => Some(1),
        _ => Some(level),
    }
}

/// Append `plaintext` compressed (at `level`, or less: see [`level_for`]) to `out`, after
/// the [`ZSTD`] mark, if that's smaller than storing it raw. Returns whether it did.
fn compress_into(plaintext: &[u8], level: u8, out: &mut Vec<u8>) -> bool {
    let z = match COMPRESSOR.take() {
        Some(z) => Some(z),
        None => zstd::bulk::Compressor::new(1).ok().map(|fast| Zstd {
            fast,
            asked: None,
            buf: Vec::new(),
        }),
    };
    let Some(mut z) = z else { return false };
    let smaller = (|| {
        let level = level_for(&mut z, plaintext, level)?;
        let compressor = if level == 1 {
            &mut z.fast
        } else {
            if z.asked.as_ref().is_none_or(|(l, _)| *l != level) {
                z.asked = Some((level, zstd::bulk::Compressor::new(i32::from(level)).ok()?));
            }
            &mut z.asked.as_mut().expect("just set").1
        };
        z.buf.clear();
        z.buf
            .reserve(zstd::zstd_safe::compress_bound(plaintext.len()));
        let n = compressor
            .compress_to_buffer(plaintext, &mut z.buf)
            .ok()
            .filter(|n| *n < plaintext.len())?;
        out.push(ZSTD);
        out.extend_from_slice(&z.buf[..n]);
        Some(())
    })()
    .is_some();
    // A buffer grown for an unusually large blob isn't kept for the life of the thread.
    if z.buf.capacity() > KEEP_BUFFER {
        z.buf = Vec::new();
    }
    COMPRESSOR.set(Some(z));
    smaller
}

/// Buffers up to this size are kept for reuse by the thread that grew them.
pub(crate) const KEEP_BUFFER: usize = 32 << 20;

/// Decode a stored blob and verify it against its id.
pub fn decode_blob(
    crypto: &Crypto,
    sealed: &[u8],
    entry: &PackEntry,
) -> Result<Vec<u8>, VaultError> {
    let payload = crypto.open(sealed)?;
    let plain = match payload.split_first() {
        Some((&RAW, body)) => body.to_vec(),
        Some((&ZSTD, body)) => zstd::bulk::decompress(body, entry.raw_length as usize)
            .map_err(|e| VaultError::Corrupt(format!("blob {}: {e}", hex_id(&entry.id))))?,
        _ => {
            return Err(VaultError::Corrupt(format!(
                "blob {}: unknown encoding",
                hex_id(&entry.id)
            )));
        }
    };
    if crypto.blob_id(&plain) != entry.id {
        return Err(VaultError::Corrupt(format!(
            "blob {} does not match its id",
            hex_id(&entry.id)
        )));
    }
    Ok(plain)
}

/// Accumulates blobs of one kind into a pack.
#[derive(Debug)]
struct Builder {
    buf: Vec<u8>,
    entries: Vec<PackEntry>,
}

impl Builder {
    fn new() -> Self {
        Self::with_buf(Vec::new())
    }

    /// A builder that fills `buf` (emptied; a written pack's, to reuse).
    fn with_buf(mut buf: Vec<u8>) -> Self {
        buf.clear();
        buf.reserve(PACK_ROOM);
        Self {
            buf,
            entries: Vec::new(),
        }
    }
}

/// The Padmé length for `len` bytes (Nikitin et al., "Reducing Metadata Leakage from
/// Encrypted Files and Communication with PURBs", 2019): rounded up so only about
/// log₂(log₂ len) bits of the length remain, at most about 12% bigger (less for big
/// lengths: a 16 MiB pack grows by at most 3%).
pub fn padme(len: u64) -> u64 {
    if len < 2 {
        return len;
    }
    let e = 63 - u64::from(len.leading_zeros()); // ⌊log₂ len⌋
    let s = 64 - u64::from(e.leading_zeros()); // ⌊log₂ e⌋ + 1
    let mask = (1u64 << (e - s)) - 1;
    (len + mask) & !mask
}

/// Serialize a pack: blobs, sealed header, header length. `pad`: random bytes between the
/// blobs and the header bring the whole file to its [`padme`] length, so a pack holding
/// one small file doesn't give away that file's size. Readers find the header from the
/// end and the blobs by their offsets, so padding needs nothing from them (Flux before 1.0
/// reads padded packs too).
fn finish(
    crypto: &Crypto,
    b: Builder,
    pad: bool,
) -> Result<(BlobId, Vec<u8>, Vec<PackEntry>), VaultError> {
    let mut bytes = b.buf;
    let header = crypto.seal(
        &postcard::to_allocvec(&b.entries).map_err(|e| VaultError::Corrupt(e.to_string()))?,
    )?;
    if pad {
        let total = (bytes.len() + header.len() + 4) as u64;
        let extra = (padme(total) - total) as usize;
        let start = bytes.len();
        bytes.resize(start + extra, 0);
        getrandom::fill(&mut bytes[start..])
            .map_err(|e| VaultError::Corrupt(format!("no randomness: {e}")))?;
    }
    bytes.extend_from_slice(&header);
    bytes.extend_from_slice(&(header.len() as u32).to_le_bytes());
    let id = *blake3::hash(&bytes).as_bytes();
    Ok((id, bytes, b.entries))
}

/// How much of a pack's end is read at once for its header: the header of a pack of
/// ordinary blobs fits, and one read then does where two were needed.
const HEADER_GUESS: u64 = 64 << 10;

/// The headers of many packs (`(pack id in hex, size)`), in a few batched reads: the end
/// of each pack, where its header's length and (nearly always) the header itself are.
pub fn read_headers(
    backend: &dyn Backend,
    crypto: &Crypto,
    packs: &[(String, u64)],
) -> Vec<Result<Vec<PackEntry>, VaultError>> {
    let reads: Vec<crate::backend::ReadAt> = packs
        .iter()
        .map(|(hex, size)| {
            let n = (*size).min(HEADER_GUESS);
            (pack_path(hex), size - n, Some(n))
        })
        .collect();
    backend
        .read_many(&reads)
        .into_iter()
        .zip(packs)
        .map(|(tail, (hex, size))| {
            let path = pack_path(hex);
            let tail = tail.map_err(|e| VaultError::io(&path, e))?;
            if tail.len() < 4 {
                return Err(VaultError::Corrupt(format!("pack {hex} is truncated")));
            }
            let (body, len) = tail.split_at(tail.len() - 4);
            let hlen = u64::from(u32::from_le_bytes(len.try_into().expect("4 bytes")));
            if hlen + 4 > *size {
                return Err(VaultError::Corrupt(format!(
                    "pack {hex} has a bad header length"
                )));
            }
            let header = if hlen as usize <= body.len() {
                body[body.len() - hlen as usize..].to_vec()
            } else {
                backend
                    .read_range(&path, size - 4 - hlen, hlen)
                    .map_err(|e| VaultError::io(&path, e))?
            };
            postcard::from_bytes(&crypto.open(&header)?)
                .map_err(|e| VaultError::Corrupt(format!("pack {hex} header: {e}")))
        })
        .collect()
}

/// Read a pack's header, given the pack's total size.
pub fn read_header(
    backend: &dyn Backend,
    crypto: &Crypto,
    pack_hex: &str,
    size: u64,
) -> Result<Vec<PackEntry>, VaultError> {
    let path = pack_path(pack_hex);
    if size < 4 {
        return Err(VaultError::Corrupt(format!("pack {pack_hex} is truncated")));
    }
    let tail = backend
        .read_range(&path, size - 4, 4)
        .map_err(|e| VaultError::io(&path, e))?;
    let hlen = u64::from(u32::from_le_bytes(tail.try_into().expect("4 bytes")));
    if hlen + 4 > size {
        return Err(VaultError::Corrupt(format!(
            "pack {pack_hex} has a bad header length"
        )));
    }
    let header = backend
        .read_range(&path, size - 4 - hlen, hlen)
        .map_err(|e| VaultError::io(&path, e))?;
    postcard::from_bytes(&crypto.open(&header)?)
        .map_err(|e| VaultError::Corrupt(format!("pack {pack_hex} header: {e}")))
}

/// Parse every blob of a whole pack file (for `check --read-data` and repacking).
pub fn split_pack(
    crypto: &Crypto,
    pack_hex: &str,
    bytes: &[u8],
) -> Result<Vec<PackEntry>, VaultError> {
    if bytes.len() < 4 {
        return Err(VaultError::Corrupt(format!("pack {pack_hex} is truncated")));
    }
    let hlen = u32::from_le_bytes(bytes[bytes.len() - 4..].try_into().expect("4 bytes")) as usize;
    let start = bytes
        .len()
        .checked_sub(4 + hlen)
        .ok_or_else(|| VaultError::Corrupt(format!("pack {pack_hex} has a bad header length")))?;
    let entries: Vec<PackEntry> =
        postcard::from_bytes(&crypto.open(&bytes[start..bytes.len() - 4])?)
            .map_err(|e| VaultError::Corrupt(format!("pack {pack_hex} header: {e}")))?;
    for e in &entries {
        if e.offset as usize + e.length as usize > start {
            return Err(VaultError::Corrupt(format!(
                "pack {pack_hex}: blob outside the data area"
            )));
        }
    }
    Ok(entries)
}

/// Thread-safe pack writer for one blob kind. Full packs are written as soon as they fill.
pub struct Packer<'a> {
    backend: &'a dyn Backend,
    crypto: &'a Crypto,
    current: Mutex<Builder>,
    written: Mutex<Vec<IndexPack>>,
    /// Buffers of packs already written, for the next packs: a new 17 MiB buffer per pack
    /// is memory the kernel has to hand over (and clear) again each time.
    spare: Mutex<Vec<Vec<u8>>>,
    /// Where full packs go while [`Packer::write_behind`]'s writers run.
    behind: Mutex<Option<std::sync::mpsc::SyncSender<Builder>>>,
    /// Pack bytes written so far.
    pub bytes_written: std::sync::atomic::AtomicU64,
    /// Pad each pack to its [`padme`] length.
    pad: bool,
    /// Write repair data with each pack: this share of it, in percent (0: none).
    parity: u8,
    /// Where pack writes are timed (the performance inspector).
    timings: Option<&'a flux_core::progress::Timings>,
    /// Why a blob handed to this packer didn't reach the repository. A failed pack write
    /// loses the blobs of many files, not only of the one whose blob filled the pack, and
    /// other files already count on them (they were deduplicated against them). So it
    /// isn't one file's error: nothing more is accepted, and [`Packer::finish`] fails.
    failed: Mutex<Option<String>>,
}

impl<'a> Packer<'a> {
    pub fn new(backend: &'a dyn Backend, crypto: &'a Crypto) -> Self {
        Self::padded(backend, crypto, false)
    }

    /// A packer that pads its packs (`pad`) to hide their exact sizes.
    pub fn padded(backend: &'a dyn Backend, crypto: &'a Crypto, pad: bool) -> Self {
        Self {
            backend,
            crypto,
            current: Mutex::new(Builder::new()),
            written: Mutex::new(Vec::new()),
            spare: Mutex::new(Vec::new()),
            behind: Mutex::new(None),
            bytes_written: Default::default(),
            pad,
            parity: 0,
            timings: None,
            failed: Mutex::new(None),
        }
    }

    /// The packer for `repo`'s settings: padding, and repair data.
    pub fn for_repo(repo: &'a crate::repo::VaultRepo) -> Self {
        let mut p = Self::padded(repo.backend(), &repo.crypto, repo.pad_packs);
        p.parity = repo.parity;
        p
    }

    /// Time each pack written into `t`.
    pub fn timed(mut self, t: &'a flux_core::progress::Timings) -> Self {
        self.timings = Some(t);
        self
    }

    /// Add a sealed blob (from [`encode_blob`]).
    pub fn add(
        &self,
        kind: BlobKind,
        id: BlobId,
        raw_length: usize,
        sealed: &[u8],
    ) -> Result<(), VaultError> {
        self.check()?;
        let full = {
            let mut cur = self.current.locked();
            let offset = cur.buf.len() as u32;
            cur.buf.extend_from_slice(sealed);
            cur.entries.push(PackEntry {
                kind,
                id,
                offset,
                length: sealed.len() as u32,
                raw_length: raw_length as u32,
            });
            (cur.buf.len() >= PACK_TARGET).then(|| {
                let buf = self.spare.locked().pop().unwrap_or_default();
                std::mem::replace(&mut *cur, Builder::with_buf(buf))
            })
        };
        let Some(b) = full else { return Ok(()) };
        let behind = self.behind.locked().clone();
        match behind {
            // Waits here only while the writers are a full queue behind: the drive (or
            // the network) is what sets the pace then.
            Some(tx) => tx
                .send(b)
                .map_err(|_| VaultError::Store("the pack writers stopped".into())),
            None => self.write(b).inspect_err(|e| self.fail(e)),
        }
    }

    /// Write full packs in the background, on [`Backend::pack_writers`] threads of `scope`,
    /// until the returned guard is dropped (then the packs queued are written, and the
    /// threads end with the scope). A thread that fills a pack goes on reading and
    /// sealing instead of waiting for the write: a 16 MiB pack takes 30–80 ms to reach a
    /// fast USB drive, and longer to a server or a slow disk. At most [`QUEUED_PACKS`]
    /// wait, so memory stays bounded when the storage is the slower side.
    pub fn write_behind<'scope, 'env>(
        &'env self,
        scope: &'scope std::thread::Scope<'scope, 'env>,
    ) -> WriteBehind<'env, 'a>
    where
        'a: 'env,
    {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Builder>(QUEUED_PACKS);
        let rx = std::sync::Arc::new(Mutex::new(rx));
        for _ in 0..self.backend.pack_writers().max(1) {
            let rx = rx.clone();
            scope.spawn(move || {
                loop {
                    let next = rx.locked().recv();
                    let Ok(b) = next else { break };
                    // After a failed write, the rest are dropped: the backup fails anyway.
                    if self.check().is_ok()
                        && let Err(e) = self.write(b)
                    {
                        self.fail(&e);
                    }
                }
            });
        }
        *self.behind.locked() = Some(tx);
        WriteBehind { packer: self }
    }

    /// Record that a blob this run counts on was lost (see `failed`).
    pub fn fail(&self, why: &VaultError) {
        self.failed.locked().get_or_insert_with(|| why.to_string());
    }

    /// An error if a blob handed to this packer was lost.
    pub fn check(&self) -> Result<(), VaultError> {
        match &*self.failed.locked() {
            Some(why) => Err(VaultError::Store(why.clone())),
            None => Ok(()),
        }
    }

    fn write(&self, b: Builder) -> Result<(), VaultError> {
        if b.entries.is_empty() {
            return Ok(());
        }
        let (id, bytes, entries) = finish(self.crypto, b, self.pad)?;
        let hex = hex_id(&id);
        let path = pack_path(&hex);
        // Durable later: the caller flushes once before writing the index (see write_pack).
        let t = std::time::Instant::now();
        self.backend
            .write_pack(&path, &bytes)
            .map_err(|e| VaultError::io(&path, e))?;
        // Its repair data, from the bytes just written (durable when the pack is).
        if self.parity > 0 {
            crate::protect::write_for(self.backend, &path, &bytes, self.parity, true)?;
        }
        if let Some(timings) = self.timings {
            timings.wrote(bytes.len() as u64, t.elapsed());
        }
        self.bytes_written
            .fetch_add(bytes.len() as u64, std::sync::atomic::Ordering::Relaxed);
        self.written.locked().push(IndexPack {
            pack: id,
            blobs: entries,
        });
        let mut spare = self.spare.locked();
        if spare.len() < SPARE_PACKS && bytes.capacity() <= KEEP_BUFFER {
            spare.push(bytes);
        }
        Ok(())
    }

    /// Write the last, partly filled pack. Returns everything written.
    pub fn finish(self) -> Result<Vec<IndexPack>, VaultError> {
        self.check()?;
        let last = std::mem::replace(&mut *self.current.locked(), Builder::new());
        self.write(last)?;
        Ok(self.written.into_value())
    }
}

/// Full packs waiting for a writer (see [`Packer::write_behind`]).
const QUEUED_PACKS: usize = 2;

/// While this lives, a [`Packer`]'s full packs are written in the background. Dropping it
/// lets the writers finish what is queued and end.
pub struct WriteBehind<'p, 'a> {
    packer: &'p Packer<'a>,
}

impl Drop for WriteBehind<'_, '_> {
    fn drop(&mut self) {
        self.packer.behind.locked().take();
    }
}

/// Where a blob is stored.
#[derive(Debug, Clone, Copy)]
pub struct Loc {
    pub pack: BlobId,
    pub entry: PackEntry,
}

/// All blobs known to the repository.
#[derive(Debug, Default)]
pub struct Index {
    pub map: HashMap<BlobId, Loc>,
    /// Ids of the index files that were loaded (not superseded).
    pub files: Vec<String>,
    /// Index files still there that a newer one supersedes: what a prune cut short left
    /// behind. Ignored here; the next prune must take them along (see `gc_locked`), or
    /// they'd count again once the file that supersedes them is itself replaced.
    pub stale: Vec<String>,
}

impl Index {
    pub fn load(backend: &dyn Backend, crypto: &Crypto) -> Result<Self, VaultError> {
        let names: Vec<String> = backend
            .list("index")
            .map_err(|e| VaultError::io("index", e))?
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        use rayon::prelude::*;
        // (Read in batches: a Vault that is seldom pruned has an index file per backup.)
        let reads: Vec<crate::backend::ReadAt> = names
            .iter()
            .map(|n| (format!("index/{n}"), 0, None))
            .collect();
        let raws = backend.read_many(&reads);
        let files: Vec<(String, IndexFile)> = names
            .par_iter()
            .zip(raws)
            .map(|(n, raw)| {
                let path = format!("index/{n}");
                let raw = raw.map_err(|e| VaultError::io(&path, e))?;
                let f: IndexFile = postcard::from_bytes(&crypto.open(&raw)?)
                    .map_err(|e| VaultError::Corrupt(format!("{path}: {e}")))?;
                Ok((n.clone(), f))
            })
            .collect::<Result<_, VaultError>>()?;
        let superseded: std::collections::HashSet<&String> = files
            .iter()
            .flat_map(|(_, f)| f.supersedes.iter())
            .collect();
        let mut idx = Index::default();
        for (name, f) in &files {
            if superseded.contains(name) {
                idx.stale.push(name.clone());
                continue;
            }
            idx.files.push(name.clone());
            for p in &f.packs {
                idx.add_pack(p);
            }
        }
        Ok(idx)
    }

    pub fn add_pack(&mut self, p: &IndexPack) {
        for e in &p.blobs {
            self.map.insert(
                e.id,
                Loc {
                    pack: p.pack,
                    entry: *e,
                },
            );
        }
    }

    pub fn contains(&self, id: &BlobId) -> bool {
        self.map.contains_key(id)
    }

    pub fn get(&self, id: &BlobId) -> Option<&Loc> {
        self.map.get(id)
    }
}

/// Write an index file; returns its id. `parity`: with repair data, this share of it in
/// percent (0: none).
pub fn write_index(
    backend: &dyn Backend,
    crypto: &Crypto,
    file: &IndexFile,
    parity: u8,
) -> Result<String, VaultError> {
    let id = hex::encode(crate::crypto::random::<8>()?);
    let path = format!("index/{id}");
    let bytes = crypto
        .seal(&postcard::to_allocvec(file).map_err(|e| VaultError::Corrupt(e.to_string()))?)?;
    backend
        .write(&path, &bytes)
        .map_err(|e| VaultError::io(&path, e))?;
    if parity > 0 {
        crate::protect::write_for(backend, &path, &bytes, parity, false)?;
    }
    Ok(id)
}

/// Read and decode one blob.
pub fn read_blob(backend: &dyn Backend, crypto: &Crypto, loc: &Loc) -> Result<Vec<u8>, VaultError> {
    let path = pack_path(&hex_id(&loc.pack));
    let sealed = backend
        .read_range(
            &path,
            u64::from(loc.entry.offset),
            u64::from(loc.entry.length),
        )
        .map_err(|e| VaultError::io(&path, e))?;
    decode_blob(crypto, &sealed, &loc.entry)
}

#[cfg(test)]
mod padding_tests {
    use super::padme;

    #[test]
    fn padme_lengths() {
        assert_eq!(padme(0), 0);
        assert_eq!(padme(1), 1);
        for len in [
            2u64,
            3,
            100,
            1000,
            4097,
            65_537,
            1 << 20,
            (16 << 20) + 1,
            1 << 40,
        ] {
            let p = padme(len);
            assert!(p >= len, "{len} → {p}");
            assert!((p - len) as f64 <= len as f64 * 0.12 + 1.0, "{len} → {p}");
            assert_eq!(padme(p), p, "a padded length stays put");
        }
        // Lengths close together share a padded length: that's what hides them.
        assert_eq!(padme(10_000), padme(10_100));
        assert!(padme((16 << 20) + 1) - ((16 << 20) + 1) <= (16 << 20) / 32);
    }
}

#[cfg(test)]
mod seal_tests {
    use super::*;
    use crate::crypto::MasterKey;

    /// Data that doesn't compress isn't compressed; data that barely does (like photos)
    /// is compressed at level 1; the rest at the level asked for. Short blobs aren't
    /// sampled.
    #[test]
    fn the_level_follows_a_sample() {
        let mut z = Zstd {
            fast: zstd::bulk::Compressor::new(1).unwrap(),
            asked: None,
            buf: Vec::new(),
        };
        let mut random = vec![0u8; 1 << 20];
        getrandom::fill(&mut random).unwrap();
        // About 7.6 bits per byte: 95% of the size at best, like a JPEG.
        let narrow: Vec<u8> = random.iter().map(|b| b % 200).collect();
        let text = b"the same line, over and over\n".repeat(40_000);
        assert_eq!(level_for(&mut z, &random, 3), None);
        assert_eq!(level_for(&mut z, &narrow, 3), Some(1));
        assert_eq!(level_for(&mut z, &text, 3), Some(3));
        assert_eq!(level_for(&mut z, &text, 19), Some(19));
        assert_eq!(
            level_for(&mut z, &random[..100_000], 3),
            Some(3),
            "too short to sample"
        );
        // What gets stored: raw, then smaller.
        let crypto = Crypto::Plain {
            id_key: [1; 32],
            chunk_seed: 0,
        };
        let mut out = Vec::new();
        seal_blob(&crypto, &random, 3, &mut out).unwrap();
        assert_eq!(out[1], RAW);
        seal_blob(&crypto, &narrow, 3, &mut out).unwrap();
        assert_eq!(out[1], ZSTD);
        assert!(out.len() < narrow.len() * 97 / 100);
    }

    /// What a backup stores with [`seal_blob`] (a buffer reused from blob to blob, the
    /// thread's compressor at more than one level) reads back as [`encode_blob`]'s does.
    #[test]
    fn sealed_in_place_blobs_decode() {
        let mut random = vec![0u8; 300_000];
        getrandom::fill(&mut random).unwrap();
        let text = b"the same line, over and over\n".repeat(10_000);
        let keys = [
            Crypto::Encrypted(MasterKey::generate().unwrap()),
            Crypto::Plain {
                id_key: [7; 32],
                chunk_seed: 1,
            },
        ];
        let mut out = b"left over from the blob before".to_vec();
        for crypto in &keys {
            for level in [3u8, 0, 19, 3] {
                for (plain, compresses) in [
                    (&random[..], false),
                    (&text[..], level > 0),
                    (&b""[..], false),
                ] {
                    seal_blob(crypto, plain, level, &mut out).unwrap();
                    assert!(
                        (out.len() < plain.len()) == compresses,
                        "level {level}: compressed only when that helps"
                    );
                    let entry = PackEntry {
                        kind: BlobKind::Data,
                        id: crypto.blob_id(plain),
                        offset: 0,
                        length: out.len() as u32,
                        raw_length: plain.len() as u32,
                    };
                    assert_eq!(decode_blob(crypto, &out, &entry).unwrap(), plain);
                    let (id, sealed) = encode_blob(crypto, plain, level).unwrap();
                    assert_eq!(id, entry.id);
                    assert_eq!(decode_blob(crypto, &sealed, &entry).unwrap(), plain);
                }
            }
        }
    }
}
