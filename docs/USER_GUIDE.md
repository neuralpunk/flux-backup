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

A short guide then asks the three things only you can answer: **What** to back up (your
home folder by default), **Where** to keep it (a connected drive, each with a meter of
how full it is; a folder; or an SSH server), and **Protection**: how it's stored (plain
folders, a Vault, or written by rsync or Borg so the backups never depend on Flux). A
Vault asks there whether to **encrypt the backups, On or Off**; On asks for the password
right away. The rest Flux chooses, from the template, to suit most backups, and the last
screen, **Ready**, shows it under *Chosen for you*: **When**, **What to keep**, and
**Flux Sentry**: whether it keeps watch for unusual changes, *normally* (recommended),
*closely* or not at all (see [Unusual changes](#what-to-keep)). Click any of them to
change it there. **Create backup** sets it up and offers **Back up
now** for the first run. Later, a backup's **Settings** show the same questions on one
page. Everything else is one click away and has sensible defaults: each group of settings is a
row showing its current value (*Exclude Files/Directories › caches*, *How backups are stored ›
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
| Compression | no | zstd (skipped for data that doesn't compress) |
| Encryption | use an encrypted drive (LUKS) | built in (XAES-256-GCM, Argon2id), or off |
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

**The drive's format matters for plain folders.** A Mirror (or rsync) backup keeps each
unchanged file once, hard-linked into every snapshot, and keeps links, owners and
permissions as they are. A drive formatted **FAT32** or **exFAT** (most USB sticks, many
drives sold for Windows and Macs) can do none of that: every snapshot is a full copy, links
and permissions are lost, and FAT32 can't hold a file of 4 GB or more. **NTFS** keeps hard
links but not Linux owners and permissions. Flux says so when you choose such a drive,
under the backup's settings, on its Overview (FAT32 and exFAT), and in `flux profile
validate` and `flux check`. A **Vault** keeps all of that in its own files, so it suits any
drive; or format the drive as ext4 (in GNOME Disks or KDE Partition Manager), which erases
it. A file too big for the drive is named in the backup's log as such.

**When the drive starts to fail.** After each backup, Flux asks the backup drive how it is,
as GNOME Disks would (through udisks, without a password): a disk's SMART report, or an
NVMe drive's warnings. If the drive says it's failing, is running out of spare space for
worn-out cells, says it's no longer reliable, or its bad sectors start growing, the
Overview says so in words and a notification says it once; *Copy the backups to a new
drive soon*. A drive that's hot gets a milder note. Bad sectors a drive already had when
Flux first looked aren't news: only more of them are. The space card says what was seen
last ("The drive reports no problems with its health"). Most USB sticks and many USB
enclosures don't pass the report on: then Flux says it can't tell. `flux check` asks the
drive too.

**When the drive is slower than it should be.** The space card and `flux check` say so,
with what to do, when:

- **TRIM doesn't reach an SSD.** TRIM tells an SSD which of its space is free. Without it,
  the drive treats everything ever written to it as still in use: long writes slow down as
  it fills, more with every old backup deleted from it (a USB SSD here wrote 860 MiB/s at
  first and 250–530 MiB/s after 170 GiB). Many USB adapters pass TRIM on once told to, and
  Flux gives the command (`echo unmap > /sys/block/sdX/device/scsi_disk/…/provisioning_mode`
  as root; a udev rule makes it last). On a drive encrypted with LUKS, TRIM must also pass
  through the encryption: `discard` in `/etc/crypttab`, or `cryptsetup refresh
  --allow-discards --persistent <name>` for LUKS2. Then `fstrim` the drive once (Linux
  distributions run it weekly after that). TRIM through LUKS shows which areas of the
  drive are unused, not what's in them.
- **A USB 3 drive runs at USB 2 speed** (480 Mbit/s: about 40 MB/s at most), on a USB 2
  port, hub or cable. Plug it into a USB 3 port with its own cable.

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
removed from the first place are removed from the copy too; damage isn't copied. A
stored file of a Vault that the drive has damaged never replaces the copy's own, and one
that has gone missing from the Vault (not removed by a prune) is kept in the copy, with a
warning to check the Vault. A Mirror or rsync backup that has lost all its snapshots
leaves its copy alone. Copying is faster and puts
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

**What a project's `.gitignore` ignores.** With *Skip what .gitignore files ignore*
(`exclude_gitignored = true`), inside a git working tree (a folder with `.git`) the
backup also leaves out what its `.gitignore` files ignore, read the way git reads them:
each relative to its folder, a subfolder's own file adding to its parents', and a nested
repository starting afresh. Build output and downloaded dependencies stay out, but so does
anything else git leaves out on purpose, such as a `.env` file with keys or a local
settings file: if you want one of those kept, add `!.env` to your own rules, which come
first. Only `.gitignore` files count (not `.git/info/exclude` or a global excludes file),
and only for Mirror and Vault backups; rsync and Borg don't read them. The rule tester
says which `.gitignore` left a file out, and an exact restore leaves such files alone.

In the app, *What to back up* › **Exclude Files/Directories** suggests common exclusions
(caches, trash, build output) with how much space each takes in your sources; the ones
you've added are shown in green.

Each folder you back up, and each folder you leave out, shows how much it holds beside it
(a pattern like `*.iso` doesn't). Counting has a few seconds; a folder too big to count in
that time shows what was counted so far as "≥ 384 GiB".

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

### Leave out or add from the file manager

Right-click a file or folder in **Nautilus** (GNOME Files; the items are under **Flux
Backup**), **Nemo** or **Dolphin**:

- **Leave out of backups…** lists the backups that take it, each ticked, with what would
  change: a rule naming just that file or folder (`~/Videos/raw/`) added to what it leaves
  out, or, when it's one of the folders the backup takes, that folder dropped. A backup
  it's the only folder of is left alone (delete the backup instead). Snapshots already
  made keep it.
- **Back up this folder…** (on a folder) asks which backup should take it: it's added to
  that backup's folders (in place of any of them inside it), or, when a rule naming just
  this folder leaves it out, that rule is removed. Or choose **A new backup** to set one up
  that starts with this folder. A folder left out by a broader rule (`node_modules/`,
  `*.iso`) isn't changed here: the dialog says which rule, to change in the backup's
  settings.

Nothing changes until you confirm. Nautilus shows whichever of the two applies (it reads
your backups' folders); Nemo and Dolphin show both, and Flux says when there's nothing to
do. From a terminal: `flux-backup --leave-out PATH`, `flux-backup --back-up FOLDER`, or,
without the app:

```sh
flux profile leave-out ~/Videos/raw              # every profile that takes it
flux profile leave-out ~/Downloads/big.iso --only home
flux profile add-folder home ~/Projects
```

### Folders on an external drive

Say you back up an external disk mounted at `/mnt/photos`. Unplugged, `/mnt/photos` is an
empty folder (or isn't there at all), and a backup that took it at face value would record
every photo as deleted. Flux doesn't: when you save a backup whose folder is on a drive of
its own (under `/run/media`, `/media` or `/mnt`), it records that drive's filesystem UUID,
and before each backup it checks the drive is there. A folder under those places that's
missing, or that turns out to be an empty folder on the system disk, also counts as an
unconnected drive; a folder there with files in it that simply lives on the system disk
(`/mnt/data` on a server) is backed up like any other. (A backup made while the drive is
connected records it for profiles from before 1.0.)

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
| `release_drive = true` | | when a backup ends, unmount the drive and power it off, so it's safe to unplug (see below) |
| `skip_on_battery = true` | | scheduled runs wait for mains power |
| `skip_on_metered = true` | | scheduled runs to a server wait while the connection is metered (mobile data, a phone's hotspot, as NetworkManager reports it) |

**Safe to unplug.** With *Unmount the drive when done* (under *When*, for a backup to a
drive; `release_drive = true`), each backup ends by unmounting its drive and powering it
off, as the file manager's *Eject* does, and a notification says it's safe to unplug. An
encrypted drive is locked again first. It applies to drives the desktop mounted (under
`/run/media` or `/media`); a drive mounted from `/etc/fstab` would need an administrator's
password, so it stays mounted and the run says why. A drive something else is using (a
file open in another program, another partition of it mounted) stays as it is, and the
notification says to eject it from the file manager before unplugging it. Together with
*Also when this drive is plugged in*, a USB drive backs up when it's plugged in and can be
unplugged when the notification comes.

```sh
flux schedule status        # every profile's timer and next run
flux schedule sync          # make timers match all profiles
flux schedule remove home
```

**Typing a time.** In the app, a backup's time takes what you type: `9` or `930` (on the
24-hour clock: 9:00 and 9:30 in the morning), `21:30`, `9:30 pm`, `9p`, `noon`. ↑ and ↓
move it by 15 minutes (with Shift, an hour), and the clock button lists times every half
hour. Something that isn't a time is outlined in red and goes back to the time before when
you leave the field. The upload limits by time of day take times the same way.

**Custom times** (*More schedule options › Custom times*) takes a systemd calendar
expression such as `Mon,Wed,Fri 18:00` or `*-*-01 03:00` (see `man systemd.time`). As you
type, the next three runs are shown under it, so a mistake shows at once. A few forms Flux
can't preview (time zones, the last day of a month with `~`) are said to be so; systemd
still runs them.

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

**Closing the window keeps Flux in the system tray.** Its icon lights up while a backup
runs (pointing at it says how far it is: "21% · about 6 minutes left"), and its menu
opens the window, backs up everything, pauses schedules or quits. The
first time, a notification says where Flux went. To quit when the window closes instead,
turn off *Preferences › App › Keep running in the tray when the window is closed*. On a
desktop with no tray to show the icon (GNOME without the AppIndicator extension, say),
closing the window quits, so the app is never left running where you can't reach it.
Starting Flux again while it runs brings back its window rather than a second copy.
Scheduled backups never need the app either way.

**How long is left** shows wherever a backup or restore runs: in its progress, in the
restore dialog, beside it in All backups, and when pointing at it in the sidebar. The
estimate follows the recent speed (about the last 20 seconds), so a slow start or a few
big files don't hold it back for long, and it's rounded to the minute.

**Preferences** (bottom of the sidebar) also hold: start in the tray when you log in, the
theme, the layout, the clock (12-hour, 24-hour, or like the system: how the app shows and
takes times), and the exclusions new backups start with.

The **layout** is *Compact* unless you change it: each backup is one line in the sidebar,
All backups shows your backups in columns (their last 30 days line up, so you can compare
them), and a backup's Overview has its settings and where it's kept in a column on the
right. *Full* is the roomier layout Flux had before: two lines a backup in the sidebar,
each backup's status on its own lines, the settings below the overview. Change it under
*Preferences › App › Layout*; it's kept on this computer. In Compact, a backup's
Settings and the Preferences page list their sections at the side, to jump to one.

The **sidebar** folds to a narrow column of icons (the sidebar button by the title, or
F9) for more room: each backup shows as an icon of what it backs up, with a dot for how
it's doing (green fine, amber worth a look, red not protected, blue running). Point at
one for its name and status. It stays folded until you open it again.

The **text size** (*Preferences › App › Text size*: Small, Default, Large) makes
everything in the window smaller or larger, 90% or 115%, as zooming a web page does; the
window's smallest size grows with it so nothing gets cramped. It's kept on this computer.

The **accent colour** (the main button, switches, progress and focus) follows the one
chosen in your desktop's settings (GNOME's *Appearance*, KDE's *Colors*), and changes
with it while Flux is open, made a shade lighter or darker where it must be to stay
readable. *Preferences › App › Accent colour › Flux blue* keeps Flux's own blue. A
desktop that doesn't say which accent it uses gets Flux's blue either way.

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

**What a backup changed.** On a backup's Overview, each recent backup says what it
changed: "214 new · 38 changed · 2 deleted" (in Compact, as a comparison does: *+214 ~38
−2*). Click it to see the files, compared with the backup before.

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

**Size limit.** *Settings › Exclude Files/Directories › Skip files larger than* takes a number
and a unit (MB, GB, TB). A limit under 1 MB leaves out nearly every file, so Flux warns
when you set one, and Overview and Snapshots say so while it's set.

**Preview.** To see a file before restoring it, select it and press Space, click the eye
next to *Restore*, or right-click › **Preview**: a picture is shown, a text file's first
lines (up to 400), and anything else what's known of it, all read from the backup as it
was. *Restore…*, *Open a copy* and *Versions…* are in the preview too. Files over 32 MB
aren't previewed.

To just look at a file, double-click it (or right-click › **Open a copy**). Flux restores a
read-only copy into a temporary folder and opens it with your usual app; nothing on your
computer changes. Save it somewhere else if you want to keep it, or restore it. Temporary
copies are removed after a day.

**Browse in Files.** To look through a snapshot with your usual file manager instead,
choose *Browse in Files* in its ⋯ menu: Flux mounts it read-only and opens it there (at the
backed-up folder, when there's one). Copy what you need out of it; nothing in it can be
changed. A line above the browser says it's open; *Close* unmounts it, as does opening
another snapshot this way or quitting Flux. It needs FUSE (`fusermount3`), like `flux
mount`, and isn't offered for system backups, whose files belong to root.

### From the file manager

Right-click a file or folder in **Nautilus** (GNOME Files; the items are under **Flux
Backup**), **Nemo** or **Dolphin**:

- **Previous versions…** opens Flux on the versions of that file or folder in the backup
  that takes it, newest first, each with *Compare*, *Open* and *Restore…*.
- **Restore deleted files here…** (on a folder, or a folder's empty space) opens the
  newest snapshot at that folder with *Only deleted* on: what the backup has that isn't
  there any more, ready for *Restore N deleted items*. For something deleted before the
  newest snapshot, pick an older one in the list.

Flux starts if it isn't running (or the running one shows its window). When none of your
backups takes the folder, it says so. The same menu can leave things out of your backups
or add a folder to one (see [Leave out or add from the file
manager](#leave-out-or-add-from-the-file-manager)). The entries come with the `.deb` and `.rpm`
packages; Nautilus also needs nautilus-python (`nautilus-python` on Fedora,
`python3-nautilus` on Debian and Ubuntu). From a terminal, the same is `flux-backup
--previous-versions PATH` or `flux-backup --restore-deleted FOLDER`.

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

An archive of a **system backup** is made by root and readable by root only, like the
files it holds (`/etc/shadow` among them), wherever you save it. Use it with `sudo`, or
restore into a folder instead.

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
computer?* opens a short checklist, with a button beside each line that isn't ready yet:

| Line | Its button |
|---|---|
| Its settings are kept with the backups (and current) | **Back up now**: each backup keeps a copy |
| The password is kept somewhere other than this computer | **Keep it elsewhere…**: shows the password from where Flux keeps it (masked until you click *Show*), copies it for a password manager (the clipboard is cleared 30 seconds later), saves a copy of a Vault's unlock file, and ticks the line. Flux can't know where you keep it, so **I have** ticks it too |
| The Borg key is exported | **Export the key…**: says why it matters, then `borg key export` into a folder you choose, as a new file; keep it with the passphrase, not next to the backups. Until it's done, the backup's Overview says so too. Exported it yourself? **I have** ticks it |
| A restore test has passed | **Test a restore now** (or *Test again*) |
| A copy is kept somewhere else | **Add a place…**: opens Settings at *Keep a copy somewhere else too* |

A system backup's password and unlock file stay readable by root only, so its checklist
doesn't show them.

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
A snapshot whose record is damaged is said and left out (`snapshots` and `verify` then
exit with 3); every other snapshot is listed and restores as usual.

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
years. A `[retention]` table that sets none of the `keep_` rules (only `min_free_space`,
say) keeps the default ones; one that sets any means exactly what it says.

`min_free_space` and `min_free_inodes_percent` go past the bucket rules: while the drive
is below them, the oldest snapshots are removed. They never take pinned or named
snapshots, the `keep_last` newest, or the newest `safety_floor` complete ones, and they do
nothing when `enabled = false`.

**Deleting by hand.** To make room now, delete snapshots yourself: *Delete…* in a
snapshot's ⋯ menu for one, or **several at once**: Ctrl-click snapshots in the list (or
*Select several…* in the menu, then click them), Shift-click to take a range, and
*Delete N…*. Pinned snapshots among them stay (unpin them first); a Vault frees the data
only the deleted snapshots used, and the toast says how much. **Changed your mind?** For
eight seconds after you confirm, the snapshots are only out of the list: *Undo* in the
corner (or Ctrl+Z) brings them back. Then they're deleted. If Flux is quit within those
seconds, nothing is deleted. In a terminal: `flux delete
home 2026-11-01_20-00-00 2026-11-02_20-00-00 …` (`--force` for pinned ones).

Pruning runs after every backup. Run it yourself with a preview:

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

**Unusual changes: Flux Sentry.** After each backup, Flux Sentry compares what changed
with what usually changes in this backup, and looks for the marks ransomware leaves: many
more files changed or deleted than is usual for this backup (below); files renamed to a new extension (`report.odt` →
`report.odt.locked`) or many new files with extensions ransomware uses; text files
(`.txt`, `.csv`…) whose contents now look encrypted (a sample is read); ransom notes
(`HOW_TO_DECRYPT.txt`).

**It looks before anything is written.** For backups in Flux's own formats (plain
folders and encrypted), Sentry compares your files as they are now with the newest
snapshot *before* the backup starts. If they show unusual changes, **the backup isn't
made**: nothing of an attack reaches your backups, on this drive or any other place.
The warning then reads *Flux Sentry stopped the last backup*. (For backups written by
rsync or Borg, and while a backup has no snapshot to compare with, it looks at the new
snapshot right after it's written.) If the changes were yours, *This was me* lets the
next backup take them without being stopped again.

**Tripwire files** (off unless you turn them on, in the setup step or *What to keep ›
Safety*; `tripwires = true`). Flux Sentry plants a small hidden file,
`.flux-sentry-tripwire.txt`, in each folder the backup covers and in its first eight
visible subfolders, at the next backup. The file says what it is. Nothing has a reason to
change it, and ransomware changes or renames every file it finds: before each backup
Sentry looks at them, and if one has been changed or removed the backup is stopped at
once, however little else has happened yet, for a backup of any kind (rsync and Borg
too). A folder you delete takes its tripwire along without an alarm, and a drive that
isn't connected says nothing. When you settle the alarm they are set again (a changed
one is moved aside as `.flux-sentry-tripwire.txt.tripped`, never deleted). Turning the
setting off, or deleting the backup, removes every tripwire that is still as Flux
planted it (one on a drive that's away goes when it's back). Two backups of the same
folder share its tripwires, and a tripwire an older snapshot's restore puts back is as
good as the one it replaces. A folder you take out of a backup loses its tripwires at the
next run. They aren't planted for system backups. `flux sentry home` says how many
there are.

When a backup stands out:

- (a folder whose drive isn't connected is never counted as deleted, and what Sentry
  learns from is its own count of files whose contents changed or that went, the same
  thing it looks at: a `chmod -R` teaches it nothing)
- **the snapshot from before is pinned**, with a note saying why, so it can't be removed
  (and a snapshot that was written with the changes is tagged *unusual changes*);
- **the backup is put on hold**: no further backup is made, by its schedule or by *Back
  up now*, until you settle it. Another backup could only store more of the damage. The
  backup's page shows *Flux Sentry · backups on hold*, and a notification says so again
  once a day for as long as it lasts;
- **its other places aren't backed up to** that time: they keep their copies from before
  the changes, the ones kept elsewhere above all. (When the main backup can't be reached,
  its drive away or its server down, each other place is looked at against its own
  newest snapshot instead, before anything is written there);
- old snapshots aren't removed until it's settled, and a notification says what was seen;
- the Overview says what was found and where, with **Restore everything…** (below),
  **Show the files**, **Browse the snapshot before** (to restore from it piece by
  piece), and the two ways to settle it:
  - **This was me**, when you made the changes: backups go on, Flux Sentry takes that
    backup as normal from now on, and the pin and the tag are removed;
  - **It's dealt with**, when you didn't, and have restored what you need: backups go
    on, Flux Sentry learns nothing from that backup, and the snapshot from before stays
    pinned.

**Restore everything…** is the short way back: it opens a restore of everything the
backup covers, from the snapshot kept from before the changes, back to where it was. You
see what it will do before anything is written. Files that differ now are kept beside
the restored ones (renamed), not deleted, unless the backup is set not to keep them
(*Keep my files when restoring over them* is off): the dialog says which, in red when
they'd be replaced. What was added since stays where it is, for you to look through and
clear out. When a restore *back to where it was* finishes without errors, the alarm is
settled as *dealt with* and backups go on. A restore into another folder, to look at the
files first, leaves the backup on hold: your files are still as they were, so settle it
yourself once they're right. The first backup after an alarm you
settled isn't stopped again for the same changes, or for what an attack left lying
about; the snapshot from before stays pinned.

If you didn't make the changes, don't delete or change anything: disconnect the computer
from the network and restore from the snapshot before. In a terminal, `flux sentry home`
says whether a backup is on hold and why, `flux sentry home --this-was-me` and
`flux sentry home --dealt-with` settle it, and `flux backup` refuses until then. Turning
*Flux Sentry* off for the backup also lets backups run.

Setting up a backup asks how Flux Sentry should watch it, and *Flux Sentry: watch for
unusual changes* (*What to keep › Safety*; `unusual_changes = "high"`) changes it later:

| | Warns when | For |
|---|---|---|
| **Normally** (recommended) | a backup changes about 6 times its usual amount or more (at least 100 files and 5% of them), 20 files are renamed to a new extension, 10 text files look encrypted, or 3 ransom notes appear | most backups |
| **Closely** | about 3 times its usual (30 files, 2%), 8 renames, 4 encrypted-looking files, or 1 ransom note | files that rarely change; a busy folder may raise a false alarm now and then |
| **Off** | never | when you don't want the warnings: backups and snapshots are made just the same |

While Sentry watches a backup, its page shows a green **Flux Sentry active** tag (a
shield with an eye) next to where it's kept and when it runs. While it's on, the snapshot before the newest always
survives one more backup, so there is one to keep.

Click that tag (or the *Unusual changes* cell under *Settings at a glance*) for Flux
Sentry's own dialog: what it has learned of this backup, how closely it watches, and the
tripwire files, without going through the other settings.

*How Flux Sentry knows what's usual.* No fixed limit fits every backup: a folder of
finished work where almost nothing changes should raise the alarm long before "30% of the
files", and a projects folder rebuilt every Monday shouldn't raise it at all. So Sentry
learns each backup's own normal from its last 60 backups, and keeps three things:

- **A level**: how much of the backup usually changes, as a moving average that leans
  toward recent backups (each new one counts for a fifth), so it follows a backup whose
  habits change. It works with the logarithm of the share, since 0.1% and 10% are both
  ordinary, for different backups; and one extraordinary backup moves it only a little.
- **A spread**: how far backups usually land from the level (the median absolute
  deviation of the last 30, which a few odd ones can't inflate).
- **A memory**: an amount of change this backup has reached at least twice before, in
  backups that weren't flagged or that you answered with *This was me*, is part of its
  normal, however rare.

A backup is unusual when it lands 6 spreads above the level (4 when watching *closely*),
changes at least 100 files and 5% of them (30 files and 2%, closely), and is nothing the
memory knows. For a steady backup that is about 6 times its usual change (3.3 times,
closely); for one whose backups vary a lot, more, up to 20 times. The warning says it in
those terms: "480 files changed or deleted, 4.8% of them: 40 times what this backup
usually changes (0.12%)". The *Unusual changes* cell of a backup's settings at a glance
says what Sentry has learned so far and where it would warn. Until a backup has run five
times, fixed limits stand in (30% of the files and 5 times the usual; 15% and 3 times,
closely). Backups whose changes were flagged teach it nothing unless you say they were
yours, so an attack is never taken for a habit; the marks of ransomware (renames,
encrypted text, ransom notes) are looked for whatever the amount of change.

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

**A Vault without encryption.** A Vault asks *Encrypt the backups: On or Off* (in the new
backup guide, under the formats, and in its Settings). Off, it is still a Vault,
compressed and deduplicated, with no password to keep or lose; anyone with the drive can
read it. It suits a drive that is encrypted already (LUKS), or backups that
needn't be private. (Plain folders are chosen under *How backups are stored*.) Like the
password itself, this is decided when the Vault is made.

**A scheduled backup can't ask.** When the password is set to *Ask every time* and the
backup runs on a schedule (or when its drive is plugged in), nobody is there to type it and
those backups fail. The app warns about it next to *When* and under *How backups are
stored*, with **Keep it in the keyring** to fix it: type the password once, Flux checks it
opens the backups and keeps it in the keyring. (For a system backup the fix is a root-only
key file.) `flux profile validate` gives the same warning.

A Vault can have several passwords. Each one opens it, and changing them never re-encrypts
data:

```sh
flux key list docs
flux key add docs --label "spare, in the safe"
flux key passwd docs
flux key remove docs <key-id>
```

**There is no way to recover a Vault whose passwords are all lost**, unless it has a
recovery code (below). Make one, or keep a second password somewhere safe.

### A recovery code

If the password is lost, an encrypted Vault can't be opened by anyone, you included. A
**recovery code** is the way back: a long random code (six groups of five letters and
digits) that opens the Vault like a password. Print it or write it down, and keep it
somewhere safe, away from the computer and the backups.

Setting up an encrypted Vault ends with an offer to make one; later, it's under *How
backups are stored › Security › Recovery code*, and the recovery checklist has a line for
it. The code is shown once, when it's made; Flux doesn't keep it. Making a new one stops
the old one working. To use it, type it wherever the password is asked for, in Flux or
`flux-vault-recover`: capitals, spaces and dashes don't matter, and O, I and L are read
as 0, 1 and 1. It doesn't need the unlock file, so keep it as safe as the password. In a
terminal: `flux key recovery home` makes one, `--remove` removes it.

**After unlocking with it in the app**, Flux asks for a new password straight away, as
the old one is lost:

- The new password replaces the old one, which stops opening the backups. When the
  Vault has several passwords, you choose which stop working (untick a spare you still
  know).
- It's kept where the old one was: in the keyring, or in the backup's key file. If the
  password comes from a command, change the command yourself.
- If the old password needed the unlock file, so does the new one. If the unlock file
  is gone too, the new password works without it; set one up again under *Security*.
- You get a new recovery code, shown once, and the one you just typed stops working: it
  may have been seen, or left in the clipboard. (For a system backup, make the new code
  with `sudo flux --system key recovery <backup>`.)

Nothing is re-encrypted, and it's instant: the passwords and the code each unlock the
same key, which only your computer saw. In a terminal, `flux key passwd` after opening
with the code adds a new password and keeps the code.

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
  up to a few percent more space and applies to backups from then on. It's on for Vaults
  set up with Flux 1.2 or later; Vaults from before keep the setting they had.
- **Lock it again in the app.** After you unlock a Vault in the app, the app keeps its
  password in memory until it closes. Choose 5 minutes to an hour to have it forgotten
  sooner when unused. Scheduled backups get the password from their own source and aren't
  affected.

The encryption itself (XAES-256-GCM with 256-bit keys, and keyed BLAKE3) isn't a
setting: neither has a stronger alternative worth choosing. XAES-256-GCM is AES-256-GCM
with a key of its own for every piece stored; AES runs in the processor's hardware, so
encrypting keeps up with the fastest drives. A Vault made before 1.3 keeps the cipher it
was made with, XChaCha20-Poly1305, and works as it always did; the Security page says
which a Vault uses.

Sparse files (disk images, some databases: files with *holes*, ranges that take no disk
space) keep their holes in a Vault: only their data is read and stored, and a restored
copy is sparse again, as with Mirror. A Vault created before Flux 1.0 stores them whole
until you upgrade it: *Upgrade the Vault* on the Overview (shown only for
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
exact command). Until it's exported, the backup's Overview says so, with **Export the
key…**, which does it for you. **Without the passphrase, the data can't be decrypted.**

## Checking your backups

```sh
flux check home              # structure: every snapshot complete, every file accounted for
flux check home --read-data  # also read and verify every byte (slow)
flux check home --repair     # Vaults: and rebuild what is damaged, or let the next backup store it again
```

Three checks, each answering one question, from quickest to most thorough. On a backup's
Overview they're together under *Checks and upkeep*:

| Question | Check | What it reads |
|---|---|---|
| Is every snapshot whole? | **Check** (`flux check`) | each snapshot's record and structure, not the data: quick |
| Can files come back? | **Restore test** (`flux restore-test`) | some files, read back the way a restore does, every byte checked; nothing written |
| Is all the stored data undamaged? | **Verify every byte** (`flux check --read-data`) | everything stored, to find a failing drive: as long as reading the whole backup |
| Can damage be put right? (Vaults) | **Repair…** (`flux check --repair`) | everything stored; rebuilds what is damaged, and lets the next backup store again what can't be rebuilt |

Vault data is authenticated, so any corruption or tampering is detected.

### Repair data: damage undone, not only found

Drives go bad a little at a time: a sector that no longer reads, a bit that flipped. A
check finds that; by itself it can't undo it, and a damaged part of a Vault takes the
files stored in it along. With **repair data** it can be undone. In *Settings › Where to
keep it › How backups are stored*, **Repair data** keeps a little extra with
everything the Vault stores from then on (2, 5, 10 or 20% more space; off by default):

```toml
[vault]
parity = 5                        # percent of repair data: 1–50, 0 for none
```

When **Verify every byte** then finds damage, it rebuilds the damaged parts, checks the
result against the same checksums as before, and says what it repaired (a drive that
needed repairs is a drive to replace). The monthly verification of *Upkeep* does the
same, with a notification. On the command line a check only reports, and says what could
be repaired; `--repair` does it:

```sh
flux check home --read-data       # "…its repair data can rebuild it"
flux check home --repair          # read every byte, rebuild what is damaged
```

Verifying every byte also adds repair data to what was stored before the setting was
on (it reads everything anyway), and writes again repair data that is itself damaged.
The storage summary on the Overview says how much there is and whether it covers
everything.

**Damage that can't be rebuilt** (too much of it, a stored file that is gone, or a Vault
with no repair data). A backup stores only what the Vault doesn't already list, so data
the Vault lists but can no longer read would never be stored again: every later snapshot
would count on it. `flux check home --repair` takes it off the list, keeping every piece
that still reads, and the next backup stores it again from your files. That also puts
older snapshots right, wherever the file is still as it was. Check again afterwards:
what it still reports is gone from your files too (a file changed or deleted since), and
it says which files of which snapshots. If the Vault's index itself can't be read, the
repair writes it again from the stored files, which each list what they hold.

```sh
flux check home --repair          # take out what can't be rebuilt
flux backup home                  # store it again from the files
flux check home --read-data       # what is left, if anything, is gone for good
```

In the app the same three steps are under *Checks and upkeep* on a Vault's Overview.
**Repair…** (*Can damage be put right?*) says what it will do and asks first; a check or
*Verify every byte* that finds problems offers it too. If something couldn't be rebuilt,
its result lists the files concerned and offers **Back up now**; verify every byte once
more after that backup. Only a repair you ask for takes anything out of a Vault: *Verify
every byte* and the monthly check rebuild from repair data and otherwise only report.

Removing old snapshots checks what it moves, too. When a prune copies the pieces still
needed out of a mostly unused pack, it first makes sure the pack is undamaged; a damaged
one is rebuilt from its repair data, or, if it can't be, left exactly as it is (and the
run says so) so that nothing that could still repair it is thrown away.

How it works, and how much it repairs: each stored file is cut into 4 KiB blocks, and
Reed–Solomon parity blocks are computed over groups of up to about 240 of them (the
arithmetic of RAID 6 and PAR2). With 5%, a group gets 13 parity blocks and can lose
**any 13** of its blocks; the groups are interleaved, so a single damaged stretch of up
to about 5% of a file (some 880 KiB of a 16 MiB one) is repaired, as is any scattering of
damage that leaves no group with more than 13 bad blocks. Hashes of every block say
which ones are damaged. What it doesn't repair: a stored file that is gone entirely, or
a drive that has died. That is what a copy in a second place is for (*Several places at
once*); copies carry the repair data too. The file format is in
[VAULT_FORMAT.md](VAULT_FORMAT.md); it adds to a Vault without changing it, so older
versions of Flux and `flux-vault-recover` read the Vault as before.

### Restore tests

A check reads what's stored; a **restore test** goes all the way: it reads some files of
the newest snapshot back the way a restore does (from the drive or the server, decrypted
and decompressed for a Vault, through `borg extract` for Borg) and checks each against the
backup's own record (its size, and its checksum when the profile records checksums; a
Vault checks every piece of data it reads anyway). A file that can't be read back, or
comes back with the wrong size or contents, fails the test. A test stopped at a limit,
and (if asked) files that differ from your copy now, are notes: the test passes with a
warning.

**Nothing is written anywhere.** Each file is checked as it streams, a piece at a time,
and each piece is wiped from memory once checked: no copy lands on a disk or in a
temporary folder, so an encrypted backup's files stay encrypted at rest, and the test
uses the same little memory for ten files as for every file. (Until 1.1, a restore test
restored its files into a temporary folder and checked their permissions and times too; a
folder one of those left behind is removed by the next test.)

On a backup's Overview, **Test a restore now** runs one; *Settings › What to keep ›
Restore tests* runs them by themselves, after a backup once due: **after every backup**,
**weekly** or **monthly** (off by default). Which files: **enough to be sure** (below),
**some at random** (10), **a share of the backup** (a percentage of its size), **every file
once a month** (a different part each time, so every file is tested within 30 days), or
**everything**. Limits on time and data read keep big tests in check. A failed test is notified and stays on the Overview and
All backups until one passes; a test that's overdue (no pass within the interval and a
bit) is a warning.

**Enough to be sure.** "Ten files came back" says little about the other hundred
thousand. With *Enough to be sure*, you say what a pass must prove instead, and Flux works
out how many files that takes: *catch damage to 1% of the files or more, 99% certain*
reads **459 random files**, and the report then says so: "if 1% or more of its 184,000
files were damaged, this test would have found it (99% certain)".

The number hardly depends on the size of the backup: 459 files say as much about ten
million files as about ten thousand (a small backup needs fewer, since no file is picked
twice: 368 of 1,000, 99 of 100). It grows with what you ask:

| Catch damage to | 90% certain | 95% | 99% | 99.9% |
|---|---|---|---|---|
| 10% of the files | 22 | 29 | 44 | 66 |
| 5% | 45 | 59 | 90 | 135 |
| 1% | 230 | 299 | 459 | 688 |
| 0.5% | 460 | 598 | 919 | 1,379 |
| 0.1% | 2,302 | 2,995 | 4,603 | 6,905 |

How it's worked out: if *D* of the snapshot's *N* files are damaged (can't be read back,
or don't match their record) and *n* are picked at random, the test misses them all with
probability C(N − D, n) / C(N, n). Flux takes the smallest *n* that brings this to
1 − certainty or less for *D* = the share you chose; more damage than that is caught more
surely still. For a large backup this is n ≥ ln(1 − certainty) / ln(1 − share). What it
doesn't say: that fewer damaged files than that would be found (only testing everything,
or every file once a month, gets to each file), or anything about a test stopped at a
limit, which makes no such statement. The files are picked with the system's randomness,
each set of *n* equally likely.

A test of *some files at random* that passes says what it was worth, too: ten files
catch damage only when 37% of the files or more are damaged (99% certain).

```sh
flux restore-test home                 # the profile's settings (10 random files)
flux restore-test home --damage 1%     # enough files to catch 1% damage, 99% certain
flux restore-test home --damage 0.1% --certainty 99.9%
flux restore-test home --files 50 --compare-source
flux restore-test home --full
flux restore-test home --history
```

```toml
[restore_test]
every = "month"                        # off | backup | week | month
mode = "confidence"                    # random_files | percent | full | rolling | confidence
# damage = "1%"                        # confidence: catch damage to this share of the files…
# certainty = "99%"                    # …this surely (0.01–50%, 50–99.99%)
# files = 10                           # random_files
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

**In the app:** on the **New backup** page, choose **Back up the whole computer** (last,
below the templates). Flux explains what it needs; then your desktop asks for an
administrator password (yours, if your account is an administrator), and setting up the
system backup begins. To see the system backups you already have, choose **System
backups…** in the **+** menu next to *Your backups*: once unlocked, they're listed under
*System backups* in the sidebar and work like your own. The app locks them again after 15
minutes without use, or with **Lock system backups** (the lock beside them, or the + menu).

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
run's log. A hook is done when its command is: something it leaves running in the
background (`ssh -f -N -L …` for a tunnel, `some-tool &`) keeps running and doesn't hold
the backup up; what it prints after that isn't logged.

## Help on the command line

Every `flux` command has a man page (`man flux`, `man flux-backup`, `man
flux-profile-create`…): what it does and what it changes, every option, examples of basic
use, its exit status (0 worked, 1 failed, 2 a wrong command line, 3 a profile with errors,
4 done but not everything went through) and related pages. `flux <command> --help` lists
the options and ends with the same examples; `flux help` lists the commands.

## Keeping an eye on things

```sh
flux status            # what's running, last result and next run of every profile
flux watch home        # live progress of a running backup
flux cancel home       # stop it; the next run continues where it stopped
flux log home          # recent runs
flux log home --run last   # everything that happened in one run, including warnings
```

The app shows the same. Next to each backup in the sidebar: a green tick when it's fine;
a quiet amber dot and a few words when it's protected but something is worth a look (*Low
on space*, *Not tested lately*, *A copy failed*…); a red ⚠ only when it isn't protected
(it failed, it's at risk, or it hasn't backed up yet); a spinner while it runs. The time
next to it is when its last good copy was made. *All backups* counts the same two kinds
("2 not protected · 3 to look at"). Live progress shows for every backup however it was
started, and an *Activity* tab has the full log of each run. *Problems* in Activity
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

**Settings at a glance.** A backup's Overview sums up its settings in a compact grid, so
you can see without opening Settings whether it has a schedule, where its password is
kept, what it keeps and so on: *Schedule*, *Backs up*, *Excluded*, *Where*, *Other
copies*, *Stored as*, *Password*, *Keeps*, *Restore tests*, *Upkeep*, *Unusual changes*,
*Notifications*, *Hooks* (and *Network* for a server), in three groups: *When and what*,
*Where and how*, *Safety and alerts*. Settings that say nothing about this backup (no
password for plain folders, no hooks) are left out. Something to fix is marked in amber
(a password asked for each time on a scheduled backup, say); a setting that's off is
greyed. Hover one for more; click it to change it right there: its section of the
Settings page (*What to back up*, *Where to keep it*, *Also keep copies elsewhere*, *When*,
*What to keep* or *More*) opens in a dialog, the same as on that page, with its own
**Save** and **Cancel**. A row in it that opens more (*More schedule options*, say) shows
it in the same dialog, with **Back**. You stay on the Overview throughout (*All settings*,
or the gear button, opens the whole Settings page). The grid folds away with its heading, and stays
folded until you open it again. The same goes for *Change the limit* on the size-limit
note and *Add a place…* in the recovery checklist.

On *All backups*, **Settings** (next to *Status*) shows every backup's settings side by
side in a table, one line each (hover a cell for all of it): schedule, next backup, where,
how it's stored, the password, what's kept, restore tests and notifications. **Columns**
adds or removes any of the others (what's excluded, other copies, upkeep, hooks…). Click a
column's heading to sort by it (again to reverse, a third time to go back); click a
setting to change it in a dialog over the table, a name to open the backup. The table uses the window's whole width,
with the headline shortened to one line above it. The filters work here too, and the app
remembers the view and the columns you chose.

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

**A weekly note when all is well.** Once a week, when every one of your backups is
protected with nothing to look at, Flux sends one quiet notification saying so: how
recent the newest and oldest copies are, and when the last restore test passed. It comes
at the end of a scheduled backup, so the app needn't be open, and never when something
needs you (those have their own warnings). The first one comes a week after the first
check. *Preferences › Notifications › A weekly note when all is well* turns it off (in
`settings.toml`: `weekly_note_off = true`).

**Quiet hours.** *Preferences › Notifications › Quiet hours* (off at first; 10 PM to 8 AM
when turned on, and the times can be changed) keeps the notifications that only inform out
of those hours: a scheduled backup that succeeded, and the weekly note, which then comes
with the first check after them. Everything that needs you is still sent at once: a failed
backup, unusual changes, a failed restore test, what a monthly check found. Scheduled
backups follow it with the app closed. In `settings.toml`:

```toml
[quiet_hours]
from = "22:00"
to = "08:00"     # earlier than from: the next morning
```

An interrupted backup (power cut, Ctrl-C, unplugged drive) doesn't damage anything. The
unfinished snapshot is never shown, and the next run continues it.

**Buttons on notifications.** When a scheduled backup, or one started from the app,
fails or finishes with problems, its notification offers **Try again** (the same backup
runs again at once) and **Open** (Flux Backup opens on that backup, started if it isn't
running); clicking the notification itself opens it too. Notifications about unusual
changes, a failed restore test or a monthly check offer **Open**. The backup waits for a
click for up to ten minutes after it ends (`FLUX_NOTIFY_WAIT` sets how many seconds), or
until you close the notification. A desktop whose notifications have no buttons shows
them as before, and `flux backup` typed in a terminal never waits.

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
exclude_gitignored = false        # in git working trees, skip what .gitignore ignores

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
release_drive = false             # unmount and power off the drive when a backup ends

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
compression_level = 3             # 0 (off) to 22; data that barely compresses
                                  # (photos, video) uses 1, data that doesn't, none
encryption = true                 # fixed when the Vault is created
password_source = { from = "keyring" }
# parity = 5                      # repair data: this percent more space (1–50), 0 = none
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
| `FLUX_NOTIFY_WAIT` | seconds a backup waits after it ends for a click on its notification's buttons (default 600) |
| `FLUX_PACK_WRITES` | `buffered`: a Vault on a local drive is written through the page cache, as before 1.3, instead of straight to the drive |
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
