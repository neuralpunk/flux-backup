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
/// An object sealed with XChaCha20-Poly1305 (formats 1 and 2).
const TAG_SEALED: u8 = 0x01;
/// An object sealed with XAES-256-GCM (format 3).
const TAG_XAES: u8 = 0x02;
/// Both ciphers take a 192-bit nonce, so random nonces never repeat in practice.
const NONCE_LEN: usize = 24;
/// The authentication tag, after the ciphertext (Poly1305's or GCM's).
const TAG_LEN: usize = 16;
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

/// The authenticated encryption of an encrypted Vault's objects and key slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Cipher {
    /// Formats 1 and 2 (Flux before 1.3).
    #[default]
    #[serde(rename = "xchacha20-poly1305")]
    XChaCha20Poly1305,
    /// Format 3 (Flux 1.3): AES-256-GCM with a key of its own for every object, derived
    /// from the master key and the first half of a 192-bit random nonce
    /// (C2SP XAES-256-GCM). AES runs in hardware on nearly every computer: about five
    /// times as fast as XChaCha20-Poly1305 here.
    #[serde(rename = "xaes-256-gcm")]
    Xaes256Gcm,
}

impl Cipher {
    /// The cipher of a Vault in `format`.
    pub fn of_format(format: u32) -> Self {
        if format >= 3 {
            Cipher::Xaes256Gcm
        } else {
            Cipher::XChaCha20Poly1305
        }
    }

    fn tag(self) -> u8 {
        match self {
            Cipher::XChaCha20Poly1305 => TAG_SEALED,
            Cipher::Xaes256Gcm => TAG_XAES,
        }
    }

    /// Its name, as people know it.
    pub fn name(self) -> &'static str {
        match self {
            Cipher::XChaCha20Poly1305 => "XChaCha20-Poly1305",
            Cipher::Xaes256Gcm => "XAES-256-GCM",
        }
    }

    fn is_default(&self) -> bool {
        *self == Cipher::default()
    }
}

/// XAES-256-GCM (C2SP): AES-256-GCM under a key derived, for each 192-bit nonce `N`,
/// from the key `K` by a NIST SP 800-108 counter KDF with CMAC-AES-256:
/// `Kx = AES_K(M1 ⊕ K1) ‖ AES_K(M2 ⊕ K1)`, `M1 = 00 01 58 00 ‖ N[..12]`, `M2 = 00 02 58 00 ‖
/// N[..12]`, `K1` the CMAC subkey of `K`; the GCM nonce is `N[12..]`. The derivation is
/// AES (RustCrypto's, in hardware where there is AES-NI); the GCM is ring's (assembly,
/// AES-NI, VAES and carry-less multiplication: about 9 GB/s on one core here).
pub(crate) struct Xaes {
    aes: aes::Aes256,
    k1: [u8; 16],
}

impl Xaes {
    fn new(key: &[u8; 32]) -> Self {
        use aes::cipher::{BlockEncrypt, KeyInit};
        let aes = aes::Aes256::new(key.into());
        let mut l = [0u8; 16].into();
        aes.encrypt_block(&mut l);
        let l = u128::from_be_bytes(l.into());
        let k1 = (l << 1) ^ if l >> 127 == 1 { 0x87 } else { 0 };
        Self {
            aes,
            k1: k1.to_be_bytes(),
        }
    }

    /// The AES-256-GCM key and nonce for `nonce`.
    fn derive(&self, nonce: &[u8; NONCE_LEN]) -> (ring::aead::LessSafeKey, ring::aead::Nonce) {
        use aes::cipher::BlockEncrypt;
        let mut blocks: [aes::Block; 2] = Default::default();
        for (i, b) in blocks.iter_mut().enumerate() {
            let m: &mut [u8] = b.as_mut_slice();
            m[1] = i as u8 + 1;
            m[2] = b'X';
            m[4..].copy_from_slice(&nonce[..12]);
            for (x, k) in m.iter_mut().zip(self.k1) {
                *x ^= k;
            }
        }
        self.aes.encrypt_blocks(&mut blocks);
        let mut kx = Zeroizing::new([0u8; 32]);
        kx[..16].copy_from_slice(&blocks[0]);
        kx[16..].copy_from_slice(&blocks[1]);
        let key = ring::aead::UnboundKey::new(&ring::aead::AES_256_GCM, kx.as_slice())
            .expect("a 32-byte key");
        let n = ring::aead::Nonce::assume_unique_for_key(nonce[12..].try_into().expect("12"));
        (ring::aead::LessSafeKey::new(key), n)
    }

