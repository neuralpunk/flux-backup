//! Replicas of a Vault: another place holding exactly the same repository (same id, keys,
//! packs and snapshots), kept up to date by copying files, never by reading or decrypting
//! them. Packs are named by their hash and never change, so only new ones are copied.
//!
//! Copied in the order a backup writes (packs, index, snapshot records, keys, config last,
//! so a replica exists only once complete), and removed in the order prune does (snapshot
//! records, index, packs): an interrupted replication leaves a replica that is still
//! readable, and the next one finishes it. The source is read under its shared lock (no
//! prune meanwhile); the replica is written under its exclusive lock.

use std::sync::atomic::Ordering::Relaxed;

use flux_core::progress::Progress;

use crate::VaultError;
use crate::backend::{Backend, LockGuard, pack_path};

/// What a replication did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VaultReplica {
    pub copied_files: u64,
    pub copied_bytes: u64,
    pub removed_files: u64,
    /// Snapshots the replica holds now.
    pub snapshots: u64,
}

fn locked(b: &dyn Backend, exclusive: bool) -> Result<LockGuard, VaultError> {
    b.lock(exclusive)
        .map_err(|e| VaultError::io("lock", e))?
        .ok_or_else(|| VaultError::Locked {
            place: b.describe(),
            held_by: b
                .lock_holders()
                .unwrap_or_default()
                .iter()
                .map(|l| l.describe())
                .collect(),
        })
}

fn names(b: &dyn Backend, dir: &str) -> Result<Vec<(String, u64)>, VaultError> {
    b.list(dir).map_err(|e| VaultError::io(dir, e))
}

