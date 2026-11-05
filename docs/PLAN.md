# Flux Backup — Plan

Flux Backup is a Linux backup tool (CLI + GUI) written in Rust. It covers the same ground as
Back in Time (profiles, scheduled snapshot backups, include/exclude, retention, restore,
root mode) and aims to be faster, more capable, and nicer to use.

License: **MIT OR Apache-2.0**. Flux is a clean-room re-implementation: Back in Time
(GPL-2.0+) was studied for *behaviour* only; no code is copied from it.

## Decisions (agreed 2026-11-04)

| Topic | Decision |
|---|---|
| Engines | **Both, chosen per profile**: *Mirror* (hard-link snapshots, plain browsable folders) and *Vault* (content-defined-chunk dedup repository, zstd, authenticated encryption) |
| Targets (v1) | Local / external drives, SSH/SFTP |
| GUI | Tauri 2 + Svelte 5 + Tailwind CSS 4, modern dark-first design with a light theme |
| Root mode | Yes — unprivileged GUI + small polkit-authorised helper; system-level systemd timers |
| Back in Time import | No |
| Scheduling | systemd timers (user and system), not cron |
| Config format | TOML, one file per profile |

## Architecture

```
flux-backup/
├── crates/
│   ├── flux-core       profiles/config, filters, retention, progress events, hooks,
│   │                   power, paths                                       (Phase 0+)
│   ├── flux-fs         shared by engines: metadata model, parallel scanner, (Phase 4)
│   │                   metadata-preserving fs ops, diff, find/history, restore
│   ├── flux-mirror     Mirror engine: hard-link snapshots, manifest,       (Phase 1)
│   │                   plan/execute, prune, check
│   ├── flux-runner     job runner, schedules, job control, passwords,     (Phase 2+)
│   │                   engine-neutral repository handle (AnyRepo)
│   ├── flux-vault      Vault engine: FastCDC chunks, BLAKE3 ids, zstd,     (Phase 4)
│   │                   XChaCha20-Poly1305 + Argon2id, packs, index, GC
│   ├── flux-transport  system ssh + ControlMaster, pipelined SFTP client,  (Phase 5)
│   │                   remote lock files
│   ├── flux-service    what the app shows and does (views, editing,       (Phase 8)
│   │                   actions, restore) + the helper protocol and client
│   ├── flux-mount      snapshots as a read-only FUSE filesystem (fuser,   (Phase 9)
│   │                   no libfuse) for every engine
│   ├── flux-cli        `flux` binary                                        (Phase 0+)
│   └── flux-helper     root helper for system profiles, started by        (Phase 8)
│                       pkexec; serves flux-service's typed protocol
└── app/                Tauri 2 shell (src-tauri) + Svelte 5 UI            (Phase 6+)
```

### Where Flux is meant to beat Back in Time

* **Speed.** BiT shells out to rsync for every backup and walks the whole snapshot tree to
  browse it. Flux's local Mirror engine is native Rust. It scans in parallel, compares
  against the previous snapshot's **manifest** (size, mtime, ctime, inode, mode) instead
  of re-reading files, hard-links unchanged files and copies changed ones with
  `copy_file_range` (reflink on Btrfs/XFS where available). Browsing, diffing, and
  searching a snapshot read the manifest, not the disk.
* **Two storage models.** Mirror is Back in Time's model and stays plain folders. Vault
  adds dedup across files and snapshots, compression, and encryption at rest. Vault
  replaces gocryptfs/EncFS.
* **Crash safety.** Snapshots are built in a `.partial` directory and renamed into place
  atomically. An interrupted backup resumes from the partial snapshot.
* **Retention.** A unified policy: keep-last N, keep-all-within, hourly/daily/weekly/
  monthly/yearly buckets, a max age, pinned/named snapshots that are never pruned,
  free-space and free-inode floors, and a dry-run preview shown in the GUI before
  anything is deleted.
* **Scheduling.** systemd timers with `Persistent=true` (catch up on missed runs, which
  is what BiT used anacron for), backup-on-drive-connect via systemd device units, skip
  on battery, nice/ionice, suspend inhibition.
* **UX.** Visual timeline, instant file browser, side-by-side diff, live progress with
  ETA and throughput, exclusion suggestions, restore preview, desktop notifications,
  tray.
* **Scriptable.** `--json` output on every CLI command, stable exit codes, shell
  completions, man pages.

## Phases

Each phase ends with passing tests, updated docs and tracker, and a git commit.
Phases are done one at a time, in order.

