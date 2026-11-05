//! An open Vault repository: configuration, keys, index, snapshots and trees.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use jiff::Timestamp;
use rayon::prelude::*;
use serde::Serialize;

use flux_fs::entry::Entry;
use flux_fs::snapshot::SnapshotMeta;

use crate::VaultError;
use crate::backend::{Backend, LockGuard};
use crate::crypto::{BlobId, Crypto, KdfParams, KeySlot, MasterKey, hex_id, parse_id, random};
use crate::format::{ChunkerParams, Config, FORMAT, PlainKeys, SnapshotRecord, Tree};
use crate::pack::{Index, read_blob};

/// Which snapshots belong to "this" profile in a shared repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
    pub profile: String,
    pub host: String,
    pub user: String,
}

impl Owner {
    pub fn matches(&self, m: &SnapshotMeta) -> bool {
        m.profile == self.profile && m.host == self.host && m.user == self.user
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct KeyInfo {
    pub id: String,
    pub label: String,
    pub created: Timestamp,
    /// This slot opened the repository in this session.
    pub current: bool,
}

pub struct VaultRepo {
    backend: Box<dyn Backend>,
    pub config: Config,
    pub crypto: Crypto,
    /// Key slot that opened the repository (encrypted repos).
    pub slot: Option<String>,
    index: Mutex<Option<Arc<Index>>>,
    /// Key derivation cost for new slots.
    pub kdf: KdfParams,
}

impl std::fmt::Debug for VaultRepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultRepo")
            .field("at", &self.backend.describe())
            .field("id", &self.config.id)
            .finish()
    }
}

impl VaultRepo {
    pub fn exists(backend: &dyn Backend) -> Result<bool, VaultError> {
        backend
            .exists("config")
            .map_err(|e| VaultError::io("config", e))
    }

    /// Create a repository. `password: None` creates an unencrypted one.
    pub fn init(
        backend: Box<dyn Backend>,
        password: Option<&str>,
        kdf: KdfParams,
    ) -> Result<Self, VaultError> {
        if Self::exists(backend.as_ref())? {
            return Err(VaultError::AlreadyExists(backend.describe()));
        }
        let id = hex::encode(random::<16>()?);
        let (crypto, plain_keys, slot) = match password {
            Some(pw) => {
                let master = MasterKey::generate()?;
                let slot = KeySlot::wrap(&master, pw, &id, kdf, "initial")?;
                (Crypto::Encrypted(master), None, Some(slot))
            }
            None => {
                let id_key: [u8; 32] = random()?;
                let seed = u64::from_le_bytes(random()?);
                (
                    Crypto::Plain {
                        id_key,
                        chunk_seed: seed,
                    },
                    Some(PlainKeys {
                        id_key: B64.encode(id_key),
                        chunk_seed: seed,
                    }),
                    None,
                )
            }
        };
        let config = Config {
            format: FORMAT,
            id,
            encrypted: password.is_some(),
            created: Timestamp::now(),
            chunker: ChunkerParams::DEFAULT,
            plain_keys,
        };
        let mut slot_id = None;
        if let Some(slot) = slot {
            let sid = hex::encode(random::<8>()?);
            let json =
                serde_json::to_vec_pretty(&slot).map_err(|e| VaultError::Corrupt(e.to_string()))?;
            backend
                .write(&format!("keys/{sid}"), &json)
                .map_err(|e| VaultError::io("keys", e))?;
            slot_id = Some(sid);
        }
        // `config` last: a repository exists only once it is complete.
        let json =
            serde_json::to_vec_pretty(&config).map_err(|e| VaultError::Corrupt(e.to_string()))?;
        backend
            .write("config", &json)
            .map_err(|e| VaultError::io("config", e))?;
        Ok(Self {
            backend,
            config,
            crypto,
            slot: slot_id,
            index: Mutex::new(None),
            kdf,
        })
    }

