# Changelog

All notable changes to Flux Backup. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [0.2.0] - 2026-11-08

The big additions: a new look for the app with an "am I protected?" start page and
step-by-step setup, backups that don't need Flux (compatibility mode with rsync or Borg),
the password chosen while setting a backup up, export to other backup tools, finding
files across all snapshots, space forecasts, pausing and upkeep for scheduled backups, and
faster backups and browsing.

### Added
**The app: protection at a glance, setup step by step**
- **All backups**, the new start page: one sentence on whether you're protected, what
  needs you and why, and every backup with its source, destination, last good copy, next
  run and a 30-day history strip.
- New backups are set up one question per step (what, where, protection, when, review),
  ending with an offer to run the first backup.
- The password of an encrypted backup is chosen while setting it up: the encrypted Vault
  or Borg repository is created then, so the first backup (even a scheduled one) asks
  nobody. On the command line: `flux key init <profile>`.
- Large files and spinning disks: Vault chunks a big file on all cores; when a source or
  the target is a hard disk, large files are copied or read two at a time.

**Compatibility mode: backups that don't need Flux**
- Two new engines that store backups in a standard tool's own format, written by that
  tool, so your backups never depend on Flux:
  - **rsync** (`engine = "rsync"`): every snapshot is a plain folder written by the
    system `rsync` with `--link-dest` hard links. Each snapshot records the exact
    rsync command and filter rules it used. Your exclusions become rsync filter rules
    with the same meaning.
  - **Borg** (`engine = "borg"`): a standard Borg 1.x repository written by the system
    `borg` (`repokey-blake2`, zstd compression). Snapshot names and pins live in the
    archive comments, inside the repository. Archives you make with borg yourself show
    up in Flux too.
- Both work on local drives and over SSH, and support browsing, restoring, "what
  changed", retention, checks and `flux mount`.
- A **recovery kit** is kept next to the backups: `FLUX-RECOVERY.txt` (written for
  newcomers and for rsync/Borg experts) and `flux-backup.sh`, which makes the same backup
  with rsync or borg alone. `flux recovery <id>` prints or saves it; the app has a
  Recovery guide dialog with copy buttons.
- rsync snapshots made without Flux are listed, browsed and adopted.
- In the app, "How backups are stored" offers Flux's own formats or "Works without
  Flux", in plain language with pros and cons, tool detection and install hints, and an
  "exactly what Flux does" section for experts.

**Export and import profiles**
- Export a profile as a **Flux profile** (to import on another computer), a
  **borgmatic** configuration, or a **restic**, **rsync** or **Borg** script. Every
  export lists what it can't carry over (pins and names, the schedule, space floors,
  tool limits).
- Import Flux profiles; an existing profile is never replaced.
- `flux profile export <id> [-f flux|borgmatic|restic|rsync|borg] [-o FILE|DIR]` and
  `flux profile import FILE [--id ID]`; in the app, *Export…* in every backup's menu and
  *Import a profile…* behind the sidebar's +.

**Finding files**
- **Find in all snapshots** (Snapshots tab): type part of a name; files you've since
  deleted are marked *Deleted*, with the last snapshot that had them. Click a result to
  show it in its snapshot, double-click to open a copy. `*`, `?` and `[…]` make a
  pattern; a `/` matches whole paths.
- **Versions of a file**: every distinct version with its date, size and snapshots;
  open or restore any of them.
- **Open a file from a snapshot**: double-click a file (or *Open a copy*) to open a
  read-only copy in its usual app, without restoring anything.
- **Restore preview**: before you restore, the dialog shows what would be added, which
  files differ (kept aside or replaced), and what's already identical. Replacing files
  takes a red "Restore and replace N files" button.

**Space**
- A **size estimate** while you set up a new backup ("About 32 GiB in 51,900 files,
  after the exclusions"), with a warning when it's more than the free space.
- A **space forecast** on the Overview for backups on a drive: a small chart of used
  space and, once there's enough history, when the drive fills up at that rate (with a
  warning under a month).

**Control**
- **Pause scheduled backups** for an hour, until tomorrow morning, or until resumed:
  from any backup's menu, Preferences, the tray, or `flux pause` / `flux resume`. The
  sidebar shows they're paused, with Resume. Backing up by hand still works.
- **Skip scheduled backups on metered connections** (backups to a server), as
  NetworkManager reports it.
