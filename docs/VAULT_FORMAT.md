# Vault repository format, version 1

A Vault is a content-addressed, deduplicating, compressed and (optionally) encrypted
store of snapshots. One Vault lives at `<target>/flux-vault/` and may hold snapshots from
any number of profiles, hosts and users. Identical data is stored once across all of
them.

## Layout

```
flux-vault/
├── config                 plaintext JSON: format, repo id, encryption on/off, chunker sizes
├── keys/<slot-id>         key slots: the master key wrapped by a password (encrypted repos)
├── data/<xx>/<pack-id>    pack files; <xx> = first two hex digits of the pack id
├── index/<index-id>       which blob is in which pack, where
├── snapshots/<snap-id>    snapshot records
└── lock                   flock(2) target (local backend)
```

All ids are lowercase hex. Files are written to a temporary name and renamed into place,
so readers never see partial files. Nothing is modified in place except snapshot records
(rename-replaced when a snapshot is named or pinned).

## Keys and encryption

The **master key** is 72 random bytes:

| bytes | use |
|---|---|
| 0–31 | `enc_key`: XChaCha20-Poly1305 key for every stored object |
| 32–63 | `id_key`: BLAKE3 key for blob ids |
| 64–71 | `chunk_seed` (u64 LE): seeds the chunker's gear table |

A **key slot** (`keys/<16 hex>`) is JSON:

```json
{ "kdf": "argon2id", "m_kib": 65536, "t": 3, "p": 4,
  "salt": "<16 bytes, base64>", "nonce": "<24 bytes, base64>",
  "ciphertext": "<72 + 16 bytes, base64>", "created": "<RFC 3339>", "label": "…" }
```

`KEK = Argon2id(password, salt, m_kib, t, p, 32 bytes)` and
`ciphertext = XChaCha20-Poly1305(KEK, nonce, master, aad = "flux-vault-key-v1:" + repo id)`.
Opening tries every slot. Adding or changing a password adds or removes a slot, and never
re-encrypts data.

Every **stored object** (blob, pack header, index file, snapshot record) is:

```
encrypted:    0x01 ‖ nonce (24 random bytes) ‖ XChaCha20-Poly1305(enc_key, nonce, payload)
unencrypted:  0x00 ‖ payload
```

Random 192-bit nonces make nonce reuse a non-issue at any realistic volume.

In an **unencrypted** Vault, `config` carries `id_key` and `chunk_seed` in the clear
(`"plain_keys"`), so that ids and chunking work the same way.

## Blobs

A blob is a file chunk (`data`) or a directory listing (`tree`).

* **id** = `BLAKE3-keyed(id_key, plaintext)`. Keyed, so ids don't reveal whether the
  repository holds a known file.
* **payload** = `0x00 ‖ plaintext` (stored raw) or `0x01 ‖ zstd(plaintext)`. Zstd is kept
  only if it saves space.
* Readers verify the id after decrypting and decompressing. Corruption or tampering is
  detected.

### Chunking

FastCDC (2020) with normalization level 1, min 512 KiB, average 1 MiB, max 8 MiB, and the
gear table seeded with `chunk_seed`. A secret seed keeps chunk boundaries from
fingerprinting the content. Files smaller than the minimum are a single chunk.

## Packs

```
blob₁ ‖ blob₂ ‖ … ‖ blobₙ ‖ header object ‖ header_len (u32 LE)
```

The header object (encrypted like any object) holds a postcard-encoded list of
`{ kind: data|tree, id, offset, length, raw_length }`. **pack id** = BLAKE3 (unkeyed) of
the whole file, so `check --read-data` can verify a pack without any key. Packs hold either
data or tree blobs, never both, and are closed at ~16 MiB.

## Index

`index/<16 hex>` is an object whose payload is postcard
`{ packs: [{ pack, blobs: [entry…] }], supersedes: [index ids] }`. Loading reads every
index file, ignoring those superseded by another. This is how prune swaps in a compacted
index without a window where blobs are unreachable.

## Trees and snapshots

A **tree** blob is a postcard-encoded, name-sorted list of nodes. Each node has the full
metadata of an [`Entry`](../crates/flux-fs/src/entry.rs) (kind, mode, owner, size, mtime,
xattrs/ACLs, symlink target, device number, source inode for hard links), plus:

* files: `content`, the ordered list of data blob ids
* directories: `subtree`, the tree blob id

The root tree represents `/`. Unchanged directories produce identical tree blobs, so they
cost nothing in later snapshots. A snapshot whose root tree id equals its parent's changed
nothing.

A **snapshot record** (`snapshots/<16 hex>`) is an object whose payload is JSON
`{ "meta": SnapshotMeta, "tree": "<root tree id>", "parent": "<snapshot id>|null" }`. It
contains the profile, host, user, times, status, name, pin, statistics and per-file errors.

## Concurrency and crash safety

* Backup and restore take a **shared** lock. Prune, key changes and `check` take an
  **exclusive** one.
* Backup order: packs → index → snapshot record. A crash leaves packs no index references.
  Prune deletes them (it holds the exclusive lock, so no backup can be mid-way).
* Prune order: delete snapshot records → write repacked packs → write the new index
  (superseding all old ones) → delete old index files → delete unused packs. After a crash,
  every snapshot still present is fully readable.

## Threat model (summary)

The whole program's threat model, including SSH, system backups and restores, is in
[THREAT_MODEL.md](THREAT_MODEL.md).

Protects the **confidentiality and integrity of contents and metadata** against someone
who can read or modify the backup target: a stolen drive, an untrusted NAS or server.
It does not hide the number and approximate sizes of packs, the number of snapshots, or
when they were written. It does not stop an attacker with write access from deleting
data; integrity checks detect such changes, but nothing can prevent them. Security rests on
the password: Argon2id with 64 MiB of memory makes guessing costly, but not impossible
for weak passwords.
