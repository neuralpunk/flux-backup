//! An open Vault repository: configuration, keys, index, snapshots and trees.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use jiff::Timestamp;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use flux_fs::entry::Entry;
use flux_fs::snapshot::SnapshotMeta;

use crate::VaultError;
use crate::backend::{Backend, LockGuard};
use crate::crypto::{
    BlobId, Crypto, KdfParams, KeySlot, MasterKey, UnlockSecret, hex_id, parse_id, random,
    slot_input,
};
use crate::format::{
    ChunkerParams, Config, Content, FORMAT, OLDEST_FORMAT, PlainKeys, SnapshotRecord, Tree,
};
use crate::pack::{Index, read_blob};
use flux_core::sync::Locked;

/// Where the settings of the profiles using a Vault are kept (encrypted, one file each).
pub const PROFILES_DIR: &str = "profiles";

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct SavedProfile {
    profile: String,
    host: String,
    user: String,
    text: String,
}

/// A file name for an owner's saved profile.
fn profile_key(owner: &Owner) -> String {
    let key = format!("{}\n{}\n{}", owner.host, owner.user, owner.profile);
    blake3::hash(key.as_bytes()).to_hex()[..32].to_owned()
}

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
    /// How hard its password is hashed.
    pub hashing: flux_core::profile::PasswordHashing,
    /// It opens only with the unlock file too.
    pub unlock_file: bool,
}

pub struct VaultRepo {
    backend: Box<dyn Backend>,
    pub config: Config,
    pub crypto: Crypto,
    /// Key slot that opened the repository (encrypted repos).
    pub slot: Option<String>,
    index: Mutex<Option<Arc<Index>>>,
    /// Packs fetched whole from a server (see [`VaultRepo::read_blob`]).
    packs: PackCache,
    /// Key derivation cost for new slots.
    pub kdf: KdfParams,
    /// The unlock file's secret this repository was opened with (new slots need it too).
    unlock: Option<UnlockSecret>,
    /// Pad new packs to hide their exact sizes (see [`crate::pack::padme`]).
    pub pad_packs: bool,
    /// Write repair data ([`crate::protect`]) with what's stored from now on: this share
    /// of each file, in percent. 0: none.
    pub parity: u8,
}

/// A pack held in memory, or being fetched (others wanting it wait for the one fetch).
type HeldPack = Arc<std::sync::OnceLock<Result<Arc<Vec<u8>>, String>>>;

/// The packs of a Vault on a server that were fetched whole, most recently wanted last.
#[derive(Default)]
struct PackCache {
    state: Mutex<PackCacheState>,
}

#[derive(Default)]
struct PackCacheState {
    /// Packs a blob was read from, alone, lately: the next blob wanted from one of
    /// them brings the whole pack.
    seen: std::collections::VecDeque<BlobId>,
    held: std::collections::VecDeque<(BlobId, HeldPack)>,
}

/// Packs kept in memory (about 16 MiB each).
const PACKS_HELD: usize = 8;
/// Packs remembered as read from once.
const PACKS_SEEN: usize = 4096;

impl PackCache {
    /// The place for `pack`'s contents, if it should be fetched whole (it was read from
    /// before): to be filled by the caller, or already filled. `None`: this is the
    /// first blob wanted from it; read that alone.
    fn wanted(&self, pack: &BlobId) -> Option<HeldPack> {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = st.held.iter().position(|(p, _)| p == pack) {
            let hit = st.held.remove(i).expect("found");
            st.held.push_back(hit.clone());
            return Some(hit.1);
        }
        if !st.seen.contains(pack) {
            if st.seen.len() >= PACKS_SEEN {
                st.seen.pop_front();
            }
            st.seen.push_back(*pack);
            return None;
        }
        if st.held.len() >= PACKS_HELD {
            st.held.pop_front();
        }
        let cell = HeldPack::default();
        st.held.push_back((*pack, cell.clone()));
        Some(cell)
    }

    fn forget(&self, pack: &BlobId) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.held.retain(|(p, _)| p != pack);
    }
}

impl std::fmt::Debug for VaultRepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultRepo")
            .field("at", &self.backend.describe())
            .field("id", &self.config.id)
            .finish()
    }
}

