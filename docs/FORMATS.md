# Flux Backup's formats

Everything a backup depends on, written down, with its version. Flux 1.0 freezes these:
within 1.x, Flux reads every older version of each, refuses a newer one with a message
saying so (update Flux), and changes a format only with a new version number and a way to
move to it. The fixtures in `crates/flux-runner/tests/fixtures/` (profiles and backups
written by Flux 0.1, 0.2, 0.3 and 1.0) are read by every build.

| What | Where | Version | Newer one |
|---|---|---|---|
| [Vault](#vault) | `<drive>/flux-vault/` | 2 (1 still read) | refused: "format N is newer than this Flux supports" |
| [Mirror repository](#mirror) | `<drive>/flux-backup/<host>/<user>/<profile>/` | 1 (`flux-repo.toml`) | refused |
| [Mirror manifest](#mirror-manifest) | `snapshots/<id>/manifest.zst` | 1 | refused: "written by a newer Flux" |
| [Snapshot `meta.json`](#snapshot-metajson) | Mirror and rsync snapshots | 1 (`format`) | refused |
| [rsync folders](#rsync-compatibility-mode) | `<drive or server>/flux-rsync/…` | 1 (`flux-repo.toml`) | refused |
| [Borg repository](#borg-compatibility-mode) | `<drive or server>/flux-borg/…/repo` | Borg's own; Flux's notes: `"flux": 1` | — |
| [Profile](#profiles) | `~/.config/flux-backup/profiles/<id>.toml` | schema 1 (`version`) | refused: "written by a newer Flux" |
| [Settings kept with the backups](#settings-kept-with-the-backups) | next to (or inside) each backup | profile schema 1 | as profiles |
| [Recovery kit](#recovery-kit) | next to rsync and Borg backups; next to Vaults on drives | text | — |

A *host* and *user* in a path are the computer and user that made the backup; a backup
recovered on another computer keeps using them (`[origin]` in its profile).

## Vault

Encrypted, deduplicated, compressed storage. Fully specified in
[VAULT_FORMAT.md](VAULT_FORMAT.md): the key slots (Argon2id, XChaCha20-Poly1305, unlock
files), packs, the index, trees, snapshot records, and how format 2 differs from 1 (holes
in sparse files, `tree_format` in records). `flux vault upgrade` moves a format 1 Vault to
format 2 (Flux before 1.0 can't open it afterwards); the app offers it on the backup's
Overview.

Also inside a Vault: `profiles/<id>`, the settings of each backup using it (encrypted
like everything else; see below). An encrypted Vault refuses unencrypted objects
(tag byte `0`): only an unencrypted Vault has them.

`flux-vault-recover` (a separate program, kept next to Vaults on drives) reads formats 1
and 2 with an implementation of its own, which the tests compare with Flux's.

## Mirror

```text
<drive>/flux-backup/<host>/<user>/<profile>/
├── flux-repo.toml          format = 1, engine = "mirror", profile, host, user, created
├── lock                    held (flock) while a job writes
├── latest -> snapshots/<id>
├── flux-profile.toml       the settings kept with the backups
└── snapshots/
    ├── .partial/           a snapshot being made (the next backup continues it)
    └── <id>/               the id is the start time, 2026-11-04_18-30-00
        ├── meta.json       what, when, how it went
        ├── manifest.zst    the metadata of every path
        └── data/           the files, at their absolute paths (home/me/…)
```

Unchanged files are hard links to the same file in the snapshot before, so every
snapshot is complete and only changes take space.

### Mirror manifest

A zstd stream holding: the 8 bytes `FLUXMAN\0`, the version (u32, little-endian, `1`),
then one record per path, sorted by path bytes: its length (u32 LE) and an `Entry` in
[postcard](https://docs.rs/postcard) encoding:

| Field | Type |
|---|---|
| `path` | bytes: the absolute path without its leading `/` |
| `kind` | enum: dir, file, symlink, fifo, char_device, block_device, socket |
| `mode` | u32, permission bits with setuid/setgid/sticky |
| `uid`, `gid` | u32 |
| `size` | u64 |
| `mtime`, `mtime_nsec` | i64, u32 |
| `dev`, `ino`, `nlink` | u64 (hard links within a snapshot) |
| `rdev` | u64 (devices) |
| `link` | optional bytes (symlink target) |
| `hash` | optional 32 bytes (BLAKE3, with checksum change detection) |
| `xattrs` | list of `{name: bytes, value: bytes}` (ACLs as `system.posix_acl_*`) |

The manifest is what restores read: the `data/` tree can't always carry owners.

## Snapshot `meta.json`

JSON, written with every Mirror and rsync snapshot (`format` is 1):

```json
{ "format": 1, "id": "2026-11-04_18-30-00", "profile": "home", "host": "laptop",
  "user": "me", "started": "2026-11-04T18:30:00Z", "finished": "…",
  "status": "complete",              // or "complete_with_errors"
  "name": null, "pinned": false,     // a name keeps a snapshot; so does a pin
  "sources": ["/home/me"],
  "stats": { "entries": 0, "files": 0, "dirs": 0, "symlinks": 0, "specials": 0,
             "total_bytes": 0, "new": 0, "changed": 0, "unchanged": 0, "deleted": 0,
             "copied_bytes": 0, "copied_files": 0, "linked_files": 0, "stored_bytes": 0 },
  "errors": [{ "path": "…", "message": "…" }], "errors_total": 0,
  "flux_version": "1.0.0",
  "sources_missing": [],             // optional: sources whose drive wasn't there
  "note": null, "tags": []           // optional
}
```

Vault snapshot records hold the same object as `meta`. Fields may be added within version
1; readers ignore what they don't know.

## rsync (compatibility mode)

```text
<target>/flux-rsync/<host>/<user>/<profile>/
  FLUX-RECOVERY.txt          how to list, browse, restore and back up without Flux
  flux-backup.sh             the same backup with nothing but rsync and sh
  flux-repo.toml             format = 1, engine = "rsync", …
  flux-profile.toml          the settings kept with the backups
  latest -> snapshots/<id>
  snapshots/<id>/
    data/<absolute path>     the files, exactly as rsync wrote them
    rsync-command.txt        the command that wrote this snapshot
    rsync-filter.txt         the exclusion rules it used
    meta.json                optional: snapshots made by flux-backup.sh have none
  snapshots/.partial/
```

## Borg (compatibility mode)

```text
<target>/flux-borg/<host>/<user>/<profile>/
  FLUX-RECOVERY.txt     how to use the backups with borg alone
  flux-backup.sh        the same backup with nothing but borg and sh
  flux-patterns.txt     the exclusions (borg --patterns-from syntax)
  flux-profile.toml     the settings kept with the backups
  repo/                 a standard Borg 1.x repository
```

Archives are named after their start time. What Flux adds lives in the archive's
comment, as JSON: `{"flux": 1, "name": …, "pinned": true, "note": …, "tags": […]}`
(absent fields are their defaults). Archives made by borg itself are listed too.

## Profiles

TOML, one file per backup, schema 1 (`version = 1`; the key is optional and means 1).
Every setting is described in the user guide's
[profile reference](USER_GUIDE.md#profile-reference). Files Flux writes start with:

```toml
# Flux Backup profile. Reference: https://github.com/neuralpunk/flux-backup
# schema 1, written by Flux Backup 1.0.0
```

A profile using a setting this version doesn't know is refused with the setting's name
(it was written by a newer Flux). A profile from before 1.0 is read as it is; the first
time Flux rewrites it (a save in the app, or a backup recording a drive), the old file
is kept as `<id>.toml.pre-1.0.bak` next to it.

## Settings kept with the backups

After each backup, a copy of its profile, without passwords, hooks or webhooks, so the
backups can be recovered on another computer (*Recover an existing backup*, `flux
recover`): `flux-profile.toml` in the backup's folder (Mirror, rsync, Borg), or
`profiles/<id>` inside a Vault (encrypted, a JSON object `{profile, host, user, text}`
where `text` is the file). The file is a profile (schema 1) with these comment lines:

```toml
# flux-profile-id = "home"
# made-on = "laptop"
# made-by = "me"
# kept-as = "home"
```

## Recovery kit

Next to rsync and Borg backups: `FLUX-RECOVERY.txt` (plain text: what the backups are and
the exact commands to list, browse, restore and back up with the standard tool) and
`flux-backup.sh` (a script running the same backup). Next to Vaults on drives:
`flux-vault-recover` (the program) and `FLUX-VAULT-RECOVERY.txt` (how to use it). These
are written for people and change wording freely; nothing reads them.

## Not frozen

On this computer only, and safe to lose: the run log (`runs.jsonl` and each run's
events, in the state folder), caches (listings, statistics, the file lists of Vault
snapshots), restore-test and recovery state, and settings (`settings.toml`). Their
formats may change in any release; Flux rebuilds or ignores what it can't read.
