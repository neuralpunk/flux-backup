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
programs from inside an AppImage. Install a package if you need system backups. So that
scheduled backups run when the app is closed, the AppImage keeps a copy of `flux` in
`~/.local/share/flux-backup/bin` (updated when you start a newer AppImage) and timers run
that copy.

**From source.** You need Rust 1.99 or newer. The app also needs Node.js 26 (LTS) and WebKitGTK
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

**In the app:** click **New backup** in the sidebar and choose where to start:

| Template | Backs up | Leaves out | When | Keeps |
|---|---|---|---|---|
| **Home folder** | your home folder | caches, the trash (your usual exclusions) | daily | the usual |
| **Documents** | `~/Documents` and `~/Desktop` | Office lock files, `*.tmp` | every hour | 24 hourly, 14 daily, 8 weekly, 12 monthly |
| **Photos** | `~/Pictures` | thumbnails, `*.tmp` | daily | 7 daily, 4 weekly, 24 monthly, 10 yearly |
| **Development** | `~/Projects` (or `~/src`, `~/code`…) | `node_modules`, `target`, `build`, `dist`, `.venv`, `__pycache__`, object files… | every hour | 24 hourly, 14 daily, 8 weekly, 6 monthly |
| **System settings** (system backups) | `/etc`, `/root`, `/usr/local`, `/opt`, `/srv` | `/root/.cache` | daily | the usual |
| **Start from nothing** | | | | |

Everything a template fills in can be changed in the next steps. To make your own, open a
backup's **⋯** menu › **Save as a template…**: new backups can then start from its
folders, exclusions, schedule, what to keep and how it's stored (not where it goes, the
other places it's copied to, or its webhooks). Your templates are listed under the
built-in ones, with a button to remove them. In a terminal: `flux profile templates`,
`flux profile create … --template development` (`-s` and `-x` replace the folders and add
exclusions), `flux profile save-template <profile> "<name>"`, `flux profile
delete-template "<name>"`.

