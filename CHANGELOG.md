# Changelog

All notable changes to Flux Backup. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [0.3.0] - 2026-11-09

Restoring is easier and finer-grained, a size limit can no longer quietly leave a backup
empty, comparing with your files explains what it shows, setup is clearer, a new backup
can start from your Back In Time settings, and scheduled backups can run with cron. The
app takes GNOME's own colours, and a backup's history is drawn as bars.

### Added
**Coming from Back In Time**
- When you set up a new backup, Flux offers each profile it finds in
  `~/.config/backintime/config` (however many there are). **Use these settings** fills in
  the folders, exclusions, destination, schedule, what to keep and options. rsync is
  preselected, since it makes the same plain, hard-linked folders Back In Time did, and
  every other way of storing backups is still offered on the Protection step. Profiles
  Back In Time encrypted with EncFS become an encrypted Vault.
- Exclusions are rewritten in Flux's syntax with the same meaning (`.cache/*` becomes
  `**/.cache/*`), and anything that didn't carry over exactly is listed.
- Back In Time's own snapshots are never touched. Flux starts a fresh set next to them, and
  says so: keep Back In Time to restore from the old ones, and turn off its schedule.
- Extra rsync options set in Back In Time come over too (as `[rsync] extra_args`), except
  those Flux sets itself, which are listed.
- On the command line: `flux profile backintime` lists the profiles, and
  `flux profile backintime 1 [--id …] [--engine …]` creates a Flux profile from one.

**Running backups**
- **Run one backup at a time** (*Preferences › Advanced*): a backup that starts while
  another is running waits for it to finish instead of competing for the disk or the
  network. It shows as *Waiting for its turn…*, says which backup it's waiting for, and
  can be stopped while it waits.

- **Shut down when done**: *Back up now, then shut down* in a backup's menu, a *Shut down
  the computer when done* box under a running backup, or `flux backup --shutdown`. The
  computer powers off a minute after the backup ends, with a countdown and **Cancel
  shutdown** in the app (`shutdown -c` works too). Stopping the backup cancels it.

**Scheduling with cron**
- Scheduled backups can run with cron instead of systemd timers: *Preferences › Advanced ›
  What runs scheduled backups*, or `flux schedule use cron` (`flux schedule use systemd`
  to go back). systemd timers stay the default. Switching moves every scheduled backup
  over, and is undone if the new scheduler can't be set up.
- Flux keeps its own block in your crontab (or `/etc/cron.d/flux-backup` for system
  backups) and never changes the lines around it, such as Back In Time's.
- Missed backups are still caught up: with cron, Flux checks every 15 minutes whether a
  scheduled time passed while the computer was off or asleep, and runs it once, as
  systemd's `Persistent=` does.
- Schedules are read by Flux the same way systemd reads them (checked against
  `systemd-analyze`), so a schedule means the same under either. Backing up when a drive
  is plugged in needs systemd; with cron, Flux says so where that's set.
- `flux schedule status` says which scheduler is in use, and hints when no cron daemon
  is running.

**Restoring**
- Every file and folder in the snapshot browser has its own **Restore** button.
- Ticked items stay ticked while you move between folders, so files from several folders
  can be restored together.
- Items that are no longer on your computer are marked **Deleted**. **Only deleted** shows
  just those, and **Restore N deleted items** brings back exactly what's missing.
- Restoring something that was deleted goes back to its own place by default.
- **Compare file versions**: *Compare* on any version of a file shows what changed between
  it and the file now, or another version, line by line in the app, with long unchanged
  stretches folded away. **Open in Meld** (or Kompare, KDiff3, Diffuse) opens both in a
  comparison program, chosen in *Preferences › Advanced* or with `diff_tool` in
  `settings.toml`.
- **Exactly as it was**: restoring a folder in place can also remove what's in it now but
  wasn't in the snapshot (*Make the folders exactly as they were*, or
  `flux restore --exact`). The dialog lists what would go first; removed items are kept
  as `name.flux-<snapshot>` unless you choose to replace your versions. What the backup
  never takes (exclusions, the size limit, caches, other drives) is never removed.
