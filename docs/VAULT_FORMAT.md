# Vault repository format, version 3

Format 3 (Flux 1.3) seals every object and key slot with **XAES-256-GCM** (AES-256-GCM
under a key derived for each nonce; below) instead of XChaCha20-Poly1305: AES runs in the
processor's hardware, about five times as fast on one core. Its trees and records are
format 2's. New Vaults are format 3. A format 1 or 2 Vault keeps its cipher: changing it
would mean sealing every object again, so there is no upgrade to format 3 (copy the
backups into a new Vault for that).

Format 2 (Flux 1.0) keeps holes in sparse files and adds `tree_format` to snapshot
records. Format 1 (Flux 0.1–0.3) is described alongside: 1.0 reads it, and keeps writing
format 1 to a format 1 Vault until `flux vault upgrade` moves it to format 2 (so computers
still on an older Flux can keep using a shared Vault). The upgrade changes only `config`;
the snapshots already stored stay as they were and stay readable. Flux refuses a format
newer than it knows, with a message saying to update.

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
├── profiles/<key>         the settings of each backup using the Vault (sealed JSON
│                          {profile, host, user, text}; <key> is 32 hex digits of the
│                          BLAKE3 of "host\nuser\nprofile"); see FORMATS.md
├── parity/…               repair data: one parity file per file above, at parity/<its path>
│                          (optional, Flux 1.1; see "Repair data")
├── locks/<lock-id>        lock files, one per holder (every backend)
└── lock                   flock(2) target (local backend)
```

Every object but `config` and the key slots starts with a tag byte: `0` plain (an
unencrypted Vault), `1` sealed with XChaCha20-Poly1305 (formats 1 and 2), `2` sealed with
XAES-256-GCM (format 3). An encrypted Vault never writes a plain object, and readers refuse
one there, or one sealed with the other cipher: it could have been put in place of a
sealed one by someone with the drive. All ids are lowercase hex. Files are written to a temporary name and renamed into place,
so readers never see partial files. Nothing is modified in place except snapshot records
(rename-replaced when a snapshot is named or pinned).

## Keys and encryption

The **master key** is 72 random bytes:

| bytes | use |
|---|---|
| 0–31 | `enc_key`: the key every stored object is sealed with (XAES-256-GCM in format 3, XChaCha20-Poly1305 before) |
| 32–63 | `id_key`: BLAKE3 key for blob ids |
| 64–71 | `chunk_seed` (u64 LE): seeds the chunker's gear table |

A **key slot** (`keys/<16 hex>`) is JSON:

```json
{ "kdf": "argon2id", "m_kib": 65536, "t": 3, "p": 4,
  "salt": "<16 bytes, base64>", "nonce": "<24 bytes, base64>",
  "ciphertext": "<72 + 16 bytes, base64>", "created": "<RFC 3339>", "label": "…",
  "unlock_file": true, "cipher": "xaes-256-gcm" }