A short guide then asks one question per step: **What** to back up (your home folder by default), **Where** to keep it (a
connected drive, each with a meter of how full it is; a folder; or an SSH server), **Protection** (plain folders, encrypted, or
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
would go. The ↻ button next to it counts again, after you change what's left out or the
files themselves. On **When**, *What happens each time it backs up* explains in plain words
what your choices add up to: when it runs, what it copies, what it keeps, and how to get
files back.

In *Settings › Performance and behaviour*, **Recommended defaults** puts every setting
there back to what Flux recommends (nothing is saved until you click **Save**).

**Coming from Back In Time?** If Back In Time is set up for you
(`~/.config/backintime/config`), the first step offers each of its profiles: **Use these
settings** fills in its folders, exclusions, schedule, what to keep and options (extra
rsync options too, except the ones Flux sets itself). Each step
can still be changed; on **Protection**, rsync is preselected because it makes the same
kind of plain, hard-linked folders Back In Time did, and the other choices are right there
(a profile Back In Time encrypted with EncFS becomes an encrypted Vault). Flux can't carry
on Back In Time's own snapshots, so it starts a fresh set next to them. They stay where
they are, untouched: keep Back In Time to restore from them, and turn off its schedule so
both don't back up at once. In a terminal: `flux profile backintime` lists the profiles,
`flux profile backintime 1` creates a Flux profile from the first (`--engine` to choose
another way of storing it, `--id` to name it).

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

The app tells you before that happens: a destination whose drive isn't plugged in is
flagged while you set the backup up or edit it, and **Back up now** says at once that the
drive isn't connected (and where it should be mounted) instead of starting.

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

### Networks and upload speed

For backups to a server, *Settings › More › Network* (shown when the backup or one of its
places is on a server) has two kinds of rules:

- **Wi-Fi networks.** Scheduled backups to a server run *only on these networks* (say,
  your home and office Wi-Fi), and/or *never on these* (a phone's hotspot). A skipped run
  says which network it saw. On a wired connection the rules don't apply, and backing up
  by hand always works. When only one of several places is on a server, only that place
  is skipped. The network's name comes from NetworkManager.
- **Upload limits by time of day.** For example *Weekdays from 09:00 to 18:00, at most
  2 MB/s*, and nothing the rest of the time. A window can run past midnight (22:00 to
  06:00). Outside every window, the connection's own limit applies (or full speed). Vault
  backups follow a change of limit as they run; rsync and Borg use the limit in force
  when they start.

```toml
[network]
wifi_only = ["Home WiFi", "Office"]
wifi_never = ["Phone hotspot"]
bandwidth = [
  { days = "Mon..Fri", from = "09:00", to = "18:00", limit = "2MB" },
  { days = "*", from = "22:00", to = "06:00" },        # no limit overnight
]
```

### Several places at once

One backup can go to more than one place: the drive at home and a USB stick that lives in
a drawer elsewhere, say, or a server at the office. In Settings, **Also keep copies
elsewhere › Add another place…** asks where, how it's stored (each place has its own:
an encrypted Vault on the stick, plain folders on the drive) and, for an encrypted one,
its password, and saves it at once. Mark a place **kept somewhere else** (offsite) when
it isn't in this building.

Every backup then goes to every place, one after the other, each read from your files
again, so the copies are independent: a fault in one never reaches another. If one place
fails (the stick isn't plugged in, the server is down), the others are still backed up,
the Overview's *Where it's kept* says which failed, and a warning stays until it works
again. The command line exits with status 4 (partial) in that case.

Each place is browsed and restored on its own: the Snapshots tab has a switch (*Snapshots
on BackupDisk · USB stick · Office NAS*), and on the command line a place is the profile
id with `+` and its name:

```sh
flux profile destination add home usb --target /run/media/$USER/STICK --engine vault
flux profile destination add home office --target me@office-nas:/srv/flux --engine rsync --offsite
flux profile destination list home
flux backup home                   # all three, one after the other
flux backup home+usb               # just the stick
flux snapshots home+office
flux restore home+usb ~/Documents/report.odt --to /tmp/back
flux profile destination remove home usb   # its backups stay on the stick
```

What to back up, when, and what to keep are shared by all places.

**Copying the first backup instead.** When adding a place, *How the copy is made* offers
**Copy the first backup**: your files are read once, for the first place, and the new
snapshots are then copied over. The copy holds exactly the same snapshots, stored the same
way, and a Vault copy opens with the same password (it is the same Vault). Snapshots
removed from the first place are removed from the copy too. Copying is faster and puts
less load on your disks; reading the files again for each place (the default) keeps the
copies independent, so a fault in the first backup can't reach the second. A Vault can be
copied anywhere (a drive or a server); plain-folder backups (Mirror, rsync) between drives
on this computer; Borg repositories can't be copied. If the first backup fails, the copy
waits for the next run. On the command line: `flux profile destination add home usb
--target /run/media/$USER/STICK --replicate`.

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
(caches, trash, build output) with how much space each takes in your sources; the ones
you've added are shown in green.

### Checking the rules

At the bottom of *Settings › Exclusions*, **Check the rules** uses the rules as they are in
the editor, before you save:

- **Test a path**: type one (`~/Downloads/film.mkv`, or a folder, even one that doesn't
  exist yet) and Flux says whether it's backed up, and what decides it: the pattern that
  leaves it out (or the folder it's in that a pattern leaves out), a `!pattern` that brings
  it back, the size limit, a cache folder or marker file, another disk, the backup's own
  storage, or that it isn't in any folder the backup takes.
- **Browse what's backed up…**: your folders, each item marked ✓ or ✗ with the reason, and
  how much below each folder is backed up and left out (counted for up to 10 seconds a
  folder).

In a terminal:

```sh
flux explain home ~/Downloads/film.mkv ~/Projects/app/node_modules
flux explain home ~/Projects --list         # each item, with what's in and out below it
```

These are the rules Flux's own scan uses (for rsync and Borg backups, Flux turns the same
rules into theirs).

### Folders on an external drive

Say you back up an external disk mounted at `/mnt/photos`. Unplugged, `/mnt/photos` is an
empty folder (or isn't there at all), and a backup that took it at face value would record
every photo as deleted. Flux doesn't: when you save a backup whose folder is on a drive of
its own (under `/run/media`, `/media` or `/mnt`), it records that drive's filesystem UUID,
and before each backup it checks the drive is there. A folder under those places that's
missing, or that turns out to be an empty folder on the system disk, also counts as an
unconnected drive. (A backup made while the drive is connected records it for profiles
from before 1.0.)

What happens when the drive isn't there is chosen per folder, under the folder list
(*If it isn't connected*), or as `when_missing`:

- **Back up the rest** (`skip`, the default). Mirror and Vault keep that folder's files
  in the new snapshot exactly as they were in the last one, without reading anything;
  rsync and Borg leave it out of that snapshot (earlier ones still have it). The run says
  so, the Activity log lists the folder, and when it happens several times in a row the
  Overview and All backups say *The drive with photos wasn't connected for the last 3
  backups*. If every folder's drive is missing, nothing is backed up and the run is
  marked skipped.
- **Wait for it** (`wait`). The backup shows *Waiting* and starts as soon as the drive is
  connected, or fails after `wait_for` (1 hour by default). It can be stopped while it
  waits.
- **Fail the backup** (`fail`), so you notice.

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

**Shut down when done.** For a backup you start before leaving: *Back up now, then shut
down* in a backup's ⋯ menu, or tick *Shut down the computer when done* under a running
backup's progress (`flux backup home --shutdown` in a terminal). A minute after the backup
ends, the computer powers off; the app shows a countdown with **Cancel shutdown**, and
`shutdown -c` cancels it too. A backup you stop doesn't shut the computer down; one that
fails still does (its error is waiting in *Activity*).

**One backup at a time.** With *Preferences › Advanced › Run one backup at a time*
(`one_at_a_time = true` in `settings.toml`), a backup that starts while another is running
waits for it to finish, so two backups don't compete for the same disk or connection. It
shows as *Waiting for its turn…* and can be stopped while it waits. This covers every
backup of one scope, whether started by hand, by its schedule or in a terminal; your own
backups and system backups each take turns among themselves.

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

### systemd timers or cron

Scheduled backups run with systemd timers unless you choose cron instead: in the app,
*Preferences › Advanced › What runs scheduled backups*; in a terminal,
`flux schedule use cron` (`flux schedule use systemd` to go back). Switching moves every
schedule over. The choice is per scope: yours, or the system's
(`sudo flux --system schedule use cron`).

With cron, Flux keeps one block in your crontab (`crontab -l` shows it, between
`# BEGIN Flux Backup` and `# END Flux Backup`), or for system backups the file
`/etc/cron.d/flux-backup`. Lines outside the block, such as Back In Time's, are never
touched. Every line runs `flux backup <id> --scheduled --if-due`, and Flux decides whether
a run is due:

- A calendar schedule fires at its times. With `catch_up`, a check every 15 minutes runs a
  backup whose time passed while the computer was off or asleep, once, as systemd's
  `Persistent=` does. Without it, a run more than 10 minutes late is skipped.
- An interval schedule is checked every few minutes, as with timers.
- Calendar expressions are read by Flux, the same way systemd reads them, to the minute.
  Time zones and `~` (last day of the month) aren't supported with cron.

| | systemd timers | cron |
|---|---|---|
| Runs with the app closed | yes | yes |
| Runs while you're logged out | with `loginctl enable-linger` | yes, but the keyring is locked then, so encrypted backups need a key file |
| Catches up after the computer was off | yes | yes (checks every 15 minutes) |
| Backs up when the drive is plugged in (`on_connect`) | yes | no |
| Needs | systemd | a cron daemon (`cronie` on Fedora, with `crond` running) |

## Getting files back

### In the app

Open **Snapshots**, pick a snapshot from the list, browse to what you need, and restore
just that: every file and folder has its own **Restore** button (it shows when you point
at the row), or tick several, even in different folders, and click **Restore N items**.
**Restore this whole folder** brings back everything in the folder you're in.

**Calendar**, above the list, shows a month at a time: days with a snapshot are marked
(green, or amber when files were skipped), and a red dot marks a day a backup failed.
Click a day to list only its snapshots (the first is opened); click it again, or the ✕ next
to the date, to see them all.

Items that aren't on your computer any more are marked **Deleted**. When a folder has
some, **Only deleted** shows just those, and **Restore N deleted items** brings back exactly
what's missing and nothing else. Restoring something that was deleted goes back to its own
place by default, since nothing is there to replace.

What's marked is checked against your computer each time a folder is shown, again as soon
as a restore finishes, and when you come back to Flux's window. If you've changed files
some other way, the **refresh** button next to the filter (or F5) checks the folder again.

You can restore:

- **into another folder**: the safest choice; nothing on your computer changes;
- **back to where it was**: identical files are skipped. Different files are kept as
  `name.flux-<snapshot>` unless you choose to replace them.

**Exactly as it was.** Restoring a folder back to where it was normally only adds and
replaces. Tick *Make the folders exactly as they were* to also remove what's in those
folders now but wasn't in the snapshot, so they end up as they were then. The dialog lists
what would be removed first. Removed items are moved out of the way as
`name.flux-<snapshot>` (deleted only when you also choose to replace your current
versions, or turned off *Keep files on restore*). What the backup never takes is never
removed: excluded files, files over the size limit, cache folders, the backup drive, other
mounted drives, and copies kept by earlier restores. Nor is anything the backup never
covered: restore the folder *above* the ones you back up, and what else is in it stays
(only the backed-up folders are made exact); files the backup couldn't read stay too, and
if a snapshot doesn't list every file it couldn't read, nothing is removed and the restore
says so. In a terminal:
`flux restore home ~/Documents --exact` (with `--dry-run` to list what it would remove).

Before you restore, the dialog shows what would happen: how many items are added, which
files differ from what's there now (and whether they'd be kept or replaced), and how many
are already identical. *Show what will be added* lists the files. Replacing files needs the
red **Restore and replace** button. If the snapshot has only empty folders where you're
restoring (because its rules left the files out), the dialog says so before you start.

**Don't know which snapshot?** Type part of the name in **Find in all snapshots** (above
the snapshot list) and press Enter. Every file or folder with that in its name is listed,
from all snapshots; files that aren't on your computer now are marked *Deleted*, and files
missing from the newest snapshot show the last snapshot that had them. Click one to show it in its snapshot. `*` works as a wildcard (`*.pdf`), and
a `/` matches whole paths (`Documents/*.odt`). `flux find` matches the same way.

The first search of a Vault, Borg, rsync or SSH backup reads every snapshot's listing;
Flux keeps an index of them in its cache folder (readable only by you), so later searches
and *Versions* are quick. Plain folders on a drive need none.

**Versions of a file.** Right-click a file › *Versions of this file…* (or select it and
click **Versions**) lists each different version with its date and size, and which
snapshots hold it. Open one to look at it, or restore it.

**Compare** on a version shows what changed between it and the file now (or any other
version: pick both at the top), line by line with the changed lines highlighted and long
unchanged stretches folded away. Text files up to 8 MB are compared in the app; for other
files, and to merge changes by hand, **Open in Meld** (or Kompare, KDiff3, Diffuse: the
first one installed) opens both in that program. *Preferences › Advanced › Compare file
versions with* chooses the program, or only Flux's own view; `diff_tool = "meld --newtab"`
in `settings.toml` takes any command (the two files are added after it). Files from
system backups can't be compared this way; restore them to a folder first.

**What changed** shows the differences from the previous snapshot; the ⋯ menu also compares with the files on
disk now. Folders whose only change is their date (adding or removing a file inside a
folder changes it) are hidden behind *N folder dates*. Point at a removed or changed file
to restore it (or its older version) from the older snapshot, or click **Restore all
removed**. Files larger than the backup's size limit are never compared: no backup holds
them, and the comparison says how many there are.

**Size limit.** *Settings › Leave some things out › Skip files larger than* takes a number
and a unit (MB, GB, TB). A limit under 1 MB leaves out nearly every file, so Flux warns
when you set one, and Overview and Snapshots say so while it's set.

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
flux restore home ~/Documents --exact           # …and remove what wasn't in the snapshot
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

### As an archive: Export

To keep a copy somewhere else, or to send files to someone, save them out of a snapshot
as one archive file. In *Snapshots*, a snapshot's **⋯ › Export…** saves all of it; in the
file list, right-click a file or folder (or a selection) › **Export…**. Choose:

- **Everything kept (.tar.zst)**: a standard tar archive compressed with zstd. It keeps
  owners, permissions, times (to the nanosecond), symbolic and hard links, device files,
  extended attributes and ACLs, as pax records that GNU tar (`tar --xattrs --acls`) and
  bsdtar read. Unpack it with `tar --zstd -xf home.tar.zst` (or most archive managers).
- **For sharing (.zip)**: opens anywhere. It keeps files, folders, their times and basic
  permissions, but not owners, symbolic links, special files, extended attributes or
  ACLs; hard links become separate copies. What was left out is listed after the export.

A **checksums file** can be saved next to the archive: SHA-256 (`ARCHIVE.sha256`, checked
with `sha256sum -c`) or BLAKE3 (`ARCHIVE.b3`, `b3sum -c`). Run the check in the folder you
unpacked into. Files keep their full paths in the archive (`home/me/Documents/…`), so the
check and the archive line up. The app names the archive after the backup, the snapshot's
date and the item exported, and never replaces an existing file.

While exporting, the files are restored into a hidden folder next to the archive and
removed afterwards, so that drive needs room for them and the archive.

```sh
flux export home ~/home.tar.zst                               # the newest snapshot
flux export home ~/photos.zip ~/Pictures/2025 --snapshot 3f2a91
flux export home /mnt/usb/docs.tar.zst ~/Documents --checksums sha256
```

### On a new computer

After each backup, Flux keeps a copy of the backup's settings with it: the folders,
exclusions, rules and schedule, without passwords, hooks or webhooks. In a Vault the copy
is encrypted like everything else; Mirror, rsync and Borg backups have it as
`flux-profile.toml` in their folder. It's written again only when the settings change.

On a new computer (or after reinstalling), install Flux Backup and choose **Recover an
existing backup…** on the welcome screen (or **+** › *Recover a backup…*). Pick the drive,
a folder, or a server; Flux lists the backups there, with the computer and user that made
them, the folders they protect, and how many snapshots they have. An encrypted Vault's
backups are listed once you type its password. Pick one, then:

- **Get files back**: the backup is set up with nothing scheduled. The Snapshots tab
  opens: restore everything or only what you need.
- **Carry on backing up from this computer**: on the saved schedule. New snapshots join
  the same backups, so the history carries on where the old computer left it.

Either way, the recovered backup remembers whose backups they are (`[origin]` in its
profile): Flux keeps them under the old computer's name and user, and finds them however
this computer is called. Places it was also copied to aren't set up again (they may be
somewhere else now): add them in its settings.

Backups made before 1.0 have no settings kept with them. They're found too, and recovered
with the folders of their newest snapshot and Flux's usual rules: check the exclusions and
the schedule in the backup's settings before carrying on.

In a terminal:

```sh
flux recover /run/media/me/Backup                        # list the backups there
flux recover nas:/srv/backup --pick 2                    # set up the second one
flux recover /run/media/me/Backup --pick 1 --carry-on --id home
```

`FLUX_PASSWORD` gives an encrypted Vault's password (otherwise it's asked on a terminal).