### Phase 0 — Foundation
Repo, licenses, README, plan and tracker, Cargo workspace, CI workflow. In `flux-core`:
the profile/config model (TOML, validation, XDG paths, atomic save) and logging setup.
In `flux-cli`: `flux profile list|show|create|delete|validate`, `flux config path`.

### Phase 1 — Mirror engine (local)
Filter engine (include/exclude globs, size limit, one-file-system, caches/tags such as
`CACHEDIR.TAG`). Parallel scanner. Snapshot layout and naming, the `.partial` workflow,
hard-link vs. copy decisions driven by the manifest, metadata preservation (mode, owner,
times, symlinks, xattrs, ACLs), per-snapshot zstd manifest, snapshot lock. Restore
(to the original place or elsewhere, with an optional backup-before-overwrite). CLI:
`flux backup`, `flux snapshots`, `flux ls`, `flux restore`. Benchmark against rsync.

### Phase 2 — Lifecycle
Retention policy engine plus `flux prune` (dry-run by default in the UI), named/pinned
snapshots, failed-snapshot marking, free-space checks, `flux diff` between snapshots or
against the live system, `flux find` across snapshots. Hooks (pre/post/error commands
with an env contract), desktop notifications, suspend inhibition, battery check,
nice/ionice, structured per-run logs.

### Phase 3 — Scheduling & job control
`flux schedule install|remove|status`: generates systemd user timers and services.
Calendar schedules, intervals with catch-up, on-drive-connect. A job runner with a
single-instance lock, and a status/event socket at `$XDG_RUNTIME_DIR/flux/` so the GUI
and CLI can watch or cancel running jobs.

### Phase 4 — Vault engine
Repository format (documented in `docs/VAULT_FORMAT.md`), FastCDC chunking, BLAKE3,
zstd, XChaCha20-Poly1305 with an Argon2id-wrapped master key (several key slots,
password change without re-encrypting). Pack files, index, tree/snapshot objects,
backup, restore, `check` (structure and optional full data read), prune with GC,
repository lock. Password handling: prompt, keyring (Secret Service), or key file.

### Phase 5 — SSH transport
Uses system `ssh` with ControlMaster multiplexing, so `~/.ssh/config`, agents, and
ProxyJump all work. Vault over SFTP. Mirror over SSH: remote snapshots via
`rsync --link-dest` (the remote host needs rsync) plus a remote manifest. Connection
test, host-key handling, key generation and `ssh-copy-id` helper, bandwidth limit.

### Phase 6 — GUI I: shell & core flows
Tauri 2 app, Svelte 5 runes, Tailwind 4 design tokens (dark default, light theme).
Profile switcher, dashboard (last/next run, health, space), snapshot timeline, file
browser backed by the manifest, restore flow, "Back up now" with live progress.

### Phase 7 — GUI II: configuration & polish
Full profile editor (target, sources, exclusions with suggestions, retention with a
live "what would be pruned" preview, schedule, hooks, advanced), snapshot diff view,
log viewer, settings, tray icon, notifications, keyboard shortcuts, accessibility pass.

### Phase 8 — Root mode
`flux-helper` with a narrow, typed request protocol, a polkit policy, system-level
profiles in `/etc/flux-backup/`, system timers, GUI integration (lock badge,
"authenticate" prompts), symlink-safe restores as root.

The app never runs as root. Its backend logic moves into `flux-service`, which the app
uses in-process for the user's profiles and which `flux-helper` runs as root for system
profiles. Both speak one enum of calls (`protocol::Call`), as JSON lines over the helper's
stdin/stdout; anything else is refused. The helper exits when the app closes the pipe or
after 15 idle minutes. Restores open destinations through directory descriptors and refuse
symlinks not owned by root or the invoking user (`flux_fs::safe`).

### Phase 9 — Release
FUSE mount for Vault (and Mirror manifests), shell completions, man pages, `.desktop`
file and AppStream metadata, RPM/DEB/AppImage builds, user docs, threat model for
Vault.

`flux mount` serves every engine through one random-access view (`flux_runner::view`):
plain `pread` for Mirror, ranged SFTP reads for Mirror over SSH, and per-chunk decryption
for Vault (chunk offsets come from the index's plaintext lengths). Packages are built by
Tauri's bundler with the CLI, helper, polkit policy, man pages, completions and AppStream
data added through `bundle.linux.*.files`; `packaging/install.sh` covers source installs.
The threat model (docs/THREAT_MODEL.md) covers the whole program, not just Vault.

## Out of scope for v1 (candidates later)
S3/B2 and other cloud backends (Vault's backend trait is designed for them), SMB/NFS
mount management, Windows/macOS, Back in Time import.
