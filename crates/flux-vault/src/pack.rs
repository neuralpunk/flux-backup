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
/// Written packs' buffers a [`Packer`] keeps for its next packs (one per thread that
/// can be writing a pack at once is plenty; more is memory held for nothing).
const SPARE_PACKS: usize = 4;
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

/// This thread's zstd compressor (and the level it's set to), and the buffer it writes to.
struct Zstd {
    level: u8,
    compressor: zstd::bulk::Compressor<'static>,
    buf: Vec<u8>,
}

thread_local! {
    static COMPRESSOR: std::cell::Cell<Option<Zstd>> = const { std::cell::Cell::new(None) };
}

/// Append `plaintext` compressed at `level` to `out` (after the [`ZSTD`] mark) if that's
/// smaller than storing it raw. Returns whether it did.
fn compress_into(plaintext: &[u8], level: u8, out: &mut Vec<u8>) -> bool {
    let mut z = match COMPRESSOR.take() {
        Some(z) if z.level == level => z,
        _ => match zstd::bulk::Compressor::new(i32::from(level)) {
            Ok(compressor) => Zstd {
                level,
                compressor,
                buf: Vec::new(),
            },
            Err(_) => return false,
        },
    };
    z.buf.clear();
    z.buf
        .reserve(zstd::zstd_safe::compress_bound(plaintext.len()));
    let smaller = match z.compressor.compress_to_buffer(plaintext, &mut z.buf) {
        Ok(n) if n < plaintext.len() => {
            out.push(ZSTD);
            out.extend_from_slice(&z.buf[..n]);
            true
        }
        _ => false,
    };
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
        buf.reserve(PACK_TARGET + (1 << 20));
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
        match full {
            Some(b) => self.write(b).inspect_err(|e| self.fail(e)),
            None => Ok(()),
        }
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