    /// Encrypt `data` in place; returns the tag.
    fn seal(
        &self,
        nonce: &[u8; NONCE_LEN],
        aad: &[u8],
        data: &mut [u8],
    ) -> Result<[u8; TAG_LEN], CryptoError> {
        let (key, n) = self.derive(nonce);
        let tag = key
            .seal_in_place_separate_tag(n, ring::aead::Aad::from(aad), data)
            .map_err(|_| CryptoError::Authentication)?;
        Ok(tag.as_ref().try_into().expect("16-byte tag"))
    }

    /// Decrypt `data` (ciphertext and tag) in place; returns the plaintext's length.
    fn open(
        &self,
        nonce: &[u8; NONCE_LEN],
        aad: &[u8],
        data: &mut [u8],
    ) -> Result<usize, CryptoError> {
        let (key, n) = self.derive(nonce);
        key.open_in_place(n, ring::aead::Aad::from(aad), data)
            .map(|p| p.len())
            .map_err(|_| CryptoError::Authentication)
    }
}

/// An encrypted Vault's keys: the master key, the cipher its objects are sealed with, and
/// (XAES) what the cipher needs of the key, worked out once.
#[derive(Clone)]
pub struct Keys {
    master: MasterKey,
    cipher: Cipher,
    xaes: Option<std::sync::Arc<Xaes>>,
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Keys({:?})", self.cipher)
    }
}

impl Keys {
    pub fn new(master: MasterKey, cipher: Cipher) -> Self {
        let xaes =
            (cipher == Cipher::Xaes256Gcm).then(|| std::sync::Arc::new(Xaes::new(&master.enc)));
        Self {
            master,
            cipher,
            xaes,
        }
    }

    pub fn master(&self) -> &MasterKey {
        &self.master
    }

    pub fn cipher(&self) -> Cipher {
        self.cipher
    }
}

/// How objects are protected in this repository.
#[derive(Debug, Clone)]
pub enum Crypto {
    Encrypted(Keys),
    /// No confidentiality; ids are still keyed (key stored in `config`).
    Plain {
        id_key: [u8; 32],
        chunk_seed: u64,
    },
}

impl Crypto {
    pub fn chunk_seed(&self) -> u64 {
        match self {
            Crypto::Encrypted(k) => k.master.chunk_seed,
            Crypto::Plain { chunk_seed, .. } => *chunk_seed,
        }
    }

    pub fn blob_id(&self, plaintext: &[u8]) -> BlobId {
        let key = match self {
            Crypto::Encrypted(k) => &k.master.id,
            Crypto::Plain { id_key, .. } => id_key,
        };
        *blake3::keyed_hash(key, plaintext).as_bytes()
    }

    /// Wrap `payload` as a stored object.
    pub fn seal(&self, payload: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let mut out = Vec::with_capacity(self.seal_prefix_len() + payload.len() + TAG_LEN);
        out.resize(self.seal_prefix_len(), 0);
        out.extend_from_slice(payload);
        self.seal_in_place(&mut out)?;
        Ok(out)
    }

    /// How many bytes [`Crypto::seal_in_place`] needs before the payload (for the object's
    /// tag, and its nonce when encrypted).
    pub fn seal_prefix_len(&self) -> usize {
        match self {
            Crypto::Plain { .. } => 1,
            Crypto::Encrypted(_) => 1 + NONCE_LEN,
        }
    }

