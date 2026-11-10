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
use std::ops::Deref;
use std::os::unix::fs::FileExt;
use std::sync::Arc;

use flux_fs::entry::{Entry, Kind};
use flux_fs::snapshot::SnapshotMeta;
use flux_vault::crypto::BlobId;
use flux_vault::format::{Content, Span};

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
    /// Vault: each entry's content (chunks and holes).
    blobs: Vec<Content>,
}

impl SnapshotView {
    pub fn load(repo: &Arc<AnyRepo>, meta: SnapshotMeta) -> Result<Self, RepoError> {
        let (entries, blobs) = load_entries(repo, &meta)?;
        Ok(Self {
            meta,
            entries,
            repo: repo.clone(),
            blobs,
        })
    }

    /// Open regular file `i` (an index into `entries`).
    pub fn open(&self, i: usize) -> io::Result<Box<dyn FileReader>> {
        open_file(self.repo.clone(), &self.meta, &self.entries, &self.blobs, i)
    }
}

/// A snapshot's entries (sorted by path), and for a Vault each one's content (chunks and
/// holes; empty for other engines).
pub fn load_entries(
    repo: &AnyRepo,
    meta: &SnapshotMeta,
) -> Result<(Vec<Entry>, Vec<Content>), RepoError> {
    Ok(match repo {
        AnyRepo::Vault { repo: v, .. } => {
            let record = v.read_record(&meta.id)?;
            v.flatten(&record)?.into_iter().unzip()
        }
        _ => (
            repo.entries(meta)?.collect::<Result<Vec<_>, _>>()?,
            Vec::new(),
        ),
    })
}

