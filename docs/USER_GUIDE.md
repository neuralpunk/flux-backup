# Flux Backup user guide

Flux Backup keeps **snapshots** of your folders: complete pictures of how they were at a
moment, on an external drive or an SSH server. It runs on a schedule, when a drive is
plugged in, or when you ask. You can use the desktop app, the `flux` command, or both;
they share the same profiles and see each other's backups.

- [Installing](#installing)
- [Ideas in two minutes](#ideas-in-two-minutes)
- [Your first backup](#your-first-backup)
- [Mirror or Vault?](#mirror-or-vault)
- [Compatibility mode: backups that don't need Flux](#compatibility-mode-backups-that-dont-need-flux)
- [Where backups go](#where-backups-go)
- [Choosing what to back up](#choosing-what-to-back-up)
- [Automatic backups](#automatic-backups)
- [Getting files back](#getting-files-back)
- [What to keep](#what-to-keep)
- [Vault passwords](#vault-passwords)
- [Checking your backups](#checking-your-backups)
- [System backups](#system-backups)
- [Hooks](#hooks)
- [Keeping an eye on things](#keeping-an-eye-on-things)
- [Exporting and importing profiles](#exporting-and-importing-profiles)
- [Profile reference](#profile-reference)
- [Environment variables](#environment-variables)
- [Troubleshooting](#troubleshooting)

## Installing

**Packages.** Releases provide `.deb` (Debian, Ubuntu), `.rpm` (Fedora, openSUSE) and an
AppImage. The deb and rpm packages install everything: the app (`flux-backup`), the
command-line tool (`flux`), the system-backup helper and its polkit policy, man pages and
shell completions.

The AppImage contains the app and `flux`, but not the system-backup helper: root can't run
programs from inside an AppImage. Install a package if you need system backups.

**From source.** You need Rust 1.91 or newer. The app also needs Node.js 20+ and WebKitGTK
4.1 (Fedora: `webkit2gtk4.1-devel libsoup3-devel`; Debian/Ubuntu:
`libwebkit2gtk-4.1-dev libsoup-3.0-dev`).

```sh
packaging/prepare-dist.sh                  # flux, flux-helper, man pages, completions
(cd app && npm install && npm run tauri build -- --no-bundle)   # the desktop app (optional)
sudo PREFIX=/usr packaging/install.sh
```

At run time Flux uses `ssh` for SSH targets, `rsync` on the *server* for Mirror over SSH,
`fusermount3` for `flux mount`, `secret-tool` for the desktop keyring, and `pkexec` for
system backups. Each is only needed for the feature that uses it.

## Ideas in two minutes

- A **profile** says what to back up, where to, how, and when. You can have as many as you
  like (say, "Home folder" daily to a USB drive, and "Documents" hourly to a NAS).
- A **snapshot** is one backup run. Every snapshot is complete: you never need an older
  one to restore from a newer one. Unchanged files cost almost nothing.
- The **engine** decides how snapshots are stored: **Mirror** (plain folders) or **Vault**
  (deduplicated, compressed, encrypted).
- **Retention** rules decide which old snapshots are removed, like "keep 7 daily, 4
  weekly, 12 monthly". Named and pinned snapshots are kept.

## Your first backup

**In the app:** click **New backup** in the sidebar. A short guide asks one question per
step: **What** to back up (your home folder by default), **Where** to keep it (a
connected drive, a folder or an SSH server), **Protection** (plain folders, encrypted, or
written by rsync or Borg so the backups never depend on Flux; an encrypted one asks for
its password right there), and **When**. **Review** shows every answer with a way back to
change it and the name it will have; **Create backup** sets it up and offers **Back up
now** for the first run. Later, a backup's **Settings** show the same questions on one
page. Everything else is one click away and has sensible defaults: each group of settings is a
row showing its current value (*Leave some things out › caches*, *How backups are stored ›
Plain folders*, *Keep and remove snapshots*, *Hooks › None*…). Click one to open just
that group in a dialog over the editor; **Done** closes it, and nothing is saved until you click
**Save**. A row with a problem says so in red.

While you choose what to back up, Flux counts what it adds up to after the exclusions
("About 32 GiB in 51,900 files") and warns if that's more than the free space where it
would go.

**In a terminal:**

```sh
flux profile create home --target /run/media/me/Backup --source ~
flux backup home
flux snapshots home
```

The first backup copies everything. Later ones only copy what changed, and take seconds.

Each backup in the sidebar shows where it goes and how it's doing ("Backed up 2 hours
ago", "Failed 2 days ago"). **Right-click** one, or use the **⋯** button next to
*Back up now*, to back it up, open its snapshots, activity or settings, see its recovery
guide, or **delete** it. Deleting a profile never deletes the backups it made: they stay
where they are. The menu key or Shift+F10 opens the same menu from the keyboard.

## Mirror or Vault?

| | Mirror | Vault |
|---|---|---|
| Stored as | plain folders, one per snapshot | packs of chunks |
| Open without Flux | yes, with any file manager | no (use Flux, or `flux mount`) |
| Unchanged files | hard links (no extra space) | stored once across *all* snapshots, files and computers |
| Renamed or moved files | copied again | not stored again |
| Compression | no | zstd |
| Encryption | use an encrypted drive (LUKS) | built in (XChaCha20-Poly1305, Argon2id) |
| SSH servers need | `rsync` | only SFTP |

Pick **Mirror** when you want to reach your files in an emergency with nothing but a
file manager. Pick **Vault** for untrusted places (a friend's server, a drive that might
be lost), for many similar machines sharing one target, or to save space.

The choice is per profile and can't be changed later; make a new profile instead.

## Compatibility mode: backups that don't need Flux

Mirror and Vault are Flux's own formats. If you'd rather your backups never depend on
Flux, for example so that you're covered even if Flux stopped being developed, use
**compatibility mode**. It stores everything in the native format of a standard tool.
Flux is then only the interface: you keep the app, the schedules, the file browser and
restores, but nothing needs Flux to read the backups later.

| | **rsync** | **Borg** |
|---|---|---|
| Stored as | plain folders written by the system `rsync`, one per snapshot | a standard Borg 1.x repository; one archive per snapshot |
| Without Flux | any file manager, `cp`, or `rsync` | `borg list`, `borg mount`, `borg extract` |
| Unchanged data | hard links (`rsync --link-dest`) | deduplicated chunks, across files and archives |
| Compression / encryption | no / use an encrypted drive (LUKS) | zstd (or any Borg compression) / `repokey-blake2` with a passphrase |
| Needs | `rsync` 3.1+ here, and on the server for SSH targets | `borg` 1.2–1.4 here (install `borgbackup`), and on the server for SSH targets |

Pick **rsync** if you want to open your backups with a file manager. Pick **Borg** if you
want deduplication, compression and encryption with a tool many people already know.

```sh
flux profile create home --target /run/media/me/Backup --source ~ --engine rsync
flux profile create docs --target me@nas:/srv/backup --source ~/Documents --engine borg
```

**You don't need to know rsync or Borg.** Flux runs them for you, and next to your
backups it keeps a **recovery kit**:

- `FLUX-RECOVERY.txt`: a plain-text guide written for *this* profile and drive or
  server. It says where your files are and which commands to type to list, browse and
  restore them (one file, a folder, everything, an older version). It starts with the
  basics for people who have never used the tool or a terminal, and ends with a
  reference section for people who know it well.
- `flux-backup.sh`: the same backup with nothing but the tool, so you can keep backing up
  without Flux (`sh flux-backup.sh`, or `sh flux-backup.sh --dry-run` to look first).
  Snapshots it makes show up in Flux like any other.

`flux recovery home` prints the guide, `--script` prints the script, and
`--save /some/folder` copies both somewhere safe, such as a USB stick or your notes.

**In the app**, the profile's *Where to keep it* section has a **Usable without Flux** row:
**Choose…** explains the options in plain words (what each is good for, whether the tool is
installed, and the exact details for experts) and switches the profile to rsync or Borg.
Compatibility-mode profiles show a **Usable without Flux** card on their Overview; its
**Recovery guide** button opens the kit, section by section with copy buttons, plus the
script, and **Save a copy…** writes both into a folder you pick. The guide is reachable even
while a Borg repository is locked, for the day the passphrase is the problem.

### If you know rsync

The layout is the classic one:

```text
<target>/flux-rsync/<computer>/<user>/<profile>/
  FLUX-RECOVERY.txt  flux-backup.sh  flux-repo.toml
  latest -> snapshots/<newest>
  snapshots/2026-11-07_18-30-00/
    data/<absolute path>   rsync --archive --hard-links --relative --link-dest=../../<previous>/data
    rsync-command.txt      the exact command line, rsync version and exit status
    rsync-filter.txt       the exclusion rules used (rsync filter syntax, first match wins)
    meta.json              Flux's notes (statistics, name, pin); optional
```

Each backup runs into `snapshots/.partial` and is renamed when rsync exits with 0, 23
or 24, so an interrupted backup continues next time. Your gitignore-style exclusions are
translated into rsync filter rules with the same meaning. rsync has no "skip folders with a
`CACHEDIR.TAG`" option, so Flux (and the script, with `find`) looks for those folders first
and lists them as ordinary rules. Extra options go in `[rsync] extra_args` (for example
`["--compress"]` for slow links); options that would break the layout are refused.
Everything else works as usual: browsing, diff, find, history, restores (Flux's own,
symlink-safe restore reads the folders), `flux mount`, retention with pins and names,
`flux check --read-data`, schedules and hooks.

### If you know Borg

```text
<target>/flux-borg/<computer>/<user>/<profile>/
  FLUX-RECOVERY.txt  flux-backup.sh  flux-patterns.txt
  repo/              borg init --encryption=repokey-blake2 --make-parent-dirs
```

- One archive per backup (also when nothing changed: deduplication makes that nearly
  free), named `%Y-%m-%d_%H-%M-%S` in local time.
- `borg create --compression=zstd,3 [--exclude-caches] [--exclude-if-present=…]
  [--one-file-system] [--noacls] [--noxattrs] --patterns-from=flux-patterns.txt`, plus
  `[borg] extra_args`. The patterns file holds your exclusions in Borg's syntax
  (first match wins; `!` for exclusions, so excluded folders aren't entered, like
  gitignore). Borg can't skip files by size or follow symlinks; those options are
  ignored with a warning.
- Flux's names and pins are JSON in the archive comment (`{"flux":1,"pinned":true}`),
  so they live in the repository. Retention is Flux's policy, applied with
  `borg delete` + `borg compact`; the guide also gives the closest `borg prune` line.
- Flux uses your normal `~/.config/borg` and `~/.cache/borg`, sets
  `BORG_RELOCATED_REPO_ACCESS_IS_OK=yes`, and passes the passphrase as
  `BORG_PASSPHRASE` (never logged). The guide's `BORG_PASSCOMMAND` reads it from the
  same place Flux does (for the keyring: `secret-tool lookup service flux-backup
  profile <id>`).
- Restores from Flux extract into a temporary folder with `borg extract`, then go through
  Flux's restore (identical files skipped, differing ones kept aside). `flux mount`
  works without Borg's FUSE extras: each file opened is extracted with
  `borg extract --stdout`.
- Archives you make with `borg create` yourself show up in Flux too.

## Where backups go

### An external drive

When you pick a drive in the app (or create a profile pointing at a mounted drive), Flux
remembers the drive's **filesystem UUID**. If that drive isn't connected, Flux refuses to
write into the empty mount point, so it never fills your system disk by mistake.

Backups go into a `flux-backup/` folder on the drive (Mirror) or `flux-vault/` (Vault).

### An SSH server

```sh
flux profile create docs --target me@nas:/srv/backup --source ~/Documents --engine vault
flux ssh test docs          # can we log in, write, and (for Mirror) run rsync?
```

Flux uses your system's `ssh`, so `~/.ssh/config`, agents and hardware keys all work.
Scheduled backups can't type passwords or accept new host keys. Set up a key once, and
point the profile at it with `target.identity_file = "~/.ssh/flux_backup_ed25519"` (or
in the app: *Where to keep it* › **Connection details**):

```sh
flux ssh keygen             # creates ~/.ssh/flux_backup_ed25519 (no passphrase, for timers)
flux ssh copy-id docs       # installs it on the profile's server (asks for your password once)
```

Mirror over SSH needs `rsync` on the server and keeps hard-linked snapshots there.
Vault over SSH only needs SFTP; the server never sees your data unencrypted.
`target.bandwidth_limit` (e.g. `"5MB"`) caps the upload rate.

## Choosing what to back up

`sources.paths` lists files and folders. Exclusions use gitignore syntax, matched against
absolute paths:

| Pattern | Matches |
|---|---|
| `*.tmp` | any file ending in `.tmp`, anywhere |
| `node_modules/` | every folder named `node_modules` |
| `~/Videos` | that folder only |
| `**/target/debug/` | `target/debug` at any depth |
| `!keep.tmp` | re-includes something an earlier pattern excluded |

Also available: skip folders with a `CACHEDIR.TAG` (on by default), skip folders that
contain a marker file (`exclude_if_present = [".nobackup"]`), skip big files
(`exclude_larger_than = "4GB"`), and stay on one filesystem (`one_file_system`).

In the app, *What to back up* › **Leave some things out** suggests common exclusions
(caches, trash, build output) with how much space each takes in your sources.

## Automatic backups

Schedules are systemd timers, so backups run even when the app is closed. The timer is
created when you save a profile with a schedule (or with `flux schedule install <id>`).

| Setting | Example | Meaning |
|---|---|---|
| `trigger = "calendar"` | `on_calendar = "daily"`, `"Mon..Fri 18:00"`, `"*-*-* 03:30"` | at those times; with `catch_up`, a time missed while the computer was off runs at the next start |
| `trigger = "interval"` | `every = "4h"` | when the last successful backup is older than that |
| `on_connect = true` | | also when the profile's drive is plugged in |
| `skip_on_battery = true` | | scheduled runs wait for mains power |
| `skip_on_metered = true` | | scheduled runs to a server wait while the connection is metered (mobile data, a phone's hotspot, as NetworkManager reports it) |

```sh
flux schedule status        # every profile's timer and next run
flux schedule sync          # make timers match all profiles
flux schedule remove home
```

Scheduled backups run at low CPU and I/O priority (`options.nice`,
`options.io_priority`), keep the computer from suspending while they run, and notify you
if something went wrong (and, if you turn it on in **Preferences**, when they succeed).

**Pausing.** For a presentation, a slow connection or a full disk: *Pause all scheduled
backups…* in any backup's menu, in **Preferences**, or in the tray, for an hour, until
tomorrow morning (7:00) or until you resume them. The sidebar shows they're paused, with
**Resume**. Backups that come due meanwhile are skipped, not queued; backing up by hand
still works.

```sh
flux pause --for 2h      # or --tomorrow, or nothing: until `flux resume`
flux resume
```

**Upkeep.** In *Settings › More schedule options*, a backup can verify every stored byte
once a month after a scheduled backup (`[maintenance] verify_monthly = true`); you're
notified if anything is wrong. Vault backups also free the space of removed snapshots once
a week (`free_space_weekly`, on by default): each backup only marks old snapshots as
removed, and this deletes the data only they used. Results appear in the run's log in
*Activity*.

**Preferences** (bottom of the sidebar) also hold: start in the tray when you log in, the
theme, and the exclusions new backups start with.

## Getting files back

### In the app

Open **Snapshots**, pick a snapshot from the list, browse to what you need, select it,
and **Restore**. You can restore:

- **into another folder**: the safest choice; nothing on your computer changes;
- **back to where it was**: identical files are skipped. Different files are kept as
  `name.flux-<snapshot>` unless you choose to replace them.

Before you restore, the dialog shows what would happen: how many items are added, which
files differ from what's there now (and whether they'd be kept or replaced), and how many
are already identical. Replacing files needs the red **Restore and replace** button.

**Don't know which snapshot?** Type part of the name in **Find in all snapshots** (above
the snapshot list) and press Enter. Every file or folder with that in its name is listed,
from all snapshots; files you've since deleted are marked *Deleted* with the last snapshot
that had them. Click one to show it in its snapshot. `*` works as a wildcard (`*.pdf`), and
a `/` matches whole paths (`Documents/*.odt`). `flux find` matches the same way.

The first search of a Vault, Borg, rsync or SSH backup reads every snapshot's listing;
Flux keeps an index of them in its cache folder (readable only by you), so later searches
and *Versions* are quick. Plain folders on a drive need none.

**Versions of a file.** Right-click a file › *Versions of this file…* (or select it and
click **Versions**) lists each different version with its date and size, and which
snapshots hold it. Open one to look at it, or restore it.

**What changed** shows the differences from the previous snapshot; the ⋯ menu also compares with the files on
disk now.

To just look at a file, double-click it (or right-click › **Open a copy**). Flux restores a
read-only copy into a temporary folder and opens it with your usual app; nothing on your
computer changes. Save it somewhere else if you want to keep it, or restore it. Temporary
copies are removed after a day.

### In a terminal

```sh
flux ls home                                   # the newest snapshot's top level
flux ls home ~/Documents --snapshot 2026-11-01  # a folder in the snapshot of that day
flux find home '*.odt'                         # search all snapshots
flux history home ~/notes.md                   # every version of one file
flux diff home --live                          # what changed since the newest snapshot
flux restore home ~/Documents/report.odt --to ~/Restored
flux restore home ~/Documents                   # back in place (asks first)
```

Snapshots are chosen by id, by a unique start of an id, by name, or as `latest`. Mirror
ids are start times (`2026-11-01_03-00-00`), so `2026-11-01` picks that day's snapshot
when there is one.

### As folders: `flux mount`

```sh
mkdir ~/snapshots
flux mount home ~/snapshots
ls ~/snapshots/latest/home/me/
```

Every snapshot appears as a read-only folder named after its start time, plus a `latest`
link. This works for every engine, including Vaults and backups on SSH servers, so you can
open old files with any program or compare folders with your usual tools. Press Ctrl-C
(or run `fusermount3 -u ~/snapshots`) to unmount. `--snapshot <spec>` shows just one
snapshot at the mount point.

## What to keep

```toml
[retention]
keep_last = 1          # always keep the newest N
keep_within = "1day"   # keep everything from the last day
keep_hourly = 24
keep_daily = 7
keep_weekly = 4
keep_monthly = 12
keep_yearly = 5
max_age = "10y"        # remove anything older, whatever the buckets say
keep_named = true      # named and pinned snapshots are never removed
min_free_space = "50GB"          # remove oldest snapshots until this much is free
min_free_inodes_percent = 5
```

Each bucket keeps the newest snapshot of each of its last N hours, days, weeks, months or
years. Pruning runs after every backup. Run it yourself with a preview:

```sh
flux prune home --dry-run
```

In the app (*What to keep* › **Keep and remove snapshots**), the retention editor shows which of your current snapshots each rule would
keep or remove, and why.

To keep a snapshot forever, **pin** it (`flux pin home <snapshot>`). To mark one, **name**
it (`flux name home <snapshot> before-upgrade`). By default, named snapshots are kept too.

## Vault passwords

An encrypted Vault (or Borg repository) needs its password to back up, browse or restore.
Where it comes from is set per profile, in `vault.password_source` or
`borg.password_source`:

| `password_source` | Use |
|---|---|
| `{ from = "keyring" }` | the desktop keyring (default): asked once, then remembered |
| `{ from = "prompt" }` | asked every time; scheduled backups can't run |
| `{ from = "file", path = "…" }` | first line of a file (make it mode 0600) |
| `{ from = "command", command = "pass show backup" }` | first line printed by a command |

`FLUX_PASSWORD` overrides all of them, for scripts.

**The password is chosen when you set the backup up, not at its first run.** In the app,
choosing an encrypted format in a new backup (or turning on *Encrypt with a password* in
its Settings) asks for it twice right there; when
you click *Create backup*, Flux creates the encrypted Vault (or Borg repository) with it
and keeps it in the keyring (or writes it to the key file, if that file doesn't exist
yet). So the first backup, even a scheduled one at night, has nothing to ask. A backup that
was set up before this, or whose drive wasn't connected at the time, shows the same fields
in its Settings until its first backup. On the command line, `flux key init <profile>`
does the same.

A Vault can have several passwords. Each one opens it, and changing them never re-encrypts
data:

```sh
flux key list docs
flux key add docs --label "spare, in the safe"
flux key passwd docs
flux key remove docs <key-id>
```

**There is no way to recover a Vault whose passwords are all lost.** Keep a second password
somewhere safe.

A Borg repository has one passphrase; `flux key store <profile>` saves it in the keyring
for scheduled backups. Borg keeps the encryption key inside the repository, so also keep
a copy of it: `borg key export <repository> ~/borg-key.txt` (the recovery guide has the
exact command). **Without the passphrase, the data can't be decrypted.**

## Checking your backups

```sh
flux check home              # structure: every snapshot complete, every file accounted for
flux check home --read-data  # also read and verify every byte (slow)
```

The app has the same under *Maintenance* on a profile's overview. Vault data is
authenticated, so any corruption or tampering is detected.

## System backups

To back up things only root can read (`/etc`, `/root`, other users' files), use **system
profiles**. They live in `/etc/flux-backup/` and run from system timers, even when nobody is
logged in.

**In the app:** click **System backups** at the bottom of the sidebar. Flux explains what
it needs; then your desktop asks for an administrator password (yours, if your account is
an administrator). System profiles then appear and work like your own. The app locks them again
after 15 minutes without use, or when you click the lock.

**In a terminal:** add `--system` and run as root:

```sh
sudo flux --system profile create etc --target /run/media/me/Backup -s /etc -s /root
sudo flux --system backup etc
```

Some differences from your own profiles:

- An encrypted system Vault can't use the desktop keyring. The first time you unlock it in
  the app, tick **Keep it for scheduled backups**: the password is saved in
  `/etc/flux-backup/keys/<id>`, which only root can read.
- Hooks of system profiles run as root.
- Restoring as root never follows a symbolic link someone else planted on the way to the
  destination. If a folder on the path is a link owned by another user, the restore stops
  and says so; restore into another folder instead. When root restores into a folder that
  belongs to someone else, setuid bits are dropped and device files are skipped.

## Hooks

Commands to run around a backup:

```toml
[hooks]
pre_backup  = ["mount /mnt/nas"]          # a failure cancels the backup
post_backup = ["notify-send 'Backup done' \"$FLUX_SNAPSHOT_ID\""]
on_error    = ["logger -t flux \"$FLUX_ERROR\""]
timeout = "10m"
```

They run with `sh -c` and get `FLUX_EVENT`, `FLUX_PROFILE`, `FLUX_PROFILE_NAME`,
`FLUX_ENGINE`, `FLUX_SOURCES`, `FLUX_TARGET`, `FLUX_TRIGGER`, `FLUX_STATUS`,
`FLUX_SNAPSHOT_ID`, `FLUX_SNAPSHOT_PATH` and `FLUX_ERROR`. Their output goes into the
run's log.

## Keeping an eye on things

```sh
flux status            # what's running, last result and next run of every profile
flux watch home        # live progress of a running backup
flux cancel home       # stop it; the next run continues where it stopped
flux log home          # recent runs
flux log home --run last   # everything that happened in one run, including warnings
```

The app shows the same: a coloured dot per profile (green: fine, amber: some files
couldn't be read, red: failed, blue: running), live progress for every backup however it
was started, and an *Activity* tab with the full log of each run. *Problems* in Activity
shows only the runs that failed or skipped files.

The *Overview* tells you in words when something needs you: the drive hasn't been
connected for a while, the last few backups failed, no backup has succeeded for longer
than its schedule suggests (3 days for a daily backup, 30 for a manual one), or the
destination is almost full.

To set up a similar backup (the same folders to a second drive, say), use **Duplicate** in
the backup's menu or `flux profile duplicate home`: the copy is called "… (copy)" and opens
in Settings.

**Space.** For a backup on a drive, the Overview shows how full the drive has been over
the last weeks and, once there are a few backups to go on, when it fills up at that rate
("full in about 3 months"), with the projection dashed on the chart. Hover the chart or
use the arrow keys for exact values, or open *Show as a table*. Under a month left, it's
also a warning at the top. Steady use (old snapshots removed as fast as new ones are added)
says so.

**Asking for help.** *Copy diagnostics* (in Activity) or `flux diagnostics home` gives a
report to paste into a bug report: versions, the profile with password commands and hooks
hidden, and the latest runs and their messages.

An interrupted backup (power cut, Ctrl-C, unplugged drive) doesn't damage anything. The
unfinished snapshot is never shown, and the next run continues it.

## Exporting and importing profiles

A profile can be exported, to keep a copy, set up another computer, or take the same backup
to another tool. Exporting changes nothing.

| Format | What you get | Works with |
|---|---|---|
| Flux profile | the profile as TOML, importable again | Flux Backup |
| borgmatic | a `config.yaml`: sources, repository, exclusions as Borg patterns, compression, passphrase command, retention, checks, hooks | borgmatic 1.8+ and Borg |
| restic | a script: repository, password command, exclusions, `restic backup` and `restic forget --prune` | restic 0.16+ |
| rsync | a script making plain-folder snapshots with `--link-dest` | rsync 3.1+ |
| Borg | a script running `borg create` with the same patterns | Borg 1.2–1.4 |

Whatever a format can't express is listed, in the export dialog and as comments in the file:
pins and names, *never keep longer than*, free-space floors, the schedule (use cron or a
systemd timer), waiting for mains power, and per tool things like Borg's lack of a size
limit. Repositories that don't exist yet come with the command to create them.

**In the app:** right-click a backup (or ⋯) › **Export…**, or *Settings* › *More* ›
**Export this profile**. Pick a format, look at the file, then **Copy** or **Save…**. To
import, click **+** next to *Your backups* › **Import a profile…**. Flux opens the imported
profile's settings: a drive may have another name on this computer, and the schedule starts
once you **Save**. An id that's taken gets `-2`.

**In a terminal:**

```sh
flux profile export home                        # the Flux profile, on stdout
flux profile export home -f borgmatic -o ~/     # ~/home-borgmatic.yaml
flux profile export home -f restic -o restic.sh
flux profile import home.flux-profile.toml      # or --id another-name
```

## Profile reference

Profiles are TOML files in `~/.config/flux-backup/profiles/<id>.toml` (system profiles:
`/etc/flux-backup/profiles/`). Edit them in the app, with `flux profile edit <id>` (opens
`$EDITOR` and validates the result), or by hand. Unknown keys are errors, so typos don't
go unnoticed.

```toml
version = 1
name = "Home folder"
engine = "mirror"                 # "vault", or "rsync" / "borg" (compatibility mode)

[target]                          # a local folder or drive…
kind = "local"
path = "/run/media/me/Backup"
fs_uuid = "3f1c…"                 # refuse to write unless this filesystem is mounted there

# [target]                        # …or an SSH server
# kind = "ssh"
# host = "nas.local"
# user = "me"
# port = 22
# path = "/srv/backup"
# identity_file = "~/.ssh/flux_backup_ed25519"
# ssh_options = ["Compression=no"]
# bandwidth_limit = "5MB"

[sources]
paths = ["/home/me"]
exclude = ["~/.cache/", "node_modules/", "*.tmp"]
exclude_larger_than = "8GB"
exclude_caches = true
exclude_if_present = [".nobackup"]
one_file_system = false

[retention]                       # see "What to keep"
enabled = true
keep_last = 1
keep_within = "1day"
keep_daily = 7
keep_weekly = 4
keep_monthly = 12
keep_yearly = 5
keep_named = true

[schedule]                        # see "Automatic backups"
trigger = "calendar"              # "manual", "calendar" or "interval"
on_calendar = "daily"
catch_up = true
skip_on_battery = false
skip_on_metered = false           # backups to a server wait on metered connections
on_connect = false

[maintenance]                     # upkeep after scheduled backups
verify_monthly = false            # read every stored byte back about once a month
free_space_weekly = true          # Vault: delete data no snapshot uses about once a week

[options]
preserve_acl = true
preserve_xattr = true
symlinks = "preserve"             # "preserve", "follow_unsafe" (copy targets outside the sources), "follow"
change_detection = "metadata"     # or "checksum": also compare contents (slow)
continue_on_errors = true         # finish even if some files can't be read
snapshot_if_unchanged = false
nice = 10
io_priority = "best_effort_low"   # "normal", "best_effort_low", "idle"
notify = true
backup_before_restore = true      # keep files that a restore would replace
threads = 0                       # 0 = automatic

[vault]                           # Vault profiles only
compression_level = 3             # 0 (off) to 22
encryption = true                 # fixed when the Vault is created
password_source = { from = "keyring" }

[rsync]                           # rsync (compatibility mode) profiles only
extra_args = ["--compress"]       # added to every backup command

[borg]                            # Borg (compatibility mode) profiles only
encryption = "repokey-blake2"     # or "repokey", "none"; fixed when the repository is created
compression = "zstd,3"            # any borg --compression value
password_source = { from = "keyring" }
# remote_path = "/usr/local/bin/borg"   # borg on the SSH server, if not on its PATH
# extra_args = ["--checkpoint-interval=600"]

[hooks]                           # see "Hooks"
```

Durations are written like `90s`, `15min`, `4h`, `1day`, `2weeks`; sizes like `500MB` or
`4GiB`.

## Environment variables

| Variable | Effect |
|---|---|
| `FLUX_PASSWORD` | Vault password (overrides the profile's source) |
| `FLUX_HOME` | keep all configuration and state under one folder (portable setups, tests) |
| `FLUX_LOG` | log filter, e.g. `debug` or `flux_vault=trace` |
| `FLUX_BIN` | the `flux` binary the app starts backups with |
| `FLUX_HELPER` | the system-backup helper the app starts |
| `FLUX_PKEXEC` | program used instead of `pkexec` (empty: run the helper directly) |
| `FLUX_SSH` | `ssh` program to use |
| `FLUX_RSYNC` | `rsync` program to use |
| `FLUX_SECRET_TOOL` | `secret-tool` program to use |
| `FLUX_SYSTEMCTL`, `FLUX_SYSTEMD_UNIT_DIR` | where timers are written and how systemd is called |

## Troubleshooting

**"the drive with filesystem UUID … is not mounted at …" / "backup target … does not
exist".** The profile's drive isn't connected or mounted. Connect it; with `on_connect`, the backup then starts by
itself.

**A scheduled backup didn't run.** Check `flux schedule status` and `flux log <id>`.
Scheduled runs skip on battery when `skip_on_battery` is set, and can't ask for a Vault
password or accept a new SSH host key. Run `flux backup <id>` once in a terminal to see the
problem.

**"Permission denied" for some files.** They're listed in the run's log, and the snapshot
is marked *completed with problems*. Exclude them, or back them up with a
[system profile](#system-backups).

**SSH: "Host key verification failed" or a password prompt.** Run `flux ssh test <id>` in
a terminal once to accept the host key, and `flux ssh copy-id <id>` to install a key.

**`flux mount`: "cannot mount".** Install `fuse3` (it provides `fusermount3`), and mount on
an empty folder you own.

**Where is everything?** `flux config path` prints the folders Flux uses.
