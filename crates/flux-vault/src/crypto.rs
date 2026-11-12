//! Keys, key slots, object encryption and blob ids. See `docs/VAULT_FORMAT.md`.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

pub type BlobId = [u8; 32];

const TAG_PLAIN: u8 = 0x00;
const TAG_SEALED: u8 = 0x01;
const NONCE_LEN: usize = 24;
const MASTER_LEN: usize = 72;

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("wrong password, or the data was modified")]
    Authentication,
    #[error("object is too short or has an unknown format")]
    Malformed,
    #[error("no random numbers available from the OS: {0}")]
    Random(String),
    #[error("key derivation failed: {0}")]
    Kdf(String),
    #[error("encrypted object found in an unencrypted repository")]
    UnexpectedlySealed,
}

pub fn random<const N: usize>() -> Result<[u8; N], CryptoError> {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).map_err(|e| CryptoError::Random(e.to_string()))?;
    Ok(b)
}

/// The repository's secret material.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct MasterKey {
    enc: [u8; 32],
    id: [u8; 32],
    chunk_seed: u64,
}

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MasterKey(…)")
    }
}

impl MasterKey {
    pub fn generate() -> Result<Self, CryptoError> {
        Ok(Self {
            enc: random()?,
            id: random()?,
            chunk_seed: u64::from_le_bytes(random()?),
        })
    }

    fn to_bytes(&self) -> Zeroizing<[u8; MASTER_LEN]> {
        let mut b = Zeroizing::new([0u8; MASTER_LEN]);
        b[..32].copy_from_slice(&self.enc);
        b[32..64].copy_from_slice(&self.id);
        b[64..].copy_from_slice(&self.chunk_seed.to_le_bytes());
        b
    }

    fn from_bytes(b: &[u8]) -> Result<Self, CryptoError> {
        if b.len() != MASTER_LEN {
            return Err(CryptoError::Malformed);
        }
        let mut k = Self {
            enc: [0; 32],
            id: [0; 32],
            chunk_seed: 0,
        };
        k.enc.copy_from_slice(&b[..32]);
        k.id.copy_from_slice(&b[32..64]);
        k.chunk_seed = u64::from_le_bytes(b[64..72].try_into().expect("8 bytes"));
        Ok(k)
    }
}

/// How objects are protected in this repository.
#[derive(Debug, Clone)]
pub enum Crypto {
    Encrypted(MasterKey),
    /// No confidentiality; ids are still keyed (key stored in `config`).
    Plain {
        id_key: [u8; 32],
        chunk_seed: u64,
    },
}

impl Crypto {
    pub fn chunk_seed(&self) -> u64 {
        match self {
            Crypto::Encrypted(k) => k.chunk_seed,
            Crypto::Plain { chunk_seed, .. } => *chunk_seed,
        }
    }

    pub fn blob_id(&self, plaintext: &[u8]) -> BlobId {
        let key = match self {
            Crypto::Encrypted(k) => &k.id,
            Crypto::Plain { id_key, .. } => id_key,
        };
        *blake3::keyed_hash(key, plaintext).as_bytes()
    }

    /// Wrap `payload` as a stored object.
    pub fn seal(&self, payload: &[u8]) -> Result<Vec<u8>, CryptoError> {
        match self {
            Crypto::Plain { .. } => {
                let mut out = Vec::with_capacity(payload.len() + 1);
                out.push(TAG_PLAIN);
                out.extend_from_slice(payload);
                Ok(out)
            }
            Crypto::Encrypted(k) => {
                let nonce: [u8; NONCE_LEN] = random()?;
                let cipher = XChaCha20Poly1305::new(&Key::from(k.enc));
                let ct = cipher
                    .encrypt(&XNonce::from(nonce), payload)
                    .map_err(|_| CryptoError::Authentication)?;
                let mut out = Vec::with_capacity(1 + NONCE_LEN + ct.len());
                out.push(TAG_SEALED);
                out.extend_from_slice(&nonce);
                out.extend_from_slice(&ct);
                Ok(out)
            }
        }
    }