- **Upkeep after scheduled backups**: verify every stored byte once a month (opt-in),
  and for Vault, free the space of removed snapshots once a week (on by default). You're
  notified of problems.
- A **Preferences** page: notify on successful scheduled backups, start in the tray at
  login, theme, and the exclusions new backups start with.

**Keeping an eye on things**
- **Health warnings in words** on the Overview: the drive hasn't been connected for a
  while, repeated failures, no successful backup for longer than the schedule suggests,
  almost no space left.
- A one-time card after the **first backup**, showing how to get a file back.
- Activity: an **All / Problems** filter and **Copy diagnostics** (also
  `flux diagnostics <id>`): versions, the profile with passwords and hooks hidden, and
  recent runs, ready to paste into a bug report.
- **Duplicate** a profile (`flux profile duplicate <id>`).

**The app**
- Right-click menus on backups, snapshots, files and runs, each also available from a ⋯
  button; Menu key and Shift+F10 open them from the keyboard.
- Easy profile deletion with a clear confirmation (the backups themselves stay).
- A new look, "At home on the desktop": GNOME's style in graphite and calm blue, with
  grouped lists, blue only for the action to take and a neutral highlight for where you
  are, Inter and JetBrains Mono, a custom title bar, Dark / Light / Auto, the "Stack" app
  icon, and a tray icon that lights up while a backup runs.
- A website (`site/`) with a landing page and the full user guide.
- `docs/UX_PRINCIPLES.md`: the usability rules the app follows, with their sources.

### Changed
- **Faster.** Reopening an encrypted Vault in the app skips the password hashing; a local Borg
  repository's listing is reused while unchanged; the run history is read from its end;
  timer status is one `systemctl` call; a Vault backup no longer syncs every pack, keeps
  its latest file list cached, and counts changes without copying entries; Mirror creates
  directories in parallel; the Snapshots browser indexes folders once per snapshot; the
  job watcher waits on inotify.
- **Settings without an "Advanced" mode.** The editor asks what, where, when and what to
  keep; every other group is a row showing its current value that opens just that group
  in a dialog over the editor. Settings is a button next to Back up now, not a tab.
  Nothing stays expanded between profiles, and rows with a problem say so in red.
- **Sidebar** lists "Your backups" with where each goes and how it's doing in words.
- **Settings rows look like what they are**: switches on the right, rows that open
  something end in ›, disabled controls say why, and descriptors (where, when, how) are
  outlined tags, not buttons.
- **System backups**: a compact row that explains what the administrator password is
  for before the system prompt appears.
- Sparse files (disk images, databases) stay sparse in Mirror backups and restores.
- Searching Vault, Borg, rsync and SSH backups uses a local index after the first time,
  so later searches and Versions are quick.
- `flux find` matches plain text as part of a name, in any case, like the app.
- App ID is now `io.github.neuralpunk.FluxBackup`; repository links point at
  github.com/neuralpunk/flux-backup.

### Fixed
- The Overview of a backup with a schedule stayed on loading placeholders (the next run
  came as a date WebKitGTK couldn't read).
- Deleting a Vault snapshot now frees its data at once (it used to stay until the next
  prune), and "Free up space" collects unused data even when no snapshot is removed.
- `flux diff` found no previous snapshot when two started in the same second.
- The encryption switch on the main settings page now changes the format; encryption
  can be chosen again after changing the format.
- Borg's passphrase environment is never logged.
- A backup could be reported as "already running" right after another finished, when a
  process forked at that moment still shared the job lock.
- Several flaky tests ("Text file busy" when running test helper scripts).

### Known limitations
- The borgmatic export was checked against borgmatic's published schema but not run
  through `borgmatic config validate`, and the restic script wasn't run (neither tool was
  available for testing).
- Start at login inside Flatpak would need the background portal; native packages and
  AppImage are covered.
- Space forecasts are for backups on a drive; SSH targets don't record free space yet.

## [0.1.0] - 2026-11-05

First release: Mirror (plain-folder, hard-linked snapshots) and Vault (deduplicated,
compressed, encrypted) engines; local drives and SSH servers; systemd schedules
(calendar, interval, on drive connect); retention; restore, diff, find and history;
hooks; the `flux` command line; the desktop app; system backups through a polkit helper;
read-only mounts of snapshots (`flux mount`); man pages and packages.

[0.2.0]: https://github.com/neuralpunk/flux-backup/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/neuralpunk/flux-backup/releases/tag/v0.1.0
