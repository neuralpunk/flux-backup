//! flux-vault-recover: read a Flux Backup Vault without Flux.
//!
//! A small, read-only program for the Vault format (formats 1 and 2): it lists snapshots
//! and their files, restores, and verifies. It shares no code with Flux's own Vault
//! engine, has no configuration, and builds as a static binary, so it can be kept next to
//! the backups and still run years from now on a computer without Flux.
//!
//! The format, briefly (docs/FORMATS.md has it in full):
//! - `config`: JSON (format, id, encrypted, and for unencrypted Vaults the id key).
//! - `keys/*`: JSON key slots; Argon2id of the password (with `\0` and the unlock file's
//!   hex secret when one is needed) unwraps the 72-byte master key with XChaCha20-Poly1305
//!   (associated data `flux-vault-key-v1:<vault id>`): encryption key, id key, chunk seed.
//! - Objects are a tag byte: `0` plain, `1` sealed (24-byte nonce, XChaCha20-Poly1305).
//! - `index/*`: sealed postcard lists of packs and the blobs in them.
//! - `data/xx/<pack>`: blobs, each sealed `[0 raw | 1 zstd] + bytes`, with a blob id that
//!   is the keyed BLAKE3 of its plaintext.
//! - `snapshots/<id>`: sealed JSON records naming a root tree; trees are postcard lists of
//!   nodes (a file's data blobs, a directory's subtree, holes in format 2).

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use serde::Deserialize;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const NEWEST_FORMAT: u32 = 2;

type Id = [u8; 32];
type Res<T> = Result<T, String>;

const USAGE: &str = "\
flux-vault-recover: read a Flux Backup Vault without Flux

Usage:
  flux-vault-recover VAULT snapshots
  flux-vault-recover VAULT files SNAPSHOT [PATH]
  flux-vault-recover VAULT restore SNAPSHOT TO [PATH...]
  flux-vault-recover VAULT verify [SNAPSHOT]

VAULT     the flux-vault folder, or the drive or folder that holds it
SNAPSHOT  latest, an id (or the start of one), or a snapshot's name
TO        an empty folder (made if missing): files go there under their full paths
PATH      only this file or folder (as listed by files, e.g. home/me/Documents)

