//! Blob encoding and pack files.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::VaultError;
use crate::backend::{Backend, pack_path};
use crate::crypto::{BlobId, Crypto, hex_id};
use crate::format::{BlobKind, IndexFile, IndexPack, PackEntry};

/// Close a pack once it reaches this size.
pub const PACK_TARGET: usize = 16 << 20;
const RAW: u8 = 0;
const ZSTD: u8 = 1;

/// Encode plaintext as a stored blob: compress if it helps, then seal.
/// Returns (id, sealed bytes).
pub fn encode_blob(
    crypto: &Crypto,
    plaintext: &[u8],
    level: u8,
) -> Result<(BlobId, Vec<u8>), VaultError> {
    let id = crypto.blob_id(plaintext);
    let mut payload = Vec::with_capacity(plaintext.len() + 1);
    let compressed = (level > 0)
        .then(|| zstd::bulk::compress(plaintext, i32::from(level)).ok())
        .flatten();
    match compressed {
        Some(c) if c.len() < plaintext.len() => {
            payload.push(ZSTD);
            payload.extend_from_slice(&c);
        }
        _ => {
            payload.push(RAW);
            payload.extend_from_slice(plaintext);
        }
    }
    Ok((id, crypto.seal(&payload)?))
}

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
        Self {
            buf: Vec::with_capacity(PACK_TARGET + (1 << 20)),
            entries: Vec::new(),
        }
    }
}

/// Serialize a pack: blobs, sealed header, header length.
fn finish(crypto: &Crypto, b: Builder) -> Result<(BlobId, Vec<u8>, Vec<PackEntry>), VaultError> {
    let mut bytes = b.buf;
    let header = crypto.seal(
        &postcard::to_allocvec(&b.entries).map_err(|e| VaultError::Corrupt(e.to_string()))?,
    )?;
    bytes.extend_from_slice(&header);
    bytes.extend_from_slice(&(header.len() as u32).to_le_bytes());
    let id = *blake3::hash(&bytes).as_bytes();
    Ok((id, bytes, b.entries))
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
    /// Pack bytes written so far.
    pub bytes_written: std::sync::atomic::AtomicU64,
}

impl<'a> Packer<'a> {
    pub fn new(backend: &'a dyn Backend, crypto: &'a Crypto) -> Self {
        Self {
            backend,
            crypto,
            current: Mutex::new(Builder::new()),
            written: Mutex::new(Vec::new()),
            bytes_written: Default::default(),
        }
    }

    /// Add a sealed blob (from [`encode_blob`]).
    pub fn add(
        &self,
        kind: BlobKind,
        id: BlobId,
        raw_length: usize,
        sealed: &[u8],
    ) -> Result<(), VaultError> {
        let full = {
            let mut cur = self.current.lock().expect("packer lock");
            let offset = cur.buf.len() as u32;
            cur.buf.extend_from_slice(sealed);
            cur.entries.push(PackEntry {
                kind,
                id,
                offset,
                length: sealed.len() as u32,
                raw_length: raw_length as u32,
            });
            (cur.buf.len() >= PACK_TARGET).then(|| std::mem::replace(&mut *cur, Builder::new()))
        };
        match full {
            Some(b) => self.write(b),
            None => Ok(()),
        }
    }

    fn write(&self, b: Builder) -> Result<(), VaultError> {
        if b.entries.is_empty() {
            return Ok(());
        }
        let (id, bytes, entries) = finish(self.crypto, b)?;
        let hex = hex_id(&id);
        let path = pack_path(&hex);
        // Durable later: the caller flushes once before writing the index (see write_pack).
        self.backend
            .write_pack(&path, &bytes)
            .map_err(|e| VaultError::io(&path, e))?;
        self.bytes_written
            .fetch_add(bytes.len() as u64, std::sync::atomic::Ordering::Relaxed);
        self.written.lock().expect("packer lock").push(IndexPack {
            pack: id,
            blobs: entries,
        });
        Ok(())
    }

    /// Write the last, partly filled pack. Returns everything written.
    pub fn finish(self) -> Result<Vec<IndexPack>, VaultError> {
        let last = std::mem::replace(
            &mut *self.current.lock().expect("packer lock"),
            Builder::new(),
        );
        self.write(last)?;
        Ok(self.written.into_inner().expect("packer lock"))
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
        let files: Vec<(String, IndexFile)> = names
            .par_iter()
            .map(|n| {
                let path = format!("index/{n}");
                let raw = backend.read(&path).map_err(|e| VaultError::io(&path, e))?;
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

/// Write an index file; returns its id.
pub fn write_index(
    backend: &dyn Backend,
    crypto: &Crypto,
    file: &IndexFile,
) -> Result<String, VaultError> {
    let id = hex::encode(crate::crypto::random::<8>()?);
    let path = format!("index/{id}");
    let bytes = crypto
        .seal(&postcard::to_allocvec(file).map_err(|e| VaultError::Corrupt(e.to_string()))?)?;
    backend
        .write(&path, &bytes)
        .map_err(|e| VaultError::io(&path, e))?;
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