**Ready for it?** At the bottom of a backup's Overview, *Could you recover it on another
computer?* opens a short checklist: whether its settings are kept with the backups (and
current), whether a restore test has passed, whether a copy is kept somewhere else, and,
for encrypted backups, whether the password is kept somewhere other than this computer
(Flux can't know: tick it when it is). For Borg, whether the key was exported
(`borg key export`).

### Without Flux: `flux-vault-recover`

A Vault is Flux's own format, so Flux comes with a second program that reads it:
`flux-vault-recover`. It's small, needs nothing installed (on 64-bit Linux it's a single
static file), has no settings, and only reads, so it can't harm the backups. It's written
separately from Flux's Vault engine, and Flux's tests check that the two read every Vault
the same way.

For a Vault on a drive, Flux keeps a copy of it next to the `flux-vault` folder, with
`FLUX-VAULT-RECOVERY.txt` explaining how to use it (*Keep the recovery tool on the drive*
in the backup's Storage settings, on by default). It's also attached to every release on
its own, and installed with Flux in `/usr/libexec/flux-backup/`.

```sh
cd /run/media/me/Backup
./flux-vault-recover flux-vault snapshots                 # asks the password if encrypted
./flux-vault-recover flux-vault files latest home/me/Documents
./flux-vault-recover flux-vault restore latest ~/Restored                    # everything
./flux-vault-recover flux-vault restore 3f2a91 ~/Restored home/me/Documents  # one folder
./flux-vault-recover flux-vault verify                    # read every stored piece back
```

A restore goes into an empty folder (nothing is overwritten), under the files' full paths,
with their permissions and times (and owners, when run as root); sparse files stay
sparse. Named pipes and device files are skipped. The password can also come from
`FLUX_PASSWORD` or `--password-file`; a Vault with an unlock file needs `--unlock-file`.

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

In the app (*What to keep* › **Keep and remove snapshots**), pick *Recommended* (the
defaults above), *Save space*, *Keep longer*, or *Custom* for the numbers themselves.
*Your snapshots with these rules* says how many of your current snapshots would go; open it
to see which each rule keeps or removes, and why.

**Safety.** Two rules protect against removing too much:

- **Always keep at least** N complete snapshots (1 by default): whatever the rules say,
  pruning never leaves fewer snapshots that finished without files left out
  (`safety_floor`). Raise it if you want more good snapshots guaranteed.
- **After a backup with problems, keep everything** (on by default, `after_problems =
  "hold"`): when a backup fails, leaves files out, misses a source's drive or can't reach
  one of its places, the next backups remove nothing until one runs cleanly. A run of bad
  backups can't push your good snapshots out. `flux prune` by hand still prunes.

**Unusual changes.** After each backup, Flux compares what changed with what usually
changes in this backup, and looks for the marks ransomware leaves: many more files changed
or deleted than usual; files renamed to a new extension (`report.odt` →
`report.odt.locked`) or many new files with extensions ransomware uses; text files
(`.txt`, `.csv`…) whose contents now look encrypted (a sample is read); ransom notes
(`HOW_TO_DECRYPT.txt`). When a backup stands out:

- its snapshot is tagged *unusual changes*, and **the snapshot before it is pinned**, with
  a note saying why, so it can't be removed;
- old snapshots aren't removed until it's settled, and a notification says what was seen;
- the Overview says what was found and where, with **Show the files**, **Browse the
  snapshot before** (to restore from it) and **This was me** (when you made the changes:
  the warning goes, the pin and the tag are removed).

If you didn't make the changes, don't delete or change anything: disconnect the computer
from the network and restore from the snapshot before. *Watch for unusual changes*
(*What to keep › Safety*: normally, closely, or not at all; `unusual_changes = "high"`)
sets how readily changes count. While it's on, the snapshot before the newest always
survives one more backup, so there is one to keep.

**Activity history** is kept separately from snapshots: the last 1000 backups by default,
or only those of the last 30 days, 3 months or a year (`keep_history_days`, or
`keep_history_runs` for a count). Removing history never removes backups.

To keep a snapshot forever, **pin** it (`flux pin home <snapshot>`). To mark one, **name**
it (`flux name home <snapshot> before-upgrade`). By default, named snapshots are kept too.

To find a snapshot again later without changing what's kept, give it a **note** or
**tags**: in Snapshots, *⋯ › Note and tags…*, or `flux note home <snapshot> Before the
upgrade to Fedora 46` and `flux tag home <snapshot> taxes paperwork` (`--remove` takes
tags off; `flux note … --clear` removes the note). Tagged snapshots show a ✎ in the list,
the tags in use appear as buttons above it to show only those snapshots, and
`flux snapshots home --tag taxes` lists them on the command line. Notes and tags are
stored with the snapshot (for Borg, in the archive's comment), so they travel with the
backups, and retention ignores them: a tagged snapshot goes when the rules say so. Pin or
name it to keep it.

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

### Security settings

An encrypted Vault backup has a **Security** row in its settings, under the encryption
switch. The everyday choices are already made for you; these are for when you want more.

- **Password hashing.** Before the password can open the Vault, it's put through Argon2id,
  which is slow on purpose so that guessing passwords for a stolen drive is slow too.
  *Standard* (64 MiB, 3 passes) unlocks in a fraction of a second; *Strong* (256 MiB, 4
  passes) and *Maximum* (1 GiB, 6 passes) make every guess, and every unlock, slower. The
  page says how long unlocking takes on your computer. Choosing one sets it for the Vault's
  new passwords; **Apply to the password** rehashes the password in use at once (only its
  key changes; no backup data is rewritten). A long password does more than any setting.
  On the command line: `flux key strengthen <profile> --level strong` (or `--memory MiB
  --passes N --threads N`).
- **Unlock file.** *Set up…* makes a small file in a folder you choose (a USB stick, for
  example). From then on the password opens the Vault only together with that file:
  backups and unlocking need it to be there, and say where they expect it when it isn't.
  Keep a copy somewhere safe; losing every copy is like losing the password. Other
  passwords of the Vault still open it without the file until you remove them (the page
  lists them). *Stop requiring it* goes back to the password alone. On the command line:
  `flux key unlock-file <profile> create /run/media/$USER/STICK/backup.flux-unlock`,
  `… use PATH` for an existing file, `… remove`.
- **Hide file sizes.** Someone holding the drive can't read anything in a Vault, but the
  size of each stored file is visible, and a backup that changed one small file stores
  something about that size. This pads what's stored so only a rough size shows. It costs
  up to a few percent more space and applies to backups from then on.
- **Lock it again in the app.** After you unlock a Vault in the app, the app keeps its
  password in memory until it closes. Choose 5 minutes to an hour to have it forgotten
  sooner when unused. Scheduled backups get the password from their own source and aren't
  affected.

The encryption itself (XChaCha20-Poly1305 with 256-bit keys, and keyed BLAKE3) isn't a
setting: neither has a stronger alternative worth choosing.

Sparse files (disk images, some databases: files with *holes*, ranges that take no disk
space) keep their holes in a Vault: only their data is read and stored, and a restored
copy is sparse again, as with Mirror. A Vault created before Flux 1.0 stores them whole
until you upgrade it: *Upgrade the Vault* in the Overview's Maintenance (shown only for
such a Vault), or `flux vault upgrade <profile>`. Upgrade once every computer that uses
the Vault runs Flux 1.0: older versions can't open it afterwards. The snapshots already
there stay as they are.

**Backups and profiles from older versions.** Flux 1.0 reads everything Flux 0.1, 0.2 and
0.3 wrote, and carries on backing up into it. A profile file written before 1.0 is read
as it is; the first time Flux rewrites it, the original is kept next to it as
`<id>.toml.pre-1.0.bak`. Something written by a *newer* Flux (a Vault, a snapshot, a
profile with a setting this version doesn't know) is refused with a message saying so,
never misread: update Flux. Every format is described in
[FORMATS.md](FORMATS.md).

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

### Restore tests

A check reads what's stored; a **restore test** goes all the way: it restores some files
of the newest snapshot into a private temporary folder, as a real restore would, and checks
each against the backup's own record (its size, and its checksum when the profile records
checksums; a Vault checks every piece of data it reads anyway). A file that can't be
restored, or comes back with the wrong size or contents, fails the test. Permissions or
times not as recorded, a test stopped at a limit, and (if asked) files that differ from
your copy now are notes: the test passes with a warning. The temporary folder is removed.

On a backup's Overview, **Test a restore now** runs one; *Settings › What to keep ›
Restore tests* runs them by themselves, after a backup once due: **after every backup**,
**weekly** or **monthly** (off by default). Which files: **some at random** (10), **a share
of the backup** (a percentage of its size), **every file once a month** (a different part
each time, so every file is tested within 30 days), or **everything**. Limits on time and
data read keep big tests in check. A failed test is notified and stays on the Overview and
All backups until one passes; a test that's overdue (no pass within the interval and a
bit) is a warning.