    /// Unwrap a stored object.
    pub fn open(&self, object: &[u8]) -> Result<Vec<u8>, CryptoError> {
        match (object.first(), self) {
            (Some(&TAG_PLAIN), Crypto::Plain { .. }) => Ok(object[1..].to_vec()),
            // Never written by an encrypted Vault: someone with the drive could have put
            // it there to be trusted without the key.
            (Some(&TAG_PLAIN), Crypto::Encrypted(_)) => Err(CryptoError::Authentication),
            (Some(&TAG_SEALED), Crypto::Encrypted(k)) => {
                if object.len() < 1 + NONCE_LEN + 16 {
                    return Err(CryptoError::Malformed);
                }
                let nonce: [u8; NONCE_LEN] =
                    object[1..1 + NONCE_LEN].try_into().expect("checked length");
                XChaCha20Poly1305::new(&Key::from(k.enc))
                    .decrypt(&XNonce::from(nonce), &object[1 + NONCE_LEN..])
                    .map_err(|_| CryptoError::Authentication)
            }
            (Some(&TAG_SEALED), Crypto::Plain { .. }) => Err(CryptoError::UnexpectedlySealed),
            _ => Err(CryptoError::Malformed),
        }
    }
}

/// Argon2id cost parameters.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct KdfParams {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

impl KdfParams {
    /// 64 MiB, 3 passes, 4 lanes: roughly 0.2–0.5 s on a current desktop.
    pub const DEFAULT: KdfParams = KdfParams {
        m_kib: 64 * 1024,
        t: 3,
        p: 4,
    };
    /// Fast parameters for tests only.
    pub const TESTING: KdfParams = KdfParams {
        m_kib: 64,
        t: 1,
        p: 1,
    };

    /// The parameters of a profile's hashing setting.
    pub fn of(h: flux_core::profile::PasswordHashing) -> Self {
        let (m_kib, t, p) = h.params();
        KdfParams { m_kib, t, p }
    }

    /// The profile setting these parameters are (a preset, or custom).
    pub fn hashing(self) -> flux_core::profile::PasswordHashing {
        flux_core::profile::PasswordHashing::from_params(self.m_kib, self.t, self.p)
    }

    /// How long hashing one password with these parameters takes on this computer.
    pub fn time_it(self) -> Result<std::time::Duration, CryptoError> {
        let start = std::time::Instant::now();
        kek("timing the password hashing", &[0u8; 16], self)?;
        Ok(start.elapsed())
    }
}

/// The secret of an unlock file: what, with the password, opens a key slot that needs one.
/// Any file works; Flux makes one of 64 random bytes.
pub type UnlockSecret = [u8; 32];

pub fn unlock_secret(file_contents: &[u8]) -> UnlockSecret {
    blake3::derive_key("flux-vault unlock file v1", file_contents)
}

/// What a key slot's password hashing gets: the password alone, or (for a slot that needs
/// an unlock file) the password with the file's secret. The secret is hex, after a NUL a
/// typed password can't contain, so no password collides with a password and file.
pub fn slot_input(password: &str, unlock: Option<&UnlockSecret>) -> Zeroizing<String> {
    match unlock {
        None => Zeroizing::new(password.to_owned()),
        Some(s) => Zeroizing::new(format!("{password}\0{}", hex::encode(s))),
    }
}

/// The label of a recovery code's key slot.
pub const RECOVERY_LABEL: &str = "recovery code";

/// Crockford's base32: no I, L, O or U, so nothing reads as something else.
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Symbols in a recovery code (5 bits each: 150 bits), in groups of five.
const CODE_SYMBOLS: usize = 30;

/// A new recovery code: `XXXXX-XXXXX-XXXXX-XXXXX-XXXXX-XXXXX`, 150 random bits.
pub fn new_recovery_code() -> Result<Zeroizing<String>, CryptoError> {
    let bytes = Zeroizing::new(random::<19>()?);
    let mut out = Zeroizing::new(String::with_capacity(CODE_SYMBOLS + 5));
    for i in 0..CODE_SYMBOLS {
        if i > 0 && i % 5 == 0 {
            out.push('-');
        }
        let bit = i * 5;
        let (byte, shift) = (bit / 8, bit % 8);
        let two = (u16::from(bytes[byte]) << 8) | u16::from(*bytes.get(byte + 1).unwrap_or(&0));
        let v = (two >> (11 - shift)) & 0x1f;
        out.push(CROCKFORD[v as usize] as char);
    }
    Ok(out)
}

