//! The snapshot manifest: one record per backed-up path, sorted by path bytes, stored as
//! length-prefixed postcard records inside a zstd stream.
//!
//! The manifest is the source of truth for metadata. The data tree can't always carry
//! ownership (a non-root backup can't `chown`), and hard-linked files share one inode with
//! older snapshots. The manifest also makes listing, diffing and change detection fast,
//! because none of them have to walk the data tree.

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

pub use flux_fs::entry::{Entry, Kind, Xattr, is_within, path_cmp};

const MAGIC: &[u8; 8] = b"FLUXMAN\0";
const VERSION: u32 = 1;
/// Refuse absurd record lengths from a corrupt file instead of allocating gigabytes.
const MAX_RECORD: u32 = 64 << 20;

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{0}: not a Flux manifest")]
    BadHeader(PathBuf),
    #[error(
        "{0} was written by a newer Flux (manifest version {1}, this one reads up to 1); update Flux Backup"
    )]
    Newer(PathBuf, u32),
    #[error("{path}: corrupt manifest record: {message}")]
    Corrupt { path: PathBuf, message: String },
}

pub struct ManifestWriter {
    path: PathBuf,
    out: BufWriter<zstd::Encoder<'static, File>>,
    buf: Vec<u8>,
    count: u64,
}

impl ManifestWriter {
    pub fn create(path: &Path) -> Result<Self, ManifestError> {
        let io = |source| ManifestError::Io {
            path: path.to_owned(),
            source,
        };
        let f = File::create(path).map_err(io)?;
        let enc = zstd::Encoder::new(f, 3).map_err(io)?;
        let mut out = BufWriter::with_capacity(256 << 10, enc);
        out.write_all(MAGIC).map_err(io)?;
        out.write_all(&VERSION.to_le_bytes()).map_err(io)?;
        Ok(Self {
            path: path.to_owned(),
            out,
            buf: Vec::with_capacity(512),
            count: 0,
        })
    }

    pub fn write(&mut self, e: &Entry) -> Result<(), ManifestError> {
        self.buf.clear();
        let bytes = postcard::to_extend(e, std::mem::take(&mut self.buf)).map_err(|err| {
            ManifestError::Corrupt {
                path: self.path.clone(),
                message: err.to_string(),
            }
        })?;
        let io = |source| ManifestError::Io {
            path: self.path.clone(),
            source,
        };
        self.out
            .write_all(&(bytes.len() as u32).to_le_bytes())
            .map_err(io)?;
        self.out.write_all(&bytes).map_err(io)?;
        self.buf = bytes;
        self.count += 1;
        Ok(())
    }

    /// Flush, finish the zstd frame and fsync. Returns the number of records.
    pub fn finish(self) -> Result<u64, ManifestError> {
        let io = |source| ManifestError::Io {
            path: self.path.clone(),
            source,
        };
        let enc = self.out.into_inner().map_err(|e| io(e.into_error()))?;
        let f = enc.finish().map_err(io)?;
        f.sync_all().map_err(io)?;
        Ok(self.count)
    }
}

/// Streaming manifest reader; yields entries in path order.
pub struct ManifestReader {
    path: PathBuf,
    input: BufReader<zstd::Decoder<'static, BufReader<File>>>,
    buf: Vec<u8>,
}

impl ManifestReader {
    pub fn open(path: &Path) -> Result<Self, ManifestError> {
        let io = |source| ManifestError::Io {
            path: path.to_owned(),
            source,
        };
        let f = File::open(path).map_err(io)?;
        let dec = zstd::Decoder::new(f).map_err(io)?;
        let mut input = BufReader::with_capacity(256 << 10, dec);
        let mut header = [0u8; 12];
        input
            .read_exact(&mut header)
            .map_err(|_| ManifestError::BadHeader(path.to_owned()))?;
        if &header[..8] != MAGIC {
            return Err(ManifestError::BadHeader(path.to_owned()));
        }
        let version = u32::from_le_bytes(header[8..].try_into().expect("4 bytes"));
        if version > VERSION {
            return Err(ManifestError::Newer(path.to_owned(), version));
        }
        Ok(Self {
            path: path.to_owned(),
            input,
            buf: Vec::new(),
        })
    }

    /// Read every entry into memory.
    pub fn read_all(path: &Path) -> Result<Vec<Entry>, ManifestError> {
        Self::open(path)?.collect()
    }

    fn next_entry(&mut self) -> Result<Option<Entry>, ManifestError> {
        let mut len = [0u8; 4];
        match self.input.read_exact(&mut len) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(source) => {
                return Err(ManifestError::Io {
                    path: self.path.clone(),
                    source,
                });
            }
        }
        let len = u32::from_le_bytes(len);
        if len > MAX_RECORD {
            return Err(ManifestError::Corrupt {
                path: self.path.clone(),
                message: format!("record length {len}"),
            });
        }
        self.buf.resize(len as usize, 0);
        self.input
            .read_exact(&mut self.buf)
            .map_err(|source| ManifestError::Io {
                path: self.path.clone(),
                source,
            })?;
        postcard::from_bytes(&self.buf)
            .map(Some)
            .map_err(|e| ManifestError::Corrupt {
                path: self.path.clone(),
                message: e.to_string(),
            })
    }
}

impl Iterator for ManifestReader {
    type Item = Result<Entry, ManifestError>;
    fn next(&mut self) -> Option<Self::Item> {
        self.next_entry().transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_fs::entry::test_entry as entry;

    #[test]
    fn round_trip() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("m.zst");
        let mut entries = vec![entry("home", Kind::Dir), entry("home/a", Kind::File)];
        entries[1].hash = Some([7; 32]);
        entries[1].xattrs.push(Xattr {
            name: b"user.x".to_vec(),
            value: vec![0, 1, 2],
        });
        let mut l = entry("home/l", Kind::Symlink);
        l.link = Some(b"\xffnon-utf8".to_vec());
        entries.push(l);

        let mut w = ManifestWriter::create(&p).unwrap();
        for e in &entries {
            w.write(e).unwrap();
        }
        assert_eq!(w.finish().unwrap(), 3);
        assert_eq!(ManifestReader::read_all(&p).unwrap(), entries);
    }

    #[test]
    fn rejects_garbage() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("m.zst");
        std::fs::write(&p, b"nope").unwrap();
        assert!(ManifestReader::open(&p).is_err());
    }

    #[test]
    fn within() {
        assert!(is_within(b"home/me", b"home"));
        assert!(is_within(b"home", b"home"));
        assert!(!is_within(b"homework", b"home"));
        assert!(is_within(b"anything", b""));
    }
}
