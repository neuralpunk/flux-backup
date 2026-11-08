//! Random access to a snapshot's files, whatever the engine: what `flux mount` serves.
//!
//! * Mirror: the snapshot's plain files, read with `pread`
//! * Mirror over SSH: ranged SFTP reads
//! * rsync (compatibility mode): like Mirror, from the snapshot's plain folder
//! * Borg (compatibility mode): each opened file extracted once with `borg extract --stdout`
//! * Vault: the file's chunks, located by their plaintext lengths in the index, decrypted
//!   one at a time (the last one is kept, so sequential reads decode each chunk once)

use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
use std::sync::Arc;

use flux_fs::entry::{Entry, Kind};
use flux_fs::snapshot::SnapshotMeta;
use flux_vault::crypto::BlobId;

use crate::repo::{AnyRepo, RepoError};

/// An open file of a snapshot.
pub trait FileReader: Send {
    /// Up to `len` bytes at `offset` (fewer at the end of the file).
    fn read_at(&mut self, offset: u64, len: usize) -> io::Result<Vec<u8>>;
}

/// A snapshot's entries (sorted by path) and a way to read its files.
pub struct SnapshotView {
    pub meta: SnapshotMeta,
    pub entries: Vec<Entry>,
    repo: Arc<AnyRepo>,
    /// Vault: each entry's content chunks.
    blobs: Vec<Vec<BlobId>>,
}

impl SnapshotView {
    pub fn load(repo: &Arc<AnyRepo>, meta: SnapshotMeta) -> Result<Self, RepoError> {
        let (entries, blobs) = match &**repo {
            AnyRepo::Vault { repo: v, .. } => {
                let record = v.read_record(&meta.id)?;
                v.flatten(&record)?.into_iter().unzip()
            }
            _ => (
                repo.entries(&meta)?.collect::<Result<Vec<_>, _>>()?,
                Vec::new(),
            ),
        };
        Ok(Self {
            meta,
            entries,
            repo: repo.clone(),
            blobs,
        })
    }

    /// Open regular file `i` (an index into `entries`).
    pub fn open(&self, i: usize) -> io::Result<Box<dyn FileReader>> {
        let e = self
            .entries
            .get(i)
            .filter(|e| e.kind == Kind::File && flux_fs::entry::is_clean_path(&e.path))
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        match &*self.repo {
            AnyRepo::Mirror(r) => {
                let f = File::open(r.data_dir(&self.meta.id).join(e.rel_path()))?;
                Ok(Box::new(LocalFile(f)))
            }
            AnyRepo::MirrorRemote(_) => Ok(Box::new(RemoteFile {
                repo: self.repo.clone(),
                snapshot: self.meta.id.clone(),
                entry: e.clone(),
            })),
            AnyRepo::Borg(r) => Ok(Box::new(LocalFile(r.extract_file(&self.meta.id, e)?))),
            AnyRepo::Rsync(r) => match r.local_data_dir(&self.meta.id) {
                Some(base) => Ok(Box::new(LocalFile(File::open(base.join(e.rel_path()))?))),
                None => Ok(Box::new(RemoteFile {
                    repo: self.repo.clone(),
                    snapshot: self.meta.id.clone(),
                    entry: e.clone(),
                })),
            },
            AnyRepo::Vault { repo: v, .. } => {
                let index = v.index().map_err(io::Error::other)?;
                let mut chunks = Vec::with_capacity(self.blobs[i].len());
                let mut start = 0u64;
                for id in &self.blobs[i] {
                    let loc = index.get(id).ok_or_else(|| {
                        io::Error::other(format!(
                            "data blob {} is missing",
                            flux_vault::crypto::hex_id(id)
                        ))
                    })?;
                    chunks.push((*id, start, u64::from(loc.entry.raw_length)));
                    start += u64::from(loc.entry.raw_length);
                }
                Ok(Box::new(VaultFile {
                    repo: self.repo.clone(),
                    chunks,
                    last: None,
                }))
            }
        }
    }
}

struct LocalFile(File);

impl FileReader for LocalFile {
    fn read_at(&mut self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let mut buf = vec![0u8; len];
        let mut done = 0;
        while done < len {
            match self.0.read_at(&mut buf[done..], offset + done as u64) {
                Ok(0) => break,
                Ok(n) => done += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        buf.truncate(done);
        Ok(buf)
    }
}

struct RemoteFile {
    repo: Arc<AnyRepo>,
    snapshot: String,
    entry: Entry,
}

impl FileReader for RemoteFile {
    fn read_at(&mut self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        match &*self.repo {
            AnyRepo::MirrorRemote(r) => {
                r.read_file_range(&self.snapshot, &self.entry, offset, len as u64)
            }
            AnyRepo::Rsync(r) => r.read_range(&self.snapshot, &self.entry, offset, len as u64),
            _ => unreachable!("remote file of a remote repository"),
        }
    }
}

struct VaultFile {
    repo: Arc<AnyRepo>,
    /// (chunk, offset in the file, plaintext length)
    chunks: Vec<(BlobId, u64, u64)>,
    /// The last chunk decoded.
    last: Option<(usize, Vec<u8>)>,
}

impl VaultFile {
    fn chunk(&mut self, i: usize) -> io::Result<&[u8]> {
        if self.last.as_ref().is_none_or(|(n, _)| *n != i) {
            let AnyRepo::Vault { repo, .. } = &*self.repo else {
                unreachable!("vault file of a vault")
            };
            let index = repo.index().map_err(io::Error::other)?;
            let loc = index
                .get(&self.chunks[i].0)
                .ok_or_else(|| io::Error::other("data blob is missing"))?;
            let data = flux_vault::pack::read_blob(repo.backend(), &repo.crypto, loc)
                .map_err(io::Error::other)?;
            self.last = Some((i, data));
        }
        Ok(&self.last.as_ref().expect("just set").1)
    }
}

impl FileReader for VaultFile {
    fn read_at(&mut self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let mut out = Vec::with_capacity(len);
        // First chunk that ends after `offset`.
        let mut i = self
            .chunks
            .partition_point(|&(_, start, n)| start + n <= offset);
        let mut pos = offset;
        while out.len() < len && i < self.chunks.len() {
            let start = self.chunks[i].1;
            let want = len - out.len();
            let data = self.chunk(i)?;
            let from = (pos - start) as usize;
            let take = want.min(data.len().saturating_sub(from));
            out.extend_from_slice(&data[from..from + take]);
            pos += take as u64;
            i += 1;
        }
        Ok(out)
    }
}
