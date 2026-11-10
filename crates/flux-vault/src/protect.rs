//! Repair data in a Vault: where the parity files ([`crate::parity`]) live, when they are
//! written, and how damage is repaired with them.
//!
//! Every file of a Vault can have one: `parity/<its path>` (`parity/data/ab/<pack>`,
//! `parity/index/<id>`, `parity/snapshots/<id>`, `parity/keys/<slot>`, `parity/config`).
//! Nothing else in the Vault changes, so a Flux that knows nothing of them reads and
//! writes the Vault as before; what it writes simply has no repair data until a newer
//! Flux adds it, and what it deletes leaves parity files that [`tidy`] removes.
//!
//! A repair is only ever of something that fails its own check (a pack whose bytes aren't
//! its name, a sealed object that doesn't open), and its result has to pass that check
//! before it replaces anything. Parity that doesn't match a file that *is* sound (a
//! snapshot renamed by an older Flux) is stale, and is written again, never used. A
//! snapshot record that was changed without its parity and *then* damaged comes back as
//! it was when its parity was written: its last change is lost, the snapshot isn't.

use std::collections::HashSet;
use std::io;

use crate::VaultError;
use crate::backend::{Backend, pack_path};
use crate::crypto::Crypto;
use crate::parity::{self, BLOCK, ParityError};
use crate::repo::VaultRepo;

/// Where parity files are kept, inside the Vault.
pub const DIR: &str = "parity";

/// The parity file of the repository file at `path`.
pub fn path_for(path: &str) -> String {
    format!("{DIR}/{path}")
}

/// Write the parity file for `path`, whose content is `bytes`. `pack`: durable later,
/// with the pack it belongs to (see [`Backend::write_pack`]).
pub fn write_for(
    backend: &dyn Backend,
    path: &str,
    bytes: &[u8],
    percent: u8,
    pack: bool,
) -> Result<(), VaultError> {
    let par = parity::encode(bytes, percent);
    let to = path_for(path);
    if pack {
        backend.write_pack(&to, &par)
    } else {
        backend.write(&to, &par)
    }
    .map_err(|e| VaultError::io(&to, e))
}

/// Remove the parity file of `path`, if it has one.
pub fn remove_for(backend: &dyn Backend, path: &str) {
    let _ = backend.remove(&path_for(path));
}

/// Read a file even if parts of it can't be read: those parts come back as zeros (their
/// blocks then fail their hashes, and count as lost). `None`: there is no such file.
pub fn read_lossy(backend: &dyn Backend, path: &str, size: u64) -> io::Result<Option<Vec<u8>>> {
    match backend.read(path) {
        Ok(b) => return Ok(Some(b)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {}
    }
    // A megabyte at a time, and block by block where a megabyte won't read.
    const STEP: u64 = 256 * BLOCK as u64;
    let mut out = vec![0u8; size as usize];
    let mut any = false;
    let mut at = 0u64;
    while at < size {
        let len = STEP.min(size - at);
        match backend.read_range(path, at, len) {
            Ok(b) if b.len() as u64 == len => {
                out[at as usize..(at + len) as usize].copy_from_slice(&b);
                any = true;
            }
            _ => {
                let mut sub = at;
                while sub < at + len {
                    let n = (BLOCK as u64).min(at + len - sub);
                    if let Ok(b) = backend.read_range(path, sub, n)
                        && b.len() as u64 == n
                    {
                        out[sub as usize..(sub + n) as usize].copy_from_slice(&b);
                        any = true;
                    }
                    sub += n;
                }
            }
        }
        at += len;
    }
    if !any && size > 0 && !backend.exists(path)? {
        return Ok(None);
    }
    Ok(Some(out))
}

/// What became of a damaged file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fix {
    /// It has no repair data.
    NoParity,
    /// Rebuilt and written back: so many 4 KiB blocks were damaged.
    Repaired { blocks: usize },
    /// It could be rebuilt (nothing was written: only asked whether).
    Repairable { blocks: usize },
    /// Its repair data can't put it right.
    Beyond(String),
}