```sh
flux restore-test home                 # the profile's settings (10 random files)
flux restore-test home --files 50 --compare-source
flux restore-test home --full
flux restore-test home --history
```

```toml
[restore_test]
every = "month"                        # off | backup | week | month
mode = "rolling"                       # random_files | percent | full | rolling
files = 10
# percent = 5
# window = "30days"                    # rolling: every file within this long
# max_runtime = "1h"
# max_read = "5GB"
# compare_with_source = true
```

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

With more than one backup, *All backups* can show only some of them: **Missed** (overdue,
or the drive wasn't there), **Failed** (the last backup, or a place it's copied to),
**Unverified** (no restore test passed recently), **Offsite** (a copy is kept elsewhere) and
**Low space**. Each says how many backups it matches; click it again, or *All*, to see
every one.

The *Overview* tells you in words when something needs you: the drive hasn't been
connected for a while, the last few backups failed, no backup has succeeded for longer
than its schedule suggests (3 days for a daily backup, 30 for a manual one), or the
destination is almost full.

To set up a similar backup (the same folders to a second drive, say), use **Duplicate** in
the backup's menu or `flux profile duplicate home`: the copy is called "… (copy)" and opens
in Settings.

**Space.** For a backup on a connected drive, the Overview has a meter of how full the
drive is now (free space, size and percent; amber from 85% full, red from 95%) and, once
there are a few backups to go on, when it fills up at that rate ("full in about 3 months").
*History and forecast* opens a chart of the last weeks with the projection dashed (open by
itself when the drive fills within two months). Hover the chart or
use the arrow keys for exact values, or open *Show as a table*. Under a month left, it's
also a warning at the top. Steady use (old snapshots removed as fast as new ones are added)
says so.