    /// [`Crypto::seal`] where the bytes lie: `buf` holds [`Crypto::seal_prefix_len`] bytes of
    /// room, then the payload, and becomes the stored object (the same bytes `seal` would
    /// return). No copy of the payload is made, so a backup can seal each blob in a buffer
    /// it keeps, instead of in three new ones.
    pub fn seal_in_place(&self, buf: &mut Vec<u8>) -> Result<(), CryptoError> {
        let prefix = self.seal_prefix_len();
        assert!(buf.len() >= prefix, "no room for the object's prefix");
        match self {
            Crypto::Plain { .. } => buf[0] = TAG_PLAIN,
            Crypto::Encrypted(k) => {
                let nonce: [u8; NONCE_LEN] = random()?;
                buf[0] = k.cipher.tag();
                buf[1..prefix].copy_from_slice(&nonce);
                let tag: [u8; TAG_LEN] = match &k.xaes {
                    Some(x) => x.seal(&nonce, &[], &mut buf[prefix..])?,
                    None => {
                        use chacha20poly1305::aead::AeadInOut;
                        XChaCha20Poly1305::new(&Key::from(k.master.enc))
                            .encrypt_inout_detached(
                                &XNonce::from(nonce),
                                &[],
                                (&mut buf[prefix..]).into(),
                            )
                            .map_err(|_| CryptoError::Authentication)?
                            .into()
                    }
                };
                buf.extend_from_slice(&tag);
            }
        }
        Ok(())
    }

