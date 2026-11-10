//! A tree blob: postcard nodes, in tree formats 1 and 2, and the sparse-file arithmetic
//! done on what it says.
#![no_main]
use flux_vault::format::Tree;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    for format in [1, 2] {
        if let Ok(t) = Tree::decode(data, format) {
            for n in t.nodes.iter().take(256) {
                let c = n.file_content();
                let _ = c.hole_bytes();
                let runs = c.data_runs(n.size);
                let _ = flux_vault::format::Content::holes_between(&runs, n.size);
                let _ = c.span_at(0, n.size);
                let _ = c.span_at(n.size / 2, n.size);
                let _ = n.to_entry(n.name.clone());
            }
        }
    }
});
