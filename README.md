# Flux Backup

Fast, good-looking snapshot backups for Linux, with a CLI (`flux`) and a desktop app.

**Website: [neuralpunk.github.io/flux-backup](https://neuralpunk.github.io/flux-backup/)** ·
[Download the latest release](https://github.com/neuralpunk/flux-backup/releases/latest)

Flux takes the proven model of [Back in Time](https://github.com/bit-team/backintime)
(profiles, scheduled snapshots, include/exclude lists, smart retention, easy restore) and
rebuilds it in Rust. The aims are more speed, more capability and a modern interface.

> **Status:** 1.2. See the [changelog](CHANGELOG.md) for what's new. Read the
> [user guide](docs/USER_GUIDE.md) to get started, the [threat model](docs/THREAT_MODEL.md)
> for what Flux protects against, and [FORMATS.md](docs/FORMATS.md) for how your backups
> are stored (frozen for 1.x). [PLAN.md](docs/PLAN.md) and [TRACKER.md](docs/TRACKER.md)
> show how it was built.

### What 1.2 adds

- **Previous versions from the file manager.** Right-click a file or folder in Nautilus,
  Nemo or Dolphin: *Previous versions…* opens its versions; *Restore deleted files here…*
  shows what's gone from that folder, ready to restore.
- **Browse in Files.** Open any snapshot, read-only, in the file manager you know.
- **A recovery code.** A long code to print or keep safe, that opens an encrypted backup
  when the password is lost.
- **Leave out, or back up, from the file manager too.** *Leave out of backups…* and
  *Back up this folder…* in the same right-click menu (one *Flux Backup* submenu in
  Nautilus).
- **Compact, by default.** More on the screen: a backup's Overview fits on one screen,
  its settings beside it, and *All backups* lines every backup up in columns. A sidebar
  that folds to icons (F9), a text size, and the desktop's accent colour. *Full* is one
  click away.
- **Preview before restoring**, and **undo** after deleting snapshots.
- **Safe to unplug.** A USB drive can be unmounted and powered off when its backup ends.
- **A warning when the drive is failing** (its SMART or NVMe report), and when a FAT32 or
  exFAT drive doesn't suit a plain-folder backup.
- **Notifications with buttons** (*Try again*, *Open*), **quiet hours**, and a **calm
  weekly note** when every backup is fine.
- **Time left** wherever a backup or restore runs, the tray's tooltip included.
- **Tidier.** Delete several snapshots at once; see what each backup changed and the size
  of every folder you back up or leave out; skip what a project's `.gitignore` ignores.
- **Sizes hidden by default** in new encrypted Vaults.

### What 1.1 refines

- **Restore tests that leave nothing behind.** Files are read back and checked as they
  stream; nothing is restored or written anywhere, so encrypted backups stay encrypted.
- **Flux Sentry.** Unusual-change detection that learns each backup's own normal: a
  quiet backup warns early, a busy one doesn't cry wolf. Setting up a backup asks how it
  should watch, and a green tag on the backup's page says it does.
- **Repair, not just detection.** A Vault can keep Reed–Solomon repair data (5% more
  space, say): *Verify every byte* then rebuilds what a failing drive has damaged.
- **Restore tests that state their confidence.** *Enough to be sure* tests as many random
  files as it takes to say "if 1% or more of the files were damaged, this test would have
  found it (99% certain)": 459 files, however big the backup.
- **Settings at a glance.** Each backup's settings summed up on its Overview, and every
  backup's side by side in a sortable table on *All backups*.
- **A checklist you can act on.** Each line of *Could you recover it on another
  computer?* has a button that does it.
- **Fewer surprises.** A warning (with the fix) when a scheduled backup's password is set
  to be asked for each time; times typed the way you write them, on a 12- or 24-hour
  clock, and custom times previewed; *Exclude Files/Directories*; closing the window
  keeps Flux in the tray.
- **Clearer at a glance.** One count of what needs you, red only for what isn't
  protected, statuses that say what's wrong; a calmer Overview; what each check proves;
  plain words for durations and storage.
- **Man pages with examples** for every command.

### What 1.0 adds

- **Recover anywhere.** On a new computer, *Recover an existing backup* finds your
  backups on a drive or server and sets them up again. Without Flux at all,
  `flux-vault-recover` (kept next to each Vault on a drive) lists, restores and checks
  them. Snapshots can be exported as `.tar.zst` or `.zip` with checksums.
- **More than one copy.** A backup can go to several places, some kept elsewhere, each
  stored its own way, or copied from the first.
- **Know it works.** Restore tests on a schedule, a recovery-readiness checklist, and a
  warning from Flux Sentry (with the snapshot before kept safe) when a backup looks like
  ransomware's work.
- **Told where you are.** Webhooks to ntfy, Gotify, chats or healthchecks-style monitors.
- **Easier to set up.** Templates, a rule tester that says why a file is or isn't backed
  up, network rules, a calendar of snapshots, and where each backup's space and time go.
- **Stronger Vaults.** Password hashing levels, an optional unlock file, hidden sizes,
  and sparse files kept sparse.

## Install

Releases ship `.deb` and `.rpm` packages and an AppImage. The packages contain the desktop
app, the `flux` command, the system-backup helper, man pages and shell completions. To
build from source, see [Building](#building).

## Two engines, one tool

| | **Mirror** | **Vault** |
|---|---|---|
| Format | Plain folders, unchanged files hard-linked | Deduplicated chunk repository |
| Browse without Flux | Yes, with any file manager | Via `flux mount`, or `flux-vault-recover` (no Flux needed) |
| Dedup | Per unchanged file | Across files and snapshots |
| Compression / encryption | — / use an encrypted disk | zstd / XAES-256-GCM |
| Targets | Local drives, SSH (server needs rsync) | Local drives, SSH/SFTP |

Each profile picks its engine.

### Compatibility mode: rsync and Borg

For backups that never depend on Flux, a profile can use **compatibility mode**: the data
is written by an industry-standard tool, in its native format, and Flux is the interface.

- `--engine rsync`: every snapshot is a plain folder written by the system `rsync`
  (`--link-dest`, so unchanged files are hard links). Each snapshot records the exact
  rsync command that wrote it.
- `--engine borg`: a standard Borg 1.x repository written by the system `borg`
  (install `borgbackup`), encrypted with `repokey-blake2` and deduplicated.

Next to the backups Flux keeps a recovery kit: `FLUX-RECOVERY.txt`, a plain-text guide
with the exact commands for that profile (list, browse, restore, back up, prune, check),
written for people new to the tool and for experts alike, and `flux-backup.sh`, which
runs the same backup with the tool alone. `flux recovery <profile>` prints the kit.
Everything else (browsing, restores, retention with pins, schedules, mount) works as with
the other engines. See the
[user guide](docs/USER_GUIDE.md#compatibility-mode-backups-that-dont-need-flux).

## Desktop app

![Overview with a backup running](docs/screenshots/overview-dark.png)

| Snapshots and file browser | Settings |
|---|---|
| ![Snapshots](docs/screenshots/snapshots-dark.png) | ![A backup's settings](docs/screenshots/settings-dark.png) |
| **Activity log** | **Light theme** |
| ![Activity](docs/screenshots/activity-dark.png) | ![Light theme](docs/screenshots/overview-light.png) |
| **Compatibility mode** | **Recovery kit** |
| ![Choosing Vault, rsync or Borg while setting up](docs/screenshots/compat-dark.png) | ![Recovery kit](docs/screenshots/recovery-dark.png) |
| **Find in all snapshots** | **Versions of a file** |
| ![Search results across snapshots](docs/screenshots/search-dark.png) | ![Versions of a file](docs/screenshots/versions-dark.png) |
| **Restore preview** | **Space forecast and drive health** |
| ![What a restore would change](docs/screenshots/restore-dark.png) | ![Used space, when the drive is full, and the drive's health](docs/screenshots/space-dark.png) |
| **Warnings in words** | **Preferences: pausing, quiet hours** |
| ![All backups, with the one that needs attention explained](docs/screenshots/health-dark.png) | ![Preferences with schedules paused, the weekly note and quiet hours](docs/screenshots/preferences-dark.png) |
| **Export to other tools** | **Size estimate** |
| ![Export dialog](docs/screenshots/export-dark.png) | ![New backup, step one, with a size estimate](docs/screenshots/new-dark.png) |
| ![Recovering an existing backup on a new computer](docs/screenshots/recover-dark.png) | ![Templates for a new backup](docs/screenshots/templates-dark.png) |
| ![What's backed up, and the rule that leaves node_modules out](docs/screenshots/rules-dark.png) | ![The snapshot calendar](docs/screenshots/calendar-dark.png) |
| ![Storage and recovery readiness](docs/screenshots/storage-dark.png) | ![Exporting a snapshot as an archive](docs/screenshots/archive-dark.png) |
| ![A backup's settings at a glance](docs/screenshots/glance-dark.png) | ![Every backup's settings side by side](docs/screenshots/table-dark.png) |
| ![Setting up a backup: Flux Sentry, chosen for you and one click to change](docs/screenshots/sentry-dark.png) | |

The app (Tauri 2, Svelte 5, Tailwind 4, and the GNOME-style "At home on the desktop" design
in [docs/DESIGN.md](docs/DESIGN.md)) does everything the CLI does:
- **Overview:** the running backup's progress, last and next backup, snapshots, its
  settings at a glance, free space, recent backups, and maintenance (check, verify every
  byte, free up space). Warnings in
  words when something needs you (a drive not seen for days, repeated failures, little
  space), and a chart of used space with a forecast of when the drive is full.
- **Snapshots:** every snapshot with an instant file browser. "What changed" compares with the
  one before; name, pin, other comparisons and delete are in the ⋯ menu. **Find in all
  snapshots** searches by name (deleted files included), **Versions** lists every version
  of a file, and double-click opens a read-only copy. Restore to the original place or
  another folder, with a preview of what would be added, kept aside or replaced.
- **Activity:** every run with its log, warnings and hook output; a Problems filter and
  **Copy diagnostics** for bug reports.
- **Setting up a backup:** a short guide asks what, where and how it's stored (a Vault:
  encrypted or not); the rest (when, what to keep, Flux Sentry) is chosen for you and
  shown on the last screen, one click to change; then an offer to run the first backup.
  Connected USB drives are offered (and tied to their filesystem, so an unplugged drive is
  never mistaken for its empty mount point). Everything else is a row with its current
  value that opens just that group: exclusions with suggestions and real sizes, connection
  details, how backups are stored (including rsync and Borg), more schedules (wait for
  power, not on metered connections, monthly verification), retention rules with a live
  preview of what would be kept, hooks, and performance. A new backup shows how much it
  will copy, following the exclusions.
- **Right-click** a backup in the sidebar (or use the **⋯** button next to *Back up now*)
  to back it up, open its snapshots, activity or settings, duplicate, export or delete it,
  or pause all scheduled backups. Snapshots, files and runs have the same kind of menu.
- **Preferences:** pause scheduled backups (an hour, until tomorrow, until resumed),
  notifications for successful backups, start in the tray at login, keep running in the
  tray when the window closes, theme, 12- or 24-hour clock, and the
  exclusions new backups start with.
- **Compatibility mode:** a **Usable without Flux** choice lets rsync or Borg write the
  backups in their own formats; the Overview's **Recovery guide** shows the kit kept next to
  them, copyable section by section and saveable to a USB stick.
- **System backups:** profiles for the whole computer (`/etc`, `/root`, other users' files)
  in their own section. Click **System backups**; Flux explains what it needs, then your
  desktop asks for an administrator password, and you manage them like your own; they lock
  again after 15 idle minutes. See
  [System backups](#system-backups-root).
- A tray icon that lights up while a backup runs (open, back up everything, pause or resume
  schedules, quit), keyboard shortcuts (`?` lists them), and dark, light or automatic
  themes.

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
flux profile export home -f borgmatic -o ~/   # or flux, restic, rsync, borg
flux profile import home.flux-profile.toml
flux profile duplicate home         # a copy to change (e.g. a second drive)
flux diagnostics home               # what to paste when asking for help

flux diff home                    # latest snapshot vs. the one before
flux diff home --live --path ~/Documents   # what changed on disk since the latest snapshot
flux find home report             # search every snapshot (part of a name; or '*.odt')
flux history home ~/thesis.tex    # every version of a file, and where to restore it from
flux prune home --dry-run         # what retention would remove, and why each one is kept
flux name home latest before-upgrade && flux pin home before-upgrade
flux note home latest "Before the upgrade" && flux tag home latest upgrade   # labels only
flux snapshots home --tag upgrade
flux profile destination add home usb --target /run/media/$USER/STICK --engine vault   # a second place
flux snapshots home+usb                                     # browse that place
flux log home                     # recent runs; `--run last` shows one run's events
flux mount home ~/snapshots       # every snapshot as a read-only folder (any engine)
flux recovery home                # compatibility mode: how to use the backups without Flux
```

Every command accepts `--json` for scripting and `--system` for root-mode profiles in
`/etc/flux-backup`. Setting `FLUX_HOME=/some/dir` keeps all of Flux's files under one
directory. Exit codes: 0 ok, 1 error, 2 usage, 3 invalid profile, 4 finished but some
files could not be backed up or restored.

### Vault: deduplicated and encrypted

```sh
flux profile create docs --target /run/media/$USER/Backup --source ~/Documents --engine vault
flux key init docs                # choose the password now: creates the Vault, keeps it in the keyring
flux backup docs                  # runs without asking (so do scheduled ones)
flux check docs --read-data       # verify every stored byte
flux key list docs                # passwords (key slots) that open the Vault
flux key add docs                 # add another password; `key passwd` changes it
flux key store docs               # save the password in the keyring (for scheduled backups)
flux prune docs                   # apply retention and reclaim space
flux key strengthen docs --level strong   # hash the password harder (instant; no data rewritten)
flux key unlock-file docs create /run/media/$USER/STICK/docs.flux-unlock   # password + file
flux unlock docs                  # clear a lock left by a job that died (no password needed)
flux vault upgrade docs           # move a Vault made before 1.0 to format 2 (keeps holes)
```

All Vault profiles on a target share one repository (`<target>/flux-vault`), so identical
files deduplicate across profiles and machines. Everything stored (contents, file names,
metadata) is encrypted with XAES-256-GCM under a key protected by Argon2id. Chunk
ids are keyed, so they don't reveal content. The format and threat model are documented in
[docs/VAULT_FORMAT.md](docs/VAULT_FORMAT.md); every format Flux writes (Mirror, snapshots,
profiles, the compatibility modes) is in [docs/FORMATS.md](docs/FORMATS.md), frozen for
1.x.

Password sources, set with `[vault] password_source`:
- `{ from = "keyring" }` (default): the desktop keyring;
- `{ from = "prompt" }`;
- `{ from = "file", path = "…" }`;
- `{ from = "command", command = "pass show backup" }`.

`FLUX_PASSWORD` overrides all of them. Backups only forget snapshots that retention no
longer keeps; `flux prune` reclaims their space.

Vault compared with Borg (`repokey-blake2`, `--compression zstd,3`) on the 1 GiB benchmark
data (tmpfs, 16 cores, median of three runs of `bench/bench.sh`; both make a snapshot even
when nothing changed):

| scenario | Flux Vault | Borg 1.4 |
|---|---:|---:|
| initial backup | 1.73 s | 15.81 s |
| no changes, new snapshot | 0.51 s | 8.17 s |
| 1% of small files + 1 large file changed | 0.51 s | 8.18 s |
| repository size | 864 MiB | 869 MiB |

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
  requests in flight, so latency costs little. Locks are lock files the same on a local
  disk and over SFTP, so a Vault on a NAS used both ways has one lock; they expire if a
  job dies, and `flux unlock` clears them at once.
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
flux pause --tomorrow         # pause scheduled backups (--for 2h; `flux resume`)
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
machine (median of three runs, rsync 3.5, Borg 1.4.5), compared with
`rsync -aHAX --link-dest` (Back in Time's approach). With borg installed, the same script
also compares Vault with Borg (the table above). It touches nothing outside its work
folder:

| scenario | Flux | rsync |
|---|---:|---:|
| initial full backup | 0.46 s | 4.28 s |
| no changes, new snapshot | 0.26 s | 7.67 s |
| 1% of small files + 1 large file changed | 0.26 s | 7.73 s |
| no changes (Flux default: skip, no snapshot) | 0.16 s | — |

With Back in Time's default flags (`rsync -aH`, no ACLs/xattrs), rsync's 1%-change run took
7.3 s in the 0.2 measurement. Much of the gap comes from Flux using every core while rsync
is single-threaded, so expect a smaller gap on spinning disks, where I/O dominates. The
snapshots were checked byte-for-byte against the source and against rsync's output.

## Building

Requires Rust 1.99+. The desktop app also needs Node.js 26 (LTS; `app/.nvmrc` says so for
nvm and fnm) and the WebKitGTK 4.1 development packages (Fedora: `webkit2gtk4.1-devel
libsoup3-devel`; Debian/Ubuntu: `libwebkit2gtk-4.1-dev libsoup-3.0-dev`). Node only builds
the app's interface; nothing in the installed app runs on it.

```sh
cargo build --release      # CLI at target/release/flux (the GUI is not a default member)
cargo test                 # everything except the GUI
cd app && npm ci && npm run tauri build         # app + .deb + .rpm + AppImage
```

`npm run tauri build` first runs `packaging/prepare-dist.sh`, which builds `flux`,
`flux-helper` and `flux-vault-recover` (statically linked when the musl target is installed:
`rustup target add x86_64-unknown-linux-musl`) and generates man pages and completions into
`target/dist/`. The packages then
include all of it, plus the polkit policy, the `.desktop` file and AppStream metadata
(`packaging/`). To install without packages, run `packaging/prepare-dist.sh`, then
`sudo PREFIX=/usr packaging/install.sh` (`DESTDIR` is honoured). Tagging `v*` builds the
packages on CI and drafts a GitHub release (`.github/workflows/release.yml`).

The app ID is `io.github.neuralpunk.FluxBackup`. The desktop file is still `flux-backup.desktop`,
because Tauri names it after `productName`, which also names the packages. A Flatpak manifest
should rename it to the app ID with `rename-desktop-file` (and the icon with `rename-icon`).

`flux completions <bash|zsh|fish>` prints a completion script. Every command has a man page:
`man flux`, `man flux-restore` and so on, installed by the packages (74 pages) and by
`install.sh`. Each says what the command does and what it changes, with examples of basic
use, its exit status and related pages; `flux <command> --help` ends with the same
examples.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at
your option. Unless you explicitly state otherwise, any contribution you intentionally
submit for inclusion in this work, as defined in the Apache-2.0 license, is dual licensed
as above, without any additional terms or conditions.

Flux Backup is an independent project. Its design is inspired by Back in Time (GPL-2.0+),
but Flux contains none of Back in Time's code.
