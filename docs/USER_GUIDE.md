# Flux Backup user guide

Flux Backup keeps **snapshots** of your folders: complete pictures of how they were at a
moment, on an external drive or an SSH server. It runs on a schedule, when a drive is
plugged in, or when you ask. You can use the desktop app, the `flux` command, or both;
they share the same profiles and see each other's backups.

- [Installing](#installing)
- [Ideas in two minutes](#ideas-in-two-minutes)
- [Your first backup](#your-first-backup)
- [Mirror or Vault?](#mirror-or-vault)
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

**In the app:** click **New profile** in the sidebar. Answer three questions (what to back
up, your home folder by default; where to keep it, a connected drive, a folder or an SSH
server; and when), decide whether to encrypt, then **Create backup** and **Back up now**.
Everything else (exclusions, retention, hooks…) is under **Advanced options**, and the
defaults are sensible.

**In a terminal:**

```sh
flux profile create home --target /run/media/me/Backup --source ~
flux backup home
flux snapshots home
```

The first backup copies everything. Later ones only copy what changed, and take seconds.

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
in the app's *Where to keep it* section, under **Advanced options**):

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

In the app, **Advanced options** under *What to back up* suggests common exclusions
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

```sh
flux schedule status        # every profile's timer and next run
flux schedule sync          # make timers match all profiles
flux schedule remove home
```

Scheduled backups run at low CPU and I/O priority (`options.nice`,
`options.io_priority`), keep the computer from suspending while they run, and notify you
if something went wrong.

## Getting files back

### In the app

Open **Snapshots**, pick a snapshot from the list, browse to what you need, select it,
and **Restore**. You can restore:

- **into another folder**: the safest choice; nothing on your computer changes;
- **back to where it was**: identical files are skipped. Different files are kept as
  `name.flux-<snapshot>` unless you choose to replace them.

**What changed** shows the differences from the previous snapshot; the ⋯ menu also compares with the files on
disk now.

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

In the app (*What to keep*, under **Advanced options**), the retention editor shows which of your current snapshots each rule would
keep or remove, and why.

To keep a snapshot forever, **pin** it (`flux pin home <snapshot>`). To mark one, **name**
it (`flux name home <snapshot> before-upgrade`). By default, named snapshots are kept too.

## Vault passwords

An encrypted Vault needs its password to back up, browse or restore. Where it comes from
is set per profile:

| `vault.password_source` | Use |
|---|---|
| `{ from = "keyring" }` | the desktop keyring (default): asked once, then remembered |
| `{ from = "prompt" }` | asked every time; scheduled backups can't run |
| `{ from = "file", path = "…" }` | first line of a file (make it mode 0600) |
| `{ from = "command", command = "pass show backup" }` | first line printed by a command |

`FLUX_PASSWORD` overrides all of them, for scripts.

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

**In the app:** under *System*, click **Authenticate** and enter an administrator
password. System profiles then appear and work like your own. The app locks them again
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
was started, and an *Activity* tab with the full log of each run.

An interrupted backup (power cut, Ctrl-C, unplugged drive) doesn't damage anything. The
unfinished snapshot is never shown, and the next run continues it.

## Profile reference

Profiles are TOML files in `~/.config/flux-backup/profiles/<id>.toml` (system profiles:
`/etc/flux-backup/profiles/`). Edit them in the app, with `flux profile edit <id>` (opens
`$EDITOR` and validates the result), or by hand. Unknown keys are errors, so typos don't
go unnoticed.

```toml
version = 1
name = "Home folder"
engine = "mirror"                 # or "vault"

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
on_connect = false

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
