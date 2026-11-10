# Fuzzing

Fuzz targets for everything Flux reads that it didn't write itself: what's on a backup
drive or server (which someone else may have changed), files people import, and requests
to the administrator helper (which runs as root).

| Target | Reads |
|---|---|
| `vault_pack` | a Vault pack: its sealed header and the blobs it points to (unencrypted and encrypted Vaults) |
| `vault_index` | a Vault index file |
| `vault_tree` | a Vault tree blob (formats 1 and 2) and the sparse-file arithmetic on it |
| `vault_json` | a Vault's `config`, key slots and snapshot records |
| `mirror_manifest` | a Mirror snapshot's `manifest.zst` |
| `snapshot_meta` | a snapshot's `meta.json` |
| `profile` | a profile file, an exported profile, the settings kept with backups |
| `helper_protocol` | a request line to the helper |

The layout is cargo-fuzz's, so `cargo fuzz run vault_tree` works. Without cargo-fuzz:

```sh
FLUX_FUZZ_TOOLCHAIN=nightly fuzz/build.sh   # libFuzzer + AddressSanitizer, overflow checks on
fuzz/run.sh 600                             # every target for 10 minutes, in parallel
```

`corpus/<target>/seed-*` are the starting inputs (real files from the fixtures and an
unencrypted Vault); what the fuzzer adds to the corpus isn't kept in git. A crash lands
in `artifacts/<target>/`; once fixed, copy it to `regressions/<target>/`, where a unit
test in the crate that reads that format replays it on every `cargo test`.

Found so far:

- `vault_tree`: a tree whose holes are out of order made restoring a sparse file subtract
  past zero (`format.rs`, `span_at`). In a release build that wraps silently and would
  have restored wrong data. Trees whose holes aren't in order, non-empty and inside the
  file are now refused as damaged (`holes_are_sound`), in Flux and in
  `flux-vault-recover`.