/// Rebuild the file at `path` (`size`: as listed) from its parity file, if what comes out
/// passes `sound`. `write`: replace the file with it (and write its parity again if that
/// was damaged too); otherwise only say whether it could be done.
pub fn repair(
    backend: &dyn Backend,
    path: &str,
    size: u64,
    sound: &dyn Fn(&[u8]) -> bool,
    write: bool,
) -> Result<Fix, VaultError> {
    let par_path = path_for(path);
    let par_size = backend
        .list(par_path.rsplit_once('/').map_or(DIR, |(dir, _)| dir))
        .ok()
        .and_then(|l| {
            let name = par_path.rsplit('/').next().unwrap_or_default();
            l.into_iter().find(|(n, _)| n == name).map(|(_, s)| s)
        });
    let Some(par_size) = par_size else {
        return Ok(Fix::NoParity);
    };
    let Some(par) =
        read_lossy(backend, &par_path, par_size).map_err(|e| VaultError::io(&par_path, e))?
    else {
        return Ok(Fix::NoParity);
    };
    let damaged = read_lossy(backend, path, size)
        .map_err(|e| VaultError::io(path, e))?
        .unwrap_or_default();
    let restored = match parity::restore(&damaged, &par) {
        Ok(r) => r,
        Err(e) => return Ok(Fix::Beyond(e.to_string())),
    };
    if !sound(&restored.bytes) {
        // Sound parity for other contents: stale (the file was rewritten since), and the
        // file is damaged on top of that.
        return Ok(Fix::Beyond(
            "its repair data was made for an earlier version of it".into(),
        ));
    }
    // At least one block: a file that failed its check was damaged somewhere, even if
    // every block that could be read matched (a read that failed, then didn't).
    let blocks = restored.bad_data.max(1);
    if !write {
        return Ok(Fix::Repairable { blocks });
    }
    backend
        .write(path, &restored.bytes)
        .map_err(|e| VaultError::io(path, e))?;
    if restored.bad_parity > 0 || restored.bad_head {
        let percent = parity::head(&par).map_or(5, |h| h.percent);
        write_for(backend, path, &restored.bytes, percent, false)?;
    }
    Ok(Fix::Repaired { blocks })
}

/// Is the pack's content what its name says?
pub fn pack_is_sound(name: &str, bytes: &[u8]) -> bool {
    blake3::hash(bytes).to_hex().as_str() == name
}

/// The state of a sound file's parity file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParityState {
    Sound,
    /// Parts of it are damaged; it should be written again.
    Damaged,
    /// It was made for other contents (the file was rewritten since).
    Stale,
}

/// Check the parity file `par` of a file known to be sound (`bytes`).
pub fn parity_state(bytes: &[u8], par: &[u8]) -> ParityState {
    match parity::head(par) {
        Err(_) => ParityState::Damaged,
        Ok(h) if h.file_hash != *blake3::hash(bytes).as_bytes() => ParityState::Stale,
        Ok(_) => match parity::restore(bytes, par) {
            Ok(r) if r.bad_parity == 0 && !r.bad_head => ParityState::Sound,
            _ => ParityState::Damaged,
        },
    }
}

/// A sealed object (index file, snapshot record) is sound if it opens.
pub fn opens(crypto: &Crypto, bytes: &[u8]) -> bool {
    crypto.open(bytes).is_ok()
}

fn list(backend: &dyn Backend, dir: &str) -> Result<Vec<(String, u64)>, VaultError> {
    backend.list(dir).map_err(|e| VaultError::io(dir, e))
}

/// Repair `config` and the key slots, which a Vault can't be opened without. Needs no
/// password. Returns what was repaired, in words.
///
/// Key slots never change once written, so one that differs from its parity is damaged.
/// `config` changes once, when the format is upgraded: if it still reads and differs from
/// its parity's version only in the format, the parity is stale, not the config.
pub fn repair_unopened(backend: &dyn Backend) -> Result<Vec<String>, VaultError> {
    let mut done = Vec::new();
    let differs = |bytes: &[u8], par: &[u8]| {
        parity::head(par).is_ok_and(|h| h.file_hash != *blake3::hash(bytes).as_bytes())
    };
    let mut objects: Vec<(String, u64)> = list(backend, "keys")?
        .into_iter()
        .map(|(n, s)| (format!("keys/{n}"), s))
        .collect();
    objects.push(("config".into(), 0));
    for (path, size) in objects {
        let Ok(par) = backend.read(&path_for(&path)) else {
            continue;
        };
        let size = if path == "config" {
            parity::head(&par).map_or(0, |h| h.geo.len)
        } else {
            size
        };
        let now = read_lossy(backend, &path, size)
            .map_err(|e| VaultError::io(&path, e))?
            .unwrap_or_default();
        if !differs(&now, &par) {
            continue;
        }
        let Ok(restored) = parity::restore(&now, &par) else {
            continue;
        };
        if path == "config" {
            let parsed = |b: &[u8]| serde_json::from_slice::<crate::format::Config>(b).ok();
            let Some(was) = parsed(&restored.bytes) else {
                continue;
            };
            // Still readable and the same Vault, at the same or a newer format: upgraded
            // since its parity was written.
            if let Some(mut is) = parsed(&now)
                && is.format >= was.format
            {
                is.format = was.format;
                if serde_json::to_vec(&is).ok() == serde_json::to_vec(&was).ok() {
                    continue;
                }
            }
        }
        backend
            .write(&path, &restored.bytes)
            .map_err(|e| VaultError::io(&path, e))?;
        done.push(format!(
            "{path}: damaged, repaired ({} blocks)",
            restored.bad_data.max(1)
        ));
    }
    Ok(done)
}

