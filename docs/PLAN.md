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

## Compatibility mode (agreed 2026-11-07)

Some people want backups that never depend on Flux: if Flux disappeared tomorrow, they
should still be able to list, browse, restore and keep taking backups with industry-standard
tools. Compatibility mode adds two engines that store everything in the native format of a
standard tool and use Flux only as the interface:

| | **rsync** | **Borg** |
|---|---|---|
| Format | Plain folders written by `rsync --link-dest` (unchanged files hard-linked) | A standard Borg 1.x repository |
| Who writes the data | the system `rsync` | the system `borg` (installed separately) |
| Without Flux | any file manager, `cp`, `rsync` | `borg list / extract / mount / create / prune` |
| Dedup / compression / encryption | per unchanged file / — / — | chunks / zstd / repokey (passphrase) |
| Targets | local drives, SSH (server needs rsync) | local drives, SSH (server needs borg) |

Rules both engines follow:
* **Nothing Flux-only is needed to get data back.** Flux's own files (snapshot metadata,
  pins, cached listings) are conveniences; losing them loses nothing.
* **The recovery kit travels with the backups.** Next to every repository Flux keeps
  `FLUX-RECOVERY.txt` (plain-text, step-by-step commands written for this profile and
  target: list, browse, restore one file, restore everything, back up without Flux, delete
  old backups), and `flux-backup.sh`, a POSIX shell script that runs the same backup with
  the standard tool only. Each rsync snapshot also stores the exact command that wrote it
  (`rsync-command.txt`). The kit is rewritten after every backup so it follows profile
  changes. `flux recovery <profile>` prints it, and the app shows it.