Options:
  --password-file FILE  read the password from the first line of FILE
                        (otherwise FLUX_PASSWORD, or it's asked)
  --unlock-file FILE    the unlock file, for a Vault that needs one
  -h, --help            this help
  -V, --version         the version

It only reads the Vault: nothing there is ever changed.
Exit status: 0 done, 1 error, 2 bad usage, 3 verify found damage.
";

// ---- the format ----

#[derive(Deserialize)]
struct Config {
    format: u32,
    id: String,
    encrypted: bool,
    #[serde(default)]
    plain_keys: Option<PlainKeys>,
}

#[derive(Deserialize)]
struct PlainKeys {
    id_key: String,
}

#[derive(Deserialize)]
struct KeySlot {
    kdf: String,
    m_kib: u32,
    t: u32,
    p: u32,
    salt: String,
    nonce: String,
    ciphertext: String,
    #[serde(default)]
    unlock_file: bool,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
enum BlobKind {
    Data,
    Tree,
}

#[derive(Deserialize, Clone, Copy)]
struct PackEntry {
    #[allow(dead_code)]
    kind: BlobKind,
    id: Id,
    offset: u32,
    length: u32,
    #[allow(dead_code)]
    raw_length: u32,
}

#[derive(Deserialize)]
struct IndexPack {
    pack: Id,
    blobs: Vec<PackEntry>,
}

#[derive(Deserialize)]
struct IndexFile {
    packs: Vec<IndexPack>,
    supersedes: Vec<String>,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Dir,
    File,
    Symlink,
    Fifo,
    CharDevice,
    BlockDevice,
    Socket,
}

#[derive(Deserialize, Clone)]
struct Xattr {
    #[serde(with = "serde_bytes")]
    #[allow(dead_code)]
    name: Vec<u8>,
    #[serde(with = "serde_bytes")]
    #[allow(dead_code)]
    value: Vec<u8>,
}

#[derive(Deserialize, Clone, Copy)]
struct Hole {
    offset: u64,
    len: u64,
}

/// A tree node, format 2 (format 1 is the same without `holes`).
#[derive(Deserialize, Clone)]
struct Node {
    #[serde(with = "serde_bytes")]
    name: Vec<u8>,
    kind: Kind,
    mode: u32,
    uid: u32,
    gid: u32,
    size: u64,
    mtime: i64,
    mtime_nsec: u32,
    #[allow(dead_code)]
    dev: u64,
    #[allow(dead_code)]
    ino: u64,
    #[allow(dead_code)]
    nlink: u64,
    #[allow(dead_code)]
    rdev: u64,
    #[serde(with = "serde_bytes")]
    link: Option<Vec<u8>>,
    #[allow(dead_code)]
    hash: Option<[u8; 32]>,
    #[allow(dead_code)]
    xattrs: Vec<Xattr>,
    content: Vec<Id>,
    subtree: Option<Id>,
    holes: Vec<Hole>,
}

#[derive(Deserialize)]
struct NodeV1 {
    #[serde(with = "serde_bytes")]
    name: Vec<u8>,
    kind: Kind,
    mode: u32,
    uid: u32,
    gid: u32,
    size: u64,
    mtime: i64,
    mtime_nsec: u32,
    dev: u64,
    ino: u64,
    nlink: u64,
    rdev: u64,
    #[serde(with = "serde_bytes")]
    link: Option<Vec<u8>>,
    hash: Option<[u8; 32]>,
    xattrs: Vec<Xattr>,
    content: Vec<Id>,
    subtree: Option<Id>,
}

#[derive(Deserialize)]
struct Tree {
    nodes: Vec<Node>,
}

#[derive(Deserialize)]
struct TreeV1 {
    nodes: Vec<NodeV1>,
}

#[derive(Deserialize)]
struct Record {
    meta: serde_json::Value,
    tree: String,
    #[serde(default = "one")]
    tree_format: u32,
}

fn one() -> u32 {
    1
}

// ---- reading ----

enum Keys {
    Sealed { enc: [u8; 32], id: [u8; 32] },
    Plain { id: [u8; 32] },
}

struct Vault {
    root: PathBuf,
    keys: Keys,
    index: HashMap<Id, (Id, PackEntry)>,
    open_pack: std::cell::RefCell<Option<(Id, File)>>,
}

/// A snapshot: its id and record.
struct Snap {
    id: String,
    record: Record,
}

fn read(root: &Path, rel: &str) -> Res<Vec<u8>> {
    fs::read(root.join(rel)).map_err(|e| format!("{}: {e}", root.join(rel).display()))
}

fn list(root: &Path, rel: &str) -> Res<Vec<String>> {
    match fs::read_dir(root.join(rel)) {
        Ok(rd) => {
            let mut v: Vec<String> = rd
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                .filter_map(|e| e.file_name().to_str().map(str::to_owned))
                .filter(|n| !n.starts_with('.'))
                .collect();
            v.sort();
            Ok(v)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("{}: {e}", root.join(rel).display())),
    }
}

fn argon2id(input: &[u8], salt: &[u8], m_kib: u32, t: u32, p: u32) -> Res<[u8; 32]> {
    let params = argon2::Params::new(m_kib, t, p, Some(32)).map_err(|e| e.to_string())?;
    let mut out = [0u8; 32];
    argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
        .hash_password_into(input, salt, &mut out)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

impl Vault {
    /// Open the Vault at `place` (its folder, or the folder that holds `flux-vault`).
    fn open(place: &Path, password: &dyn Fn() -> Res<String>, unlock: Option<&Path>) -> Res<Vault> {
        let root = if place.join("config").is_file() {
            place.to_path_buf()
        } else if place.join("flux-vault/config").is_file() {
            place.join("flux-vault")
        } else {
            return Err(format!("no Vault in {}", place.display()));
        };
        let config: Config =
            serde_json::from_slice(&read(&root, "config")?).map_err(|e| format!("config: {e}"))?;
        if config.format == 0 || config.format > NEWEST_FORMAT {
            return Err(format!(
                "this Vault has format {}; this program reads formats 1 to {NEWEST_FORMAT}",
                config.format
            ));
        }
        let keys = if config.encrypted {
            Self::unwrap_key(&root, &config.id, &password()?, unlock)?
        } else {
            let pk = config.plain_keys.as_ref().ok_or("config has no id key")?;
            let id = B64
                .decode(&pk.id_key)
                .ok()
                .and_then(|k| k.try_into().ok())
                .ok_or("config has a bad id key")?;
            Keys::Plain { id }
        };
        let mut v = Vault {
            root,
            keys,
            index: HashMap::new(),
            open_pack: std::cell::RefCell::new(None),
        };
        v.load_index()?;
        Ok(v)
    }

    fn unwrap_key(root: &Path, vault_id: &str, password: &str, unlock: Option<&Path>) -> Res<Keys> {
        let secret = match unlock {
            Some(f) => {
                let bytes = fs::read(f).map_err(|e| format!("{}: {e}", f.display()))?;
                Some(hex::encode(blake3::derive_key(
                    "flux-vault unlock file v1",
                    &bytes,
                )))
            }
            None => None,
        };
        let aad = format!("flux-vault-key-v1:{vault_id}");
        let mut needs_file = false;
        let slots = list(root, "keys")?;
        if slots.is_empty() {
            return Err("this encrypted Vault has no keys".into());
        }
        for name in slots {
            let Ok(slot) = serde_json::from_slice::<KeySlot>(&read(root, &format!("keys/{name}"))?)
            else {
                continue;
            };
            if slot.kdf != "argon2id" {
                continue;
            }
            let input = match (slot.unlock_file, &secret) {
                (false, _) => password.to_owned(),
                (true, Some(s)) => format!("{password}\0{s}"),
                (true, None) => {
                    needs_file = true;
                    continue;
                }
            };
            let (Ok(salt), Ok(nonce), Ok(ct)) = (
                B64.decode(&slot.salt),
                B64.decode(&slot.nonce),
                B64.decode(&slot.ciphertext),
            ) else {
                continue;
            };
            let Ok(nonce) = <[u8; 24]>::try_from(nonce.as_slice()) else {
                continue;
            };
            let kek = argon2id(input.as_bytes(), &salt, slot.m_kib, slot.t, slot.p)?;
            let Ok(master) = XChaCha20Poly1305::new(&Key::from(kek)).decrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: &ct,
                    aad: aad.as_bytes(),
                },
            ) else {
                continue;
            };
            if master.len() != 72 {
                return Err("a key slot holds a key of the wrong size".into());
            }
            let mut enc = [0u8; 32];
            let mut id = [0u8; 32];
            enc.copy_from_slice(&master[..32]);
            id.copy_from_slice(&master[32..64]);
            return Ok(Keys::Sealed { enc, id });
        }
        Err(if needs_file {
            "wrong password, or this Vault also needs its unlock file (--unlock-file)".into()
        } else {
            "wrong password".into()
        })
    }

    /// Open an object: plain in an unencrypted Vault, sealed in an encrypted one (an
    /// unencrypted object in an encrypted Vault is refused: it could have been put there).
    fn open_object(&self, obj: &[u8], what: &str) -> Res<Vec<u8>> {
        match (obj.first(), &self.keys) {
            (Some(0), Keys::Plain { .. }) => Ok(obj[1..].to_vec()),
            (Some(1), Keys::Sealed { enc, .. }) if obj.len() >= 1 + 24 + 16 => {
                let nonce: [u8; 24] = obj[1..25].try_into().expect("24 bytes");
                XChaCha20Poly1305::new(&Key::from(*enc))
                    .decrypt(&XNonce::from(nonce), &obj[25..])
                    .map_err(|_| format!("{what}: damaged (it doesn't decrypt)"))
            }
            (Some(0), Keys::Sealed { .. }) => Err(format!(
                "{what}: unencrypted in an encrypted Vault (refused: it may have been tampered with)"
            )),
            (Some(1), Keys::Plain { .. }) => {
                Err(format!("{what}: encrypted in an unencrypted Vault"))
            }
            _ => Err(format!("{what}: unknown format")),
        }
    }

    fn blob_id(&self, plain: &[u8]) -> Id {
        let key = match &self.keys {
            Keys::Sealed { id, .. } | Keys::Plain { id } => id,
        };
        *blake3::keyed_hash(key, plain).as_bytes()
    }

    fn load_index(&mut self) -> Res<()> {
        let mut files = Vec::new();
        for n in list(&self.root, "index")? {
            let path = format!("index/{n}");
            let raw = self.open_object(&read(&self.root, &path)?, &path)?;
            let f: IndexFile = postcard::from_bytes(&raw).map_err(|e| format!("{path}: {e}"))?;
            files.push((n, f));
        }
        let superseded: HashSet<String> = files
            .iter()
            .flat_map(|(_, f)| f.supersedes.iter().cloned())
            .collect();
        for (name, f) in &files {
            if superseded.contains(name) {
                continue;
            }
            for p in &f.packs {
                for e in &p.blobs {
                    self.index.insert(e.id, (p.pack, *e));
                }
            }
        }
        Ok(())
    }

    /// A blob's plaintext, checked against its id.
    fn blob(&self, id: &Id) -> Res<Vec<u8>> {
        let hex_id = hex::encode(id);
        let (pack, e) = self
            .index
            .get(id)
            .ok_or_else(|| format!("blob {hex_id} is missing from the index"))?;
        let mut cached = self.open_pack.borrow_mut();
        if cached.as_ref().is_none_or(|(p, _)| p != pack) {
            let h = hex::encode(pack);
            let path = self.root.join("data").join(&h[..2]).join(&h);
            let f = File::open(&path).map_err(|err| format!("{}: {err}", path.display()))?;
            *cached = Some((*pack, f));
        }
        let (_, f) = cached.as_mut().expect("just opened");
        let mut sealed = vec![0u8; e.length as usize];
        f.seek(SeekFrom::Start(u64::from(e.offset)))
            .and_then(|_| f.read_exact(&mut sealed))
            .map_err(|err| format!("blob {hex_id} in pack {}: {err}", hex::encode(pack)))?;
        drop(cached);
        let payload = self.open_object(&sealed, &format!("blob {hex_id}"))?;
        let plain = match payload.split_first() {
            Some((0, body)) => body.to_vec(),
            Some((1, body)) => {
                let mut out = Vec::new();
                ruzstd::decoding::StreamingDecoder::new(body)
                    .map_err(|err| format!("blob {hex_id}: {err}"))?
                    .read_to_end(&mut out)
                    .map_err(|err| format!("blob {hex_id}: {err}"))?;
                out
            }
            _ => return Err(format!("blob {hex_id}: unknown encoding")),
        };
        if self.blob_id(&plain) != *id {
            return Err(format!(
                "blob {hex_id}: damaged (its content doesn't match its id)"
            ));
        }
        Ok(plain)
    }

    fn tree(&self, id: &Id, format: u32) -> Res<Vec<Node>> {
        let raw = self.blob(id)?;
        let what = || format!("tree {}", hex::encode(id));
        if format >= 2 {
            let t: Tree = postcard::from_bytes(&raw).map_err(|e| format!("{}: {e}", what()))?;
            // Holes in order, not empty, inside the file; otherwise the tree is damaged.
            for n in &t.nodes {
                let mut pos = 0u64;
                for h in &n.holes {
                    match h.offset.checked_add(h.len) {
                        Some(end) if h.len > 0 && h.offset >= pos && end <= n.size => pos = end,
                        _ => return Err(format!("{}: damaged (holes out of order)", what())),
                    }
                }
            }
            Ok(t.nodes)
        } else {
            let t: TreeV1 = postcard::from_bytes(&raw).map_err(|e| format!("{}: {e}", what()))?;
            Ok(t.nodes
                .into_iter()
                .map(|n| Node {
                    name: n.name,
                    kind: n.kind,
                    mode: n.mode,
                    uid: n.uid,
                    gid: n.gid,
                    size: n.size,
                    mtime: n.mtime,
                    mtime_nsec: n.mtime_nsec,
                    dev: n.dev,
                    ino: n.ino,
                    nlink: n.nlink,
                    rdev: n.rdev,
                    link: n.link,
                    hash: n.hash,
                    xattrs: n.xattrs,
                    content: n.content,
                    subtree: n.subtree,
                    holes: Vec::new(),
                })
                .collect())
        }
    }

    /// Every snapshot, oldest first.
    fn snapshots(&self) -> Res<Vec<Snap>> {
        let mut out = Vec::new();
        for n in list(&self.root, "snapshots")? {
            if n.is_empty() || !n.bytes().all(|b| b.is_ascii_hexdigit()) {
                continue;
            }
            let path = format!("snapshots/{n}");
            let raw = self.open_object(&read(&self.root, &path)?, &path)?;
            let record: Record =
                serde_json::from_slice(&raw).map_err(|e| format!("{path}: {e}"))?;
            out.push(Snap { id: n, record });
        }
        out.sort_by(|a, b| started(a).cmp(&started(b)).then(a.id.cmp(&b.id)));
        Ok(out)
    }

    /// `latest`, an id or the start of one, or a name.
    fn find(&self, spec: &str) -> Res<Snap> {
        let mut all = self.snapshots()?;
        if all.is_empty() {
            return Err("this Vault has no snapshots".into());
        }
        if spec == "latest" {
            return Ok(all.pop().expect("not empty"));
        }
        if let Some(i) = all.iter().position(|s| s.id == spec) {
            return Ok(all.swap_remove(i));
        }
        let by_name: Vec<usize> = (0..all.len())
            .filter(|&i| all[i].record.meta["name"].as_str() == Some(spec))
            .collect();
        let matches: Vec<usize> = if by_name.is_empty() {
            (0..all.len())
                .filter(|&i| all[i].id.starts_with(spec))
                .collect()
        } else {
            by_name
        };
        match matches.as_slice() {
            [i] => Ok(all.swap_remove(*i)),
            [] => Err(format!("no snapshot {spec:?} (see: snapshots)")),
            _ => Err(format!(
                "{spec:?} matches several snapshots: give more of the id"
            )),
        }
    }

    /// Every entry of a snapshot with its path, sorted by path.
    fn entries(&self, s: &Snap) -> Res<Vec<(Vec<u8>, Node)>> {
        let root: Id = hex::decode(&s.record.tree)
            .ok()
            .and_then(|b| b.try_into().ok())
            .ok_or("the snapshot's root tree id is damaged")?;
        let mut out = Vec::new();
        let mut stack = vec![(Vec::new(), root)];
        while let Some((prefix, tid)) = stack.pop() {
            for node in self.tree(&tid, s.record.tree_format)? {
                let mut path = prefix.clone();
                if !path.is_empty() {
                    path.push(b'/');
                }
                path.extend_from_slice(&node.name);
                // Only a folder has something below it. (A forged tree could hang a
                // subtree on a link, to have its files written through the link.)
                if let Some(sub) = node.subtree
                    && node.kind == Kind::Dir
                {
                    stack.push((path.clone(), sub));
                }
                out.push((path, node));
            }
        }
        out.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }
}

fn started(s: &Snap) -> String {
    s.record.meta["started"].as_str().unwrap_or("").to_owned()
}

fn when(s: &Snap) -> String {
    started(s).get(..16).unwrap_or("").replace('T', " ")
}

fn under(path: &[u8], prefixes: &[Vec<u8>]) -> bool {
    prefixes.is_empty()
        || prefixes.iter().any(|p| {
            path == p.as_slice() || (path.starts_with(p) && path.get(p.len()) == Some(&b'/'))
        })
}

fn human(n: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1000.0 && u < units.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", units[u])
    }
}