/// A recovery code as typed (any case, spaces or dashes anywhere, O for 0, I or L for 1),
/// in its canonical form; `None` if it isn't one.
pub fn normalize_recovery_code(typed: &str) -> Option<Zeroizing<String>> {
    let mut symbols = Zeroizing::new(String::with_capacity(CODE_SYMBOLS));
    for c in typed.chars() {
        let c = match c.to_ascii_uppercase() {
            ' ' | '-' | '\t' => continue,
            'O' => '0',
            'I' | 'L' => '1',
            c if CROCKFORD.contains(&(c as u8)) && c.is_ascii() => c,
            _ => return None,
        };
        symbols.push(c);
    }
    if symbols.len() != CODE_SYMBOLS {
        return None;
    }
    let mut out = Zeroizing::new(String::with_capacity(CODE_SYMBOLS + 5));
    for (i, c) in symbols.chars().enumerate() {
        if i > 0 && i % 5 == 0 {
            out.push('-');
        }
        out.push(c);
    }
    Some(out)
}

/// `keys/<id>`: the master key wrapped with a password.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeySlot {
    pub kdf: String,
    #[serde(flatten)]
    pub params: KdfParams,
    pub salt: String,
    pub nonce: String,
    pub ciphertext: String,
    pub created: Timestamp,
    #[serde(default)]
    pub label: String,
    /// This slot opens only with the password and an unlock file (see [`slot_input`]).
    /// Left out when false, so other slots look as they always did.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unlock_file: bool,
}

fn kek(password: &str, salt: &[u8], p: KdfParams) -> Result<Zeroizing<[u8; 32]>, CryptoError> {
    let params = argon2::Params::new(p.m_kib, p.t, p.p, Some(32))
        .map_err(|e| CryptoError::Kdf(e.to_string()))?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut out = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(password.as_bytes(), salt, out.as_mut_slice())
        .map_err(|e| CryptoError::Kdf(e.to_string()))?;
    Ok(out)
}

fn aad(repo_id: &str) -> Vec<u8> {
    format!("flux-vault-key-v1:{repo_id}").into_bytes()
}

impl KeySlot {
    pub fn wrap(
        master: &MasterKey,
        password: &str,
        repo_id: &str,
        params: KdfParams,
        label: &str,
    ) -> Result<Self, CryptoError> {
        let salt: [u8; 16] = random()?;
        let nonce: [u8; NONCE_LEN] = random()?;
        let kek = kek(password, &salt, params)?;
        let ct = XChaCha20Poly1305::new(&Key::from(*kek))
            .encrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: master.to_bytes().as_slice(),
                    aad: &aad(repo_id),
                },
            )
            .map_err(|_| CryptoError::Authentication)?;
        Ok(Self {
            kdf: "argon2id".into(),
            params,
            salt: B64.encode(salt),
            nonce: B64.encode(nonce),
            ciphertext: B64.encode(ct),
            created: Timestamp::now(),
            label: label.into(),
            unlock_file: false,
        })
    }

    pub fn unwrap(&self, password: &str, repo_id: &str) -> Result<MasterKey, CryptoError> {
        if self.kdf != "argon2id" {
            return Err(CryptoError::Kdf(format!("unsupported kdf {:?}", self.kdf)));
        }
        let salt = B64.decode(&self.salt).map_err(|_| CryptoError::Malformed)?;
        let nonce: [u8; NONCE_LEN] = B64
            .decode(&self.nonce)
            .ok()
            .and_then(|n| n.try_into().ok())
            .ok_or(CryptoError::Malformed)?;
        let ct = B64
            .decode(&self.ciphertext)
            .map_err(|_| CryptoError::Malformed)?;
        let kek = kek(password, &salt, self.params)?;
        let plain = Zeroizing::new(
            XChaCha20Poly1305::new(&Key::from(*kek))
                .decrypt(
                    &XNonce::from(nonce),
                    Payload {
                        msg: &ct,
                        aad: &aad(repo_id),
                    },
                )
                .map_err(|_| CryptoError::Authentication)?,
        );
        MasterKey::from_bytes(&plain)
    }
}

pub fn hex_id(id: &[u8]) -> String {
    hex::encode(id)
}