/// How much of a Vault has repair data.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Coverage {
    /// Pack files, and how many of them have a parity file.
    pub packs: u64,
    pub protected: u64,
    /// Bytes of pack files, and of their parity files.
    pub bytes: u64,
    pub parity_bytes: u64,
}

/// How much of the stored data has repair data (two listings; nothing is read).
pub fn coverage(backend: &dyn Backend) -> Result<Coverage, VaultError> {
    let packs = list(backend, "data")?;
    let par = list(backend, &path_for("data"))?;
    let have: HashSet<&str> = par.iter().map(|(n, _)| n.as_str()).collect();
    Ok(Coverage {
        packs: packs.len() as u64,
        protected: packs
            .iter()
            .filter(|(n, _)| have.contains(n.as_str()))
            .count() as u64,
        bytes: packs.iter().map(|(_, s)| s).sum(),
        parity_bytes: par.iter().map(|(_, s)| s).sum(),
    })
}

/// What [`tidy`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tidied {
    /// Parity files written for small objects that had none (or a stale one).
    pub added: u64,
    /// Parity files removed, whose file is gone.
    pub removed: u64,
}

/// Keep the parity files in step with the Vault: remove those whose file is gone (packs
/// and index files deleted by pruning, snapshots deleted, keys removed), and, when this
/// Vault is written with repair data (`repo.parity`), give `config`, the key slots, index
/// files and snapshot records one where it's missing. Packs without one get theirs when
/// all data is verified (it has to be read for that; see [`crate::check`]).
pub fn tidy(repo: &VaultRepo) -> Result<Tidied, VaultError> {
    let backend = repo.backend();
    let mut out = Tidied::default();
    for dir in ["data", "index", "snapshots", "keys"] {
        let par = list(backend, &path_for(dir))?;
        if par.is_empty() && repo.parity == 0 {
            continue;
        }
        let files = list(backend, dir)?;
        let have: HashSet<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        let object = |name: &str| {
            if dir == "data" {
                pack_path(name)
            } else {
                format!("{dir}/{name}")
            }
        };
        for (name, _) in &par {
            if !have.contains(name.as_str()) {
                remove_for(backend, &object(name));
                out.removed += 1;
            }
        }
        if repo.parity == 0 || dir == "data" {
            continue;
        }
        let protected: HashSet<&str> = par.iter().map(|(n, _)| n.as_str()).collect();
        for (name, _) in files
            .iter()
            .filter(|(n, _)| !protected.contains(n.as_str()))
        {
            let path = object(name);
            let Ok(bytes) = backend.read(&path) else {
                continue;
            };
            // Never parity for something already damaged: it would vouch for the damage.
            if dir != "keys" && !opens(&repo.crypto, &bytes) {
                continue;
            }
            write_for(backend, &path, &bytes, repo.parity, false)?;
            out.added += 1;
        }
    }
    // `config`: parity where it's missing, or stale (the format was upgraded since).
    if repo.parity > 0
        && let Ok(bytes) = backend.read("config")
        && serde_json::from_slice::<crate::format::Config>(&bytes).is_ok()
        && backend
            .read(&path_for("config"))
            .map_or(true, |par| parity_state(&bytes, &par) != ParityState::Sound)
    {
        write_for(backend, "config", &bytes, repo.parity, false)?;
        out.added += 1;
    }
    Ok(out)
}

/// Words for a [`ParityError`] or a [`Fix`] in a check's report.
pub fn words(fix: &Fix) -> String {
    match fix {
        Fix::NoParity => "it has no repair data".into(),
        Fix::Repaired { blocks } => format!("damaged, repaired ({blocks} blocks)"),
        Fix::Repairable { blocks } => {
            format!("damaged; its repair data can rebuild it ({blocks} blocks)")
        }
        Fix::Beyond(why) => why.clone(),
    }
}

impl From<ParityError> for VaultError {
    fn from(e: ParityError) -> Self {
        VaultError::Corrupt(e.to_string())
    }
}
