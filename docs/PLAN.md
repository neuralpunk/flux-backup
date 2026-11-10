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

## Toward 1.0 (agreed 2026-11-09)

The owner's list for 1.0, grouped into phases. Phases 24–52 were done between 0.2 and 0.3
(see TRACKER.md); 1.0 starts at Phase 53.

**The rule for every phase: keep the main experience simple.** The app's first answer stays
*what is protected, where, and how to get it back*. Everything deeper (security tuning,
replication, restore tests, rules, inspectors) goes behind clearly named settings groups
or an **Advanced** section, closed by default, never in front of the common path. Each
phase also adds its changes to CHANGELOG.md (an *Unreleased* section, becoming 1.0.0 in
Phase 76), the user guide, and the CLI where the app gains something (the CLI keeps parity).

**All new testing is simulated** (the owner's rule, 2026-11-09; the existing tests stay as
they are). Nothing in a new test or check
powers the machine off, needs root or an administrator password, touches real systemd
timers, the real crontab, the user's own profiles, keyring or desktop, or a real network
service. Tests use stand-ins instead, as the existing ones do: temporary folders and
`FLUX_HOME`, the local `fake_ssh`, fake `shutdown`/`systemctl`/`nmcli`/`curl`-style
commands on `PATH`, a local HTTP listener for webhooks, small loop-free "volumes" faked
by directory and UUID stand-ins, `FLUX_PKEXEC=""` for the helper, and the app's demo build
for screens.

The version stays 0.3.x while this is built. It becomes 1.0.0 in Phase 76, so no build
says 1.0 before 1.0 is finished.

Overlapping items were merged: "restore verification frequency" goes with restore tests,
"source availability rules" with source protection, ".tar.zst/ZIP export" and the
"checksums manifest" with snapshot export, "versioned metadata" with the format freeze.

Order: features that change what's stored on disk come first, so the formats are frozen
once, in Phase 72, after everything that touches them. Fuzzing, property tests and the
panic audit come after the freeze, so they test the final code.

Translations (Spanish, Swedish) were planned and then dropped by the owner (2026-11-09):
1.0 is English only.

### Phase 53 — Version in the app
An **About Flux Backup** dialog (from the main menu and Preferences): version, the
`flux` and helper versions, the license, links to the website, the guide and bug reports,
and "Copy version info". The version also shows at the foot of Preferences. Starts the
CHANGELOG's *Unreleased* section.

### Phase 54 — Vault: one lock everywhere, and holes kept
* **One lock for local and SFTP.** A Vault reached both locally and over SFTP (the same
  NAS, for example) uses the same lock: a lock file in `locks/` with host, process and
  time, which both backends write and respect, plus `flock` locally as before. Stale
  locks from a dead process on the same host are cleared; from another host, Flux says
  whose lock it is and how old, and `flux unlock` removes it.
* **Holes kept.** Vault records hole runs in sparse files (`SEEK_DATA`/`SEEK_HOLE`)
  instead of storing zero chunks, and restores them as holes, as Mirror already does.
  This is a format change: Vault format 2 (read and written by 1.0; format 1 Vaults are
  still read, and are upgraded only on request with `flux vault upgrade`). VAULT_FORMAT.md
  updated.