* **Only native features.** Options the tool can't do natively are either done the way
  the tool does them (CACHEDIR.TAG for rsync: a marker search whose results are written
  into the snapshot's filter file, and that the script repeats with `find`) or reported as
  not supported by validation, never silently emulated with Flux-only data.
* **Written for experts and for people who know nothing about the tool.** Every guide and
  every screen starts with what a newcomer needs (where the files are, copy-paste
  commands, what each part means) and keeps the exact commands, flags and layout one step
  away for people who know rsync or Borg well.
* Browsing, diff, find, history, restore (same keep/overwrite rules, symlink-safe as root),
  mount, retention with pins and names, check, schedules, hooks and the app all work as for
  the other engines.

### Phase 10 — Compatibility mode I: rsync engine
`engine = "rsync"`. New crate `flux-compat`: a target-neutral store (local folder, or SFTP
plus remote shell), the rsync engine (backup with `--link-dest` into `.partial`, renamed into
place; unchanged runs detected from `--stats`; per-file errors from exit codes 23/24;
CACHEDIR.TAG/marker pass; gitignore → rsync filter translation that keeps the
last-match-wins meaning), snapshot listing (also snapshots made by the script, without
Flux's metadata), entries (local scan or remote `find -printf`, cached), restore through
the generic restore, name/pin, delete, prune, check, mount, and the recovery kit. Runner,
`AnyRepo`, CLI (`flux recovery`, `--engine rsync`) and service integration. Tests against
real rsync, locally and over the SSH stand-in, including running the generated script.

### Phase 11 — Compatibility mode II: Borg engine
`engine = "borg"` with `[borg]` options (encryption `repokey-blake2`/`repokey`/`none`,
compression, password source, remote borg path). Borg detection and version check;
init and create with JSON progress; archives as snapshots (names and pins kept in the
archive comment, set with `borg recreate --comment`); entries from
`borg list --json-lines` (cached; archives never change); restore by extracting into a
staging folder on the destination's side, then the generic restore; delete + compact,
prune, check (`--verify-data` for "verify every byte"), mount through `borg mount`,
`borg key export` into the recovery kit's advice. Tests against the real borg (skipped
when borg isn't installed).

### Phase 12 — Compatibility mode III: the app
Engine choice gains "Compatibility mode" (rsync or Borg) with an explanatory modal, tool
detection (version, missing tool with install hints), Borg settings and passphrase flow;
a "Recovery kit" modal (Overview) with the commands for this profile and copy buttons;
engine badges; restore dialog notes the equivalent command; demo data; screenshots;
website and user guide.

## UX rework (agreed 2026-11-08)

A review against usability research (docs/UX_PRINCIPLES.md) found that much of the app
didn't look like what it was: actionable rows read as information, tags looked like
buttons, the sidebar read like settings pages, an administrator prompt appeared without
explanation, and deleting a profile was hard to find. The visual design stays; the
interaction design changes.

### Phase 13 — UX I: foundations
Research and rules (docs/UX_PRINCIPLES.md); one app-wide menu for right-click and ⋯
buttons (keyboard: Menu key, Shift+F10, arrows, Escape; target outlined; inside the
window); settings rows, right-aligned switches, descriptor tags, confirm dialog; the
webview's own context menu replaced; snapshots get the shared menu.

### Phase 14 — UX II: sidebar, profiles, system backups
"Backups" sidebar with destination icons and a status line per backup; right-click and
⋯ menus for backups (back up now, browse, settings, recovery guide, delete); delete with
a clear confirmation, also from Settings; a compact "System backups" row whose unlock
explains first; header tags with icons.

### Phase 15 — UX III: settings and the rest
Profile editor rebuilt from rows (switches right, values with chevrons, reasons for what
can't change). Advanced options stop being one long page: a short list of named groups
(exclusions, storage format, schedule details, what to keep, hooks, performance and
behaviour), each with a one-line summary of its current values, opening just that group
in a focused panel. The editor always opens in the basic view (advanced is never
remembered across profiles or new ones). Context menus on files and activity;
screenshots, website and guide.

### Phase 16 — Export and import profiles (asked 2026-11-08)
Export a profile as a Flux profile (TOML, importable again with `flux profile import` and
the app), and for other tools where a faithful translation exists: borgmatic
(config.yaml), restic (a script and an exclude file) and rsync (a script). Exclusions are
translated per tool; whatever a format can't express (pins, cache markers, max age…) is
listed as a comment in the export, never dropped silently. In the app: the backup's
⋯/right-click menu and Settings; on the command line: `flux profile export|import`.

## Toward 0.2: smaller improvements (agreed 2026-11-08)

### Phase 17 — Quick wins
Open a file from a snapshot (a temporary copy in its usual app); duplicate a profile;
a one-time card after the first backup (how to get a file back); Overview health warnings
in words (no successful backup for days, drive not seen since…); Activity filter (problems
only); copy diagnostics (versions, profile without secrets, last run's log).

### Phase 18 — Finding files
Search all snapshots by name from the Snapshots tab; "Versions of this file" (every
version with date and size, open or restore one); a restore preview (what will be replaced,
kept aside or new) before restoring in place.

### Phase 19 — Space
A size estimate (files, bytes) in the new-backup page that follows the exclusions; a space
forecast on Overview from the snapshot history ("fills in about 5 weeks") with a small
growth chart.

### Phase 20 — Control
Pause scheduled backups (1 hour, until tomorrow, until resumed) from the tray and menus;
skip scheduled backups on metered connections; optional scheduled maintenance (monthly
verify, weekly free-up-space for Vault and Borg; notify on problems); an app preferences
page (notifications incl. optional success, start at login, theme, default exclusions).

### Phase 21 — Robustness
Sparse files copied sparsely (SEEK_DATA/SEEK_HOLE); a path index so find and history don't
decode every manifest; the flaky remote-lock test; a pass of the real desktop app (WebKitGTK)
as far as this environment allows.

### Phase 22 — Release 0.2
Version 0.2.0 everywhere (workspace, app, packaging metadata, website); CHANGELOG.md with
everything since 0.1.

### Phase 23 — Website, guide and README for 0.2
New screenshots of the app; the website (index and HTML guide) and README brought up to
date with everything in 0.2 (asked 2026-11-08).

## Out of scope for v1 (candidates later)
S3/B2 and other cloud backends (Vault's backend trait is designed for them), SMB/NFS
mount management, Windows/macOS, Back in Time import.