// ---- commands ----

fn cmd_snapshots(v: &Vault) -> Res<()> {
    let snaps = v.snapshots()?;
    if snaps.is_empty() {
        println!("No snapshots.");
        return Ok(());
    }
    println!(
        "{:<16}  {:<16}  {:<28}  NAME",
        "ID", "STARTED (UTC)", "MADE BY"
    );
    for s in &snaps {
        let m = &s.record.meta;
        let by = format!(
            "{}@{} ({})",
            m["user"].as_str().unwrap_or("?"),
            m["host"].as_str().unwrap_or("?"),
            m["profile"].as_str().unwrap_or("?")
        );
        println!(
            "{:<16}  {:<16}  {:<28}  {}",
            s.id.get(..16).unwrap_or(&s.id),
            when(s),
            by,
            m["name"].as_str().unwrap_or("")
        );
    }
    Ok(())
}

fn cmd_files(v: &Vault, spec: &str, path: Option<&str>) -> Res<()> {
    let s = v.find(spec)?;
    let prefixes: Vec<Vec<u8>> = path
        .map(|p| p.trim_matches('/').as_bytes().to_vec())
        .into_iter()
        .collect();
    let mut out = std::io::stdout().lock();
    for (p, n) in v.entries(&s)? {
        if !under(&p, &prefixes) {
            continue;
        }
        let k = match n.kind {
            Kind::Dir => 'd',
            Kind::File => '-',
            Kind::Symlink => 'l',
            _ => '?',
        };
        let _ = writeln!(
            out,
            "{k} {:>10}  {}",
            if n.kind == Kind::File {
                human(n.size)
            } else {
                String::new()
            },
            String::from_utf8_lossy(&p)
        );
    }
    Ok(())
}