    /// Open a repository; `password` is required if it is encrypted.
    pub fn open(backend: Box<dyn Backend>, password: Option<&str>) -> Result<Self, VaultError> {
        let raw = match backend.read("config") {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(VaultError::NoRepository(backend.describe()));
            }
            Err(e) => return Err(VaultError::io("config", e)),
        };
        let config: Config = serde_json::from_slice(&raw)
            .map_err(|e| VaultError::Corrupt(format!("config: {e}")))?;
        if config.format > FORMAT {
            return Err(VaultError::Corrupt(format!(
                "repository format {} is newer than this Flux supports ({FORMAT})",
                config.format
            )));
        }
        let (crypto, slot) = if config.encrypted {
            let pw = password.ok_or(VaultError::PasswordRequired)?;
            let (master, sid) = Self::try_slots(backend.as_ref(), &config.id, pw)?;
            (Crypto::Encrypted(master), Some(sid))
        } else {
            let pk = config
                .plain_keys
                .as_ref()
                .ok_or_else(|| VaultError::Corrupt("config lacks plain keys".into()))?;
            let id_key: [u8; 32] = B64
                .decode(&pk.id_key)
                .ok()
                .and_then(|k| k.try_into().ok())
                .ok_or_else(|| VaultError::Corrupt("bad id key in config".into()))?;
            (
                Crypto::Plain {
                    id_key,
                    chunk_seed: pk.chunk_seed,
                },
                None,
            )
        };
        Ok(Self {
            backend,
            config,
            crypto,
            slot,
            index: Mutex::new(None),
            kdf: KdfParams::DEFAULT,
        })
    }

    fn try_slots(
        backend: &dyn Backend,
        repo_id: &str,
        password: &str,
    ) -> Result<(MasterKey, String), VaultError> {
        let slots = backend
            .list("keys")
            .map_err(|e| VaultError::io("keys", e))?;
        if slots.is_empty() {
            return Err(VaultError::Corrupt(
                "encrypted repository without key slots".into(),
            ));
        }
        for (sid, _) in slots {
            let raw = backend
                .read(&format!("keys/{sid}"))
                .map_err(|e| VaultError::io("keys", e))?;
            let Ok(slot) = serde_json::from_slice::<KeySlot>(&raw) else {
                tracing::warn!("ignoring unreadable key slot {sid}");
                continue;
            };
            if let Ok(master) = slot.unwrap(password, repo_id) {
                return Ok((master, sid));
            }
        }
        Err(VaultError::WrongPassword)
    }

    pub fn backend(&self) -> &dyn Backend {
        self.backend.as_ref()
    }

    pub fn lock(&self, exclusive: bool) -> Result<LockGuard, VaultError> {
        self.backend
            .lock(exclusive)
            .map_err(|e| VaultError::io("lock", e))?
            .ok_or_else(|| VaultError::Locked(self.backend.describe()))
    }

    /// The blob index, loaded on first use.
    pub fn index(&self) -> Result<Arc<Index>, VaultError> {
        let mut g = self.index.lock().expect("index lock");
        if let Some(i) = g.as_ref() {
            return Ok(i.clone());
        }
        let i = Arc::new(Index::load(self.backend(), &self.crypto)?);
        *g = Some(i.clone());
        Ok(i)
    }

    /// Forget the cached index (after writing index files).
    pub fn invalidate_index(&self) {
        *self.index.lock().expect("index lock") = None;
    }

    // ---- snapshots ----

    /// All snapshot records, oldest first.
    pub fn records(&self) -> Result<Vec<SnapshotRecord>, VaultError> {
        let names = self
            .backend
            .list("snapshots")
            .map_err(|e| VaultError::io("snapshots", e))?;
        let mut v: Vec<SnapshotRecord> = names
            .par_iter()
            .filter_map(|(n, _)| match self.read_record(n) {
                Ok(r) => Some(r),
                Err(e) => {
                    tracing::warn!("skipping snapshot {n}: {e}");
                    None
                }
            })
            .collect();
        v.sort_by(|a, b| {
            a.meta
                .started
                .cmp(&b.meta.started)
                .then_with(|| a.meta.id.cmp(&b.meta.id))
        });
        Ok(v)
    }

    /// Snapshots of one owner, oldest first.
    pub fn snapshots(&self, owner: &Owner) -> Result<Vec<SnapshotMeta>, VaultError> {
        Ok(self
            .records()?
            .into_iter()
            .map(|r| r.meta)
            .filter(|m| owner.matches(m))
            .collect())
    }

    pub fn read_record(&self, id: &str) -> Result<SnapshotRecord, VaultError> {
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(VaultError::SnapshotNotFound(id.into()));
        }
        let path = format!("snapshots/{id}");
        let raw = match self.backend.read(&path) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(VaultError::SnapshotNotFound(id.into()));
            }
            Err(e) => return Err(VaultError::io(&path, e)),
        };
        let mut r: SnapshotRecord = serde_json::from_slice(&self.crypto.open(&raw)?)
            .map_err(|e| VaultError::Corrupt(format!("{path}: {e}")))?;
        r.meta.id = id.to_owned();
        Ok(r)
    }

    pub fn write_record(&self, r: &SnapshotRecord) -> Result<(), VaultError> {
        let path = format!("snapshots/{}", r.meta.id);
        let json = serde_json::to_vec(r).map_err(|e| VaultError::Corrupt(e.to_string()))?;
        self.backend
            .write(&path, &self.crypto.seal(&json)?)
            .map_err(|e| VaultError::io(&path, e))
    }

    pub fn delete_record(&self, id: &str) -> Result<(), VaultError> {
        self.read_record(id)?;
        let path = format!("snapshots/{id}");
        self.backend
            .remove(&path)
            .map_err(|e| VaultError::io(&path, e))
    }

    /// Find one of `owner`'s snapshots by `latest`, id, unique id prefix, or name.
    pub fn find(&self, owner: &Owner, spec: &str) -> Result<SnapshotMeta, VaultError> {
        let mut all = self.snapshots(owner)?;
        if spec == "latest" {
            return all
                .pop()
                .ok_or_else(|| VaultError::SnapshotNotFound(spec.into()));
        }
        if let Some(i) = all.iter().position(|s| s.id == spec) {
            return Ok(all.swap_remove(i));
        }
        let m: Vec<_> = all
            .into_iter()
            .filter(|s| s.id.starts_with(spec) || s.name.as_deref() == Some(spec))
            .collect();
        match m.len() {
            1 => Ok(m.into_iter().next().expect("one")),
            0 => Err(VaultError::SnapshotNotFound(spec.into())),
            n => Err(VaultError::Ambiguous(spec.into(), n)),
        }
    }

    // ---- trees ----

    pub fn load_tree(&self, id: &BlobId) -> Result<Tree, VaultError> {
        let index = self.index()?;
        let loc = index
            .get(id)
            .ok_or_else(|| VaultError::Corrupt(format!("tree {} is missing", hex_id(id))))?;
        let raw = read_blob(self.backend(), &self.crypto, loc)?;
        postcard::from_bytes(&raw)
            .map_err(|e| VaultError::Corrupt(format!("tree {}: {e}", hex_id(id))))
    }

    /// Every entry of a snapshot with its content blobs, sorted by path.
    pub fn flatten(
        &self,
        record: &SnapshotRecord,
    ) -> Result<Vec<(Entry, Vec<BlobId>)>, VaultError> {
        let root =
            parse_id(&record.tree).ok_or_else(|| VaultError::Corrupt("bad root tree id".into()))?;
        let mut out = Vec::new();
        let mut stack: Vec<(Vec<u8>, BlobId)> = vec![(Vec::new(), root)];
        while let Some((prefix, tid)) = stack.pop() {
            for node in self.load_tree(&tid)?.nodes {
                let mut path = prefix.clone();
                if !path.is_empty() {
                    path.push(b'/');
                }
                path.extend_from_slice(&node.name);
                if let Some(sub) = node.subtree {
                    stack.push((path.clone(), sub));
                }
                let content = node.content.clone();
                out.push((node.to_entry(path), content));
            }
        }
        out.sort_unstable_by(|a, b| a.0.path.cmp(&b.0.path));
        Ok(out)
    }

    // ---- keys ----

    pub fn keys(&self) -> Result<Vec<KeyInfo>, VaultError> {
        let mut out = Vec::new();
        for (sid, _) in self
            .backend
            .list("keys")
            .map_err(|e| VaultError::io("keys", e))?
        {
            let raw = self
                .backend
                .read(&format!("keys/{sid}"))
                .map_err(|e| VaultError::io("keys", e))?;
            if let Ok(slot) = serde_json::from_slice::<KeySlot>(&raw) {
                out.push(KeyInfo {
                    current: self.slot.as_deref() == Some(sid.as_str()),
                    id: sid,
                    label: slot.label,
                    created: slot.created,
                });
            }
        }
        out.sort_by_key(|k| k.created);
        Ok(out)
    }

    /// Add a password. Returns the new slot id.
    pub fn add_key(&self, password: &str, label: &str) -> Result<String, VaultError> {
        let Crypto::Encrypted(master) = &self.crypto else {
            return Err(VaultError::NotEncrypted);
        };
        let slot = KeySlot::wrap(master, password, &self.config.id, self.kdf, label)?;
        let sid = hex::encode(random::<8>()?);
        let json =
            serde_json::to_vec_pretty(&slot).map_err(|e| VaultError::Corrupt(e.to_string()))?;
        self.backend
            .write(&format!("keys/{sid}"), &json)
            .map_err(|e| VaultError::io("keys", e))?;
        Ok(sid)
    }

    /// Remove a password. The last remaining key can't be removed.
    pub fn remove_key(&self, slot: &str) -> Result<(), VaultError> {
        let keys = self.keys()?;
        if !keys.iter().any(|k| k.id == slot) {
            return Err(VaultError::NoSuchKey(slot.into()));
        }
        if keys.len() == 1 {
            return Err(VaultError::LastKey);
        }
        self.backend
            .remove(&format!("keys/{slot}"))
            .map_err(|e| VaultError::io("keys", e))
    }
}

/// Blob ids of a snapshot's content, by file path (for restore).
pub type ContentMap = HashMap<Vec<u8>, Vec<BlobId>>;