### Phase 55 — Vault security settings
A *Security* group in a Vault backup's settings (Advanced), with presets and a custom
choice, each saying what it costs:
* **Password hashing strength** (Argon2id memory, passes, threads): *Standard* (64 MiB,
  3 passes, today's default), *Strong* (256 MiB, 4), *Maximum* (1 GiB, 6), or custom.
  Applied by re-wrapping the master key in its key slot: instant, no data rewritten.
  Opening shows how long unlocking takes on this computer.
* **Hide file sizes** (padding): blobs padded up to size buckets, so stored sizes say less
  about file sizes. Costs some space; applies to data written from then on.
* **Where the password lives**: the desktop keyring (default), ask every time, or a key
  file. Scheduled backups can't ask, so "ask every time" is offered only for manual
  backups and says so.
* **Lock again after** N idle minutes in the app.
* **Password plus key file** (two factors): the master key needs both.
The cipher (XChaCha20-Poly1305) and the id hash (keyed BLAKE3) are fixed and explained,
not offered as choices: neither has a stronger alternative worth a setting.

### Phase 56 — Source drives: protection and availability rules
Sources get the protection destinations have: a source on its own volume (an external
disk at `/mnt/photos`) is recognised by filesystem UUID, recorded when the backup is set
up. A missing, unmounted or different volume is never read as "everything was deleted":
the backup doesn't make a snapshot of an empty mount point. Per source: **wait** (until
it's back, up to a time limit), **skip it** (back up the rest, and say so), or **fail**.
Warnings in words on the Overview ("Photos disk wasn't connected for the last 3
backups").

### Phase 57 — Notes and tags on snapshots
Free-text notes and tags on a snapshot ("before the upgrade", `taxes`), separate from
names and pins: they never change what retention keeps. Stored in the snapshot's metadata
for every engine (Borg: the archive comment). Shown in the snapshot list, searchable, and
filterable. `flux snapshot note|tag`.

### Phase 58 — Several destinations I: the model and independent backups
One backup plan (what, when, what to keep) with several destinations, each with its own
engine and target, and an optional **offsite** mark. **Independent** mode: each
destination gets its own backup from the source, in turn, so a fault in one never
reaches another. The Overview shows each destination's state; failures say which one.
Profile schema change (the destinations list), with a migration from one target.

### Phase 59 — Several destinations II: replication
**Replicate** mode: back up once to the first destination, then copy the new snapshot
from there to the others, without reading the source again. Vault copies the missing
packs, index and snapshot records (an exact replica, same keys); Mirror and rsync copy
the new snapshot hard-linked against the replica's previous one. Borg 1.x has no safe
way to copy archives between repositories, so Borg destinations are always independent,
and the app says so. Replicas are checked like any destination.

### Phase 60 — Retention safety
* **Safety floor**: prune never leaves fewer than N healthy (complete, checked)
  snapshots, whatever the rules say.
* **After problems**: when the last run failed or was partial, or unusual changes were
  found, scheduled pruning holds off (or keeps everything extra from the last N days),
  so a bad run can't push out good snapshots.
* **Activity history kept separately**: run logs have their own retention (keep N days
  or N runs), independent of snapshots.

### Phase 61 — Network rules and bandwidth schedules
* **Wi-Fi rules**: scheduled backups only on named networks, or never on named networks
  (NetworkManager's SSID). Applies to backups to a server; skipped runs say why.
* **Bandwidth schedule**: limits by time of day and weekday (full speed overnight, 2 MB/s
  during working hours), for SSH targets (rsync `--bwlimit` and Flux's SFTP throttle),
  adjusted while a backup runs.

### Phase 62 — Restore tests
After Duplicati's restore test policies: Flux regularly restores a sample of a snapshot
into a temporary folder and checks every restored file against the backup's own record
(and, optionally, against the source file, reporting differences as warnings).
* **When**: after every backup, weekly, monthly, or only by hand ("Test a restore now").
  A test is due once its interval has passed since the last passing test; an overdue
  test is a warning on the Overview.
* **What**: random files (default 10, newest snapshot), a percentage of the size,
  everything, or *rolling* (every file tested within a window, 30 days by default).
  Limits on time and data read.
* **Results**: passed, warning (skipped files, source changed) or failed, with a report
  (which snapshot, how chosen, counts, data read, and each failed file's problem:
  content differs, size differs, missing or damaged data, couldn't be restored, metadata
  differs). A failed test doesn't change the backup's own result.
* The "unverified" state (no passing test within the interval) feeds the status filters.
`flux restore-test <profile>`.

### Phase 63 — Unusual-change detection
After each backup, Flux compares the run with that backup's own history: files changed,
deleted and added, against a baseline of recent runs; many files gaining an unfamiliar
extension, or files renamed to the same new extension; changed files whose contents look
encrypted (an entropy jump on a sample read during the backup); known ransom-note names.
When a run stands out: the snapshot is marked **Unusual changes**, the snapshot before it
is pinned automatically as the last one before the change, pruning holds off (Phase 60),
a notification says what was seen, and the app lists the affected files (by folder and
extension) with a direct restore from the earlier snapshot. Sensitivity: off, normal,
high; "This was me" dismisses it. (Rubrik's page refused automated reading; the signals
are the standard ones for ransomware detection.)

### Phase 64 — Notifications off the computer: webhooks
A generic webhook per backup or for all: events (started, succeeded, failed, warning,
missed, restore test failed, unusual changes, low space), a JSON body (documented), method,
URL and headers, with presets for ntfy, Gotify, healthchecks.io-style pings (start,
success, fail URLs) and Slack/Discord-compatible messages. "Send a test". Secrets in
headers are stored like passwords and never exported.

### Phase 65 — Recovery on a new computer
* **Automatic profile export**: after each backup, a redacted copy of the profile (no
  passwords, no hook commands' secrets) is saved next to the backups, with the recovery
  kit.
* **Recover an existing backup** on the welcome screen (and *+ › Recover a backup*): pick
  a drive or a server, Flux finds the backups there (Vault, Mirror, rsync, Borg), reads
  the saved profile, asks for the password, and offers to restore files, everything, or
  to reconnect the backup so it carries on from this computer. `flux recover`.
* **Recovery readiness** on a backup's Overview (under Advanced): a short checklist of
  whether this backup could be recovered on another computer: is the profile saved next
  to it, is the recovery kit current, is the password stored somewhere other than this
  computer's keyring (asked, not known: "Have you kept your password elsewhere?"), was
  `borg key export` done, has a restore test passed.

### Phase 66 — Standalone Vault recovery tool
`flux-vault-recover`: a small, separate, read-only program for the Vault format, with
no GUI, no config and few dependencies, built as a static binary (musl) and attached to
releases on its own. It lists snapshots, lists files, restores and verifies. The recovery
kit explains it, and Flux can keep a copy of it next to each Vault (on by default for
drives). Format compatibility is tested against the main engine.

### Phase 67 — Snapshot export
Save files out of a snapshot as standard archives:
* **.tar.zst** of a whole snapshot or selected folders: Unix metadata kept (owners, modes,
  times, links, xattrs and ACLs as PAX records).
* **ZIP** of selected files, for sharing, with a clear note of what ZIP can't keep
  (owners, permissions beyond basic, links, xattrs).
* An optional **checksums manifest** next to the archive (SHA-256, `sha256sum -c`-ready,
  or BLAKE3, `b3sum -c`-ready).
In the snapshot browser's menu and *Export…*; `flux export`.

### Phase 68 — What will be backed up: rule tester and coverage
* **Exclusion-rule tester**: type a path, see whether it's backed up and which rule (or
  the size limit, a cache marker, another filesystem) decides it.
* **Coverage**: browse the source folders with each item marked backed up or left out,
  and why; folder totals of what's in and out.
In the exclusions settings group; `flux explain <profile> <path>`.

### Phase 69 — Backup templates
Setup starts from a template: **Home**, **Documents**, **Photos**, **Development**
(skips build output and dependency folders), **System** (a system backup of `/etc` and
more), or blank. Each fills sources, exclusions, schedule and what to keep, and
explains itself in a line. Your own settings can be saved as a template too.

### Phase 70 — Calendar and status filters
* **Calendar** on Snapshots: a month view, days with snapshots marked (and failures),
  click a day to see its snapshots. Next to the list, not replacing it or the bars.
* **Filters** on All backups: missed, failed, unverified, offsite, low space.

### Phase 71 — Storage view and performance inspector
* **Storage**: how much original data is protected, how much space it takes, and what
  deduplication and compression save (Vault; Borg from its own statistics; Mirror's
  hard-link sharing).
* **Performance inspector** (Advanced, hidden from the normal path): for each run, time
  spent scanning, reading, chunking, deduplicating, compressing, writing or uploading and
  verifying; throughput; storage latency. Recorded in the run log.

### Phase 72 — The formats, frozen
Every format a backup depends on is written down, versioned and checked: Vault (format
2), the Mirror manifest, snapshot `meta.json`, the profile schema, and the recovery kit's
layout. Each says its version; Flux refuses newer versions with a clear message, reads
every older one, and migrates profiles (with a backup copy of the old file). Fixtures:
small repositories and profiles written by 0.1, 0.2, 0.3 and 1.0 are checked in, and
every build must read them. `docs/FORMATS.md`.

### Phase 73 — Property tests for filter translation
proptest: gitignore-style rules → rsync filter rules, and Back In Time → Flux exclusions,
must decide every generated path the same way, checked against the real `rsync`
(`--dry-run`) as well as Flux's own filter.

### Phase 74 — Fuzzing
cargo-fuzz targets for everything that reads bytes Flux didn't write itself: Vault pack
headers, index files, tree blobs and snapshot records (with and without encryption), the
Mirror manifest, `meta.json`, profiles, and the helper's JSON protocol (which runs as root).
Each run long enough to stop finding things; crashes fixed and kept as regression tests.

### Phase 75 — Panic audit
Every `unwrap()`/`expect()` outside tests reviewed: on backup, restore, prune and helper
paths, replaced by errors or kept with a comment saying why it can't fail. A clippy lint
(`unwrap_used`) on the engines and the helper so new ones need a reason.

### Phase 76 — Release 1.0
Version 1.0.0 everywhere; CHANGELOG's *Unreleased* becomes 1.0.0; AppStream; README status
(no longer a beta caution); website, guide and screenshots. Publishing (sync to the public
repository and the tag) only when the owner says so.

## Toward 1.1: refinement (agreed 2026-11-09)

The owner's list for 1.1: a refinement release that makes what's there better, with few new
features. The rules for 1.0 still hold (the main experience stays simple; new tests are
simulated; the CLI keeps parity; CHANGELOG *Unreleased* becomes 1.1.0 at the release).
The version stayed 1.0.x until Phase 85, which made it 1.1.0.

### Phase 78 — Clearer settings: exclusions, and passwords that can't be asked for
*Leave some things out* becomes **Exclude Files/Directories**. When a backup starts by
itself (a schedule, or when its drive is plugged in) but its password is set to *Ask every
time*, those backups fail: warn at *When* and under *How backups are stored*, with a
one-click fix (keep it in the keyring, typed once and checked; a key file for system
backups), and in `flux profile validate`.

### Phase 79 — Easier time entry, and a 24-hour clock
Typing a backup's time is clunky. A time field that takes what people type ("9", "930",
"9:30 pm", "21:30"), steps with the arrow keys, and offers common times; a Preferences
choice of 12-hour or 24-hour clock (default: the system's), used wherever the app shows or
takes a time.

### Phase 80 — Close to the system tray
Closing the window keeps Flux running in the system tray (the tray icon reopens it, its
menu quits). On by default, with a choice in Preferences to quit on close instead; the
first time, a short note says where Flux went.

### Phase 81 — A recovery checklist you can act on
Each line of *Could you recover it on another computer?* gets a button that does it: back
up now (keeps the settings with the backups), show the recovery kit / password help, export
the Borg key, test a restore, add a place kept elsewhere.

### Phase 82 — Restore tests that leave nothing behind
A restore test no longer writes restored files to a temporary folder. Each picked file is
read back through the engine and checked as it streams (size, checksum, against the file
now when asked), in a small fixed buffer that's wiped after use: nothing restored touches
the disk, memory use stays the same for ten files or every file, and an encrypted backup
stays encrypted at rest throughout. Borg's single-file reads stream from `borg extract
--stdout` instead of a temporary file.

### Phase 83 — At a glance: each backup's settings, and every backup in a table
A compact summary of a backup's settings on its Overview (schedule and next run, where the
password is kept, encryption, destinations, what's excluded, retention, restore tests,
checks, notifications…), each line linking to its setting; and on *All backups*, a dense
sortable table of the same for every backup. Modern and compact, not big tiles.

### Phase 84 — Man pages with examples
Every `flux` command's man page says more (what it does, what it changes, exit codes where
they matter) and has an EXAMPLES section with basic usage; `flux(1)` gains an overview of
common tasks.

### Phase 85 — Release 1.1
Version 1.1.0 everywhere; CHANGELOG; AppStream; README; guide and screenshots. Publishing
only when the owner says so.

## 1.1, continued: clarity and visual polish (agreed 2026-11-09)

After Phase 85 the owner asked for every clarity and visual idea from the review of 1.1
(not yet published, so they join 1.1.0's release notes; screenshots are retaken at the
end).

### Phase 86 — Statuses that agree and don't shout
One count of what needs you (not protected, or protected with something to look at) for
the sidebar, All backups' headline and the line under it. Status words that say what it is
("Low on space", "Not tested lately") instead of "Check". A severity ladder: red ⚠ only
for what isn't protected, an amber dot for something worth a look. Toasts: an "i" for
news, and above an editor's Save bar.

### Phase 87 — Plain words
Durations in words ("every 4 hours", "in 1 hour"); storage named in plain words first,
the engine names where they help; "profile" tied to "backup" in the app; validation
messages without config syntax in the app.

### Phase 88 — Checks explained, the checklist's progress, one warning card
Check, Verify every byte and Restore test explained side by side; a progress meter for
the recovery checklist, "to do" instead of ✕; one shared warning card.

### Phase 89 — Custom times, previewed
The next runs of a custom systemd calendar expression, shown as it's typed.

### Phase 90 — All backups' table and the glance grid
A compact headline in the Settings view; the table full width, short values on one
line, a choice of columns; the glance grid in labelled groups, cells that don't apply
left out.

### Phase 91 — The Overview's order
What's always shown and what folds away; only the answer at the top raised, the rest
flat sections. The demo's checklist agrees with its offsite copy.

### Phase 92 — Release 1.1, again
CHANGELOG and AppStream, a *New in 1.1* row on the website, screenshots retaken.

## Out of scope for v1 (candidates later)
S3/B2 and other cloud backends (Vault's backend trait is designed for them), SMB/NFS
mount management, Windows/macOS. (Back In Time import was done in 0.3.)
