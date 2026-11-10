//! A snapshot's meta.json from a drive (Mirror, rsync), and a Borg archive's comment.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(m) = serde_json::from_slice::<flux_fs::snapshot::SnapshotMeta>(data) {
        let _ = m.check_format();
        let _ = flux_fs::snapshot::clean_note(m.note.as_deref());
        let _ = flux_fs::snapshot::clean_tags(m.tags.iter());
    }
});
