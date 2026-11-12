//! Packs written past the page cache (`O_DIRECT`, on a disk's filesystem: the build
//! directory, not /tmp, which is often tmpfs) read back exactly, whatever their length.

use flux_vault::backend::{Backend, LocalBackend};

#[test]
fn direct_pack_writes_read_back() {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let b = LocalBackend::new(dir.path());
    let mut data = vec![0u8; (16 << 20) + 5];
    getrandom::fill(&mut data).unwrap();
    for (i, len) in [0, 1, 4095, 4096, 4097, 65_536, 1 << 20, data.len()]
        .into_iter()
        .enumerate()
    {
        let path = format!("data/0{i}/pack{i}");
        b.write_pack(&path, &data[..len]).unwrap();
        assert_eq!(b.read(&path).unwrap(), &data[..len], "{len} bytes");
    }
    b.flush_packs().unwrap();
    // No temporaries left behind.
    let left: Vec<_> = walk(dir.path())
        .into_iter()
        .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with(".tmp"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

fn walk(d: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(d).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}