pub fn parse_id(s: &str) -> Option<BlobId> {
    hex::decode(s).ok()?.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_round_trip_and_tamper_detection() {
        let c = Crypto::Encrypted(MasterKey::generate().unwrap());
        let obj = c.seal(b"secret").unwrap();
        assert_eq!(obj[0], TAG_SEALED);
        assert!(!obj.windows(6).any(|w| w == b"secret"));
        assert_eq!(c.open(&obj).unwrap(), b"secret");
        let mut bad = obj.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(matches!(c.open(&bad), Err(CryptoError::Authentication)));
        // Another key can't read it.
        let other = Crypto::Encrypted(MasterKey::generate().unwrap());
        assert!(other.open(&obj).is_err());
        // Same plaintext, different ciphertext (random nonce).
        assert_ne!(c.seal(b"secret").unwrap(), obj);
    }

    #[test]
    fn unlock_files_change_the_slot_input() {
        let s = unlock_secret(b"random bytes on a usb stick");
        assert_eq!(s, unlock_secret(b"random bytes on a usb stick"));
        assert_ne!(s, unlock_secret(b"another file"));
        assert_eq!(slot_input("pw", None).as_str(), "pw");
        let with = slot_input("pw", Some(&s));
        assert!(with.starts_with("pw\0") && with.len() == 2 + 1 + 64);
        // A slot made with a file doesn't open without it, and the reverse.
        let m = MasterKey::generate().unwrap();
        let slot = KeySlot::wrap(&m, &with, "repo", KdfParams::TESTING, "x").unwrap();
        assert!(slot.unwrap(&with, "repo").is_ok());
        assert!(slot.unwrap("pw", "repo").is_err());
        let other = slot_input("pw", Some(&unlock_secret(b"another file")));
        assert!(slot.unwrap(&other, "repo").is_err());
    }

    #[test]
    fn hashing_presets_round_trip() {
        use flux_core::profile::PasswordHashing as H;
        for h in [H::Standard, H::Strong, H::Maximum] {
            assert_eq!(KdfParams::of(h).hashing(), h);
        }
        assert_eq!(KdfParams::of(H::Standard), KdfParams::DEFAULT);
        let c = H::Custom {
            memory_mib: 128,
            passes: 2,
            threads: 8,
        };
        assert_eq!(KdfParams::of(c).hashing(), c);
        assert!(KdfParams::TESTING.time_it().is_ok());
    }

    #[test]
    fn an_encrypted_vault_refuses_unencrypted_objects() {
        let c = Crypto::Encrypted(MasterKey::generate().unwrap());
        // What someone with the drive could write in place of a sealed record.
        let forged = [&[TAG_PLAIN][..], b"{\"forged\":true}"].concat();
        assert!(matches!(c.open(&forged), Err(CryptoError::Authentication)));
        let sealed = c.seal(b"real").unwrap();
        assert_eq!(c.open(&sealed).unwrap(), b"real");
    }

    #[test]
    fn plain_mode() {
        let c = Crypto::Plain {
            id_key: [7; 32],
            chunk_seed: 1,
        };
        let obj = c.seal(b"data").unwrap();
        assert_eq!(obj, b"\0data");
        assert_eq!(c.open(&obj).unwrap(), b"data");
    }

    #[test]
    fn keyed_ids() {
        let a = Crypto::Encrypted(MasterKey::generate().unwrap());
        let b = Crypto::Encrypted(MasterKey::generate().unwrap());
        assert_eq!(a.blob_id(b"x"), a.blob_id(b"x"));
        assert_ne!(
            a.blob_id(b"x"),
            b.blob_id(b"x"),
            "ids depend on the repository key"
        );
        assert_ne!(a.blob_id(b"x"), *blake3::hash(b"x").as_bytes());
    }

    #[test]
    fn key_slots() {
        let master = MasterKey::generate().unwrap();
        let slot = KeySlot::wrap(
            &master,
            "correct horse",
            "repo1",
            KdfParams::TESTING,
            "test",
        )
        .unwrap();
        let json = serde_json::to_string(&slot).unwrap();
        let slot: KeySlot = serde_json::from_str(&json).unwrap();
        let back = slot.unwrap("correct horse", "repo1").unwrap();
        assert_eq!(back.to_bytes().as_slice(), master.to_bytes().as_slice());
        assert!(matches!(
            slot.unwrap("wrong", "repo1"),
            Err(CryptoError::Authentication)
        ));
        assert!(
            slot.unwrap("correct horse", "other-repo").is_err(),
            "bound to the repo id"
        );
    }
}