/// Make `replica` hold the same Vault as `source`.
pub fn replicate(
    source: &dyn Backend,
    replica: &dyn Backend,
    progress: &Progress,
) -> Result<VaultReplica, VaultError> {
    let config = match source.read("config") {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(VaultError::NoRepository(source.describe()));
        }
        Err(e) => return Err(VaultError::io("config", e)),
    };
    // Never overwrite another Vault: a replica is empty, or a copy of this one.
    if let Ok(there) = replica.read("config")
        && there != config
    {
        let id = |raw: &[u8]| {
            serde_json::from_slice::<serde_json::Value>(raw)
                .ok()
                .and_then(|v| v["id"].as_str().map(str::to_owned))
        };
        if id(&there) != id(&config) {
            return Err(VaultError::Corrupt(format!(
                "another Vault is already at {}; a copy needs an empty place, or a copy of this Vault",
                replica.describe()
            )));
        }
    }
    let _src = locked(source, false)?;
    let _rep = locked(replica, true)?;
    let mut out = VaultReplica::default();
    let copy = |path: &str, pack: bool, out: &mut VaultReplica| -> Result<(), VaultError> {
        if progress.is_cancelled() {
            return Err(VaultError::Cancelled);
        }
        let data = source.read(path).map_err(|e| VaultError::io(path, e))?;
        if pack {
            replica.write_pack(path, &data)
        } else {
            replica.write(path, &data)
        }
        .map_err(|e| VaultError::io(path, e))?;
        out.copied_files += 1;
        out.copied_bytes += data.len() as u64;
        progress.bytes_done.fetch_add(data.len() as u64, Relaxed);
        Ok(())
    };

    // Which index files and snapshots to copy is settled before the packs are listed: a
    // backup that finishes on the source meanwhile (it only needs the shared lock) would
    // otherwise get its index and snapshot copied without its packs, and the replica's
    // newest snapshot couldn't be restored until the next run.
    let have_index = names(source, "index")?;
    let have_snapshots = names(source, "snapshots")?;
    let have_packs = names(source, "data")?;
    let there_packs = names(replica, "data")?;
    for (name, size) in &have_packs {
        if !there_packs.iter().any(|(n, s)| n == name && s == size) {
            copy(&pack_path(name), true, &mut out)?;
        }
    }
    // Their repair data (a parity file is its pack's, by name: it never changes either).
    let par_data = crate::protect::path_for("data");
    let have_par = names(source, &par_data)?;
    let there_par = names(replica, &par_data)?;
    for (name, size) in &have_par {
        if have_packs.iter().any(|(n, _)| n == name)
            && !there_par.iter().any(|(n, s)| n == name && s == size)
        {
            copy(&crate::protect::path_for(&pack_path(name)), true, &mut out)?;
        }
    }
    replica
        .flush_packs()
        .map_err(|e| VaultError::io("data", e))?;
    // Index files and keys never change once written (new ones get new names); snapshot
    // records change when named, pinned, noted or tagged, so they are compared.
    let sync = |dir: &str,
                have: Option<&[(String, u64)]>,
                compare: bool,
                out: &mut VaultReplica|
     -> Result<(), VaultError> {
        let there = names(replica, dir)?;
        let listed;
        let have = match have {
            Some(h) => h,
            None => {
                listed = names(source, dir)?;
                &listed
            }
        };
        for (name, _) in have {
            let path = format!("{dir}/{name}");
            let differs = if !there.iter().any(|(n, _)| n == name) {
                true
            } else if compare {
                source.read(&path).ok() != replica.read(&path).ok()
            } else {
                false
            };
            if differs {
                copy(&path, false, out)?;
            }
        }
        Ok(())
    };
    sync("index", Some(&have_index), false, &mut out)?;
    sync("snapshots", Some(&have_snapshots), true, &mut out)?;
    sync("keys", None, true, &mut out)?;
    sync(crate::repo::PROFILES_DIR, None, true, &mut out)?;
    // The repair data of the small files (a snapshot record's changes with the record).
    for dir in ["index", "snapshots", "keys"] {
        sync(
            &crate::protect::path_for(dir),
            None,
            dir != "index",
            &mut out,
        )?;
    }
    let par_config = crate::protect::path_for("config");
    if let Ok(par) = source.read(&par_config)
        && replica.read(&par_config).ok().as_deref() != Some(par.as_slice())
    {
        replica
            .write(&par_config, &par)
            .map_err(|e| VaultError::io(&par_config, e))?;
        out.copied_files += 1;
    }
    if replica.read("config").ok().as_deref() != Some(config.as_slice()) {
        replica
            .write("config", &config)
            .map_err(|e| VaultError::io("config", e))?;
        out.copied_files += 1;
    }

    // What the source no longer has. A folder that isn't there lists as empty, so a
    // source that went away meanwhile (its drive pulled, its folder moved) would look
    // like a Vault with nothing in it and the replica would be emptied: the source must
    // still be there, and still be this Vault.
    match source.read("config") {
        Ok(now) if now == config => {}
        Ok(_) => {
            return Err(VaultError::Corrupt(format!(
                "the Vault at {} changed while it was being copied; nothing was removed from the copy",
                source.describe()
            )));
        }
        Err(e) => return Err(VaultError::io("config", e)),
    }
    let par = crate::protect::path_for;
    for dir in [
        "snapshots".to_owned(),
        "index".into(),
        "data".into(),
        "keys".into(),
        par("snapshots"),
        par("index"),
        par("data"),
        par("keys"),
    ] {
        let dir = dir.as_str();
        let have = names(source, dir)?;
        for (name, _) in names(replica, dir)? {
            if !have.iter().any(|(n, _)| *n == name) {
                let path = if dir == "data" {
                    pack_path(&name)
                } else if dir == par_data {
                    par(&pack_path(&name))
                } else {
                    format!("{dir}/{name}")
                };
                replica
                    .remove(&path)
                    .map_err(|e| VaultError::io(&path, e))?;
                out.removed_files += 1;
            }
        }
    }
    out.snapshots = names(replica, "snapshots")?.len() as u64;
    Ok(out)
}
