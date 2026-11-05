# Flux Backup

Fast, good-looking snapshot backups for Linux, with a CLI (`flux`) and a desktop app.

Flux takes the proven model of [Back in Time](https://github.com/bit-team/backintime)
(profiles, scheduled snapshots, include/exclude lists, smart retention, easy restore) and
rebuilds it in Rust. The aims are more speed, more capability and a modern interface.

> **Status:** 0.1, the first release. Read the [user guide](docs/USER_GUIDE.md) to get
> started, and the [threat model](docs/THREAT_MODEL.md) for what Flux protects against.
> [PLAN.md](docs/PLAN.md) and [TRACKER.md](docs/TRACKER.md) show how it was built. As with
> any new backup tool, keep another copy of anything precious until you trust it.

## Install

Releases ship `.deb` and `.rpm` packages and an AppImage. The packages contain the desktop
app, the `flux` command, the system-backup helper, man pages and shell completions. To
build from source, see [Building](#building).

## Two engines, one tool

| | **Mirror** | **Vault** |
|---|---|---|
| Format | Plain folders, unchanged files hard-linked | Deduplicated chunk repository |
| Browse without Flux | Yes, with any file manager | Via Flux or `flux mount` |
| Dedup | Per unchanged file | Across files and snapshots |
| Compression / encryption | — / use an encrypted disk | zstd / XChaCha20-Poly1305 |
| Targets | Local drives, SSH (server needs rsync) | Local drives, SSH/SFTP |

Each profile picks its engine.

## Desktop app

![Overview with a backup running](docs/screenshots/overview-dark.png)

| Snapshots and file browser | Profile editor |
|---|---|
| ![Snapshots](docs/screenshots/snapshots-dark.png) | ![Profile editor](docs/screenshots/settings-dark.png) |
| **Activity log** | **Light theme** |
| ![Activity](docs/screenshots/activity-dark.png) | ![Light theme](docs/screenshots/overview-light.png) |

The app (Tauri 2, Svelte 5, Tailwind 4, the "Graphite Tonal" design in
[docs/DESIGN.md](docs/DESIGN.md)) does everything the CLI does:
- **Overview:** the running backup's progress, last and next backup, snapshots, free space,
  recent backups, and maintenance (check, verify every byte, free up space).
- **Snapshots:** every snapshot with an instant file browser. "What changed" compares with the
  one before; name, pin, other comparisons and delete are in the ⋯ menu. Restore to the
  original place or another folder, keeping differing files by default.
- **Activity:** every run with its log, warnings and hook output.
- **Setting up a backup:** three questions (what, where, when) and an encryption switch.
  Connected USB drives are offered (and tied to their filesystem, so an unplugged drive is
  never mistaken for its empty mount point). **Advanced options** opens the rest:
  exclusions with suggestions and real sizes, SSH details, the storage engine, schedules,
  retention rules with a live preview of what would be kept, hooks and more.
- **System backups:** profiles for the whole computer (`/etc`, `/root`, other users' files)
  in their own section. Click **Authenticate**, enter an administrator password, and manage
  them like your own; they lock again after 15 idle minutes. See
  [System backups](#system-backups-root).
- A tray icon that lights up while a backup runs (open, back up everything, quit), keyboard
  shortcuts (`?` lists them), and dark, light or automatic themes.

"Back up now" runs `flux backup` as a separate process, so a backup keeps going when you
close the window. The app follows every running backup, scheduled or started from a
terminal, live through its status socket. Encrypted Vaults are unlocked in the app;
passwords stay in memory only.

```sh
cd app && npm install
npm run tauri dev        # the app, against your real profiles
npm run dev              # the UI in a browser with demo data (no backend needed)
```

The app runs backups with the `flux` binary from `FLUX_BIN`, from next to the app's own
executable, or from `PATH`, in that order. In a source tree, build the CLI first
(`cargo build -p flux-cli`) or set `FLUX_BIN=target/debug/flux`. For system backups it
looks for `flux-helper` in `FLUX_HELPER`, next to the app (a build tree), or in
`/usr/libexec/flux-backup/`. `cargo build -p flux-helper` builds it; without the installed
polkit policy (`packaging/polkit/`), pkexec asks for the password every time.

## CLI quick start

```sh
flux profile create home --target /run/media/$USER/Backup --source ~ -x '.cache/' -x '*.tmp'
flux backup home --dry-run        # what would be copied
flux backup home                  # take a snapshot (Ctrl-C keeps a resumable partial)
flux snapshots home               # list snapshots
flux info home latest             # details and any per-file problems
flux ls home ~/Documents -l       # browse a snapshot (instant: reads the manifest)
flux restore home ~/Documents/report.odt            # back to where it was
flux restore home ~/Documents --to /tmp/rescue -s 2026-11-04_18-30-00
flux profile edit home            # opens $EDITOR, validates before saving

flux diff home                    # latest snapshot vs. the one before
flux diff home --live --path ~/Documents   # what changed on disk since the latest snapshot
flux find home '*.odt' -i         # search every snapshot
flux history home ~/thesis.tex    # every version of a file, and where to restore it from
flux prune home --dry-run         # what retention would remove, and why each one is kept
flux name home latest before-upgrade && flux pin home before-upgrade
flux log home                     # recent runs; `--run last` shows one run's events
flux mount home ~/snapshots       # every snapshot as a read-only folder (any engine)
```

Every command accepts `--json` for scripting and `--system` for root-mode profiles in
`/etc/flux-backup`. Setting `FLUX_HOME=/some/dir` keeps all of Flux's files under one
directory. Exit codes: 0 ok, 1 error, 2 usage, 3 invalid profile, 4 finished but some
files could not be backed up or restored.

### Vault: deduplicated and encrypted

```sh
flux profile create docs --target /run/media/$USER/Backup --source ~/Documents --engine vault
flux backup docs                  # first run asks for a new password and saves it in the keyring
flux check docs --read-data       # verify every stored byte
flux key list docs                # passwords (key slots) that open the Vault
flux key add docs                 # add another password; `key passwd` changes it
flux key store docs               # save the password in the keyring (for scheduled backups)
flux prune docs                   # apply retention and reclaim space
```

All Vault profiles on a target share one repository (`<target>/flux-vault`), so identical
files deduplicate across profiles and machines. Everything stored (contents, file names,
metadata) is encrypted with XChaCha20-Poly1305 under a key protected by Argon2id. Chunk
ids are keyed, so they don't reveal content. The format and threat model are documented in
[docs/VAULT_FORMAT.md](docs/VAULT_FORMAT.md).

Password sources, set with `[vault] password_source`:
- `{ from = "keyring" }` (default): the desktop keyring;
- `{ from = "prompt" }`;
- `{ from = "file", path = "…" }`;
- `{ from = "command", command = "pass show backup" }`.

`FLUX_PASSWORD` overrides all of them. Backups only forget snapshots that retention no
longer keeps; `flux prune` reclaims their space.

Vault compared with Borg (`repokey-blake2`, `--compression zstd,3`) on the 1 GiB benchmark
data (tmpfs, 16 cores):

| scenario | Flux Vault | Borg 1.4 |
|---|---:|---:|
| initial backup | 1.8 s | 15.3 s |
| no changes | 0.5 s | 7.7 s |
| 1% of files changed | 0.6–1.0 s | 8.0 s |
| repository size | 864 MiB | 873 MiB |

Full restore took 1.2 s and was byte-identical to the source. `check --read-data`
verified all 860 MiB in 0.5 s.

### Backing up to an SSH server

```sh
flux profile create nas --target me@nas:/srv/backup --source ~ --engine vault
flux ssh keygen                   # ~/.ssh/flux_backup_ed25519, no passphrase (for timers)
flux ssh copy-id nas              # install it on the server (asks for the password once)
flux ssh test nas                 # connects, checks SFTP, write access, free space, rsync
flux backup nas
```

Flux runs your system `ssh`, so `~/.ssh/config` host aliases, agents, ProxyJump and
known_hosts all apply. One shared connection (ControlMaster) carries every command and
SFTP session of a run. Scheduled runs use `BatchMode`, so they never hang on a prompt.
`target.bandwidth_limit` caps upload speed.

* **Vault** needs only SFTP on the server. Flux's own pipelined SFTP client keeps many
  requests in flight, so latency costs little. Remote locks are lock files that expire
  if a job dies.
* **Mirror** needs `rsync` on the server. Flux scans and compares locally, then hands
  rsync the exact file list with `--link-dest`, so unchanged files are hard-linked on the
  server as they are locally. Manifests are cached locally, so browsing and diffing don't
  download anything.

### Automatic backups

```toml
[schedule]
trigger = "calendar"        # or "interval" with: every = "4h"
on_calendar = "Mon..Fri 18:00"
catch_up = true             # run a missed calendar backup after boot
skip_on_battery = true
on_connect = true           # also back up when the drive (target.fs_uuid) is mounted
```

```sh
flux schedule install home    # write and enable systemd user units for this profile
flux schedule sync            # make all timers match all profiles
flux schedule status          # timer state, next and last run
flux status                   # all profiles: running now, last run, next run
flux watch home               # follow a running backup (also one started by a timer)
flux cancel home              # stop it; the next run resumes the partial snapshot
```

Flux writes `flux-backup-<id>.service` and `.timer` into `~/.config/systemd/user/`. It
never touches unit files it didn't generate. Calendar schedules use `OnCalendar=` with
`Persistent=` (validated with `systemd-analyze` first). Interval schedules check every
15 minutes whether the last successful backup is older than the interval, so a backup
missed while the machine was off runs soon after you log in. `on_connect` adds a service
that the drive's mount unit pulls in. Editing or deleting a profile updates its timer.
Only one backup per profile runs at a time. User timers run while you're logged in; use
`loginctl enable-linger` for backups while logged out.

### Retention

After each backup, Flux removes snapshots that no rule keeps (`--no-prune` skips this;
`flux prune --dry-run` previews it). A snapshot survives if it is pinned, named
(`keep_named`), the newest, among the `keep_last`, younger than `keep_within`, or the
newest of one of the last N hours/days/weeks/months/years (`keep_hourly` … `keep_yearly`).
Within a bucket, Flux prefers a snapshot that finished without errors. `max_age` overrides
the bucket rules. If the target is short of space (`min_free_space`,
`min_free_inodes_percent`, or not enough room for the next backup), Flux removes the oldest
unprotected snapshots first. The newest, pinned and named snapshots are never removed
automatically.

Defaults: newest 1, everything from the last day, then 7 daily, 4 weekly, 12 monthly and
5 yearly.

### System backups (root)

![System profiles in the app](docs/screenshots/system-dark.png)

System profiles back up what only root can read. They live in `/etc/flux-backup/`, keep
their history in `/var/lib/flux-backup/`, and run from system timers
(`/etc/systemd/system/flux-backup-<id>.timer`), so they work with nobody logged in.

```sh
sudo flux --system profile create etc --target /run/media/me/Backup -s /etc -s /root
sudo flux --system schedule install etc    # or set a schedule in the profile
sudo flux --system backup etc
sudo flux --system restore etc /etc/fstab --to /root/restored
```

In the app, system profiles appear under **System**. The app itself never runs as root: it
starts `flux-helper` through polkit (`pkexec`), which asks for an administrator's password
once. The helper serves only a fixed list of typed requests (the same ones the app uses for
your own profiles), refuses everything else, and exits when the app closes or after 15
minutes without requests.

Restores as root are hardened against symlink tricks. Every destination folder is opened one
step at a time without following symlinks, except symlinks owned by root or by the
administrator who asked. Files are created fresh (never written through an existing link),
and ownership, permissions and times are set on the open file rather than by path. When
root restores into a folder that another user owns, setuid/setgid bits are dropped and
device files are skipped.

System Vaults can't use a desktop keyring. Unlock the Vault once in the app and tick
**Keep it for scheduled backups**: the password goes into `/etc/flux-backup/keys/<id>`,
readable by root only. In a terminal, use `password_source = { file = "…" }` yourself.
Hooks of system profiles run as root.

### Hooks

```toml
[hooks]
pre_backup  = ["mount /mnt/nas"]          # non-zero exit aborts the backup
post_backup = ["notify-send 'Backup done' \"$FLUX_SNAPSHOT_ID\""]
on_error    = ["logger -t flux \"$FLUX_ERROR\""]
timeout = "10m"
```

Hooks run with `sh -c` and get `FLUX_EVENT`, `FLUX_PROFILE`, `FLUX_PROFILE_NAME`,
`FLUX_ENGINE`, `FLUX_SOURCES`, `FLUX_TARGET`, `FLUX_TRIGGER`, `FLUX_STATUS`,
`FLUX_SNAPSHOT_ID`, `FLUX_SNAPSHOT_PATH` and `FLUX_ERROR`. Their output goes to the run log.

### While a backup runs

Flux blocks system suspend through logind, and sends a desktop notification if a backup
fails or finishes with unreadable files. Scheduled runs also skip when on battery (if
`schedule.skip_on_battery`) and lower their CPU (`options.nice`) and I/O
(`options.io_priority`) priority. Manual runs keep normal priority.

### Exclude patterns

gitignore syntax, matched against absolute paths: `*.tmp` and `node_modules/` match
anywhere, `/home/me/Downloads` and `~/Videos` are anchored, `**/target/debug` matches
at any depth, and `!keep.tmp` re-includes. Directories with a valid `CACHEDIR.TAG` are
skipped by default.

## How a Mirror backup works

```
<target>/flux-backup/<host>/<user>/<profile>/
├── latest -> snapshots/2026-11-04_18-30-00
└── snapshots/
    └── 2026-11-04_18-30-00/
        ├── meta.json       when, status, statistics, per-file problems
        ├── manifest.zst    metadata of every path (owner, mode, xattrs, ACLs, hash)
        └── data/home/me/…  the files: open them with any file manager
```

1. **Scan** the sources in parallel.
2. **Compare** with the previous snapshot's manifest (streamed, no tree walk). Unchanged
   files become hard links, and the exact number of bytes to copy is known up front.
3. **Copy** new and changed files in parallel. Flux tries a reflink first (instant on
   Btrfs/XFS), then in-kernel `copy_file_range`.
4. **Commit**: write the manifest and metadata, `syncfs`, and atomically rename
   `.partial` to the snapshot id. An interrupted run resumes from `.partial`.

Safety: Flux refuses to write to `/run/media/…`, `/media/…` or `/mnt/…` when no drive is
mounted there, so it can't fill the system disk. With `target.fs_uuid` set, it only
writes to that exact filesystem. It also never backs up its own backups.

### Benchmark

`bench/bench.sh`: 100,000 small files plus 20 × 20 MiB (1 GiB), on tmpfs, on a 16-core
machine, compared with `rsync -aHAX --link-dest` (Back in Time's approach):

| scenario | Flux | rsync |
|---|---:|---:|
| initial full backup | 0.6 s | 4.1 s |
| no changes, new snapshot | 0.37 s | 9.1 s |
| 1% of small files + 1 large file changed | 0.37 s | 9.2 s |
| no changes (Flux default: skip, no snapshot) | 0.13 s | — |

With Back in Time's default flags (`rsync -aH`, no ACLs/xattrs), the 1%-change run took
7.3 s against Flux's 0.37 s. Much of the gap comes from Flux using every core while rsync
is single-threaded, so expect a smaller gap on spinning disks, where I/O dominates. The
snapshots were checked byte-for-byte against the source and against rsync's output.

## Building

Requires Rust 1.91+. The desktop app also needs Node.js 20+ and the WebKitGTK 4.1
development packages (Fedora: `webkit2gtk4.1-devel libsoup3-devel`; Debian/Ubuntu:
`libwebkit2gtk-4.1-dev libsoup-3.0-dev`).

```sh
cargo build --release      # CLI at target/release/flux (the GUI is not a default member)
cargo test                 # everything except the GUI
cd app && npm install && npm run tauri build    # app + .deb + .rpm + AppImage
```

`npm run tauri build` first runs `packaging/prepare-dist.sh`, which builds `flux` and
`flux-helper` and generates man pages and completions into `target/dist/`. The packages then
include all of it, plus the polkit policy, the `.desktop` file and AppStream metadata
(`packaging/`). To install without packages, run `packaging/prepare-dist.sh`, then
`sudo PREFIX=/usr packaging/install.sh` (`DESTDIR` is honoured). Tagging `v*` builds the
packages on CI and drafts a GitHub release (`.github/workflows/release.yml`).

The app ID is `io.github.neuralpunk.FluxBackup`. The desktop file is still `flux-backup.desktop`,
because Tauri names it after `productName`, which also names the packages. A Flatpak manifest
should rename it to the app ID with `rename-desktop-file` (and the icon with `rename-icon`).

`flux completions <bash|zsh|fish>` prints a completion script; `man flux-restore` and
friends document every command.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at
your option. Unless you explicitly state otherwise, any contribution you intentionally
submit for inclusion in this work, as defined in the Apache-2.0 license, is dual licensed
as above, without any additional terms or conditions.

Flux Backup is an independent project. Its design is inspired by Back in Time (GPL-2.0+),
but Flux contains none of Back in Time's code.