```

`KEK = Argon2id(input, salt, m_kib, t, p, 32 bytes)` and
`ciphertext = AEAD(KEK, nonce, master, aad = "flux-vault-key-v1:" + repo id)`, where AEAD
is the slot's `cipher`: `"xaes-256-gcm"` (format 3), or XChaCha20-Poly1305 when `cipher`
is left out (formats 1 and 2, exactly as before 1.3).
`input` is the password, or, for a slot with `"unlock_file": true`, the password, a NUL
byte and the lowercase hex of `BLAKE3-derive_key("flux-vault unlock file v1", file)`, where
`file` is the whole content of an unlock file (any file; Flux makes them of 64 random
bytes). `unlock_file` is left out when false, so other slots are exactly as before 1.0.
A slot that needs a file can't be opened by an older Flux.

Opening tries every slot. Adding or changing a password adds or removes a slot, and never
re-encrypts data. Each slot has its own Argon2id cost: the presets are *standard* (64 MiB,
3 passes, 4 lanes), *strong* (256 MiB, 4, 4) and *maximum* (1 GiB, 6, 4), and any of
8–4096 MiB, 1–20 passes and 1–16 lanes can be used. Strengthening a password, or adding or
removing its unlock file, writes a new slot for the same master key and then removes the
old one.

Every **stored object** (blob, pack header, index file, snapshot record) is:

```
format 3:     0x02 ‖ nonce (24 random bytes) ‖ XAES-256-GCM(enc_key, nonce, payload) ‖ tag (16)
formats 1–2:  0x01 ‖ nonce (24 random bytes) ‖ XChaCha20-Poly1305(enc_key, nonce, payload) ‖ tag (16)
unencrypted:  0x00 ‖ payload
```

Random 192-bit nonces make nonce reuse a non-issue at any realistic volume, with either
cipher. AES-256-GCM's own nonce is 96 bits, too short for random nonces over a Vault's
life (about 2³² objects per key is the usual limit); XAES-256-GCM keeps 192.

### XAES-256-GCM

As specified by C2SP ([c2sp.org/XAES-256-GCM](https://c2sp.org/XAES-256-GCM)): from the
key `K` and the 24-byte nonce `N`,

```
L  = AES-256_K(0¹²⁸)
K1 = L << 1, XOR 0x87 into the last byte if L's top bit was 1     (CMAC subkey)
M1 = 00 01 58 00 ‖ N[0..12]          M2 = 00 02 58 00 ‖ N[0..12]
Kx = AES-256_K(M1 ⊕ K1) ‖ AES-256_K(M2 ⊕ K1)
ciphertext ‖ tag = AES-256-GCM(Kx, nonce = N[12..24], plaintext, aad)
```

Every object gets its own GCM key. The spec's test vectors are in Flux's tests, which also
check it byte for byte against RustCrypto's `xaes-256-gcm` (what `flux-vault-recover`
decrypts with); Flux's GCM is ring's.

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

With *hide file sizes* on (encrypted Vaults; the default for profiles made from 1.2), random bytes between the last blob and the
header object bring the whole pack to its **Padmé** length (Nikitin et al., 2019): with
`E = ⌊log₂ L⌋` and `S = ⌊log₂ E⌋ + 1`, the length `L` is rounded up to a multiple of
`2^(E−S)`, which leaves about log₂ log₂ L bits of the length and costs at most about 12%
(3% for a full pack). Readers find the header from the end and blobs by their offsets, so
padded packs need nothing new from them; any Flux reads them.

## Index

`index/<16 hex>` is an object whose payload is postcard
`{ packs: [{ pack, blobs: [entry…] }], supersedes: [index ids] }`. Loading reads every
index file, ignoring those superseded by another. A prune supersedes and removes every index file
there, including ones a newer file already supersedes (left by a prune that was cut
short): such a leftover must not outlive the file that names it. This is how prune swaps in a compacted
index without a window where blobs are unreachable.

## Trees and snapshots

A **tree** blob is a postcard-encoded, name-sorted list of nodes. Each node has the full
metadata of an [`Entry`](../crates/flux-fs/src/entry.rs) (kind, mode, owner, size, mtime,
xattrs/ACLs, symlink target, device number, source inode for hard links), plus:

* files: `content`, the ordered list of data blob ids
* directories: `subtree`, the tree blob id
* format 2 only, last: `holes`, a list of `{ offset, len }` (u64 each), the runs of a
  sparse file that hold no data, sorted, not overlapping, not empty, inside the file

A file is its data (the content blobs, concatenated) laid into the ranges between its
holes, in order; the holes read as zeros. With no holes, the data is the whole file.
Format 1 nodes have no `holes` field and always store the whole file as data. The two
encodings differ, so a tree written in one format never has the id of a tree in the other
(except the empty tree, which reads the same in both).

The backup finds holes with `SEEK_DATA`/`SEEK_HOLE` (only for files with fewer blocks
allocated than their size), chunks only the data, and records the holes. Restores write
the data at its offsets and leave the holes unwritten, so the restored file is sparse too.

The root tree represents `/`. Unchanged directories produce identical tree blobs, so they
cost nothing in later snapshots. A snapshot whose root tree id equals its parent's changed
nothing.

A **snapshot record** (`snapshots/<16 hex>`) is an object whose payload is JSON
`{ "meta": SnapshotMeta, "tree": "<root tree id>", "parent": "<snapshot id>|null",
"tree_format": 2 }`. It contains the profile, host, user, times, status, name, pin,
statistics and per-file errors. `tree_format` says how this snapshot's trees are encoded;
it is left out when 1, so format 1 records look exactly as Flux 0.3 wrote them, and a
missing `tree_format` means 1. `meta.format` is the same number. Every tree of a snapshot
is written in its tree format (a backup writes all of its trees).

## Concurrency and crash safety

* Backup and restore take a **shared** lock. Prune, key changes, `check` and the format
  upgrade take an **exclusive** one.
* A lock is a JSON file in `locks/` (`{ host, user, pid, exclusive, time, machine }`), written to
  a temporary name and renamed, checked again after writing (so two clients racing for
  an exclusive lock can't both win), refreshed every 5 minutes, and removed on release.
  Every backend writes and respects them, so a Vault reached both on a local disk and
  over SFTP (a NAS used from two computers) has one lock. Locally Flux also takes
  `flock(2)` on `lock`, as before 1.0. A lock is stale, and ignored, when it hasn't been
  refreshed for 30 minutes, or when it was taken on this computer by a process that no
  longer runs. "This computer" is `machine`: the first 12 bytes, in hex, of
  `BLAKE3-derive_key("flux-backup 1.0 lock machine token", machine-id)`, where
  `machine-id` is the trimmed content of `/etc/machine-id`. The host name alone doesn't
  say so (two computers can share one), and a lock without `machine` (written before
  1.0, or where there is no machine id) is stale only by its age. `flux unlock` removes
  stale locks (`--force`: all of them).
* Backup order: packs → index → snapshot record. A crash leaves packs no index references.
  Prune deletes them (it holds the exclusive lock, so no backup can be mid-way). A pack
  that can't be written ends the backup: no index, no record. Before the index is written
  the backup checks that every index file it started from and every pack it wrote is
  still there, and stops if not: another computer that took its lock for stale (this one
  slept, or the clocks disagree) may have pruned meanwhile.
* Prune reads every snapshot record before it deletes anything, and deletes nothing if
  one can't be read (its data would look unused).
* Prune order: delete snapshot records → write repacked packs → write the new index
  (superseding all old ones) → delete old index files → delete unused packs. After a crash,
  every snapshot still present is fully readable.
* A pack is repacked only if its bytes are its name (its BLAKE3). Its blobs are copied
  sealed, as they lie, so a damaged pack would otherwise pass its damage on under a new
  name, with new repair data made from the damage. One that fails is rebuilt from its
  repair data first; one that can't be stays as it is, in the new index, with its parity
  file, and the prune goes on with the rest.

## Repair data

Since Flux 1.1 a Vault can keep **repair data**: for each file it stores, a *parity file*
that can rebuild parts of that file after a drive has damaged them. The parity file of
`<path>` is `parity/<path>`: `parity/data/<xx>/<pack-id>`, `parity/index/<id>`,
`parity/snapshots/<id>`, `parity/keys/<slot-id>`, `parity/config`.

It is an addition, not a new format: nothing else in the Vault changes, a Vault with
`parity/` is still format 2 (or 1), and a Flux that knows nothing of repair data reads and
writes it as before. What such a Flux writes has no repair data until a newer one adds
it, and what it deletes leaves parity files behind, which the newer one removes (when it
frees space). Whether repair data is written is a setting of the profile
(`[vault] parity`), not of the Vault.

### The code

The file is cut into **blocks** of `B` = 4096 bytes (the last may be shorter; it counts as
padded with zeros). With `N` blocks, the blocks are dealt to `G` **stripes** in turn:
block `i` is column `c = i div G` of stripe `g = i mod G`. So a run of damaged blocks is
spread over all the stripes. Each stripe has `m` parity blocks, and `k + m ≤ 256` for the
number `k` of data blocks in the largest stripe (`k = ⌈N / G⌉`).

Parity block `j` (0 ≤ j < m) of a stripe with data blocks `d₀ … d₍ₖ₋₁₎` is, byte by byte,

```
p_j = Σ  C[j][c] · d_c          C[j][c] = 1 / ((255 − j) ⊕ c)
     c<k
