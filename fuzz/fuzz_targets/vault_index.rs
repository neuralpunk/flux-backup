//! An index file from a drive: a sealed postcard list of packs and their blobs.
#![no_main]
use flux_vault::crypto::Crypto;
use flux_vault::format::{IndexFile, PackEntry};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let plain = Crypto::Plain {
        id_key: [7; 32],
        chunk_seed: 1,
    };
    if let Ok(raw) = plain.open(data) {
        if let Ok(f) = postcard::from_bytes::<IndexFile>(&raw) {
            let mut idx = flux_vault::pack::Index::default();
            for p in &f.packs {
                idx.add_pack(p);
            }
        }
        let _ = postcard::from_bytes::<Vec<PackEntry>>(&raw);
    }
});
