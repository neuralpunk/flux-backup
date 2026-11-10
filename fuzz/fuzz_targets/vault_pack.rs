//! A pack file from a drive: its header (sealed postcard entries at the end) and the blobs
//! it points to, in an unencrypted and an encrypted Vault.
#![no_main]
use flux_vault::crypto::{Crypto, MasterKey};
use libfuzzer_sys::fuzz_target;
use std::sync::OnceLock;

fn sealed() -> &'static Crypto {
    static C: OnceLock<Crypto> = OnceLock::new();
    C.get_or_init(|| Crypto::Encrypted(MasterKey::generate().unwrap()))
}

fuzz_target!(|data: &[u8]| {
    let plain = Crypto::Plain {
        id_key: [7; 32],
        chunk_seed: 1,
    };
    for crypto in [&plain, sealed()] {
        if let Ok(entries) = flux_vault::pack::split_pack(crypto, "00", data) {
            for e in entries.iter().take(64) {
                let start = e.offset as usize;
                let end = start.saturating_add(e.length as usize);
                if let Some(blob) = data.get(start..end) {
                    let _ = flux_vault::pack::decode_blob(crypto, blob, e);
                }
            }
        }
    }
});