- The restore dialog lists what will be added, and warns before restoring when the
  snapshot holds only empty folders there.
- From **What changed**, restore a removed file, or the older version of a changed one,
  straight from the list, or use **Restore all removed**.

**Setup and settings**
- **Skip files larger than** takes a number and a unit (MB, GB, TB), and warns when the
  limit is so small that it leaves out almost every file.
- While Flux backs up with a size limit, Snapshots says so (with a link to change it), and
  Overview warns when the limit is tiny.
- Step 1's size count has a refresh button, and its numbers stand out.
- **Storage meters**: each drive offered on the *Where* step, and a backup's Overview
  (*Space on …*, now shown whenever the drive is connected, not only once a forecast
  exists), show how full the drive is: free space, size and percent, amber when it's
  getting full and red when there's almost no room left.
- Step 4 explains in plain words what happens each time the backup runs.
- **Recommended defaults** in *Performance and behaviour* puts those settings back to
  what Flux recommends.

### Changed
**A new look**
- The app takes GNOME's own colours (Adwaita): neutral greys with no blue tint, and in the
  dark theme a sidebar a step lighter than the rest of the window, as in GNOME Files and
  Settings. The main button of each page is a deeper blue with white text, and so is a
  switch that's on.
- In the light theme, groups of rows sit on Adwaita's soft card shadow.
- The last 30 days are drawn as a row of small bars instead of dots: a taller bar when a
  day's backups copied more, a full red bar for a day that only failed, and a flat stub
  for a day with nothing. On a backup's own page it is a small bar chart with the first
  day and today named under it. Pointing at any day still says what happened.
- The snapshot list marks each snapshot with the same kind of bar, on the same scale.
- **Recent backups** on a backup's page keeps each backup's size next to its time.
- Red text is a little brighter in the dark theme, so it stays easy to read on buttons.
  Every colour was checked again for contrast (WCAG AA) in both themes.
- The screenshots in the README and on the website show the new look.

**Everything else**
- Exclusion suggestions you've added ("Often left out") are shown in green.
- Step 3's "What exactly Flux does" is an information block (still closed until clicked).
- **What changed** hides folders whose only change is their date (shown with one click),
  names what changed in words (contents, date, permissions…), and shows the size change
  only when there is one.
- Sizes in profiles are written the way they read best ("500MB", "2GiB").

### Fixed
- With a size limit, rsync backups recorded the files they skipped as if they were stored
  (an empty backup could say "21 files, 252 MiB"). Snapshots now count what they hold,
  and the run log says how many files the limit left out.
- Comparing with your files on disk ignored the size limit's effect: files bigger than
  the limit showed as removed when an older snapshot held them, or the comparison showed
  next to nothing. Big files a snapshot holds are now compared, and the others are counted
  and explained. Borg, which can't skip files by size, compares them all.
- Restoring a folder together with something inside it failed with "not in snapshot".
- **Back up now** with the backup drive unplugged left the app on "Backing up…" for good,
  and **Stop** then said "no backup … is running": the backup had failed in its first
  milliseconds, before the app could follow it. Now Flux checks the drive first and says
  at once that it isn't connected and where it should be mounted. A backup that stops
  straight away for any other reason reports its error instead of spinning, the app
  double-checks a backup that never reports in, and **Stop** on a backup that already
  ended shows how it went.
- Setting up or editing a backup whose drive isn't plugged in now says so on the
  destination step, and the final step offers **Done** instead of a first backup that
  could only fail.
- Files you'd just restored stayed marked **Deleted** in the snapshot browser until you
  moved to another folder and back. The folder is now checked again as soon as a restore
  finishes (and when you come back to the window), and a new **refresh** button next to
  the filter (or F5) checks it again whenever you like. Search results now mark a file
  **Deleted** only when it isn't on your computer now: before, anything missing from the
  newest snapshot was, even right after you'd restored it.

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

[0.3.0]: https://github.com/neuralpunk/flux-backup/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/neuralpunk/flux-backup/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/neuralpunk/flux-backup/releases/tag/v0.1.0