/// Open regular file `i` of `entries` (from [`load_entries`]) where it's stored, to read
/// in place. `repo` is anything that leads to the repository: an `Arc` for a reader that
/// outlives the call (`flux mount`), or a plain reference.
pub fn open_file<'a, R>(
    repo: R,
    meta: &SnapshotMeta,
    entries: &[Entry],
    blobs: &[Content],
    i: usize,
) -> io::Result<Box<dyn FileReader + 'a>>
where
    R: Deref<Target = AnyRepo> + Send + 'a,
{
    let e = entries
        .get(i)
        .filter(|e| e.kind == Kind::File && flux_fs::entry::is_clean_path(&e.path))
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    match &*repo {
        AnyRepo::Mirror(r) => {
            let f = File::open(r.data_dir(&meta.id).join(e.rel_path()))?;
            Ok(Box::new(LocalFile(f)))
        }
        AnyRepo::MirrorRemote(_) => Ok(Box::new(RemoteFile::new(repo, meta.id.clone(), e.clone()))),
        AnyRepo::Borg(r) => Ok(Box::new(LocalFile(r.extract_file(&meta.id, e)?))),
        AnyRepo::Rsync(r) => match r.local_data_dir(&meta.id) {
            Some(base) => Ok(Box::new(LocalFile(File::open(base.join(e.rel_path()))?))),
            None => Ok(Box::new(RemoteFile::new(repo, meta.id.clone(), e.clone()))),
        },
        AnyRepo::Vault { repo: v, .. } => {
            let index = v.index().map_err(io::Error::other)?;
            let content = blobs
                .get(i)
                .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
            let mut chunks = Vec::with_capacity(content.blobs.len());
            let mut start = 0u64;
            for id in &content.blobs {
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
                repo,
                chunks,
                content: Content {
                    blobs: Vec::new(),
                    holes: content.holes.clone(),
                },
                size: e.size,
                last: None,
            }))
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

struct RemoteFile<R> {
    repo: R,
    snapshot: String,
    entry: Entry,
    /// What was read ahead: where it starts in the file, and the bytes.
    ahead: Option<(u64, Vec<u8>)>,
    /// Where the last read ended (a read that starts there is reading the file through).
    last_end: u64,
}

/// How much is fetched at once for a file that is being read through. Each fetch from
/// the server is an open, a read and a close: three round trips whatever its size. A
/// mount asks for 128 KiB at a time, which made that 1–2 MB/s on any line.
const READ_AHEAD: usize = 4 << 20;

impl<R: Deref<Target = AnyRepo> + Send> RemoteFile<R> {
    fn new(repo: R, snapshot: String, entry: Entry) -> Self {
        Self {
            repo,
            snapshot,
            entry,
            ahead: None,
            last_end: 0,
        }
    }

    fn fetch(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        match &*self.repo {
            AnyRepo::MirrorRemote(r) => {
                r.read_file_range(&self.snapshot, &self.entry, offset, len as u64)
            }
            AnyRepo::Rsync(r) => r.read_range(&self.snapshot, &self.entry, offset, len as u64),
            _ => unreachable!("remote file of a remote repository"),
        }
    }
}

impl<R: Deref<Target = AnyRepo> + Send> FileReader for RemoteFile<R> {
    fn read_at(&mut self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let end = offset + len as u64;
        // All of it is in what was read ahead.
        if let Some((from, data)) = &self.ahead
            && offset >= *from
            && end <= *from + data.len() as u64
        {
            self.last_end = end;
            let at = (offset - from) as usize;
            return Ok(data[at..at + len].to_vec());
        }
        // Reading the file through (from its start, or on from the last read): fetch
        // well ahead. A read from somewhere else gets just what it asked for.
        let through = offset == self.last_end;
        let want = if through { len.max(READ_AHEAD) } else { len };
        let want = (want as u64).min(self.entry.size.saturating_sub(offset).max(len as u64));
        let data = self.fetch(offset, want as usize)?;
        self.last_end = offset + data.len().min(len) as u64;
        let out = data[..data.len().min(len)].to_vec();
        self.ahead = (data.len() > len).then_some((offset, data));
        Ok(out)
    }
}

struct VaultFile<R> {
    repo: R,
    /// (chunk, offset in the file's data, plaintext length)
    chunks: Vec<(BlobId, u64, u64)>,
    /// The file's holes (no blobs: those are `chunks`), and its length.
    content: Content,
    size: u64,
    /// The last chunk decoded.
    last: Option<(usize, Vec<u8>)>,
}

/// The last chunk decoded is plaintext: wiped when the file is closed.
impl<R> Drop for VaultFile<R> {
    fn drop(&mut self) {
        if let Some((_, data)) = &mut self.last {
            zeroize::Zeroize::zeroize(data);
        }
    }
}

impl<R: Deref<Target = AnyRepo>> VaultFile<R> {
    fn chunk(&mut self, i: usize) -> io::Result<&[u8]> {
        if self.last.as_ref().is_none_or(|(n, _)| *n != i) {
            let AnyRepo::Vault { repo, .. } = &*self.repo else {
                unreachable!("vault file of a vault")
            };
            let index = repo.index().map_err(io::Error::other)?;
            let loc = index
                .get(&self.chunks[i].0)
                .ok_or_else(|| io::Error::other("data blob is missing"))?;
            let data = repo.read_blob(loc).map_err(io::Error::other)?;
            self.last = Some((i, data));
        }
        Ok(&self.last.as_ref().expect("just set").1)
    }
}

impl<R: Deref<Target = AnyRepo>> VaultFile<R> {
    /// Up to `len` bytes of the file's data stream (the file without its holes).
    fn read_data(&mut self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
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

impl<R: Deref<Target = AnyRepo> + Send> FileReader for VaultFile<R> {
    fn read_at(&mut self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let end = self.size.min(offset.saturating_add(len as u64));
        let mut out = Vec::with_capacity(end.saturating_sub(offset) as usize);
        let mut pos = offset;
        while pos < end {
            match self.content.span_at(pos, self.size) {
                Span::Hole { end: h } => {
                    let to = h.min(end);
                    out.resize(out.len() + (to - pos) as usize, 0);
                    pos = to;
                }
                Span::Data {
                    data_offset,
                    end: d,
                } => {
                    let want = (d.min(end) - pos) as usize;
                    let data = zeroize::Zeroizing::new(self.read_data(data_offset, want)?);
                    if data.is_empty() {
                        break; // less data stored than the file's length says
                    }
                    pos += data.len() as u64;
                    out.extend_from_slice(&data);
                }
            }
        }
        Ok(out)
    }
}