/// `config`, checked: a format this build reads.
fn parse_config(raw: &[u8]) -> Result<Config, VaultError> {
    let config: Config =
        serde_json::from_slice(raw).map_err(|e| VaultError::Corrupt(format!("config: {e}")))?;
    if config.format > FORMAT {
        return Err(VaultError::Corrupt(format!(
            "repository format {} is newer than this Flux supports ({FORMAT}); update Flux Backup",
            config.format
        )));
    }
    if config.format < OLDEST_FORMAT {
        return Err(VaultError::Corrupt(format!(
            "repository format {} is unknown",
            config.format
        )));
    }
    Ok(config)
}

/// The result of [`VaultRepo::upgrade_format`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Upgrade {
    pub from: u32,
    pub to: u32,
}

/// Snapshot records that couldn't be read: file name and why.
pub type Unreadable = Vec<(String, VaultError)>;

impl VaultRepo {
    /// Move a repository to the newest format ([`FORMAT`]). Only `config` changes: the
    /// snapshots already there stay as they are and stay readable, and new ones use the
    /// new format. Flux before 1.0 can't open it afterwards. Needs no password, but the
    /// exclusive lock (no job may be using the repository).
    pub fn upgrade_format(backend: &dyn Backend) -> Result<Upgrade, VaultError> {
        let raw = match backend.read("config") {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(VaultError::NoRepository(backend.describe()));
            }
            Err(e) => return Err(VaultError::io("config", e)),
        };
        let mut config = parse_config(&raw)?;
        let from = config.format;
        if from == FORMAT {
            return Ok(Upgrade { from, to: from });
        }
        let _lock = backend
            .lock(true)
            .map_err(|e| VaultError::io("lock", e))?
            .ok_or_else(|| VaultError::Locked {
                place: backend.describe(),
                held_by: backend
                    .lock_holders()
                    .unwrap_or_default()
                    .iter()
                    .map(|l| l.describe())
                    .collect(),
            })?;
        config.format = FORMAT;
        let json =
            serde_json::to_vec_pretty(&config).map_err(|e| VaultError::Corrupt(e.to_string()))?;
        backend
            .write("config", &json)
            .map_err(|e| VaultError::io("config", e))?;
        // Its repair data, if it has any, is for the config as it was.
        if let Ok(par) = backend.read(&crate::protect::path_for("config")) {
            let percent = crate::parity::head(&par).map_or(5, |h| h.percent);
            crate::protect::write_for(backend, "config", &json, percent, false)?;
        }
        Ok(Upgrade { from, to: FORMAT })
    }

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
        Self::init_with(backend, password, kdf, None)
    }

    /// [`VaultRepo::init`], with an unlock file the password works only together with.
    pub fn init_with(
        backend: Box<dyn Backend>,
        password: Option<&str>,
        kdf: KdfParams,
        unlock: Option<UnlockSecret>,
    ) -> Result<Self, VaultError> {
        if Self::exists(backend.as_ref())? {
            return Err(VaultError::AlreadyExists(backend.describe()));
        }
        if password == Some("") {
            return Err(VaultError::EmptyPassword);
        }
        let id = hex::encode(random::<16>()?);
        let (crypto, plain_keys, slot) = match password {
            Some(pw) => {
                let master = MasterKey::generate()?;
                let mut slot = KeySlot::wrap(
                    &master,
                    &slot_input(pw, unlock.as_ref()),
                    &id,
                    kdf,
                    "initial",
                )?;
                slot.unlock_file = unlock.is_some();
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
            packs: PackCache::default(),
            kdf,
            unlock,
            pad_packs: false,
            parity: 0,
        })
    }

    /// Open a repository; `password` is required if it is encrypted.
    pub fn open(backend: Box<dyn Backend>, password: Option<&str>) -> Result<Self, VaultError> {
        Self::open_with(backend, password, None)
    }

    /// [`VaultRepo::open`] with the secret of an unlock file, for key slots that need one.
    pub fn open_with(
        backend: Box<dyn Backend>,
        password: Option<&str>,
        unlock: Option<UnlockSecret>,
    ) -> Result<Self, VaultError> {
        let raw = match backend.read("config") {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(VaultError::NoRepository(backend.describe()));
            }
            Err(e) => return Err(VaultError::io("config", e)),
        };
        let config = parse_config(&raw)?;
        let config_encrypted = config.encrypted;
        let (crypto, slot) = if config.encrypted {
            let pw = password.ok_or(VaultError::PasswordRequired)?;
            let (master, sid, used_file) =
                Self::try_slots(backend.as_ref(), &config.id, pw, unlock.as_ref())?;
            (Crypto::Encrypted(master), Some((sid, used_file)))
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
            index: Mutex::new(None),
            packs: PackCache::default(),
            kdf: KdfParams::DEFAULT,
            unlock: unlock.filter(|_| config_encrypted && slot.as_ref().is_some_and(|s| s.1)),
            slot: slot.map(|s| s.0),
            pad_packs: false,
            parity: 0,
        })
    }

    fn try_slots(
        backend: &dyn Backend,
        repo_id: &str,
        password: &str,
        unlock: Option<&UnlockSecret>,
    ) -> Result<(MasterKey, String, bool), VaultError> {
        // Opened before with this password (and file), and its key slot is still there:
        // skip Argon2id.
        let inputs: Vec<_> = std::iter::once(slot_input(password, None))
            .chain(unlock.map(|u| slot_input(password, Some(u))))
            .collect();
        for (i, input) in inputs.iter().enumerate() {
            if let Some((master, sid)) = unlocked::get(repo_id, input)
                && backend.exists(&format!("keys/{sid}")).unwrap_or(false)
            {
                return Ok((master, sid, i == 1));
            }
        }
        let (master, sid, input) = Self::try_slots_slow(backend, repo_id, password, unlock)?;
        unlocked::put(repo_id, &input, &master, &sid);
        let used_file = *input != *inputs[0];
        Ok((master, sid, used_file))
    }

    fn try_slots_slow(
        backend: &dyn Backend,
        repo_id: &str,
        password: &str,
        unlock: Option<&UnlockSecret>,
    ) -> Result<(MasterKey, String, zeroize::Zeroizing<String>), VaultError> {
        let slots = backend
            .list("keys")
            .map_err(|e| VaultError::io("keys", e))?;
        if slots.is_empty() {
            return Err(VaultError::Corrupt(
                "encrypted repository without key slots".into(),
            ));
        }
        let mut needs_file = false;
        for (sid, _) in slots {
            let raw = backend
                .read(&format!("keys/{sid}"))
                .map_err(|e| VaultError::io("keys", e))?;
            let Ok(slot) = serde_json::from_slice::<KeySlot>(&raw) else {
                tracing::warn!("ignoring unreadable key slot {sid}");
                continue;
            };
            // A recovery code opens its slot however it was typed (case, spaces, dashes).
            let typed = (slot.label == crate::crypto::RECOVERY_LABEL)
                .then(|| crate::crypto::normalize_recovery_code(password))
                .flatten();
            let password = typed.as_deref().map_or(password, |s| s.as_str());
            let input = match (slot.unlock_file, unlock) {
                (false, _) => slot_input(password, None),
                (true, Some(u)) => slot_input(password, Some(u)),
                (true, None) => {
                    needs_file = true;
                    continue;
                }
            };
            if let Ok(master) = slot.unwrap(&input, repo_id) {
                return Ok((master, sid, input));
            }
        }
        Err(if needs_file {
            VaultError::UnlockFileNeeded
        } else {
            VaultError::WrongPassword
        })
    }

    pub fn backend(&self) -> &dyn Backend {
        self.backend.as_ref()
    }

    pub fn lock(&self, exclusive: bool) -> Result<LockGuard, VaultError> {
        self.backend
            .lock(exclusive)
            .map_err(|e| VaultError::io("lock", e))?
            .ok_or_else(|| VaultError::Locked {
                place: self.backend.describe(),
                held_by: self
                    .backend
                    .lock_holders()
                    .unwrap_or_default()
                    .iter()
                    .map(|l| l.describe())
                    .collect(),
            })
    }

    /// Remove lock files left by jobs that died: the stale ones, or (`force`) all of them
    /// (only when no job is using the Vault from anywhere). Returns who held them.
    pub fn clear_locks(&self, force: bool) -> Result<Vec<String>, VaultError> {
        Ok(self
            .backend
            .clear_locks(force)
            .map_err(|e| VaultError::io("locks", e))?
            .iter()
            .map(|l| l.describe())
            .collect())
    }

    /// The blob index, loaded on first use.
    pub fn index(&self) -> Result<Arc<Index>, VaultError> {
        let mut g = self.index.locked();
        if let Some(i) = g.as_ref() {
            return Ok(i.clone());
        }
        let i = Arc::new(Index::load(self.backend(), &self.crypto)?);
        *g = Some(i.clone());
        Ok(i)
    }

    /// Forget the cached index (after writing index files).
    pub fn invalidate_index(&self) {
        *self.index.locked() = None;
    }

    // ---- snapshots ----

    /// All snapshot records that can be read, oldest first. A record that can't be read
    /// or understood is skipped with a warning: right for showing what's there, wrong
    /// for deciding what may be deleted (see [`VaultRepo::records_strict`]).
    pub fn records(&self) -> Result<Vec<SnapshotRecord>, VaultError> {
        let (v, unreadable) = self.record_listing()?;
        for (n, e) in unreadable {
            tracing::warn!("skipping snapshot {n}: {e}");
        }
        Ok(v)
    }

    /// Every snapshot record, oldest first, or an error if even one can't be read. For
    /// anything that deletes data by what the snapshots use: a snapshot left out here
    /// would have its data taken for unused.
    pub fn records_strict(&self) -> Result<Vec<SnapshotRecord>, VaultError> {
        let (v, unreadable) = self.record_listing()?;
        match unreadable.into_iter().next() {
            None => Ok(v),
            Some((n, e)) => Err(VaultError::UnreadableSnapshot(n, e.to_string())),
        }
    }

    /// The readable snapshot records, oldest first, and the ones that couldn't be read
    /// (file name and why).
    pub fn record_listing(&self) -> Result<(Vec<SnapshotRecord>, Unreadable), VaultError> {
        let names = self
            .backend
            .list("snapshots")
            .map_err(|e| VaultError::io("snapshots", e))?;
        // Only records: a stray file someone left in the folder isn't a snapshot.
        let names: Vec<String> = names
            .into_iter()
            .map(|(n, _)| n)
            .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_hexdigit()))
            .collect();
        // All of them in a few batched reads (on a server, one at a time is a minute of
        // waiting for a few thousand snapshots, each time they are listed).
        let reads: Vec<crate::backend::ReadAt> = names
            .iter()
            .map(|n| (format!("snapshots/{n}"), 0, None))
            .collect();
        let raws = self.backend.read_many(&reads);
        let read: Vec<(String, Result<SnapshotRecord, VaultError>)> = names
            .into_par_iter()
            .zip(raws)
            .map(|(n, raw)| {
                let r = self.record_from(&n, raw);
                (n, r)
            })
            .collect();
        let mut v = Vec::new();
        let mut unreadable = Vec::new();
        for (n, r) in read {
            match r {
                Ok(r) => v.push(r),
                Err(e) => unreadable.push((n, e)),
            }
        }
        v.sort_by(|a, b| {
            a.meta
                .started
                .cmp(&b.meta.started)
                .then_with(|| a.meta.id.cmp(&b.meta.id))
        });
        Ok((v, unreadable))
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
        self.record_from(id, self.backend.read(&format!("snapshots/{id}")))
    }

    /// The record `id` from what reading its file gave.
    fn record_from(
        &self,
        id: &str,
        raw: std::io::Result<Vec<u8>>,
    ) -> Result<SnapshotRecord, VaultError> {
        let path = format!("snapshots/{id}");
        let raw = match raw {
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
        let sealed = self.crypto.seal(&json)?;
        self.backend
            .write(&path, &sealed)
            .map_err(|e| VaultError::io(&path, e))?;
        if self.parity > 0 {
            crate::protect::write_for(self.backend.as_ref(), &path, &sealed, self.parity, false)?;
        }
        Ok(())
    }

    // ---- the settings of the profiles using it ----

    /// Keep `text` (a profile's [settings copy], see `flux_compat::export::recovery_copy`)
    /// for `owner`, encrypted like everything else, unless it's already there.
    pub fn save_profile(&self, owner: &Owner, text: &str) -> Result<bool, VaultError> {
        let path = format!("{PROFILES_DIR}/{}", profile_key(owner));
        let saved = SavedProfile {
            profile: owner.profile.clone(),
            host: owner.host.clone(),
            user: owner.user.clone(),
            text: text.to_owned(),
        };
        if let Ok(raw) = self.backend.read(&path)
            && let Ok(plain) = self.crypto.open(&raw)
            && serde_json::from_slice::<SavedProfile>(&plain).ok().as_ref() == Some(&saved)
        {
            return Ok(false);
        }
        let json = serde_json::to_vec(&saved).map_err(|e| VaultError::Corrupt(e.to_string()))?;
        self.backend
            .write(&path, &self.crypto.seal(&json)?)
            .map_err(|e| VaultError::io(&path, e))?;
        Ok(true)
    }

    /// The profile settings kept in the Vault, by owner.
    pub fn saved_profiles(&self) -> Result<Vec<(Owner, String)>, VaultError> {
        let names = self
            .backend
            .list(PROFILES_DIR)
            .map_err(|e| VaultError::io(PROFILES_DIR, e))?;
        let mut out = Vec::new();
        for (n, _) in names {
            let path = format!("{PROFILES_DIR}/{n}");
            let read = || -> Result<SavedProfile, VaultError> {
                let raw = self
                    .backend
                    .read(&path)
                    .map_err(|e| VaultError::io(&path, e))?;
                serde_json::from_slice(&self.crypto.open(&raw)?)
                    .map_err(|e| VaultError::Corrupt(format!("{path}: {e}")))
            };
            match read() {
                Ok(s) => out.push((
                    Owner {
                        profile: s.profile,
                        host: s.host,
                        user: s.user,
                    },
                    s.text,
                )),
                Err(e) => tracing::warn!("skipping {path}: {e}"),
            }
        }
        out.sort_by(|a, b| {
            (&a.0.host, &a.0.user, &a.0.profile).cmp(&(&b.0.host, &b.0.user, &b.0.profile))
        });
        Ok(out)
    }

    /// Remove a snapshot's record. A record that is there but damaged (it can't be
    /// decrypted or understood) is removed too: it is the only way to be rid of it.
    pub fn delete_record(&self, id: &str) -> Result<(), VaultError> {
        match self.read_record(id) {
            Ok(_) | Err(VaultError::Corrupt(_) | VaultError::Crypto(_)) => {}
            Err(e) => return Err(e),
        }
        let path = format!("snapshots/{id}");
        self.backend
            .remove(&path)
            .map_err(|e| VaultError::io(&path, e))?;
        crate::protect::remove_for(self.backend.as_ref(), &path);
        Ok(())
    }

    /// Whether `id` is the full id of a record that is there but damaged (it can't be
    /// decrypted or understood). Such a snapshot isn't listed; it can only be deleted.
    pub fn is_damaged_record(&self, id: &str) -> bool {
        matches!(
            self.read_record(id),
            Err(VaultError::Corrupt(_) | VaultError::Crypto(_))
        )
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

    /// Read and decode one blob.
    ///
    /// From a local drive, just that blob's bytes. On a server every read is a round
    /// trip (three, with opening and closing the pack), and restores, mounts and tree
    /// walks want blob after blob from the same packs: thousands of small files sit in
    /// one pack, a big file's pieces follow one another, and folders' trees fill packs
    /// of their own. So the second blob wanted from a pack brings the whole pack, once,
    /// and the next ones come from memory. (The first is fetched alone: getting one
    /// file back shouldn't download 16 MiB for it.) A few packs are kept.
    pub fn read_blob(&self, loc: &crate::pack::Loc) -> Result<Vec<u8>, VaultError> {
        if !self.backend.remote() {
            return read_blob(self.backend(), &self.crypto, loc);
        }
        let Some(cell) = self.packs.wanted(&loc.pack) else {
            return read_blob(self.backend(), &self.crypto, loc);
        };
        let path = crate::backend::pack_path(&hex_id(&loc.pack));
        let pack = cell
            .get_or_init(|| {
                self.backend
                    .read(&path)
                    .map(Arc::new)
                    .map_err(|e| e.to_string())
            })
            .clone();
        let pack = match pack {
            Ok(p) => p,
            // The whole pack couldn't be had (now): the blob alone, as ever.
            Err(_) => {
                self.packs.forget(&loc.pack);
                return read_blob(self.backend(), &self.crypto, loc);
            }
        };
        let (from, len) = (loc.entry.offset as usize, loc.entry.length as usize);
        let sealed = pack.get(from..from + len).ok_or_else(|| {
            VaultError::Corrupt(format!(
                "pack {} is shorter than its index says",
                hex_id(&loc.pack)
            ))
        })?;
        crate::pack::decode_blob(&self.crypto, sealed, &loc.entry)
    }

    /// A tree blob, in tree format `format` (its snapshot record's `tree_format`).
    pub fn load_tree(&self, id: &BlobId, format: u32) -> Result<Tree, VaultError> {
        let index = self.index()?;
        let loc = index
            .get(id)
            .ok_or_else(|| VaultError::Corrupt(format!("tree {} is missing", hex_id(id))))?;
        let raw = self.read_blob(loc)?;
        Tree::decode(&raw, format)
            .map_err(|e| VaultError::Corrupt(format!("tree {}: {e}", hex_id(id))))
    }

    /// The tree format new snapshots are written in: the repository's format (1 until the
    /// repository is upgraded).
    pub fn tree_format(&self) -> u32 {
        self.config.format
    }

    /// Every entry of a snapshot with its content, sorted by path.
    pub fn flatten(&self, record: &SnapshotRecord) -> Result<Vec<(Entry, Content)>, VaultError> {
        self.flatten_under(record, &[])
    }

    /// The entries of a snapshot at or below one of `paths` (as entries name them, no
    /// leading slash), and the folders on the way to them, sorted by path. None given:
    /// every entry. Only the trees on the way and below are read: getting one file back
    /// doesn't read every folder of the snapshot.
    pub fn flatten_under(
        &self,
        record: &SnapshotRecord,
        paths: &[Vec<u8>],
    ) -> Result<Vec<(Entry, Content)>, VaultError> {
        let within = |path: &[u8], of: &[u8]| {
            of.is_empty() || (path.starts_with(of) && path.get(of.len()).is_none_or(|b| *b == b'/'))
        };
        // Wanted, or a folder on the way to something wanted.
        let wanted = |path: &[u8]| {
            paths.is_empty() || paths.iter().any(|w| within(path, w) || within(w, path))
        };
        let root =
            parse_id(&record.tree).ok_or_else(|| VaultError::Corrupt("bad root tree id".into()))?;
        let mut out = Vec::new();
        let mut stack: Vec<(Vec<u8>, BlobId)> = vec![(Vec::new(), root)];
        while let Some((prefix, tid)) = stack.pop() {
            for node in self.load_tree(&tid, record.tree_format)?.nodes {
                let mut path = prefix.clone();
                if !path.is_empty() {
                    path.push(b'/');
                }
                path.extend_from_slice(&node.name);
                if !wanted(&path) {
                    continue;
                }
                if let Some(sub) = node.subtree {
                    stack.push((path.clone(), sub));
                }
                let content = node.file_content();
                out.push((node.to_entry(path), content));
            }
        }
        out.sort_unstable_by(|a, b| a.0.path.cmp(&b.0.path));
        Ok(out)
    }

    // ---- keys ----

    pub fn keys(&self) -> Result<Vec<KeyInfo>, VaultError> {
        let mut out = Self::list_keys(self.backend.as_ref())?;
        for k in &mut out {
            k.current = self.slot.as_deref() == Some(k.id.as_str());
        }
        Ok(out)
    }

    /// A Vault's key slots, read without a password (they're plain JSON; none is marked
    /// current). Empty when there's no Vault.
    pub fn list_keys(backend: &dyn Backend) -> Result<Vec<KeyInfo>, VaultError> {
        let mut out = Vec::new();
        for (sid, _) in backend
            .list("keys")
            .map_err(|e| VaultError::io("keys", e))?
        {
            let raw = backend
                .read(&format!("keys/{sid}"))
                .map_err(|e| VaultError::io("keys", e))?;
            if let Ok(slot) = serde_json::from_slice::<KeySlot>(&raw) {
                out.push(KeyInfo {
                    current: false,
                    id: sid,
                    label: slot.label,
                    created: slot.created,
                    hashing: slot.params.hashing(),
                    unlock_file: slot.unlock_file,
                });
            }
        }
        out.sort_by_key(|k| k.created);
        Ok(out)
    }

    /// Add a password. Returns the new slot id. When this repository was opened with an
    /// unlock file, the new password needs the same file.
    pub fn add_key(&self, password: &str, label: &str) -> Result<String, VaultError> {
        if password.is_empty() {
            return Err(VaultError::EmptyPassword);
        }
        self.write_slot(password, label, self.kdf, self.unlock.as_ref())
    }

    /// [`VaultRepo::add_key`], needing the unlock file `unlock` (or none) whatever opened
    /// this repository: after opening with the recovery code (which needs no file), the
    /// new password still needs the file the old one did.
    pub fn add_key_with(
        &self,
        password: &str,
        label: &str,
        unlock: Option<&UnlockSecret>,
    ) -> Result<String, VaultError> {
        if password.is_empty() {
            return Err(VaultError::EmptyPassword);
        }
        self.write_slot(password, label, self.kdf, unlock)
    }

    /// Was this repository opened with its recovery code (not a password)?
    pub fn opened_by_recovery_code(&self) -> Result<bool, VaultError> {
        Ok(self
            .keys()?
            .iter()
            .any(|k| k.current && k.label == crate::crypto::RECOVERY_LABEL))
    }

    fn write_slot(
        &self,
        password: &str,
        label: &str,
        kdf: KdfParams,
        unlock: Option<&UnlockSecret>,
    ) -> Result<String, VaultError> {
        let Crypto::Encrypted(master) = &self.crypto else {
            return Err(VaultError::NotEncrypted);
        };
        let mut slot = KeySlot::wrap(
            master,
            &slot_input(password, unlock),
            &self.config.id,
            kdf,
            label,
        )?;
        slot.unlock_file = unlock.is_some();
        let sid = hex::encode(random::<8>()?);
        let json =
            serde_json::to_vec_pretty(&slot).map_err(|e| VaultError::Corrupt(e.to_string()))?;
        let path = format!("keys/{sid}");
        self.backend
            .write(&path, &json)
            .map_err(|e| VaultError::io("keys", e))?;
        if self.parity > 0 {
            crate::protect::write_for(self.backend.as_ref(), &path, &json, self.parity, false)?;
        }
        Ok(sid)
    }

    /// The unlock file's secret the opening key slot needed (`None`: password only).
    pub fn unlock_in_use(&self) -> Option<&UnlockSecret> {
        self.unlock.as_ref()
    }

    /// Re-make the key slot that opened this repository: the same `password` (checked
    /// against that slot), hashed with `kdf`, and needing the unlock file `unlock` (or
    /// none). Nothing else changes and no data is rewritten: the new slot is written, then
    /// the old one removed. Returns the new slot's id.
    pub fn rewrap(
        &self,
        password: &str,
        kdf: KdfParams,
        unlock: Option<&UnlockSecret>,
    ) -> Result<String, VaultError> {
        let Some(old) = self.slot.clone() else {
            return Err(VaultError::NotEncrypted);
        };
        let raw = self
            .backend
            .read(&format!("keys/{old}"))
            .map_err(|e| VaultError::io("keys", e))?;
        let slot: KeySlot = serde_json::from_slice(&raw)
            .map_err(|e| VaultError::Corrupt(format!("key slot {old}: {e}")))?;
        // The password must be the one this slot holds (not merely any that opens it).
        let input = slot_input(password, self.unlock.as_ref().filter(|_| slot.unlock_file));
        if slot.unwrap(&input, &self.config.id).is_err() {
            return Err(VaultError::WrongPassword);
        }
        let sid = self.write_slot(password, &slot.label, kdf, unlock)?;
        self.backend
            .remove(&format!("keys/{old}"))
            .map_err(|e| VaultError::io("keys", e))?;
        crate::protect::remove_for(self.backend.as_ref(), &format!("keys/{old}"));
        Ok(sid)
    }

    /// Make a recovery code: a long random code kept as one more key slot (labelled
    /// [`crate::crypto::RECOVERY_LABEL`]), for when every password is lost. It opens the
    /// Vault without the unlock file, as a LUKS recovery key does. An earlier recovery
    /// code stops working. Returns the code, shown once: it isn't stored anywhere else.
    pub fn set_recovery_code(&self) -> Result<zeroize::Zeroizing<String>, VaultError> {
        let old: Vec<String> = self
            .keys()?
            .into_iter()
            .filter(|k| k.label == crate::crypto::RECOVERY_LABEL)
            .map(|k| k.id)
            .collect();
        let code = crate::crypto::new_recovery_code()?;
        let sid = self.write_slot(&code, crate::crypto::RECOVERY_LABEL, self.kdf, None)?;
        for o in old.iter().filter(|o| **o != sid) {
            self.remove_key(o)?;
        }
        Ok(code)
    }

    /// Remove the recovery code, if there is one (`false`: there wasn't).
    pub fn remove_recovery_code(&self) -> Result<bool, VaultError> {
        let codes: Vec<String> = self
            .keys()?
            .into_iter()
            .filter(|k| k.label == crate::crypto::RECOVERY_LABEL)
            .map(|k| k.id)
            .collect();
        for c in &codes {
            self.remove_key(c)?;
        }
        Ok(!codes.is_empty())
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
            .map_err(|e| VaultError::io("keys", e))?;
        // And its repair data: a removed password must not be rebuildable.
        crate::protect::remove_for(self.backend.as_ref(), &format!("keys/{slot}"));
        Ok(())
    }
}

/// Blob ids of a snapshot's content, by file path (for restore).
pub type ContentMap = HashMap<Vec<u8>, Vec<BlobId>>;

/// Master keys this process has unlocked, so opening the same Vault again with the same
/// password skips Argon2id (0.2–0.5 s of CPU per try, by design). The app opens a Vault for
/// nearly every view; without this, each one paid that cost again.
///
/// Passwords are not kept: entries are found by a keyed hash of the password, under a key
/// that exists only in this process. A key whose slot was removed (`flux key remove`) is
/// not used. The master key is what the open repository would hold in memory anyway.
mod unlocked {
    use std::sync::{LazyLock, Mutex};

    use crate::crypto::{MasterKey, random};

    struct Unlocked {
        repo: String,
        tag: [u8; 32],
        sid: String,
        master: MasterKey,
    }

    const KEEP: usize = 16;
    static TAG_KEY: LazyLock<[u8; 32]> = LazyLock::new(|| random().unwrap_or([7; 32]));
    static CACHE: Mutex<Vec<Unlocked>> = Mutex::new(Vec::new());

    fn tag(password: &str) -> [u8; 32] {
        *blake3::keyed_hash(&TAG_KEY, password.as_bytes()).as_bytes()
    }

    pub(super) fn get(repo: &str, password: &str) -> Option<(MasterKey, String)> {
        let t = tag(password);
        let c = CACHE.lock().ok()?;
        c.iter()
            .find(|u| u.repo == repo && u.tag == t)
            .map(|u| (u.master.clone(), u.sid.clone()))
    }

    pub(super) fn put(repo: &str, password: &str, master: &MasterKey, sid: &str) {
        let t = tag(password);
        let Ok(mut c) = CACHE.lock() else { return };
        c.retain(|u| !(u.repo == repo && u.tag == t));
        if c.len() >= KEEP {
            c.remove(0);
        }
        c.push(Unlocked {
            repo: repo.to_owned(),
            tag: t,
            sid: sid.to_owned(),
            master: master.clone(),
        });
    }
}
