//! A Vault's JSON from a drive: its config, key slots and snapshot records (and the
//! settings kept in it).
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = serde_json::from_slice::<flux_vault::format::Config>(data);
    if let Ok(slot) = serde_json::from_slice::<flux_vault::crypto::KeySlot>(data) {
        // Unwrapping costs a full Argon2id: only with tiny parameters.
        if slot.params.m_kib <= 64 && slot.params.t <= 1 && slot.params.p <= 1 {
            let _ = slot.unwrap("password", "repo");
        }
    }
    if let Ok(r) = serde_json::from_slice::<flux_vault::format::SnapshotRecord>(data) {
        let _ = r.meta.check_format();
    }
});