**Storage.** Under the space meter (or on its own when the space isn't shown), a line says how much the newest snapshot holds,
what all the snapshots would take as full copies, and what they take: the difference is
what storing each piece once (deduplication) and compression save. For a Vault it's
measured from its index (for the whole Vault when other backups share it), for Borg it's
`borg info`'s own numbers, and for plain folders (Mirror, rsync) it's an estimate from
what each snapshot copied, the rest being shared with the snapshot before.

**Where the time went.** In *Activity*, a run's *Where the time went* (folded) shows how
long each phase took (scanning, comparing, copying, finalizing, and a restore test or the
unusual-change check after it), the rate while copying, and for Vaults the time spent
reading, cutting files into chunks, finding what's already stored, compressing and
encrypting, and writing or uploading, with how many writes the storage took and how long
one took on average. That's where to look when a backup is slower than it should be: a
slow drive or link shows in *Writing and uploading*, a busy computer in *Compressing*.
The numbers are kept in the run log (`flux log --json`).

**Asking for help.** *Copy diagnostics* (in Activity) or `flux diagnostics home` gives a
report to paste into a bug report: versions, the profile with password commands and hooks
hidden, and the latest runs and their messages.

**Which version do I have?** The main menu (☰ in the title bar) › *About Flux Backup*
shows the version, with links to this guide, what's new and bug reports. Its *Technical
details* list the version of the `flux` command and of the administrator helper; if they
differ from the app's (after a partial upgrade), it says so, since scheduled backups run
the `flux` command. *Copy version info* copies it all. The version is also at the foot of
Preferences, and `flux --version` prints it.

An interrupted backup (power cut, Ctrl-C, unplugged drive) doesn't damage anything. The
unfinished snapshot is never shown, and the next run continues it.

### Webhooks: notifications off this computer

A desktop notification only helps while you're at the computer. A **webhook** tells
something else how backups go: your phone through [ntfy](https://ntfy.sh), a Gotify
server, a chat, or a monitoring service that raises the alarm when the pings stop.

*Preferences › Notifications › Webhooks* holds the ones told about every backup;
a backup can add its own in *Settings › More › Notify elsewhere*. For each, choose:

- **Kind**, which shapes the message:
  - *ntfy*: the address of your topic (`https://ntfy.sh/my-backups-x7k2`); the message as
    text, high priority for failures.
  - *Gotify*: `https://gotify.example.org/message?token=…`.
  - *Slack-style* (`{"text": …}`, also Mattermost and Rocket.Chat) and *Discord*
    (`{"content": …}`): the channel's webhook address.
  - *healthchecks.io-style pings*: the check's ping address. Flux pings `…/start` when a
    backup starts, the address itself when it succeeds and `…/fail` when it fails or
    finishes with problems, so the service also notices when backups stop happening.
  - *JSON*: anything else (Home Assistant, n8n, your own script). Flux POSTs:
    ```json
    { "event": "failed", "profile": "home", "name": "Home folder", "host": "laptop",
      "message": "Backup of “Home folder” failed: …", "time": "2026-11-09T10:00:00Z",
      "status": "failed", "snapshot": null, "error": "…" }
    ```
    `event` is one of `started`, `succeeded`, `warning`, `failed`, `skipped`,
    `restore_test_failed`, `unusual_changes`, `low_space` and `test`; `status`,
    `snapshot` and `error` are there when they apply.
- **When**: failed, finished with problems (files left out, a drive or another place
  missing), unusual changes and failed restore tests, unless you choose others: little
  space left (under 5% or 1 GiB after a backup), a scheduled backup skipped (paused, on
  battery, a refused network), succeeded, started.
- **Extra headers**, one per line (`Authorization: Bearer …`).

*Send a test* sends a test message at once and says what the other end answered.

Webhooks are sent with `curl`, after the backup is done; one that fails (no network, a
wrong address) is a warning in that run's log and doesn't fail the backup. Addresses and
headers can be secrets: they're kept in the profile and in `settings.toml`, both readable
only by you, and left out of profile exports and diagnostics.

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

[[sources.volumes]]               # a folder on a drive of its own (recorded when saved)
path = "/mnt/photos"              # one of sources.paths
fs_uuid = "6F2A-91C3"             # the drive it must be on
when_missing = "skip"             # skip | wait | fail ("Folders on an external drive")
# wait_for = "1h"                 # with wait: how long at most

# offsite = true                  # the main target is kept somewhere else

[[destinations]]                  # more places ("Several places at once")
name = "usb"                      # its id is home+usb
label = "USB stick"
engine = "vault"
offsite = false
target = { kind = "local", path = "/run/media/me/STICK" }
vault = { password_source = { from = "keyring" } }
# mode = "replicate"              # copy the main backup instead of reading the sources

[retention]                       # see "What to keep"
enabled = true
keep_last = 1
keep_within = "1day"
keep_daily = 7
keep_weekly = 4
keep_monthly = 12
keep_yearly = 5
keep_named = true
safety_floor = 1                  # never fewer complete snapshots than this
after_problems = "hold"           # hold | prune: after a backup with problems
# keep_history_days = 90          # Activity history (default: the last 1000 runs)

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
                                  # (a Mirror on an SSH server only takes "preserve")
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
# keep_recovery_tool = false      # don't keep flux-vault-recover next to it on a drive

[rsync]                           # rsync (compatibility mode) profiles only
extra_args = ["--compress"]       # added to every backup command

[borg]                            # Borg (compatibility mode) profiles only
encryption = "repokey-blake2"     # or "repokey", "none"; fixed when the repository is created
compression = "zstd,3"            # any borg --compression value
password_source = { from = "keyring" }
# remote_path = "/usr/local/bin/borg"   # borg on the SSH server, if not on its PATH
# extra_args = ["--checkpoint-interval=600"]

[hooks]                           # see "Hooks"

# [origin]                        # set by a recovery: whose backups these are
# host = "old-laptop"             # the computer that made them
# user = "me"
# profile = "home"                # its profile id there

[[webhooks]]                      # see "Webhooks"; as many as you like
url = "https://ntfy.sh/my-backups-x7k2"
preset = "ntfy"                   # json, ntfy, gotify, slack, discord, healthchecks
# events = ["failed", "warning", "unusual_changes", "low_space"]
# headers = ["Authorization: Bearer …"]
# enabled = false
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
| `FLUX_CURL` | `curl` program webhooks are sent with |
| `FLUX_VAULT_RECOVER` | the `flux-vault-recover` program kept next to Vaults on drives |
| `FLUX_CRONTAB`, `FLUX_CRON_DIR` | the `crontab` program and the `/etc/cron.d` folder used with cron |

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

**"another Flux job is using the Vault".** A backup, check or prune is using it, here or
on another computer; the message says who and since when. If that job died (the computer
was switched off, the connection dropped), its lock lapses after 30 minutes, or right
away when it was on this computer. `flux unlock <profile>` removes lapsed locks now, and
`flux unlock <profile> --force` removes them all: only when you're sure nothing is
using the backup.

**`flux mount`: "cannot mount".** Install `fuse3` (it provides `fusermount3`), and mount on
an empty folder you own.

**Where is everything?** `flux config path` prints the folders Flux uses.