    pub fn open(&self, object: &[u8]) -> Result<Vec<u8>, CryptoError> {
        match (object.first(), self) {
            (Some(&TAG_PLAIN), Crypto::Plain { .. }) => Ok(object[1..].to_vec()),
            // Never written by an encrypted Vault: someone with the drive could have put
            // it there to be trusted without the key.
            (Some(&TAG_PLAIN), Crypto::Encrypted(_)) => Err(CryptoError::Authentication),
            // Only the Vault's own cipher: an object sealed another way wasn't written by it.
            (Some(&tag), Crypto::Encrypted(k)) if tag == k.cipher.tag() => {
                if object.len() < 1 + NONCE_LEN + TAG_LEN {
                    return Err(CryptoError::Malformed);
                }
                let nonce: [u8; NONCE_LEN] =
                    object[1..1 + NONCE_LEN].try_into().expect("checked length");
                match &k.xaes {
                    Some(x) => {
                        let mut data = object[1 + NONCE_LEN..].to_vec();
                        let n = x.open(&nonce, &[], &mut data)?;
                        data.truncate(n);
                        Ok(data)
                    }
                    None => XChaCha20Poly1305::new(&Key::from(k.master.enc))
                        .decrypt(&XNonce::from(nonce), &object[1 + NONCE_LEN..])
                        .map_err(|_| CryptoError::Authentication),
                }
            }
            (Some(&(TAG_SEALED | TAG_XAES)), Crypto::Encrypted(_)) => {
                Err(CryptoError::Authentication)
            }
            (Some(&(TAG_SEALED | TAG_XAES)), Crypto::Plain { .. }) => {
                Err(CryptoError::UnexpectedlySealed)
            }
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
    /// How the master key is sealed: the Vault's cipher. Left out for XChaCha20-Poly1305,
    /// as slots were before format 3.
    #[serde(default, skip_serializing_if = "Cipher::is_default")]
    pub cipher: Cipher,
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
    /// A slot for `master`, sealed with `cipher` (the Vault's).
    pub fn wrap(
        master: &MasterKey,
        password: &str,
        repo_id: &str,
        params: KdfParams,
        label: &str,
        cipher: Cipher,
    ) -> Result<Self, CryptoError> {
        let salt: [u8; 16] = random()?;
        let nonce: [u8; NONCE_LEN] = random()?;
        let kek = kek(password, &salt, params)?;
        let ct = match cipher {
            Cipher::Xaes256Gcm => {
                let mut data = master.to_bytes().to_vec();
                let tag = Xaes::new(&kek).seal(&nonce, &aad(repo_id), &mut data)?;
                data.extend_from_slice(&tag);
                data
            }
            Cipher::XChaCha20Poly1305 => XChaCha20Poly1305::new(&Key::from(*kek))
                .encrypt(
                    &XNonce::from(nonce),
                    Payload {
                        msg: master.to_bytes().as_slice(),
                        aad: &aad(repo_id),
                    },
                )
                .map_err(|_| CryptoError::Authentication)?,
        };
        Ok(Self {
            kdf: "argon2id".into(),
            params,
            salt: B64.encode(salt),
            nonce: B64.encode(nonce),
            ciphertext: B64.encode(ct),
            created: Timestamp::now(),
            label: label.into(),
            unlock_file: false,
            cipher,
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
        let plain = Zeroizing::new(match self.cipher {
            Cipher::Xaes256Gcm => {
                let mut data = ct;
                let n = Xaes::new(&kek).open(&nonce, &aad(repo_id), &mut data)?;
                data.truncate(n);
                data
            }
            Cipher::XChaCha20Poly1305 => XChaCha20Poly1305::new(&Key::from(*kek))
                .decrypt(
                    &XNonce::from(nonce),
                    Payload {
                        msg: &ct,
                        aad: &aad(repo_id),
                    },
                )
                .map_err(|_| CryptoError::Authentication)?,
        });
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

    const CIPHERS: [Cipher; 2] = [Cipher::Xaes256Gcm, Cipher::XChaCha20Poly1305];

    fn encrypted(cipher: Cipher) -> Crypto {
        Crypto::Encrypted(Keys::new(MasterKey::generate().unwrap(), cipher))
    }

    #[test]
    fn seal_open_round_trip_and_tamper_detection() {
        for cipher in CIPHERS {
            let c = encrypted(cipher);
            let obj = c.seal(b"secret").unwrap();
            assert_eq!(obj[0], cipher.tag());
            assert_eq!(obj.len(), 1 + NONCE_LEN + 6 + TAG_LEN);
            assert!(!obj.windows(6).any(|w| w == b"secret"));
            assert_eq!(c.open(&obj).unwrap(), b"secret");
            for at in [1, NONCE_LEN, NONCE_LEN + 2, obj.len() - 1] {
                let mut bad = obj.clone();
                bad[at] ^= 1;
                assert!(
                    matches!(c.open(&bad), Err(CryptoError::Authentication)),
                    "{cipher:?}: a changed byte {at} is caught"
                );
            }
            // Another key can't read it.
            assert!(encrypted(cipher).open(&obj).is_err());
            // Same plaintext, different ciphertext (random nonce).
            assert_ne!(c.seal(b"secret").unwrap(), obj);
            assert_eq!(c.open(&c.seal(b"").unwrap()).unwrap(), b"");
        }
    }

    /// A Vault reads only objects sealed with its own cipher: one sealed the other way
    /// (with the same key) isn't something it wrote.
    #[test]
    fn a_vault_reads_only_its_own_cipher() {
        let master = MasterKey::generate().unwrap();
        let xaes = Crypto::Encrypted(Keys::new(master.clone(), Cipher::Xaes256Gcm));
        let xchacha = Crypto::Encrypted(Keys::new(master, Cipher::XChaCha20Poly1305));
        let a = xaes.seal(b"data").unwrap();
        let b = xchacha.seal(b"data").unwrap();
        assert!(matches!(xchacha.open(&a), Err(CryptoError::Authentication)));
        assert!(matches!(xaes.open(&b), Err(CryptoError::Authentication)));
    }

    /// The C2SP XAES-256-GCM test vectors: the derived key, and the ciphertext.
    #[test]
    fn xaes_256_gcm_vectors() {
        let nonce: [u8; 24] = *b"ABCDEFGHIJKLMNOPQRSTUVWX";
        for (key, kx, aad, ct) in [
            (
                [0x01u8; 32],
                "c8612c9ed53fe43e8e005b828a1631a0bbcb6ab2f46514ec4f439fcfd0fa969b",
                &b""[..],
                "ce546ef63c9cc60765923609b33a9a1974e96e52daf2fcf7075e2271",
            ),
            (
                [0x03u8; 32],
                "e9c621d4cdd9b11b00a6427ad7e559aeedd66b3857646677748f8ca796cb3fd8",
                &b"c2sp.org/XAES-256-GCM"[..],
                "986ec1832593df5443a179437fd083bf3fdb41abd740a21f71eb769d",
            ),
        ] {
            let x = Xaes::new(&key);
            // Kx, through the same derivation `derive` makes ring's key from.
            use aes::cipher::BlockEncrypt;
            let mut blocks: [aes::Block; 2] = Default::default();
            for (i, b) in blocks.iter_mut().enumerate() {
                b[1] = i as u8 + 1;
                b[2] = b'X';
                b[4..].copy_from_slice(&nonce[..12]);
                for (v, k) in b.iter_mut().zip(x.k1) {
                    *v ^= k;
                }
            }
            x.aes.encrypt_blocks(&mut blocks);
            assert_eq!(
                hex::encode([blocks[0].as_slice(), blocks[1].as_slice()].concat()),
                kx
            );
            let mut data = b"XAES-256-GCM".to_vec();
            let tag = x.seal(&nonce, aad, &mut data).unwrap();
            data.extend_from_slice(&tag);
            assert_eq!(hex::encode(&data), ct);
            let n = x.open(&nonce, aad, &mut data).unwrap();
            assert_eq!(&data[..n], b"XAES-256-GCM");
        }
    }

    /// The same bytes as RustCrypto's XAES-256-GCM (an implementation of its own), for
    /// random keys, nonces, lengths and associated data.
    #[test]
    fn xaes_matches_rustcrypto() {
        use xaes_256_gcm::aead::{Aead, KeyInit, Payload};
        for len in [0usize, 1, 15, 16, 17, 255, 4096, 100_003] {
            let key: [u8; 32] = random().unwrap();
            let nonce: [u8; 24] = random().unwrap();
            let mut msg = vec![0u8; len];
            getrandom::fill(&mut msg).unwrap();
            let aad = &msg[..len.min(40)];
            let theirs = xaes_256_gcm::Xaes256Gcm::new(&key.into())
                .encrypt(&nonce.into(), Payload { msg: &msg, aad })
                .unwrap();
            let mut ours = msg.clone();
            let tag = Xaes::new(&key).seal(&nonce, aad, &mut ours).unwrap();
            ours.extend_from_slice(&tag);
            assert_eq!(ours, theirs, "{len} bytes");
        }
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
        for cipher in CIPHERS {
            let slot = KeySlot::wrap(&m, &with, "repo", KdfParams::TESTING, "x", cipher).unwrap();
            assert!(slot.unwrap(&with, "repo").is_ok());
            assert!(slot.unwrap("pw", "repo").is_err());
            let other = slot_input("pw", Some(&unlock_secret(b"another file")));
            assert!(slot.unwrap(&other, "repo").is_err());
        }
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
        for cipher in CIPHERS {
            let c = encrypted(cipher);
            // What someone with the drive could write in place of a sealed record.
            let forged = [&[TAG_PLAIN][..], b"{\"forged\":true}"].concat();
            assert!(matches!(c.open(&forged), Err(CryptoError::Authentication)));
            let sealed = c.seal(b"real").unwrap();
            assert_eq!(c.open(&sealed).unwrap(), b"real");
        }
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
        let sealed = encrypted(Cipher::Xaes256Gcm).seal(b"data").unwrap();
        assert!(matches!(
            c.open(&sealed),
            Err(CryptoError::UnexpectedlySealed)
        ));
    }

    #[test]
    fn keyed_ids() {
        let a = encrypted(Cipher::Xaes256Gcm);
        let b = encrypted(Cipher::Xaes256Gcm);
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
        for cipher in CIPHERS {
            let slot = KeySlot::wrap(
                &master,
                "correct horse",
                "repo1",
                KdfParams::TESTING,
                "test",
                cipher,
            )
            .unwrap();
            let json = serde_json::to_string(&slot).unwrap();
            // A slot of format 3 says how it's sealed; older slots look as they did.
            assert_eq!(
                json.contains("\"cipher\":\"xaes-256-gcm\""),
                cipher == Cipher::Xaes256Gcm
            );
            assert!(!json.contains("xchacha"));
            let slot: KeySlot = serde_json::from_str(&json).unwrap();
            assert_eq!(slot.cipher, cipher);
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
}