```

in GF(2⁸): bytes are polynomials over GF(2) modulo x⁸ + x⁴ + x³ + x² + 1 (0x11d),
addition is XOR, `1 / a` is the multiplicative inverse. `C` is a Cauchy matrix (its
points `255 − j` and `c` never coincide, as `c + j ≤ k + m − 2 < 255`), so every square
submatrix is invertible: any `e ≤ m` lost data blocks of a stripe are the solution of the
`e` equations given by any `e` intact parity blocks (a systematic Reed–Solomon code; no
code with `m` parity blocks rebuilds more).

Flux writes, for a setting of `p` percent (1–50): `G = ⌈N / ⌊25500 / (100 + p)⌋⌉` and
`m = max(2, ⌈k·p / 100⌉)`: stripes as large as the field allows, about `p` percent of
parity, and never fewer than two parity blocks. A 16 MiB pack at 5% has 4096 blocks in 17
stripes of up to 241, with 13 parity blocks each. Readers take `B`, `G` and `m` from the
file, not from this rule.

### The parity file

```
head ‖ parity blocks ‖ head ‖ head_len (u64 LE)
```

The **head** (stored twice, identical) is, with integers little-endian:

| bytes | |
|---|---|
| 8 | `FLUXPAR1` |
| 4 | `B`, the block size (4096) |
| 4 | `G`, the number of stripes (0 for an empty file) |
| 2 | `m`, parity blocks per stripe |
| 2 | the percentage it was written with |
| 8 | the length of the protected file |
| 32 | BLAKE3 of the protected file |
| 16 × N | for each data block, the first 16 bytes of its BLAKE3 (of its actual bytes: the last block unpadded) |
| 16 × G·m | the same for each parity block, in the order they are stored |
| 32 | BLAKE3 of all of the head before this field |

The **parity blocks** follow, `B` bytes each, row by row: block `j` of stripe `g` is the
`(j·G + g)`-th.

### Repairing

A block whose hash doesn't match (or that can't be read) is lost; so is a parity block.
A stripe with more lost data blocks than intact parity blocks can't be rebuilt, and then
the file isn't touched. Otherwise the lost blocks are solved for, and the result must
have the BLAKE3 in the head **and** pass the file's own check before it replaces the
file: a pack's BLAKE3 is its name; an index file or snapshot record must open (decrypt
and authenticate, in an encrypted Vault). The head is read from its first copy, or from
the second (found by `head_len` at the end) if the first fails its checksum.

Only something that fails its own check is repaired. Parity that doesn't match a file
that passes its check is stale (a snapshot record rewritten by a Flux that keeps no
repair data) and is written again, not used. A key slot never changes, so one that
differs from its parity is repaired; `config` changes only when the format is upgraded,
and one that still parses and differs from its parity's version in nothing but the
format is taken as upgraded.

What this repairs: up to `m` damaged blocks in every stripe, so any `m` blocks of a
file wherever they are, any one run of up to `m·G` blocks (about `p` percent of the
file), and usually much more scattered damage. What it doesn't: a file that is missing,
or damaged beyond that. A copy in another place is for those.

## Healing

A backup stores a blob only if no index file names it, and takes an unchanged file's
blobs from the snapshot before without reading the file. A blob that is named but can't
be read (its pack is damaged past its repair data, has none, or is gone) would therefore
never be stored again. A repairing check (`flux check --repair`) deals with what repair
data doesn't, under the exclusive lock:

* **Lost packs** (missing, or failing their check and not rebuilt): each of their blobs
  that still decodes and has its id is copied, sealed as it is, to a new pack; the others
  are left out. A new index file names everything that remains and supersedes every
  other; then the old index files and the lost packs (with their parity files) are
  removed, in a prune's order. The next backup finds those blobs unlisted, reads the
  files that had them and stores them: under the same ids, so older snapshots whose
  files are unchanged are whole again. A check lists what is still missing, by snapshot
  and file.
* **An index file that doesn't open:** a new index file is written from every pack's own
  header (a pack whose header can't be read is left out and reported), superseding all
  the others. It names every blob in every pack, including ones no snapshot uses; the
  next prune drops those.

## Copies (replicas)

A copy of a Vault is the same repository in another place: `config`, `keys/`, `index/`,
`snapshots/`, `data/` and `parity/` copied as files, never decrypted. Each replication copies, under
the source's shared lock and the copy's exclusive lock: packs missing from the copy (by
name and size; a pack's name is its hash, so a pack never changes), then index files,
then snapshot records that are missing or differ (naming, pinning, notes and tags rewrite
them), keys, and `config` last. The index files and snapshot records to copy are listed
before the packs are, so a backup finishing on the source meanwhile is left whole for the
next replication. Then it removes what the source no longer has: snapshot records, index
files, packs, keys, in that order, after reading the source's `config` again: if it is
gone or different (the drive was pulled out), nothing is removed.

Damage isn't copied. A pack is read whole to be copied, and checked against its name: a
damaged one never replaces a pack of that name in the copy (whatever its size), and where
the copy has none it is copied with its parity file, which may rebuild it there. Packs
(and their parity files) that the source no longer has are removed from the copy only if
at least one index file the copy held before this replication is gone from the source:
a prune, the only thing that removes packs, always writes a new index file and removes
the old ones. If all of them are still there, the missing packs were lost, the index
still names them, and the copy keeps its own. A copy with no index file yet (a first
replication cut short) has nothing that names its packs, and follows the source.

An interrupted replication leaves a copy
that is still readable, and the next one finishes it. A place that holds a different
Vault (another repository id) is never written to.

## Threat model (summary)

The whole program's threat model, including SSH, system backups and restores, is in
[THREAT_MODEL.md](THREAT_MODEL.md).

Protects the **confidentiality and integrity of contents and metadata** against someone
who can read or modify the backup target: a stolen drive, an untrusted NAS or server.
It does not hide the number and approximate sizes of packs (exact sizes, unless *hide
file sizes* pads them), the number of blobs in a pack (the header's length), the number of
snapshots, or when they were written. It does not stop an attacker with write access from deleting
data; integrity checks detect such changes, but nothing can prevent them. Security rests on
the password: Argon2id (64 MiB by default, up to 1 GiB) makes guessing costly, but not
impossible for weak passwords; an unlock file adds a second thing an attacker needs.
