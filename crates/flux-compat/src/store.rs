//! The folder that holds a compatibility-mode repository: a local directory, or a
//! directory on an SSH server (files over SFTP, the rest through the remote shell).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use flux_core::progress::Progress;
use flux_fs::fsops::Copier;
use flux_transport::lock::RemoteLock;
use flux_transport::{SftpPool, SshTarget, shell_quote};

use crate::CompatError;

pub enum Store {
    Local {
        root: PathBuf,
    },
    Remote {
        pool: Arc<SftpPool>,
        /// Path on the server.
        root: String,
    },
}

/// A held repository lock; released on drop.
pub enum StoreLock {
    Local(#[allow(dead_code)] File),
    Remote(#[allow(dead_code)] RemoteLock),
}

impl Store {
    pub fn local(root: impl Into<PathBuf>) -> Self {
        Store::Local { root: root.into() }
    }

    pub fn remote(pool: Arc<SftpPool>, root: String) -> Self {
        Store::Remote { pool, root }
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, Store::Remote { .. })
    }

    /// The repository root as a path on the machine that holds it.
    pub fn root_path(&self) -> String {
        match self {
            Store::Local { root } => root.display().to_string(),
            Store::Remote { root, .. } => root.clone(),
        }
    }

    /// For people: a local path or `ssh://user@host/path`.
    pub fn describe(&self) -> String {
        match self {
            Store::Local { root } => root.display().to_string(),
            Store::Remote { pool, root } => format!(
                "ssh://{}/{}",
                pool.target().destination(),
                root.trim_start_matches('/')
            ),
        }
    }

    pub fn ssh(&self) -> Option<&SshTarget> {
        match self {
            Store::Remote { pool, .. } => Some(pool.target()),
            Store::Local { .. } => None,
        }
    }

    pub fn pool(&self) -> Option<&Arc<SftpPool>> {
        match self {
            Store::Remote { pool, .. } => Some(pool),
            Store::Local { .. } => None,
        }
    }

    /// Local path of `rel` (local stores only).
    pub fn local_path(&self, rel: &str) -> Option<PathBuf> {
        match self {
            Store::Local { root } => Some(join_local(root, rel)),
            Store::Remote { .. } => None,
        }
    }

    /// `rel` as a path on the machine that holds the repository.
    pub fn path(&self, rel: &str) -> String {
        match self {
            Store::Local { root } => join_local(root, rel).display().to_string(),
            Store::Remote { root, .. } => join_remote(root, rel),
        }
    }

    pub fn read(&self, rel: &str) -> io::Result<Vec<u8>> {
        match self {
            Store::Local { root } => fs::read(join_local(root, rel)),
            Store::Remote { pool, root } => pool.with(|s| s.read_file(&join_remote(root, rel))),
        }
    }

    /// [`Store::read`] for many files at once, answered in order: on a server, a batch
    /// in two round trips instead of four for each file.
    pub fn read_many(&self, rels: &[String]) -> Vec<io::Result<Vec<u8>>> {
        match self {
            Store::Local { root } => rels.iter().map(|r| fs::read(join_local(root, r))).collect(),
            Store::Remote { pool, root } => {
                let reqs: Vec<flux_transport::sftp::ReadReq> = rels
                    .iter()
                    .map(|r| flux_transport::sftp::ReadReq::whole(&join_remote(root, r)))
                    .collect();
                match pool.with(|s| Ok(s.read_many(&reqs))) {
                    Ok(v) => v,
                    Err(e) => rels
                        .iter()
                        .map(|_| Err(io::Error::new(e.kind(), e.to_string())))
                        .collect(),
                }
            }
        }
    }

    /// The small files among `files` (path in the repository, size), to be fetched in
    /// batches during a restore from a server (`None` for a local store).
    pub fn small_files(
        &self,
        files: impl IntoIterator<Item = (String, u64)>,
    ) -> Option<flux_transport::SmallFiles> {
        match self {
            Store::Local { .. } => None,
            Store::Remote { pool, root } => Some(flux_transport::SmallFiles::new(
                pool.clone(),
                files
                    .into_iter()
                    .map(|(rel, size)| (join_remote(root, &rel).into_bytes(), size)),
            )),
        }
    }

    /// Where `rel` is on the server (remote stores).
    pub fn remote_path(&self, rel: &str) -> Option<String> {
        match self {
            Store::Local { .. } => None,
            Store::Remote { root, .. } => Some(join_remote(root, rel)),
        }
    }

    /// Replace `rel` atomically (write a temporary file, then rename it).
    pub fn write(&self, rel: &str, data: &[u8], mode: u32) -> Result<(), CompatError> {
        match self {
            Store::Local { root } => {
                let path = join_local(root, rel);
                flux_core::store::write_atomic(&path, data, mode)
                    .map_err(|e| CompatError::io(&path, io::Error::other(e)))
            }
            Store::Remote { pool, root } => {
                let path = join_remote(root, rel);
                let tmp = format!("{path}.flux-tmp");
                pool.with(|s| {
                    let _ = s.remove(&tmp);
                    s.write_new(&tmp, data, mode)?;
                    s.rename(&tmp, &path)
                })
                .map_err(CompatError::remote)
            }
        }
    }

    pub fn exists(&self, rel: &str) -> bool {
        match self {
            Store::Local { root } => join_local(root, rel).symlink_metadata().is_ok(),
            Store::Remote { pool, root } => pool.with(|s| s.lstat(&join_remote(root, rel))).is_ok(),
        }
    }

    pub fn is_dir(&self, rel: &str) -> bool {
        match self {
            Store::Local { root } => join_local(root, rel).is_dir(),
            Store::Remote { pool, root } => pool
                .with(|s| s.lstat(&join_remote(root, rel)))
                .is_ok_and(|a| a.is_dir()),
        }
    }

    /// Names in directory `rel` with whether each is a directory (symlinks are not).
    pub fn list(&self, rel: &str) -> io::Result<Vec<(String, bool)>> {
        match self {
            Store::Local { root } => {
                let mut out = Vec::new();
                for e in fs::read_dir(join_local(root, rel))? {
                    let e = e?;
                    let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
                    out.push((e.file_name().to_string_lossy().into_owned(), is_dir));
                }
                Ok(out)
            }
            Store::Remote { pool, root } => Ok(pool
                .with(|s| s.readdir(&join_remote(root, rel)))?
                .into_iter()
                .filter(|(n, _)| n != "." && n != "..")
                .map(|(n, a)| {
                    let d = a.is_dir();
                    (n, d)
                })
                .collect()),
        }
    }

    /// Create folder `rel` and its parents, private to the owner (mode 0700), so other
    /// users of the machine can't list the backups.
    pub fn mkdir_p(&self, rel: &str) -> Result<(), CompatError> {
        match self {
            Store::Local { root } => {
                use std::os::unix::fs::DirBuilderExt;
                let p = join_local(root, rel);
                fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(&p)
                    .map_err(|e| CompatError::io(&p, e))
            }
            Store::Remote { pool, root } => pool
                .with(|s| s.mkdir_p(&join_remote(root, rel), 0o700))
                .map_err(CompatError::remote),
        }
    }

    pub fn rename(&self, from: &str, to: &str) -> Result<(), CompatError> {
        match self {
            Store::Local { root } => {
                let (a, b) = (join_local(root, from), join_local(root, to));
                fs::rename(&a, &b).map_err(|e| CompatError::io(&a, e))
            }
            Store::Remote { pool, root } => pool
                .with(|s| s.rename(&join_remote(root, from), &join_remote(root, to)))
                .map_err(CompatError::remote),
        }
    }

    /// Remove `rel` and everything below it (read-only folders included).
    pub fn remove_tree(&self, rel: &str) -> Result<(), CompatError> {
        match self {
            Store::Local { root } => {
                let p = join_local(root, rel);
                match p.symlink_metadata() {
                    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
                    Ok(m) if !m.is_dir() => {
                        return fs::remove_file(&p).map_err(|e| CompatError::io(&p, e));
                    }
                    _ => {}
                }
                let _ = flux_fs::fsops::make_tree_writable(&p);
                fs::remove_dir_all(&p).map_err(|e| CompatError::io(&p, e))
            }
            Store::Remote { pool, root } => {
                let p = shell_quote(&join_remote(root, rel));
                pool.target()
                    .run(&format!("chmod -R u+w -- {p} 2>/dev/null; rm -rf -- {p}"))
                    .map(|_| ())
                    .map_err(CompatError::remote)
            }
        }
    }

    /// Point symlink `link` at `target` (relative to the link's folder), atomically where
    /// the filesystem allows; `None` removes it.
    pub fn set_link(&self, link: &str, target: Option<&str>) -> Result<(), CompatError> {
        match self {
            Store::Local { root } => {
                let path = join_local(root, link);
                let Some(target) = target else {
                    return match fs::remove_file(&path) {
                        Err(e) if e.kind() != io::ErrorKind::NotFound => {
                            Err(CompatError::io(&path, e))
                        }
                        _ => Ok(()),
                    };
                };
                let tmp = path.with_file_name(".flux-link.tmp");
                let _ = fs::remove_file(&tmp);
                std::os::unix::fs::symlink(target, &tmp).map_err(|e| CompatError::io(&tmp, e))?;
                fs::rename(&tmp, &path).map_err(|e| CompatError::io(&path, e))
            }
            Store::Remote { pool, root } => {
                let path = shell_quote(&join_remote(root, link));
                let cmd = match target {
                    Some(t) => format!("ln -sfn -- {} {path}", shell_quote(t)),
                    None => format!("rm -f -- {path}"),
                };
                pool.target()
                    .run(&cmd)
                    .map(|_| ())
                    .map_err(CompatError::remote)
            }
        }
    }

    /// Free bytes and the percentage of free inodes where the repository lives.
    pub fn free_space(&self) -> Option<(u64, f64)> {
        match self {
            Store::Local { root } => {
                let mut p = root.as_path();
                while !p.exists() {
                    p = p.parent()?;
                }
                flux_mirror::prune::free_space(p)
            }
            Store::Remote { pool, root } => pool
                .with(|s| s.statvfs(root))
                .ok()
                .map(|v| (v.free_bytes(), 100.0)),
        }
    }

    /// Take the repository lock without waiting.
    pub fn lock(&self) -> Result<StoreLock, CompatError> {
        match self {
            Store::Local { root } => {
                fs::create_dir_all(root).map_err(|e| CompatError::io(root, e))?;
                let path = root.join(".flux-lock");
                let file = OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .write(true)
                    .mode(0o644)
                    .open(&path)
                    .map_err(|e| CompatError::io(&path, e))?;
                match rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
                {
                    Ok(()) => Ok(StoreLock::Local(file)),
                    Err(rustix::io::Errno::WOULDBLOCK) => Err(CompatError::Locked(self.describe())),
                    Err(e) => Err(CompatError::io(&path, e.into())),
                }
            }
            Store::Remote { pool, root } => {
                flux_transport::lock::acquire(pool.clone(), &join_remote(root, ".flux-locks"), true)
                    .map_err(CompatError::remote)?
                    .map(StoreLock::Remote)
                    .ok_or_else(|| CompatError::Locked(self.describe()))
            }
        }
    }

    /// Up to `len` bytes of file `rel` at `offset`.
    pub fn read_range(&self, rel: &str, offset: u64, len: u64) -> io::Result<Vec<u8>> {
        match self {
            Store::Local { root } => {
                let f = File::open(join_local(root, rel))?;
                let mut buf = vec![0u8; len.min(64 << 20) as usize];
                let mut done = 0;
                while done < buf.len() {
                    match f.read_at(&mut buf[done..], offset + done as u64) {
                        Ok(0) => break,
                        Ok(n) => done += n,
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                        Err(e) => return Err(e),
                    }
                }
                buf.truncate(done);
                Ok(buf)
            }
            Store::Remote { pool, root } => {
                let mut out = Vec::with_capacity(len.min(1 << 20) as usize);
                pool.with(|s| {
                    s.read_range(&join_remote(root, rel), offset, len, |chunk| {
                        out.extend_from_slice(chunk);
                        Ok(())
                    })
                })?;
                Ok(out)
            }
        }
    }

    /// Copy file `rel` into `out` (a new empty file), counting bytes in `progress`.
    pub fn copy_to(
        &self,
        rel: &str,
        out: &File,
        copier: &Copier,
        progress: &Progress,
    ) -> io::Result<u64> {
        match self {
            Store::Local { root } => copier.copy_to(&join_local(root, rel), out, progress),
            Store::Remote { pool, root } => {
                let mut out = out;
                let n = pool.with(|s| {
                    s.read_range(&join_remote(root, rel), 0, u64::MAX, |chunk| {
                        if progress.is_cancelled() {
                            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
                        }
                        out.write_all(chunk)?;
                        progress.bytes_done.fetch_add(chunk.len() as u64, Relaxed);
                        Ok(())
                    })
                })?;
                // (Made durable by the restore, once at its end: not file by file.)
                Ok(n)
            }
        }
    }
}

fn join_local(root: &Path, rel: &str) -> PathBuf {
    if rel.is_empty() {
        root.to_owned()
    } else {
        root.join(rel)
    }
}

fn join_remote(root: &str, rel: &str) -> String {
    let root = root.trim_end_matches('/');
    let root = if root.is_empty() { "/" } else { root };
    match (rel.is_empty(), root == "/") {
        (true, _) => root.to_owned(),
        (false, true) => format!("/{rel}"),
        (false, false) => format!("{root}/{rel}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_store_basics() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::local(dir.path().join("repo"));
        s.mkdir_p("snapshots/a/data").unwrap();
        s.write("snapshots/a/meta.json", b"{}", 0o600).unwrap();
        assert_eq!(s.read("snapshots/a/meta.json").unwrap(), b"{}");
        assert!(s.is_dir("snapshots/a"));
        assert!(!s.is_dir("snapshots/a/meta.json"));
        s.set_link("latest", Some("snapshots/a")).unwrap();
        assert!(s.exists("latest"));
        let names = s.list("").unwrap();
        assert!(names.contains(&("snapshots".into(), true)));
        assert!(names.contains(&("latest".into(), false)));
        let lock = s.lock().unwrap();
        assert!(matches!(s.lock(), Err(CompatError::Locked(_))));
        drop(lock);
        s.lock().unwrap();
        // Read-only folders are removed too.
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            dir.path().join("repo/snapshots/a/data"),
            fs::Permissions::from_mode(0o500),
        )
        .unwrap();
        s.remove_tree("snapshots/a").unwrap();
        assert!(!s.exists("snapshots/a"));
        s.set_link("latest", None).unwrap();
        assert!(!s.exists("latest"));
        assert_eq!(join_remote("/", "x"), "/x");
        assert_eq!(join_remote("/srv/", "x"), "/srv/x");
        assert_eq!(join_remote("rel", ""), "rel");
    }
}
