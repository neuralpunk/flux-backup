# Math in Flux Backup: what's there, and what could be

Notes for later review (2026-11-09), from a question asked during the 1.1 work: is there
any intense math in Flux, and would there ever be in a backup app like this?

**Short answer:** there's real math, but nearly all of it lives in libraries Flux uses or
is light in Flux's own code. The deep pieces worth writing ourselves are about
*guarantees*: how sure a restore test makes us, and repairing damage instead of only
detecting it.

## What Flux leans on (libraries, not our code)

| Area | What | Where |
|---|---|---|
| Cryptography | XChaCha20-Poly1305 (authenticated encryption), Argon2id (password hashing), BLAKE3 (checksums, chunk ids) | flux-vault, via audited crates |
| Content-defined chunking | FastCDC: a rolling "gear" hash cuts files where the content says, so an edit in the middle of a file only changes the chunks around it, and the rest deduplicate | flux-vault (`fastcdc` crate) |
| Compression | zstd | flux-vault, Borg |

This is the deepest math in the app, and it's rightly left to well-reviewed libraries.

One number worth knowing: chunk ids are 256-bit BLAKE3 hashes. By the birthday bound, the
chance of two different chunks ever sharing an id is about n² / 2²⁵⁷; even at 2⁶⁴
chunks (far beyond any backup) that's around 2⁻¹²⁹. Deduplication by hash is safe.

## What Flux does itself (light)

- **Entropy** (`crates/flux-runner/src/unusual.rs`): Shannon entropy of a file's bytes,
  H = −Σ p(b)·log₂ p(b), in bits per byte. Random or encrypted data is close to 8;
  text is well under 5. A text-like file that suddenly reads ≥ 7.5 is one of the signals
  of ransomware.
- **Space forecast** (`crates/flux-service/src/space.rs`): how fast free space is
  shrinking per day over the recorded history, and from that "full in about 3 months".
- **Rolling restore tests** (`crates/flux-runner/src/restore_test.rs`): each file gets a
  position on a 64-bit circle (FNV-1a of its path); each test moves a cursor forward by
  the share of the 30-day window that has passed and tests the files in between, so every
  file is tested once per window whatever the timing of the tests.
- **Retention and calendars**: keeping 7 daily, 4 weekly, 12 monthly…, and reading
  systemd calendar expressions. Careful date logic rather than math.

## Where real math could earn its place

### 1. Restore tests with a stated confidence (small, very useful)

**Done in 1.1 (Phase 93):** `mode = "confidence"`, *Enough to be sure* in the app; the
arithmetic, exact for any number of files, is in `crates/flux-runner/src/sample.rs`.

Today a test checks "10 random files". Instead, choose the sample so the result means
something: *"if more than 1% of the files were damaged, this test would have caught it
with 99% certainty."*

If a fraction *d* of the files is damaged and *n* files are picked at random, the chance
of missing every damaged one is about (1 − d)ⁿ (exactly, a hypergeometric probability for
a finite number of files). For it to be at most 1 − c:

> n ≥ ln(1 − c) / ln(1 − d)

For d = 1% and c = 99%: n ≥ ln(0.01) / ln(0.99) ≈ **459 files**. The striking part: this
hardly depends on how big the backup is. 459 files say as much about a backup of 10,000
files as about one of 10 million. (For a small backup, the exact hypergeometric version
needs fewer.)

A setting could read: *Catch damage to 1% of files with 99% certainty*, and the report:
*"Tested 459 files: if 1% or more were damaged, this test would have found it (99%)."*

### 2. Repair, not just detection (the big one)

**Done in 1.1 (Phase 94):** `[vault] parity`, `flux check --repair`. Not format 3 after
all: the parity lives in files of its own (`parity/…`), so the Vault's format is
unchanged and older versions read it as before. The code is our own (a Cauchy
Reed–Solomon over GF(2⁸), `crates/flux-vault/src/gf256.rs` and `parity.rs`) rather than
`reed-solomon-simd`, so that the format can be written down exactly
(docs/VAULT_FORMAT.md) and doesn't depend on a library's construction.

A Vault detects any damage (every chunk is authenticated) but can't repair it: a damaged
pack loses the chunks in it. **Reed–Solomon** erasure coding, the math behind RAID-6 and
PAR2, adds parity: for every *k* blocks of data, *m* extra blocks, computed in a finite
field (GF(2⁸)), let any *k* of the *k + m* rebuild the rest. With a few percent of extra
space, a pack with damaged sectors can be rebuilt.

This is the one genuinely intense piece, and the most valuable for a backup tool: it
turns "your backup is damaged" into "your backup was damaged and repaired". Libraries
exist (e.g. `reed-solomon-simd`); the work is in the format (where parity lives, how
it's checked and used), which would be a Vault format 3.

### 3. A better space forecast

Fit the free-space history robustly (Theil–Sen, or a regression that ignores outliers),
so the day you deleted 200 GB doesn't skew the trend, and show an uncertainty band:
*"full in 2 to 4 months"* rather than a single date.

### 4. Unusual-change detection that learns each backup's normal

**Done in 1.1 (Phase 95), as Flux Sentry** (`crates/flux-runner/src/sentry.rs`): the
moving average and the median absolute deviation as described, on the logarithm of the
share of files changed, with a clamped (Huber) step, and a memory of amounts of change
the backup has reached before.

Today it uses fixed thresholds. Instead, compare each backup with its own history: an
exponentially weighted moving average of how many files change, and how far today is
from it (in median absolute deviations). Fewer false alarms on busy backups, earlier
alerts on quiet ones.

### 5. Smarter transfers over SSH

rsync-style rolling checksums (a weak rolling sum plus a strong hash per block), so a
large file changed in a few places sends only the changed blocks to a server. Borg and
rsync already do this; Flux's own SFTP transfer for plain-folder (Mirror) backups sends
changed files whole.

## Suggested order

1 is small and makes restore tests a real guarantee; 2 is the largest and most valuable.
Both are features, so they fit 1.2 better than the 1.1 refinement release. 3 and 4 are
modest improvements to existing features; 5 matters only for Mirror backups to servers
with large, slightly changing files.