fn safe_relative(p: &[u8]) -> bool {
    !p.is_empty()
        && p[0] != b'/'
        && p.split(|&b| b == b'/')
            .all(|c| !c.is_empty() && c != b"." && c != b"..")
}

/// Make `rel` (a safe relative path) a real folder under `to`, making what's missing.
/// Every part of the way must be a folder itself, never a link: a snapshot that isn't
/// what Flux wrote (a Vault without encryption can be rewritten by whoever has the drive)
/// could first restore a link and then a file "inside" it, to be written wherever the
/// link points. Flux's own restore refuses that the same way.
fn real_dir(to: &Path, rel: &[u8]) -> Res<PathBuf> {
    let mut dir = to.to_path_buf();
    for part in rel.split(|&b| b == b'/').filter(|c| !c.is_empty()) {
        dir.push(std::ffi::OsStr::from_bytes(part));
        match fs::symlink_metadata(&dir) {
            Ok(m) if m.file_type().is_dir() => {}
            Ok(_) => {
                return Err(format!(
                    "{} is in the way and isn't a folder",
                    dir.display()
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            Err(e) => return Err(format!("{}: {e}", dir.display())),
        }
    }
    Ok(dir)
}

/// The folder part of a relative path (`a/b` of `a/b/c`; empty for `c`).
fn parent_of(rel: &[u8]) -> &[u8] {
    rel.iter()
        .rposition(|&b| b == b'/')
        .map_or(&[][..], |i| &rel[..i])
}

fn write_file(v: &Vault, n: &Node, dest: &Path) -> Res<()> {
    // A new file, never one reached through a link: what is there already (a file from an
    // earlier restore, or a link) is removed first, and creating fails if anything is
    // there after all.
    match fs::symlink_metadata(dest) {
        Ok(m) if m.file_type().is_dir() => {
            return Err(format!("{} is a folder; not replaced", dest.display()));
        }
        Ok(_) => fs::remove_file(dest).map_err(|e| format!("{}: {e}", dest.display()))?,
        Err(_) => {}
    }
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)
        .map_err(|e| format!("{}: {e}", dest.display()))?;
    // The data, laid out between the holes.
    let mut runs = Vec::new();
    let mut pos = 0;
    for h in &n.holes {
        if h.offset > pos {
            runs.push((pos, h.offset - pos));
        }
        pos = h.offset + h.len;
    }
    if n.size > pos {
        runs.push((pos, n.size - pos));
    }
    let mut blobs = n.content.iter();
    let mut buf: Vec<u8> = Vec::new();
    let mut at = 0usize;
    for (start, len) in runs {
        f.seek(SeekFrom::Start(start))
            .map_err(|e| format!("{}: {e}", dest.display()))?;
        let mut left = len;
        while left > 0 {
            if at == buf.len() {
                let id = blobs.next().ok_or_else(|| {
                    format!("{}: the backup has less data than its size", dest.display())
                })?;
                buf = v.blob(id)?;
                at = 0;
                continue;
            }
            let take = ((buf.len() - at) as u64).min(left) as usize;
            f.write_all(&buf[at..at + take])
                .map_err(|e| format!("{}: {e}", dest.display()))?;
            at += take;
            left -= take as u64;
        }
    }
    f.set_len(n.size)
        .map_err(|e| format!("{}: {e}", dest.display()))
}

fn set_time(path: &Path, n: &Node) {
    if let Ok(f) = File::open(path) {
        let t = if n.mtime >= 0 {
            std::time::UNIX_EPOCH
                + std::time::Duration::new(n.mtime as u64, n.mtime_nsec.min(999_999_999))
        } else {
            std::time::UNIX_EPOCH - std::time::Duration::from_secs(n.mtime.unsigned_abs())
        };
        let _ = f.set_modified(t);
    }
}

fn cmd_restore(v: &Vault, spec: &str, to: &Path, paths: &[String]) -> Res<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let s = v.find(spec)?;
    if to.exists() {
        let empty = fs::read_dir(to)
            .map_err(|e| format!("{}: {e}", to.display()))?
            .next()
            .is_none();
        if !empty {
            return Err(format!(
                "{} isn't empty: restore into an empty folder (nothing is ever overwritten)",
                to.display()
            ));
        }
    }
    fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    let as_root = fs::metadata(to).map(|m| m.uid() == 0).unwrap_or(false);
    let prefixes: Vec<Vec<u8>> = paths
        .iter()
        .map(|p| p.trim_matches('/').as_bytes().to_vec())
        .collect();
    let entries: Vec<(Vec<u8>, Node)> = v
        .entries(&s)?
        .into_iter()
        .filter(|(p, _)| under(p, &prefixes))
        .collect();
    if entries.is_empty() {
        return Err("nothing in that snapshot matches".into());
    }
    let (mut files, mut bytes, mut skipped) = (0u64, 0u64, Vec::new());
    let mut dirs = Vec::new();
    for (p, n) in &entries {
        if !safe_relative(p) {
            skipped.push(format!("{} (an unsafe path)", String::from_utf8_lossy(p)));
            continue;
        }
        let dest = to.join(std::ffi::OsStr::from_bytes(p));
        // The way there, and a folder itself, are real folders and never links.
        let made = match n.kind {
            Kind::Dir => real_dir(to, p),
            _ => real_dir(to, parent_of(p)),
        };
        if let Err(e) = made {
            skipped.push(format!("{} ({e})", String::from_utf8_lossy(p)));
            continue;
        }
        match n.kind {
            Kind::Dir => dirs.push((dest, n)),
            Kind::File => {
                write_file(v, n, &dest)?;
                let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(n.mode & 0o7777));
                if as_root {
                    let _ = std::os::unix::fs::chown(&dest, Some(n.uid), Some(n.gid));
                }
                set_time(&dest, n);
                files += 1;
                bytes += n.size;
            }
            Kind::Symlink => {
                let target = n.link.clone().unwrap_or_default();
                std::os::unix::fs::symlink(std::ffi::OsStr::from_bytes(&target), &dest)
                    .map_err(|e| format!("{}: {e}", dest.display()))?;
                if as_root {
                    let _ = std::os::unix::fs::lchown(&dest, Some(n.uid), Some(n.gid));
                }
            }
            other => skipped.push(format!(
                "{} (a {} isn't restored by this program)",
                String::from_utf8_lossy(p),
                match other {
                    Kind::Fifo => "named pipe",
                    Kind::Socket => "socket",
                    _ => "device",
                }
            )),
        }
    }
    // Folders last, deepest first, so their times stay as they were.
    for (dest, n) in dirs.iter().rev() {
        let _ = fs::set_permissions(dest, fs::Permissions::from_mode(n.mode & 0o7777));
        if as_root {
            let _ = std::os::unix::fs::chown(dest, Some(n.uid), Some(n.gid));
        }
        set_time(dest, n);
    }
    println!(
        "Restored {files} file{} ({}) from snapshot {} into {}.",
        if files == 1 { "" } else { "s" },
        human(bytes),
        s.id.get(..16).unwrap_or(&s.id),
        to.display()
    );
    for s in &skipped {
        eprintln!("skipped {s}");
    }
    Ok(())
}

