//! A Mirror snapshot's manifest from a drive: zstd, a header, length-prefixed postcard
//! entries.
#![no_main]
use libfuzzer_sys::fuzz_target;
use std::io::Write;

fuzz_target!(|data: &[u8]| {
    // The reader takes a file: half the inputs as they are, half compressed first (so
    // the fuzzer reaches the records too).
    let path = std::env::temp_dir().join(format!("flux-fuzz-manifest-{}", std::process::id()));
    let bytes = if data.first().is_some_and(|b| b & 1 == 1) {
        zstd_compress(&data[1..])
    } else {
        data.to_vec()
    };
    std::fs::File::create(&path)
        .unwrap()
        .write_all(&bytes)
        .unwrap();
    if let Ok(r) = flux_mirror::manifest::ManifestReader::open(&path) {
        for e in r.take(10_000) {
            if e.is_err() {
                break;
            }
        }
    }
});

fn zstd_compress(data: &[u8]) -> Vec<u8> {
    let mut header = b"FLUXMAN\0\x01\0\0\0".to_vec();
    header.extend_from_slice(data);
    // A raw zstd frame of stored blocks: compressing needs no zstd library here.
    let mut out = vec![0x28, 0xb5, 0x2f, 0xfd, 0x00, 0x58];
    for (i, chunk) in header.chunks(1 << 16).enumerate() {
        let last = (i + 1) * (1 << 16) >= header.len();
        let h = ((chunk.len() as u32) << 3) | u32::from(last);
        out.extend_from_slice(&h.to_le_bytes()[..3]);
        out.extend_from_slice(chunk);
    }
    if header.is_empty() {
        out.extend_from_slice(&[1, 0, 0]);
    }
    out
}