/// Read back every blob the snapshots use. `Ok(false)`: damage found (reported).
fn cmd_verify(v: &Vault, spec: Option<&str>) -> Res<bool> {
    let snaps = match spec {
        Some(s) => vec![v.find(s)?],
        None => v.snapshots()?,
    };
    let mut seen: HashSet<Id> = HashSet::new();
    let mut problems = Vec::new();
    let mut checked = 0u64;
    for s in &snaps {
        let entries = match v.entries(s) {
            Ok(e) => e,
            Err(e) => {
                problems.push(format!("snapshot {}: {e}", s.id));
                continue;
            }
        };
        for (p, n) in entries {
            for id in &n.content {
                if seen.insert(*id) {
                    checked += 1;
                    if let Err(e) = v.blob(id) {
                        problems.push(format!("{}: {e}", String::from_utf8_lossy(&p)));
                    }
                }
            }
        }
    }
    println!(
        "Checked {} snapshot{} and {checked} pieces of data.",
        snaps.len(),
        if snaps.len() == 1 { "" } else { "s" }
    );
    if problems.is_empty() {
        println!("Everything reads back as it was stored.");
        Ok(true)
    } else {
        for p in &problems {
            eprintln!("damaged: {p}");
        }
        eprintln!(
            "{} problem{} found.",
            problems.len(),
            if problems.len() == 1 { "" } else { "s" }
        );
        Ok(false)
    }
}

// ---- the command line ----

fn ask_password() -> Res<String> {
    if let Ok(p) = std::env::var("FLUX_PASSWORD")
        && !p.is_empty()
    {
        return Ok(p);
    }
    let mut tty = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(
            |_| "this Vault is encrypted: give its password (FLUX_PASSWORD or --password-file)",
        )?;
    let _ = write!(tty, "Password: ");
    let stty = |arg: &str| {
        let _ = std::process::Command::new("stty")
            .arg(arg)
            .stdin(
                fs::File::open("/dev/tty")
                    .map(std::process::Stdio::from)
                    .unwrap_or(std::process::Stdio::null()),
            )
            .status();
    };
    stty("-echo");
    let mut line = String::new();
    let r = std::io::BufRead::read_line(&mut std::io::BufReader::new(&mut tty), &mut line);
    stty("echo");
    let _ = writeln!(tty);
    r.map_err(|e| e.to_string())?;
    Ok(line.trim_end_matches(['\n', '\r']).to_owned())
}

fn run(args: Vec<String>) -> Result<ExitCode, (u8, String)> {
    let mut password_file: Option<PathBuf> = None;
    let mut unlock: Option<PathBuf> = None;
    let mut rest = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(ExitCode::SUCCESS);
            }
            "-V" | "--version" => {
                println!("flux-vault-recover {VERSION} (Vault formats 1 to {NEWEST_FORMAT})");
                return Ok(ExitCode::SUCCESS);
            }
            "--password-file" => {
                password_file = Some(
                    it.next()
                        .ok_or((2, "--password-file needs a file".into()))?
                        .into(),
                );
            }
            "--unlock-file" => {
                unlock = Some(
                    it.next()
                        .ok_or((2, "--unlock-file needs a file".into()))?
                        .into(),
                );
            }
            s if s.starts_with("--") => return Err((2, format!("unknown option {s}"))),
            _ => rest.push(a),
        }
    }
    let usage = |m: &str| (2u8, format!("{m}\n\n{USAGE}"));
    let [vault, cmd, args @ ..] = rest.as_slice() else {
        return Err(usage("give a Vault and what to do"));
    };
    let password = || -> Res<String> {
        match &password_file {
            Some(f) => fs::read_to_string(f)
                .map(|s| s.lines().next().unwrap_or("").to_owned())
                .map_err(|e| format!("{}: {e}", f.display())),
            None => ask_password(),
        }
    };
    let err = |e: String| (1u8, e);
    let v = Vault::open(Path::new(vault), &password, unlock.as_deref()).map_err(err)?;
    match (cmd.as_str(), args) {
        ("snapshots", []) => cmd_snapshots(&v).map_err(err)?,
        ("files", [spec]) => cmd_files(&v, spec, None).map_err(err)?,
        ("files", [spec, path]) => cmd_files(&v, spec, Some(path)).map_err(err)?,
        ("restore", [spec, to, paths @ ..]) => {
            cmd_restore(&v, spec, Path::new(to), paths).map_err(err)?
        }
        ("verify", []) => {
            if !cmd_verify(&v, None).map_err(err)? {
                return Ok(ExitCode::from(3));
            }
        }
        ("verify", [spec]) => {
            if !cmd_verify(&v, Some(spec)).map_err(err)? {
                return Ok(ExitCode::from(3));
            }
        }
        _ => return Err(usage(&format!("can't {cmd} with those arguments"))),
    }
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(c) => c,
        Err((code, msg)) => {
            eprintln!("flux-vault-recover: {msg}");
            ExitCode::from(code)
        }
    }
}
