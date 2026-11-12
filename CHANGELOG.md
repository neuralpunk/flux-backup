# Changelog

All notable changes to Flux Backup. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [1.2.1] - 2026-11-12

A fix for starting at login, and a nudge to keep a Borg backup's key safe.

### Added

- **Export the Borg key.** An encrypted Borg backup whose key hasn't been exported now
  says so on its Overview: Borg keeps the key inside the repository, and if that copy is
  damaged, the passphrase alone can't open the backups. **Export the key…** opens a
  dialog that explains why, saves the key to a folder you choose (a new file, never over
  one), and shows where it went; **I already have** is for a key exported by hand. The
  recovery checklist's *The Borg key is exported* uses the same dialog.

### Fixed

- **Start at login** no longer opens a blank "Could not connect to localhost" window.
  An entry written by a development build now names the installed app, and the app
  repairs an existing entry that points at a copy it can't start from (a dev build, or
  an AppImage that has moved).
- **Activity** marks the run whose details are shown; it showed them with no row picked.
- **Activity's log** wraps a long line between words where it can, not mid-word ("err" /
  "or 13").
- **All backups**, in its wide layout, puts a "·" between a backup's status and its
  folders, as the narrow layout does ("Protected · ~ → BackupDisk").
- **What changed** no longer shows a chip for a kind of change with none ("0 changed
  type").
- **Overview**'s status line wraps between its parts, not inside one ("since" left at the
  end of a line, "Oct 10" alone on the next).

## [1.2.0] - 2026-11-11

A release for the person backing up their own computer, about the moments that matter.
Getting a file back now starts where it was lost: **Previous versions…** and **Restore
deleted files here…** in the file manager's right-click menu, and **Browse in Files** for
a whole snapshot. A **recovery code** means a forgotten password is no longer the end of
an encrypted backup: unlocking with it sets a new password and a new code. A USB drive
can be **safe to unplug** the moment a backup ends, and Flux **warns when the drive
itself is failing**. The app is **Compact** by default, so a backup's Overview fits on one
screen; files can be **previewed before restoring**, deleted snapshots **undone**, and
notifications have **buttons** and **quiet hours**. Several snapshots can be deleted at
once, every folder shows its size, a project's `.gitignore` can decide what's skipped,
and once a week a **calm note** says all is well.
Under the hood, new encrypted Vaults hide file sizes by default.

### Added
- **A Compact layout, now the default.** More on the screen: in the sidebar each backup
  is one line, its name and a word or two of status ("21%", "Failed", "Low on space");
  All backups lines the backups up in columns (backup, last 30 days, last good copy,
  next), so the 30-day strips compare at a glance; a backup's Overview, Snapshots and
  Activity switcher shares the title's line; and the Overview puts *Settings at a
  glance* (open to start with) and *Where it's kept* in a column beside the rest. Rows are a step shorter.
  In a narrow window the pages fall back to the Full arrangement.
- In Compact, **a backup's Overview fits on one screen** (1200×780) when nothing extra
  needs saying: the answer and the last 30 days share one card; recent backups are
  tighter; the restore test and the space are rows of one list (the space opens to its
  forecast and what the snapshots take); storage and *Where it's kept* fold to a line
  each; the header leaves out the tags the settings column already shows; and each
  setting at a glance is one line, its value on the right. The two columns start and end
  level, each under a heading of the same kind, and a running backup's progress sits in
  the answer card.
- In Compact's settings column, **a value too long for its line is said shortly**
  ("7d · 4w · 12m · 5y" for what's kept, "USB stick +1" for other copies), so every
  setting stays on one line; the full wording is in its tooltip. The All backups
  settings table uses the same short forms.
- **A sidebar that folds into a row of icons.** The sidebar button by the title (or F9)
  folds it to a column of icons, animated: each backup by an icon of what it backs up,
  with its status as a coloured dot, and its name and status on pointing. Remembered on
  this computer.
- **What each backup changed**, on a backup's Overview: "214 new · 38 changed · 2
  deleted" by each recent backup (*+214 ~38 −2* in Compact); a click compares it with
  the backup before.
- **A preview before restoring.** In a snapshot's files, Space (or the eye beside
  *Restore*, or right-click › *Preview*) shows a picture, a text file's first lines, or
  what's known of anything else, read from the backup; restore, open or see its versions
  from there. Up to 32 MB.
- **Undo after deleting snapshots.** For eight seconds after a deletion is confirmed, the
  snapshots are only out of the list: *Undo* (or Ctrl+Z) brings them back. If the app is
  quit within those seconds, nothing is deleted. `flux delete` is unchanged.
- **Buttons on notifications.** A failed backup, or one that finished with problems, offers
  *Try again* and *Open* in its notification (scheduled backups and ones started from the
  app); unusual changes, failed restore tests and monthly checks offer *Open*. The app
  opens on that backup, and is started if it isn't running (`flux-backup --show-backup
  ID`).
- In Compact, **a backup's Settings and Preferences list their sections at the side**
  (What to back up, Where to keep it, When…): the one you're in is marked, and a click
  goes to another.
- **Text size**: *Preferences › App › Text size* makes everything in the window smaller
  (90%) or larger (115%), the window's smallest size with it.
- **The desktop's accent colour.** The main button, switches, progress and focus follow
  the accent chosen in GNOME's or KDE's settings, live, a shade lighter or darker where it
  must be to stay readable (as readable as Flux's own blue). *Preferences › App › Accent
  colour* can keep Flux's blue instead.
- In Nautilus, **Flux Backup's items are under one *Flux Backup* submenu** in the
  right-click menu, so the menu stays short.
- **Leave out of backups, back up this folder: from the file manager.** Right-click a
  file or folder in Nautilus, Nemo or Dolphin: *Leave out of backups…* lists the backups
  that take it, each with the rule it would add (or the folder it would stop taking);
  *Back up this folder…* adds a folder to the backup you choose, or starts a new backup
  with it. Nothing changes until it's confirmed. Nautilus shows whichever applies. Also
  `flux profile leave-out PATH [--only ID]` and `flux profile add-folder ID PATH`.
- **Whether the drive suits the backup.** A plain-folder (Mirror or rsync) backup on a
  FAT32 or exFAT drive can't hard-link unchanged files, so every snapshot was quietly a
  full copy; links and permissions were lost, and FAT32 can't hold a file of 4 GB or
  more. Choosing such a drive now says so (with a Vault, or formatting it as ext4, as the
  way out), and so do the backup's settings, its Overview, `flux profile validate` and
  `flux check`. NTFS, which keeps hard links but not Linux permissions, gets a milder
  note. A file too big for the drive is named in the log as such, not "File too large".
- **Time left, wherever a backup or restore runs**: in the restore dialog (with the
  speed), beside a running backup in All backups, when pointing at it in the sidebar, and
  in the tray icon's tooltip ("21% · about 6 minutes left"). The estimate now follows the
  recent speed rather than the average since copying began, so a slow start no longer
  holds it back, and it's said to the minute.
- **Quiet hours** for notifications: *Preferences › Notifications › Quiet hours* (off at
  first, 10 PM to 8 AM when turned on) keeps a successful backup's notification and the
  weekly note out of those hours; the note comes after them. Failures and warnings are
  always sent. Scheduled backups follow it with the app closed (`[quiet_hours]` in
  `settings.toml`).
- **A warning when the backup drive is failing.** After each backup Flux asks the drive
  how it is (its SMART report, or an NVMe drive's warnings, through udisks, without a
  password). When it says it's failing, its spare space is running out, it's no longer
  reliable, or its bad sectors start growing, the Overview says so and a notification
  says it once. The space card says what was seen last, or that a drive which reports
  nothing (most USB sticks) can't be told about. `flux check` asks too.
- **Full**, the layout of 1.1 and before, stays one click away: *Preferences › App ›
  Layout*. Every install starts Compact, existing ones too, until Full is chosen there;
  the choice is kept on this computer, like the theme.
- **Safe to unplug.** A backup to a USB drive can end by unmounting the drive and
  powering it off, as the file manager's *Eject* does, with a notification that says
  it's safe to unplug: *Unmount the drive when done*, under *When* (in a profile file,
  `release_drive = true`). Encrypted drives are locked again first. Drives the desktop
  didn't mount (from `/etc/fstab`) are left mounted, since that would need an
  administrator's password; a drive that's in use stays mounted, and the notification
  says to eject it before unplugging. Off by default.
- **Delete several snapshots at once.** Ctrl-click snapshots in the list, Shift-click for
  a range, or choose *Select several…* in a snapshot's menu and click them; *Delete N…*
  removes them together. Pinned ones among them are kept, and a Vault says how much
  space was freed.
- **Sizes next to every folder.** The folders a backup takes and the folders it leaves out
  each show how much they hold, so it's clear what's worth leaving out. Counting has a few
  seconds; a folder too big for that shows "≥" and what was counted.
- **Skip what a project's .gitignore ignores.** In git working trees, a backup can leave
  out what their `.gitignore` files ignore (build output, `node_modules`, and also files
  git skips on purpose, like `.env`), read as git reads them: *Skip what .gitignore files
  ignore* under *Exclude Files/Directories*, or `exclude_gitignored = true`. Off by
  default; the backup's own rules come first (`!.env` keeps one); Mirror and Vault only.
  The rule tester names the `.gitignore` and line that left a file out, and an exact
  restore doesn't remove such files.
- **Browse in Files.** A snapshot's ⋯ menu opens it in the desktop's file manager,
  mounted read-only (at the backed-up folder when there's one), for copying files out
  with the tools you know. *Close*, another snapshot, or quitting Flux unmounts it.
  Not for system backups; needs FUSE, as `flux mount` does.
- **Previous versions from the file manager.** Right-click a file or folder in Nautilus,
  Nemo or Dolphin: *Previous versions…* opens Flux on its versions in the backup that
  takes it; *Restore deleted files here…* opens the newest snapshot at that folder with
  *Only deleted* on. Flux starts if needed, or the running copy is handed the request.
  Installed by the `.deb` and `.rpm` packages (Nautilus needs nautilus-python); also
  `flux-backup --previous-versions PATH` and `--restore-deleted FOLDER`.
- **A recovery code.** An encrypted Vault can have a recovery code: a long random code,
  shown once to print or keep somewhere safe, that opens the backups when the password
  is lost (also without the unlock file). Setting up an encrypted backup ends with an
  offer to make one; it's also under *Security › Recovery code*, on the recovery
  checklist, and `flux key recovery`. It can be typed loosely (any case, with or without
  dashes, O for 0). A new one replaces the old; the format is unchanged (it's one more
  key slot).
- **A forgotten password, replaced.** After unlocking with the recovery code, Flux asks
  for a new password before going on: it replaces the lost one (with several, you choose
  which stop working), is kept where the old one was (keyring, key file), needs the
  unlock file if the old one did, and comes with a new recovery code in place of the one
  just used. Nothing is re-encrypted: the lost password's key slot is removed. The
  unlock dialog says the code can be pasted there.
- **A calm weekly note.** Once a week, when every backup is protected with nothing to
  look at, one quiet notification says so: how recent the copies are and when the last
  restore test passed. It's checked when a scheduled backup ends (the app needn't be
  open), never sent when something needs you, and first comes a week after the first
  check. On by default; *Preferences › Notifications* turns it off.

### Security
- **New encrypted Vaults hide file sizes by default.** Published 2025 research recovers
  the secret that keyed content-defined chunkers use to cut files into pieces, from
  files an attacker knows or planted ([ePrint 2025/558](https://eprint.iacr.org/2025/558),
  [arXiv 2504.02095](https://arxiv.org/abs/2504.02095)). Flux's Vault chunker is of that
  kind (FastCDC, its table XORed with a secret 64-bit seed), and whether the attacks
  reach it through its encrypted pack headers hasn't been assessed. Until it is,
  *Hide file sizes* (packs padded to a Padmé length, at most about 12% bigger, about 3%
  for a full pack) is on for Vaults set up from 1.2, in the app and with
  `flux profile create --engine vault`, which takes away the exact sizes such an attack
  would read. File contents were never at risk: they're encrypted either way. Existing
  Vaults keep the setting they had; turn it on under *How backups are stored ›
  Security*. THREAT_MODEL.md no longer says chunk boundaries can't reveal that a known
  file is in a Vault.

## [1.1.0] - 2026-11-10

A refinement release: what was there, made clearer, safer and quicker to use, and three
things that put numbers behind what a backup promises. **Flux Sentry** learns how much of
each backup normally changes and warns when one doesn't fit. A Vault can keep **repair
data**, so damage from a failing drive is undone, not only found. Restore tests can be
sized by **what a pass must prove** ("if 1% of the files were damaged, this would have
found it, 99% certain"). Restore tests also no longer write a single file anywhere, so an encrypted backup stays encrypted
while it's tested. Each backup's settings can be read at a glance, and every backup's side
by side. Statuses agree everywhere and only what isn't protected is red; the Overview is
calmer, and the words are plainer. The recovery checklist has a button for each thing it
asks, a scheduled backup that can't get its password says so before it fails, times are
typed the way you write them (on a 12- or 24-hour clock) and custom times are previewed,
closing the window leaves Flux in the tray, and every man page has examples.

### Added
- **Flux Sentry: unusual-change detection that learns each backup's normal.** "Far more
  changes than usual" used to mean fixed limits (30% of the files, 5 times the median).
  Flux Sentry learns instead how much of each backup normally changes, from its last 60
  backups: a level (an exponentially weighted moving average of the share of files
  changed, on a logarithmic scale, which one odd backup barely moves), a spread (the
  median absolute deviation around it) and a memory (an amount of change the backup has
  reached twice before, unflagged or answered with *This was me*, is normal for it). A
  backup is unusual when it lands 6 spreads above its level (4 when watching closely).
  A quiet backup now warns at 5% of its files instead of 30%; a busy one whose changes
  swing widely, or that is rebuilt every Monday, no longer cries wolf. The warning says
  "480 files changed or deleted, 4.8% of them: 40 times what this backup usually changes
  (0.12%)", and the *Unusual changes* cell on the Overview says what Sentry has learned
  and where it would warn. Fixed limits stand in until a backup has run five times.
  Setting up a backup now asks about it in a step of its own, *Should Flux Sentry keep
  watch?*: normally (recommended), closely, or off, each with what it means (off stops
  only the warnings; backups and snapshots are made just the same). A backup that's
  watched shows a green **Flux Sentry active** tag on its page, with Sentry's own icon,
  a shield with an eye.
- **Flux Sentry looks before anything is written.** For backups in Flux's own formats
  (plain folders and encrypted), the files are compared with the newest snapshot *before*
  the backup starts. If they show unusual changes, the backup isn't made at all: nothing
  of an attack reaches the backups, here or in any other place. The warning reads *Flux
  Sentry stopped the last backup*, the newest snapshot is pinned, and the backup is on
  hold as below. *This was me* lets the next backup take those changes without being
  stopped for them again. Backups written by rsync or Borg are looked at right after
  they're written, as before.
- **Restore everything, in one step.** Flux Sentry's warning has **Restore
  everything…**: a restore of all the backup covers, from the snapshot kept from before
  the changes, back to where it was. It shows what it will do first; files that differ
  now are kept aside, not deleted. When it finishes without errors the alarm is settled
  and backups go on. The first backup after a settled alarm isn't stopped again for the
  same changes or for what an attack left behind.
- **Tripwire files.** Turn on *Plant tripwire files* (in the Flux Sentry step of setting
  up a backup, or *What to keep*; `tripwires = true`; off by default) and Sentry plants a
  small hidden file, `.flux-sentry-tripwire.txt`, in each folder the backup covers and
  its first subfolders. Ransomware changes every file it finds: if a tripwire is changed
  or removed, the next backup is stopped before it starts, however little else has
  happened, for backups of every kind. They are set again when the alarm is settled
  and removed when the setting is turned off or the backup deleted; Flux never deletes
  one that isn't exactly as it planted it.
- **Flux Sentry locks a backup down.** When a backup shows unusual changes, the backup is
  put **on hold**: no further backup is made, by schedule or by hand, until you settle
  it, since another one could only store more of the damage; and that backup isn't
  passed on to the profile's **other places**, which keep their copies from before.
  Settle it with **This was me** (the changes were yours) or the new **It's dealt
  with** (they weren't, and you've restored what you need: backups go on, Sentry learns
  nothing from that backup, and the snapshot from before stays pinned). The backup's
  page shows *Flux Sentry · backups on hold* and its next backup as on hold; a
  notification repeats it once a day so backups never stop unnoticed. New command:
  `flux sentry <profile>` says what Sentry has learned and whether it holds the backup,
  and settles it (`--this-was-me`, `--dealt-with`); `flux backup` refuses while on hold.
- **Repair, not just detection.** A Vault can keep **repair data**: Reed–Solomon parity
  for everything it stores, so that parts a drive damages (a bad sector, a flipped bit, a
  stretch of garbage) are rebuilt instead of costing the files stored there. Turn it on
  in *Settings › Where to keep it › How backups are stored › Repair data* (2, 5, 10 or 20%
  more space; `[vault] parity = 5`). *Verify every byte* and the monthly verification
  then repair what they find and say so; `flux check --read-data` says what could be
  repaired and `flux check --repair` does it. With 5%, any 13 of each group of about 240
  4 KiB blocks can be lost, or one stretch of up to 5% of a stored file. Packs, index
  files, snapshot records, key slots and the Vault's `config` are all covered (the last
  two are repaired before the Vault is opened, without the password); every repair is
  checked against the same hashes and authentication as before, and only what fails its
  own check is ever repaired. Verifying also adds repair data to what was stored
  before, and the Overview says how much there is and what it covers. Copies in other
  places carry it. It adds a `parity/` folder to the Vault without changing the Vault's
  format: older versions of Flux and `flux-vault-recover` read the Vault as before
  (docs/VAULT_FORMAT.md, *Repair data*).
- **Restore tests with a stated confidence.** A new choice of files, *Enough to be sure*:
  say what a pass must prove (*catch damage to 1% of the files or more, 99% certain*) and
  Flux tests as many random files as that takes, 459 for those two, almost whatever the
  size of the backup (fewer for a small one: the count is exact, from the hypergeometric
  probability of missing every damaged file). The report says what passing proved: "if 1%
  or more of its 184,000 files were damaged, this test would have found it (99%
  certain)", on the Overview, in the log and from `flux restore-test`, which gains
  `--damage` and `--certainty`; in a profile, `mode = "confidence"` with `damage` and
  `certainty`. A test of so many random files says what it was worth too (ten files:
  damage to 37% or more). A test stopped at a limit claims nothing. Samples are now drawn
  with the system's randomness, every set of files equally likely.
- **Settings at a glance.** A backup's Overview sums up its settings in a compact grid
  (schedule, what's excluded, where, how it's stored, where the password is kept, what's
  kept, restore tests, notifications…), with anything to fix in amber; a click opens that
  setting's section of the Settings page (*When*, *What to keep*…) in a dialog right
  there, without leaving the Overview; it's grouped (*When and what*, *Where and how*, *Safety and alerts*) and leaves
  out what doesn't apply. *All backups* gains a **Settings** view: every backup's settings
  side by side in a sortable table, one line per backup, across the whole window (the
  headline above it shrinks to one line), with a choice of **Columns**.
- **A 24-hour clock**: Preferences › App › *Clock* (12-hour, 24-hour, or like the
  system), used wherever the app shows or takes a time.
- **Closing the window keeps Flux in the system tray**, where its icon shows backups as
  they run; quit from the tray's menu. The first time, a notification says where it went.
  Turn it off in Preferences › App. Where no tray shows the icon, closing quits as before.
- Starting Flux while it already runs brings its window back instead of a second copy.

### Changed
- **Flux Sentry has a dialog of its own.** Its settings sat in *What to keep*, among the
  retention rules. The *Flux Sentry active* tag on a backup's page (and *Flux Sentry off*,
  which is now shown too) and the *Unusual changes* cell of *Settings at a glance* open
  one small dialog instead: what Sentry has learned of this backup, how closely it
  watches (normally, closely, off), and the tripwire files. They are still in *What to
  keep* as well.
- *This was me* is remembered for every backup it was said of (before, only the latest):
  Flux Sentry learns from those, and from no other flagged backup.
- **Statuses agree, and only the serious is red.** The sidebar and *All backups* count the
  same two things: backups that aren't protected, and protected ones with something worth
  a look ("2 not protected · 3 to look at"); they used to give different numbers. Red ⚠ is
  only for what isn't protected; the rest get a quiet amber dot and say what it is (*Low
  on space*, *Not tested lately*, *A copy failed*) instead of "Check". News in a toast
  ("Timer updated") has an "i", not a warning sign, and toasts no longer cover an
  editor's Save button.
- **What each check proves.** *Maintenance* is *Checks and upkeep*: *Is every snapshot
  whole?* (Check), *Can files come back?* (a restore test) and *Is all the stored data
  undamaged?* (Verify every byte), quickest first.
- Warnings, errors and notes share one look everywhere, with their actions under them.
- System backups no longer sit locked at the foot of the sidebar: set one up with *Back up
  the whole computer* on the New backup page, and open the ones you have with *System
  backups…* in the + menu; once unlocked, they're listed in the sidebar.
- Toggles you switch on and off (a webhook's events, the tag filters in Snapshots) are
  plainly on: green, outlined and ticked; before, on and off looked almost the same.
- **A calmer Overview.** Problems sit right under the one-sentence answer; only the
  answer, the history, the settings at a glance and space are raised as cards, while
  places, recent backups and the restore test are flat lists, and the checklist and
  checks stay folded.
- **The recovery checklist does what it asks.** Each line of *Could you recover it on
  another computer?* has a button beside it: *Back up now*, *Keep it elsewhere…* (shows or
  copies the password from where Flux keeps it, saves a copy of the unlock file), *Export
  the key…* (Borg), *Test a restore now*, *Add a place…*. A meter beside its heading
  shows how many are ready, and what's left is marked as a thing to do, not a failure.
- **Times are easy to type.** A backup's time (and the upload limits by time of day)
  takes `9`, `930`, `21:30`, `9:30 pm`, `noon`…; ↑ and ↓ move it by 15 minutes (Shift: an
  hour), and a clock button lists times to pick from. It replaces the system's time field,
  which had to be filled in part by part. *Custom times* shows the next three runs of
  the calendar expression as you type it.
- A backup's schedule reads "Every day at 8:00 PM" or "Every 4 hours" in its header, not
  `*-*-* 20:00:00` or "every 4h"; "in 60 minutes" is "in 1 hour".
- Plain words first: places are tagged *Plain folders*, *Encrypted*, *rsync folders*,
  *Borg repository* rather than engine names; a backup's settings say which profile it is
  on the command line (`flux backup docs`); validation messages no longer quote config
  syntax (the command line still names the setting).
- **The man pages say more, with examples.** Each of the 74 pages describes what its
  command does and changes, shows examples of basic use, its exit status and related
  pages; `flux(1)` adds the environment variables and files. `flux <command> --help` ends
  with the same examples. Notes such as the archive kinds are no longer run together into
  one line.
- *Leave some things out* in a backup's settings is now **Exclude Files/Directories**.

- **Faster, and lighter on memory, where a performance check found it slow** (after
  1.1; each of these was found by reading the code at the scale of millions of files,
  thousands of snapshots and backups on a server, and is listed with what it was):
  - **Flux Sentry's look before a backup no longer holds the file list four times
    over.** It kept the whole previous snapshot, a whole scan, and a copy of each in a
    table: about 4 GB for 3 million files, before every backup. The two lists are now
    compared as they stream by, and only what differs is kept. The look after an rsync
    or Borg backup works the same way.
  - **…and no longer reads an encrypted backup's file list from the backup itself.**
    That is one read per folder: tens of seconds on a local drive, hours on a server
    (an estimated 5 to 7 hours for 2–3 million files), before each backup. It now uses
    the file list the last backup left in this computer's cache, as the backup itself
    does.

  - **Encrypted backups on a server: restores, mounts and browsing fetch packs, not
    pieces.** Every piece of every file, and every folder's list, was its own
    open-read-close on the server: three round trips each. Restoring 2 million small
    files came to an estimated 12 hours of waiting, and a snapshot of that size took
    about 5 hours to list before a restore, a mount or a prune could begin (prune
    walked every snapshot that way, holding the lock). The second piece wanted from a
    pack now brings the whole pack, once, and the rest come from memory; a single file
    is still fetched alone.
  - **Getting a few files back from an encrypted backup reads only the folders on the
    way to them**, not the whole snapshot's folders (twice over, to open one file from
    the app; four times to compare one).
  - **Snapshot records, index files and pack headers are read in batches.** Over SFTP,
    a batch of files is opened in one round trip and read and closed in a second,
    instead of three or four round trips for each. Listing 3,000 snapshots took about a
    minute, several times in every backup; the quick check of a 2 TB backup spent about
    an hour and a half on pack headers (two reads each, now one).
  - **Restored files are made durable once, at the end**, for each drive they went to.
    Restores from an encrypted backup or from a server synced every file as it was
    written: on a spinning or USB disk, hours for a million files.
  - **A backup to an encrypted place lets go of the previous snapshot's file list** as
    soon as it has decided what to reuse, instead of holding it (a second full list,
    with every file's pieces) until the backup ends.
  - `flux-vault-recover verify` reads each folder's list once, however many snapshots
    share it. With thousands of snapshots it re-read them all for each: hours before
    any data was checked.

  - **Plain-folder and rsync backups on a server list their snapshots in a second, not
    a minute.** Each snapshot's notes were read one after another, four round trips
    apiece: about 44 seconds for a year of daily snapshots, several times in every
    backup, on every visit to the Overview, and again after each snapshot a prune
    removed. They are read in batches now.
  - **Restoring small files from a server fetches them in batches.** One file at a
    time, a restore waited about a round trip per file whatever the line's speed: some
    8 hours for a million files. Files up to 256 KiB now come 32 at a time.
  - **A mounted snapshot on a server reads ahead.** Every read of a file through the
    mount opened and closed it on the server: three round trips for 128 KiB, about
    1.4 MB/s on any line. A file read through is now fetched 4 MiB at a time (a read
    from somewhere in the middle still gets just what it asked for). Restore tests of
    such backups read the same way.
  - **`flux check` of an rsync backup no longer keeps every snapshot's list in
    memory.** It held them all until the end, only to count files: about half a
    gigabyte per snapshot of 2 million files, so 15 GB for a month of dailies.
  - **`flux check --read-data` reads each stored file once.** An unchanged file is one
    and the same file in every snapshot of a plain-folder or rsync backup, but it was
    read again for each: a 1 TB backup with 400 snapshots meant reading about 400 TB.

  - **Searching all snapshots, and a file's history, no longer load whole file lists.**
    For every kind of backup but plain folders on a local drive, each snapshot's list
    was read into memory in full, one per processor core at a time: an estimated 8 GB
    for snapshots of 2 million files on 16 cores, inside the app. The lists now stream
    from their index files, and an index is written as the list comes, not after it was
    all held.
  - **Borg, in a mount:** opening a file runs `borg extract` for the whole of it, which
    reads through the archive first; file managers and viewers open the same file
    several times. The last 16 extracted files (up to 2 GiB) are kept for the next
    open.
  - **Borg, after this computer's notes are gone** (a new machine, a cleared cache):
    the statistics of every archive were fetched with one `borg info` each before a
    backup could start. One `borg info` now asks about all of them.
  - **A Borg restore gives back the room it borrows as it goes.** Borg extracts into a
    staging folder and the files are then copied into place, so a restore needed twice
    the room of what it restored. Each staged file is now removed once it's copied.

  - **The file browser no longer chokes on big folders.** A folder was sent to the page
    whole and drawn whole: a few megabytes and a few hundred thousand page elements for
    10,000 items (a second or more with the window frozen), and likely the end of the
    page for 100,000. It was fetched again each time the window came back to the front
    and re-sorted with every letter typed in the filter. A folder now comes filtered
    and in order, at most 2,000 items at a time, with a line saying how many there are
    and that the filter finds the rest. *Only deleted* and *Restore N deleted items*
    still count and cover the whole folder.
  - **The tray icon is set when a backup starts and ends**, not five times a second for
    as long as one runs (each time wrote a new image file and had the panel load it).
  - **A long snapshot list draws without stalling.** Every date on the page built a new
    formatter; with a couple of thousand snapshots that was most of a second each time
    the list was drawn, which is whenever any backup finishes. Formatters are made once.
  - **The app lets go of the snapshots you browsed** ten minutes after the last look.
    It kept the file lists of the last four in memory for as long as it ran: around a
    gigabyte after looking into four snapshots of a million files, in the tray for weeks.

  - **Flux Sentry no longer costs a second pass over your files.** Its look before a
    backup walked every folder, and the backup then walked them all again; with changes
    detected by checksum, every file was *read* twice. The look now makes the backup's
    own scan and hands it on, so the folders are walked (and the files read) once.
    Measured on 60,000 files (1.1 GB, in memory, 16 cores), a backup after one edited
    file: 0.20 s before, 0.15 s now, 0.15 s with Flux Sentry off; with checksums
    0.66 s before, 0.40 s now, 0.35–0.40 s with it off. What Sentry adds is now the
    comparing of two lists. (On a real disk the walk and the reading cost far more
    than here, and it was all of that, twice.)

### Fixed
- The demo (the app in a browser) showed an empty Overview late in the evening during
  the week after the clocks go back: its 30-day history named that 25-hour day twice.
- **A scheduled backup that can't get its password says so before it fails.** With the
  password set to *Ask every time*, nobody is there to type it when a backup starts on its
  own, so those backups failed. The app now warns next to *When* and under *How backups
  are stored*, with **Keep it in the keyring** (a key file for system backups): type the
  password once, Flux checks it opens the backups and keeps it. `flux profile validate`
  warns too. The old hint said such backups "wait for you"; they fail.
- A password typed to set up a new key file is written only after it has opened (or
  created) the backups, so a mistyped one is never kept.

- **Keeping free space no longer removes snapshots it shouldn't.** A bug check after
  1.1 found that *Keep this much free* (`min_free_space`, `min_free_inodes_percent`) could
  take far more than it should:
  - It ignored the safety floor: on a drive where deleting couldn't reach the minimum,
    every snapshot but the newest, pinned and named ones went, whatever `safety_floor`
    said. It now never takes the newest `safety_floor` complete snapshots or the
    `keep_last` newest.
  - A Mirror on a server removed old snapshots to make room for a backup even with
    retention switched off (`enabled = false`). It no longer does, as on a local drive.
  - With Flux Sentry on, the snapshot before the newest is the one to go back to after an
    attack. A backup written by rsync or Borg that filled the drive could remove it for
    space before Sentry had looked, and then no warning was raised. It stays now.
  - A Borg backup below its minimum deleted every archive it was allowed to: Borg frees
    nothing until `borg compact` has run, and Flux only ran it at the end. It now compacts
    after each archive it removes for space and stops as soon as there is enough.
  - **Free up space** in the app removed snapshots for space without asking, then said
    *Nothing to remove*. It now asks first, and says how many went and why.
    `flux prune --dry-run` says when older snapshots would also go for space.
- **A `[retention]` table with no keep rule no longer means "keep only the newest".** A
  hand-written table with just `keep_history_days` or `min_free_space` turned every
  `keep_` rule to 0, without a word. Such a table now keeps the default rules, the same
  as no table at all. A table that sets any `keep_` rule means what it says, as before.
  Profiles written by Flux always had every rule and are read as they were.

- **A prune cut short can no longer damage a Vault later.** A prune that was interrupted
  after writing its new index, before removing every old index file (a dropped
  connection, a power cut), left one behind. It was ignored until the *next* prune, and
  then counted again, naming stored data that was gone: later backups could skip data as
  "already stored" and make snapshots that couldn't be restored. A prune now takes such
  leftovers along. A Vault this has already happened to shows it in `flux check`
  ("pack … is missing").
- **An rsync backup continued after an interruption is never called unchanged.** If only
  the contents of files had changed, the run that continued an interrupted one sent
  nothing new, was taken for "nothing changed", and the files already brought over were
  deleted: no snapshot held the changes until the run after. It now makes the snapshot.
- **A folder under `/mnt`, `/media` or `/run/media` that isn't on a drive of its own is
  backed up.** Any such folder on the system disk (`/mnt/data` on a server, a folder on a
  drive mounted at `/mnt` itself, a link to a folder elsewhere) was taken for an
  unplugged drive and skipped, every time, with the backup still reported as fine. Only
  an *empty* folder there (what an unplugged drive leaves), or one that's gone, counts as
  a drive that isn't connected, as the guide always said.
- **Copies of plain-folder and rsync backups are written out before they count.** A copy
  to another place (*Copy the main backup*) became a snapshot there before its files
  were safely on the drive, so a drive unplugged right after could hold a snapshot with
  an empty `meta.json` or manifest; later runs took it for present and never repaired
  it. Copies are now synced to the drive first, and a copy that lacks those files is
  made again.

- **With the main backup out of reach, Flux Sentry looks before the other places are
  written.** When the main backup's drive was away (or its server down), Sentry had no
  snapshot to compare the files with, the main backup failed, and the other places were
  backed up without any look: an attack's files went straight to the copy kept
  elsewhere. Each other place is now compared with its own newest snapshot first (right
  after writing, for rsync and Borg); unusual changes stop it and every place after it,
  and put the backup on hold.
- **A restore into another folder no longer lifts Flux Sentry's hold.** *Restore
  everything…* settled the alarm after any restore that finished without errors, also one
  into another folder, where the files on the computer are still as the attack left
  them. Only a restore back to where it was settles it now; after one into another
  folder the app says backups stay on hold.
- **The restore dialog says what will happen to files that differ.** With *Keep my files
  when restoring over them* off for a backup (also what a Back In Time import sets when
  it was off there), the dialog still said differing files are "kept, renamed" and
  showed a plain *Restore* button, then replaced them. It now goes by what the restore
  will do: the text says they're replaced and why, the button is the red *Restore and
  replace N files*, and it waits for the preview. `flux restore` asks the same honest
  question.
- **A system backup's other places open in the app.** Their ids lost the `system:` mark
  on the way to the page, so browsing or restoring from a system backup's second place
  failed with "profile not found", or, with a backup of your own of the same name and
  place, showed and acted on that one instead.
- **A hook that leaves something running no longer hangs the backup.** A *before* hook
  such as `ssh -f -N -L …` or `some-tool &` finishes at once, but what it started keeps
  the hook's output open; Flux waited for that output to end, past the hook timeout, with
  the backup's lock held, so scheduled backups stopped. A hook is now done when its
  command is (its output is waited for two seconds more at most).
- **`flux-vault-recover` no longer refuses everything over one damaged snapshot record.**
  One unreadable record made `snapshots`, `files`, `restore` and `verify` fail for every
  snapshot. The record is now said and left out, the others list and restore, and
  `snapshots` and `verify` exit with 3.
- **Importing from Back In Time keeps its one snapshot a year.** Back In Time's smart
  remove always keeps one snapshot per year, for every year. The imported backup had no
  yearly rule, so everything older than the monthly ones would have been removed. It now
  gets `keep_yearly = 99`.
- A plain-folder backup with a second plain-folder place showed more than 100% progress
  for the second one (and stopped with an error in development builds). An error inside
  a running backup that should never happen now ends the run instead of leaving it
  waiting for ever.

- **Flux Sentry, nine smaller faults** (a second bug check, of Sentry alone):
  - *rsync and Borg:* after an alarm was dealt with and the files restored, the next,
    clean backup was compared with the damaged snapshot, flagged as a second attack,
    and the damaged snapshot pinned as "the one from before". It is now compared with
    the newest snapshot that isn't itself marked unusual.
  - A folder left out of a backup because its drive is away (plain folders on a server,
    rsync, Borg) looked like a mass deletion and put the backup on hold. It isn't
    counted as deleted.
  - Sentry learned from the engine's statistics, which count a changed permission or
    owner as a change, while its look counts only contents: two `chmod -R` runs made any
    later amount of change look normal. It now records what its own look counted and
    learns from that. This also gives Borg backups their deletions, which Borg's
    statistics never have, so a regular clear-out is learned after *This was me*.
  - It read "the last 60 runs", not the last 60 that made a snapshot: sixty failed or
    unchanged runs (a drive away for a few days of an hourly backup) sent it back to the
    fixed limits.
  - With Flux Sentry turned off, an alarm from before still showed *Backups are on hold*
    on the Overview and in `flux sentry --json`, and held the backup again when Sentry
    was turned back on, however many backups had been made since. An alarm holds a
    backup only until a backup is made after it.
  - Tripwires on a drive that was away when an alarm was settled, or when the setting
    was turned off, were forgotten: never watched again, and left behind. They are
    remembered until the drive is back.
  - Of two backups of the same folder, only the first got tripwires. They share them
    now: either can set them again without stopping the other, and neither removes one
    the other still watches. A tripwire put back by a restore of an older snapshot is
    no longer taken for a changed one.
  - Tripwires in a folder taken out of a backup stayed there, and stayed counted. They
    are removed at the next backup.
  - `flux profile delete` left the tripwire files in your folders (the app's delete
    removed them). Both now remove them and close an alarm still open, so a backup made
    later under the same name doesn't start out on hold.

- **Flux Sentry, nine small faults** (the same check):
  - The reminder that a backup is on hold, meant for once a day, came with every
    scheduled attempt until an alarm had been settled once.
  - *It's dealt with* promises that nothing is learned, but the backup made right after
    it (which isn't looked at) was. It isn't any more.
  - *This was me* said the snapshot that was kept "is no longer pinned" when it still
    was: with the backups out of reach, or when that snapshot had a note of yours. The
    pin Flux put on is now taken off (your note stays), and when it can't be, that is
    what's said.
  - A snapshot that couldn't be pinned (the drive read-only, a lock held) was reported
    as "kept for sure (pinned)". The log, the notification and the Overview now say it
    couldn't be, and to pin it yourself.
  - The warning said the snapshot from before "is kept (pinned)" when there was none
    (a first backup, or the backups out of reach).
  - *Files involved* of a stopped backup read "in snapshot before:2026-…" (the alarm's
    name), and files without an extension were listed as ".(none)".
  - "It would warn at about…" could name a share that never warns: 100% for a backup
    that changes much and widely, 5% for one with fewer files than a warning needs
    changed. It now says such a backup is warned about by the marks of ransomware only.
  - The *looks encrypted* sign couldn't see files under about 400 bytes (a short sample
    never reaches the entropy a long one does), and they counted against it. The bar
    now follows the length of what was read.
  - A system backup with tripwire files turned on said they were planted, or would be
    by the next backup. They aren't planted for system backups, and it says so.

- Started while a copy is already running (hidden in the tray, say), the app asks that
  copy to show its window and ends: it now says so in the terminal instead of ending
  without a word.

### Security
- **A forged snapshot can no longer make a restore write outside its destination.**
  Found by the bug check after 1.1, and the most serious thing it found. Someone able to
  write to an unencrypted backup (a plain-folder one on a shared drive or server) could
  list a symbolic link in its manifest and then files "below" it. A restore made the link
  and wrote those files to wherever it pointed: for a system backup restored as root,
  anywhere. Entries listed below a link are now skipped and reported. A link planted
  among a backup's stored files is no longer followed when they are read either (it
  could make a restore as root copy a file only root may read into the restored folder).
  The threat model said both were so; now they are.
- **A system backup saved as an archive is readable by root only.** Exporting a snapshot
  of a system backup wrote the archive with ordinary permissions, so `/etc/shadow` and
  root's keys in it could be read by every user who could reach the folder. The archive
  and its checksum file are also made new now, never written through a link someone
  left there under their name, and the staged files are read back without following
  links. Your own exports keep the usual permissions. (To use a system backup's archive
  as yourself, restore to a folder instead, or copy it with `sudo`.)
- **The recovery guide's "restore everything" no longer changes `/`.** For rsync
  backups, `FLUX-RECOVERY.txt` said `rsync … latest/data/ /` and to put `sudo` in front.
  rsync then gave `/` the permissions and owner of the backup's private data folder,
  locking every other account and service out of the system. The guide now names the
  backup's top folders instead. Guides already on a drive are rewritten by the next
  backup; if you ran the old command, `sudo chmod 755 / && sudo chown root:root /` puts
  it right.
- **Settings read from a drive can't put commands into scripts.** A profile's name and
  exclusions went as they were into the recovery script, the exported rsync, Borg and
  restic scripts and the borgmatic settings. With a line break in them (in a
  `flux-profile.toml` on a drive anyone can write to, or an imported profile), the rest
  became a command in the script. Control characters in them are now replaced, a
  here-document's end marker is never a line of its contents, and a name with a line
  break doesn't validate. The restic script's repository path is quoted (a folder with a
  space in its name used to break it).
- Repair data is computed from what's stored, after encryption, and says nothing the
  stored files don't. A key slot's repair data is removed with its password. With a Flux
  older than 1.1 on the same Vault: remove passwords with 1.1 or later (an older one
  leaves the removed slot's repair data until space is next freed by a newer one).
- **Restore tests leave nothing behind.** They used to restore their sample into a
  temporary folder, so an encrypted backup's files sat there decrypted while it ran (and
  after a crash). Now each file is read back as a restore reads it and checked as it
  streams, a piece at a time, each piece wiped once checked: nothing is written anywhere,
  and memory use is the same for ten files as for every file. Borg's files stream from
  `borg extract --stdout`. A folder an earlier test left behind is removed. Restore tests
  no longer check permissions and times, which needed the files restored.
- `flux mount` of a Vault wipes each decrypted piece of a file from memory once it's done
  with it (it shares the reader restore tests use).
- The recovery checklist's new actions (show or copy the password, export the Borg key,
  copy the unlock file) are for your own backups only: a system backup's password and
  unlock file stay readable by root alone.

## [1.0.0] - 2026-11-09

The 1.0 release. Your backups can be recovered anywhere: on a new computer from the
welcome screen, without Flux through a small program kept next to each Vault, or as
standard archives; the formats they're in are written down and frozen for 1.x, and every
version since 0.1 is read. A backup can go to several places, test its own restores on a
schedule, warn of ransomware-like changes, tell your phone or a chat how it went, and
start from a template. Vaults gain stronger key settings and keep sparse files sparse.
The new depth stays out of the way: the main screens still say what's protected, where,
and how to get it back.

### Added
- **About Flux Backup**, in the new main menu (☰ in the title bar) and at the foot of
  Preferences: the version, links to the user guide, what's new, bug reports and the
  website, and *Technical details* with the versions of the `flux` command and the
  administrator helper, the system and WebKitGTK. If the `flux` command or the helper is
  a different version from the app (after a partial upgrade), it says so. *Copy version
  info* copies it all for a bug report.
- The version is shown at the foot of Preferences.
- **Sparse files keep their holes in a Vault** (disk images, some databases): only their
  data is read and stored, and a restore makes them sparse again, as Mirror already did.
  This is Vault format 2. New Vaults use it; a Vault made before 1.0 keeps format 1 (so
  computers still on 0.3 can keep using it) until `flux vault upgrade <profile>`, after
  which Flux before 1.0 can't open it. Snapshots already stored are untouched.
- `flux unlock <profile>` removes locks left by a Vault job that died, without a
  password; `--force` removes them all.
- **Security settings for encrypted Vaults** (a *Security* row in the backup's settings):
  - *Password hashing*: Standard (64 MiB, 3 passes, as before), Strong (256 MiB, 4),
    Maximum (1 GiB, 6) or custom, with how long unlocking takes on this computer.
    *Apply to the password* rehashes the password in use at once, without rewriting
    data. `flux key strengthen <profile> [--level …]`.
  - *Unlock file*: the password then opens the Vault only together with a small file
    (on a USB stick, say). Backups say where they expect it when it's missing.
    `flux key unlock-file <profile> create|use|remove`.
  - *Hide file sizes*: pads what's stored so someone with the drive can't tell the size
    of a file you changed (up to a few percent more space). Any Flux reads padded Vaults.
  - *Lock it again in the app* after 5 minutes to an hour unused.
- `flux key list` shows each password's hashing and whether it needs the unlock file.
- **Folders on an external drive are recognised.** When a backup's folder is on a drive
  of its own (`/mnt/photos`, `/run/media/…`), Flux records the drive's filesystem UUID
  when the backup is saved (or at its next backup), and checks it's there before each
  backup. An unplugged drive is never read as deleted files. Per folder, choose what
  happens without it: **back up the rest** (the default: Mirror and Vault keep its files
  as they were in the last snapshot, rsync and Borg leave it out of that one), **wait for
  it** (up to an hour, or `wait_for`), or **fail the backup**. A folder under
  `/run/media`, `/media` or `/mnt` that's missing or is an empty folder on the system disk
  counts as an unconnected drive too.
- Overview and All backups say when a folder's drive missed the last backups ("The drive
  with photos wasn't connected for the last 3 backups"); runs record which folders were
  missing (`sources_missing` in `flux log --json`, and in the snapshot's details).

### Changed
- A calmer app: every screen was gone over so it says what you need and keeps the rest a
  click away.
  - **Overview**: the three latest backups; space, the forecast chart (folded unless the
    drive fills within two months) and storage in one card; *Could you recover it on
    another computer?* and *Maintenance* folded at the bottom; a failed place shows its
    error on hover instead of under it.
  - **All backups** shows *Last good copy* only when it differs from the last backup.
  - **Settings**: *Where to keep it* shows the chosen drive, with *Change where it's
    kept…* for the others. *Keep and remove snapshots* offers *Recommended*, *Save
    space* and *Keep longer*, with the numbers under *Custom*, and folds the list of
    which snapshots would go behind its count.
  - **Preferences**: theme and starting share one group; default exclusions, running
    one at a time, the scheduler and the comparison program fold under *Advanced*,
    which opens by itself when one of them was changed. The scheduler's pros and cons
    are behind *What's the difference?*
  - **Snapshots**: an ordinary file-size limit is a one-line note, not a banner.
  - **New backup**: a shorter Back In Time offer, one line per profile.
  - Shorter texts throughout.
- **The website** is redesigned for 1.0 ("Linux backups, done right."), dark and precise:
  the app rises out of the page
  in a brushed-metal frame and settles flat as you scroll, a spec sheet of measured
  facts counts up, and chapters show restoring, the ransomware watch, a new computer and
  backups that run on their own, with a mosaic of details, a speed race, the four
  formats beside a terminal, and what 1.0 adds. The user guide shares the look, and both
  work on phones (the light/dark switch is gone: the site is dark). Fonts are served from
  the site itself, with no third-party requests.
- **Dependencies brought up to date** for 1.0: Rust 1.99 is the minimum (the current
  stable); `sha2` 0.11, `similar` 3, `ruzstd` 0.9 and `zbus` 6 (whose blocking API is
  gone, so the logind sleep inhibitor now runs its call with `zbus::block_on`); every
  other crate at its newest compatible release; TypeScript 6, Vite 8.3, Tailwind 4.3.5;
  the GitHub Actions at their current majors (checkout and setup-node v7, the Pages
  actions v5/v6). TypeScript 7 waits until svelte-check supports it.
- Building the app needs **Node.js 26**, the current LTS (Node.js 20 is past its end of
  life). CI and the release builds use it, and `app/package.json` and `app/.nvmrc` say so.
- The title bar's **?** button became GNOME's main menu: Preferences, Keyboard
  shortcuts, User guide and About. The **?** key still lists the shortcuts.
- *Copy diagnostics* and `flux diagnostics` include the versions of the `flux` command
  and the helper.
- A Vault used both on a local disk and over SSH (a NAS used from two computers) now has
  one lock: the local side writes the same lock files SFTP clients do, and respects
  theirs. Before, the two sides couldn't see each other's jobs.
- **Several places for one backup.** *Also keep copies elsewhere* in a backup's settings
  adds more places (a USB stick, a server), each stored its own way (an encrypted Vault on
  the stick, plain folders on the drive). Every backup goes to all of them, one after the
  other, each read from your files (independent copies); one failing doesn't stop the
  others, and says so in words. A place can be marked as kept somewhere else (offsite).
  The Overview lists the places with how each did; the Snapshots tab switches between
  them. On the command line: `flux profile destination add|remove|list`, and each place
  is the profile id with `+` and its name (`flux snapshots home+usb`, `flux backup
  home+usb` for just that one). `flux backup` exits with 4 when a place failed.
- **The formats, written down and frozen for 1.x** (`docs/FORMATS.md`): the Vault, the
  Mirror repository and manifest, snapshot `meta.json`, rsync and Borg layouts, profiles
  (schema 1), the settings kept with the backups and the recovery kit. Backups and
  profiles made by Flux 0.1, 0.2 and 0.3 themselves are checked in and read by every
  build: listed, restored file for file, and backed up into again.
- *Upgrade the Vault* on the Overview for a Vault from before 1.0 (format 1), with what it
  means for computers still on an older Flux.
- **Storage** on the Overview: what the newest snapshot holds, what all snapshots would
  take as full copies, what they take, and what deduplication and compression save
  (Vault from its index, Borg from `borg info`, Mirror and rsync estimated from what each
  snapshot copied).
- **Where the time went**, in a run's details in Activity (folded): each phase's time,
  the rate while copying, and for Vaults the time spent reading, chunking, finding what's
  stored, compressing and encrypting, and writing, with the storage's average write time.
  Kept in the run log (`performance`).
- **Calendar** on Snapshots: a month view above the list, days with snapshots marked (and
  days a backup failed); click a day to list only its snapshots.
- **Filters** on All backups: Missed, Failed, Unverified, Offsite, Low space, each with
  how many backups it matches. Health notices carry a `kind` (`overdue`, `failing`,
  `low_space`…) for them.
- **Templates**: a new backup starts from *Home folder*, *Documents* (hourly), *Photos*
  (kept for years), *Development* (hourly, without dependencies and build output),
  *System settings* or nothing, each explained in a line. Save a backup's settings as your
  own template from its ⋯ menu (without where it goes or its webhooks).
  `flux profile templates`, `flux profile create --template`, `flux profile
  save-template`, `flux profile delete-template`.
- **Check the rules** (*Settings › Exclusions*, with the rules as edited, before saving):
  *Test a path* says whether it's backed up and what decides it (the pattern, a folder
  it's in, a `!pattern`, the size limit, a cache or marker file, another disk, the
  backup's own storage); *Browse what's backed up…* marks every item of your folders,
  with how much is in and left out below each folder. `flux explain <profile> <paths…>
  [--list]`.
- **Export** files out of a snapshot as an archive (a snapshot's ⋯ › *Export…*, or a file,
  folder or selection in the file list; `flux export <profile> <archive> [paths…]`):
  `.tar.zst` keeps everything (owners, permissions, nanosecond times, symbolic and hard
  links, device files, extended attributes and ACLs as pax records), `.zip` is for sharing
  and lists what it left out. Optionally with a checksums file next to it, SHA-256 for
  `sha256sum -c` or BLAKE3 for `b3sum -c`. Works for every kind of backup.
- **`flux-vault-recover`**: a small, separate program that reads Vaults without Flux:
  lists snapshots and files, restores (into an empty folder, keeping permissions, times,
  symlinks and sparse files) and verifies every stored piece. No settings, read-only, a
  single static file on 64-bit Linux, written independently of Flux's Vault engine and
  tested against it. Flux keeps it next to Vaults on drives with a note on using it
  (*Keep the recovery tool on the drive*, on by default); it's installed in
  `/usr/libexec/flux-backup/` and attached to releases on its own.
- **Recover an existing backup** on a new computer (or after reinstalling): on the welcome
  screen and in **+** › *Recover a backup…*. Pick a drive, folder or server; Flux lists
  the backups there (Vault, Mirror, rsync, Borg) with the computer and user that made
  them, an encrypted Vault's once its password is typed. Then *Get files back* (nothing
  scheduled) or *Carry on backing up from this computer*, adding to the same backups.
  `flux recover <where> [--pick N] [--carry-on] [--id …]`.
- After each backup, a copy of its settings (no passwords, hooks or webhooks) is kept
  with it: encrypted in a Vault, `flux-profile.toml` next to Mirror, rsync and Borg
  backups. A recovered backup records whose backups they are (`[origin]` in the profile),
  so they're found and continued under the old computer's name.
- **Could you recover it on another computer?**, at the bottom of a backup's Overview: a
  checklist (settings kept with the backups, a restore test passed, a copy kept
  elsewhere, the password kept somewhere other than this computer, the Borg key exported).
- **Webhooks**: tell your phone (ntfy, Gotify), a chat (Slack-style, Discord) or a
  monitoring service (healthchecks.io-style start/success/fail pings) how backups go, or
  POST a documented JSON body anywhere. For every backup (*Preferences › Notifications*)
  or one (*Settings › More › Notify elsewhere*); events: failed, finished with problems,
  unusual changes, restore test failed (by default), little space left, skipped,
  succeeded, started. Extra headers for tokens; *Send a test*. Addresses and headers stay
  out of exports and diagnostics, and `settings.toml` is readable only by you when it has
  webhooks. A webhook that can't be reached is a warning, never a failed backup.
- **Unusual-change detection**: after each backup, Flux compares the changes with this
  backup's usual ones and looks for ransomware's marks (many more files changed than
  usual, files renamed to a new extension, text that now looks encrypted, ransom notes).
  When a backup stands out, the snapshot before it is pinned, pruning waits, you're
  notified, and the Overview shows the files involved with *Browse the snapshot before*
  and *This was me*. Sensitivity: normally (default), closely, or off.
- **Restore tests**: restore a sample of the newest snapshot into a temporary folder and
  check every file against the backup's own record (*Test a restore now* on the Overview,
  `flux restore-test`), and on a schedule: after every backup, weekly or monthly. Some
  random files, a share of the backup, every file once a month (rolling), or everything;
  limits on time and data; an optional comparison with your files now. Failed tests are
  notified and shown until one passes; overdue ones are a warning. After Duplicati's
  restore test policies.
- **Network rules for backups to a server** (*Settings › More › Network*): scheduled
  backups run only on chosen Wi-Fi networks, or never on some (a phone's hotspot); and
  upload limits by time of day (2 MB/s during working hours, full speed overnight), which
  Vault follows as it runs. `[network]` in the profile.
- **Retention safety.** *Always keep at least* N complete snapshots (1 by default),
  whatever the rules say; and *after a backup with problems* (failed, files left out, a
  source's drive missing, a place unreachable) the next backups remove nothing until one
  runs cleanly (on by default). Activity history can be kept for 30 days, 3 months or a
  year, separately from snapshots (`keep_history_days`, `keep_history_runs`).
- **Copy the first backup** to another place instead of reading your files again
  (*How the copy is made* when adding a place; `--replicate`): the copy holds the same
  snapshots, stored the same way (a Vault copy is the same Vault, with the same password),
  only new snapshots are copied, unchanged files stay shared between snapshots in the copy,
  and snapshots removed from the first place go from the copy too. Vaults copy anywhere,
  plain-folder backups between drives on this computer; Borg can't be copied.
- *Note and tags* on snapshots, separate from names and pins: they help find a snapshot
  and never change what retention keeps. In Snapshots: *⋯ › Note and tags…*, the note
  under the snapshot's date, its tags beside it, a ✎ in the list, and the tags in use as
  buttons that show only those snapshots. On the command line: `flux note`, `flux tag`
  (`--remove`), `flux snapshots --tag`, and `flux info` shows both. Stored with the
  snapshot for every engine (Borg: in the archive's comment).
- All backups names the most serious problem first (a failed backup before a warning).
- "Another Flux job is using the Vault" now says who (user, computer, process, since
  when) and how to clear the lock if that job died. A lock left by a process on this
  computer that no longer runs is ignored at once instead of after 30 minutes.

- A snapshot's `meta.json` or a manifest written by a newer Flux is refused with a
  message saying so (before, a newer manifest was "not a Flux manifest"); a profile file
  from before 1.0 is kept as `<id>.toml.pre-1.0.bak` the first time Flux rewrites it, and
  profile files now say their schema (`# schema 1, written by Flux Backup …`).
- Importing a Back In Time profile: an exclusion like `/home/me/**/build` now leaves out
  only what Back In Time did (`build` at least one folder down); it also left out
  `/home/me/build`. Found by new property tests, which check on random folders and rules
  that Flux's exclusions and their rsync translation, and Back In Time's patterns and
  their Flux translation, leave out exactly the same files (against the real rsync).
- A Vault snapshot whose sparse-file holes are out of order (a damaged or forged tree)
  is now refused as damaged; restoring it would have written wrong data. Found by
  fuzzing: everything Flux reads that it didn't write (Vault packs, indexes, trees,
  records and keys, Mirror manifests, `meta.json`, profiles, and the requests the
  administrator helper takes) now has a fuzz target (`fuzz/`).
- Every `unwrap()` in the engines, the runner, the service and the administrator helper
  was reviewed: none was left outside tests (the lint `clippy::unwrap_used` now refuses
  new ones there); the `expect`s that remain say why they can't fail. Locks no longer
  panic again after another worker thread panicked; the work in flight carries on.

### Fixed
- A new backup without a destination yet said " is not an absolute path" under
  *Continue*; it now says to choose a drive or folder.
- In Snapshots, a file's *Restore* button was cut off at the panel's edge, and in a small
  window the file list could shrink to nothing. The table now fits (dropping *Modified*
  when narrow), and a small window scrolls the page instead of crushing the lists.
- While a backup ran, its overview and All backups said "Next running now"; they now
  just say it's backing up.
- Small wording fixes: "files over 4 GB" (was "4GB"), the restore-test and password
  hashing hints.

- **An exact restore (*Make the folders exactly as they were*, `--exact`) removed things
  the backup never covered.** Restoring the folder above the ones a backup covers (your
  home folder, when only `Documents` is backed up) removed everything else in it; it also
  emptied mounted drives the backup hadn't gone into, and removed files the backup had
  failed to read. Now only folders the backup covered are made exact: folders above them
  get back what the snapshot holds and nothing is removed there, a mount point the backup
  didn't go into is left as it is, and files listed among the snapshot's errors stay. If a
  snapshot doesn't list all its errors, or doesn't record what it covered (snapshots taken
  over from another tool), nothing is removed. The preview, the restore and
  `--dry-run` say when they hold back.
- **Pruning or deleting in a Vault could delete the data of a snapshot whose record
  couldn't be read at that moment** (a dropped SFTP connection, a flaky USB drive): the
  snapshot was skipped when working out what is still used, so its data looked unused.
  Now nothing is deleted unless every snapshot record was read, and `flux check` reports a
  record it can't read. A record that is damaged for good is deleted with
  `flux delete <profile> <full id> --force`.
- **A Vault backup that couldn't write part of its data (the drive filled up, the
  connection dropped) still made a snapshot**, listing one file as failed while the data
  of other files, lost with the same write, was missing too. Such a backup now fails as a
  whole and makes no snapshot; what it did store is reused by the next run.
- **A sparse file (a disk image, a download being preallocated) cut short while a Vault
  backup read it made the whole snapshot unreadable**: nothing in it could be restored,
  and pruning and the next backup failed. The file is now left out of that snapshot and
  listed among its errors, and is backed up next time.
- **Two computers with the same name sharing a Vault** (two fresh installs both called
  `fedora`) each took the other's lock for one left by a dead process of their own, so one
  could prune while the other backed up. A lock now says which computer took it. And a
  Vault backup checks, before it commits, that what it stored and what it counted on is
  still there (another computer may have taken its lock for abandoned after a long sleep,
  or with a wrong clock, and pruned): if not, it stops without making a snapshot.
- **Copying a Vault to another place could empty the copy** if the source went away in
  the middle (its drive pulled out): its folders then looked empty, and everything "no
  longer there" was removed from the copy. The source is now checked again before
  anything is removed. A backup finishing on the source during a copy also no longer
  leaves the copy with a newest snapshot that can't be restored until the next copy.
- **A Vault's weekly upkeep removed snapshots a backup had just decided to keep.** It
  applied the retention rules itself, without the two protections a backup has: keeping
  every snapshot after a run with problems, and keeping the snapshot before one with
  unusual changes. With tight rules (*keep 1*) the only good snapshot could be gone before
  the warning about the bad one. Upkeep now only gives back the space of snapshots already
  removed; which snapshots go is decided by the backup alone.
- **With cron as the scheduler and catching up off, a missed run cost the next one too.**
  After a day or more off, the first check saw the oldest missed time, called it missed,
  and never looked whether a run was due right then: a computer off at weekends skipped
  every Monday. A run that is due now runs, whatever was missed before it.
- **A Mirror on an SSH server, detecting changes by checksum, didn't store a change that
  kept the file's size and time** (the case checksums are for): the snapshot recorded the
  new content, the server kept the old, and a restore gave back the old. The transfer now
  compares contents too.
- **A Mirror on an SSH server couldn't restore, or mount, a file whose name isn't valid
  UTF-8.** It was backed up correctly but asked for under a changed name. Names are now
  sent to the server exactly as they are.
- **A Mirror on an SSH server set to follow symbolic links stored the links instead**,
  while its snapshots listed files: a restore found nothing there. That combination is
  now refused when the backup is saved and when it runs, saying to store links as links
  or to use the Vault or rsync format (both follow links to a server correctly).
- **A restore from Borg could replace a whole file with a cut-off one and report
  success.** When `borg extract` couldn't write a file out in full (no room where it
  extracts to, which needs as much space again as the files restored), Borg only warned,
  and Flux restored the short file over the one that was there. Every extracted file is
  now compared with the archive's listing: one that came out short, or not at all, is not
  restored and is reported, and the restore ends as "restored with problems" (exit 4).
- **Restoring a folder above the backed-up ones from Borg reset that folder to
  owner-only permissions and today's date** (your home folder, or `/var` in a system
  restore). Borg archives don't hold those folders; Flux now only creates them when
  they're missing, and leaves one that is there exactly as it is.
- **`flux check` on an rsync-format backup checked the listing Flux remembered, not the
  disk**: with a snapshot's files deleted it still reported no problems. It now lists
  each snapshot afresh and reports files the backup stored that are no longer there.
- **A backup started from the app could stop, or hang for good, once the app was closed**
  (or, for system backups, once the administrator helper went idle after 15 minutes).
  What the backup wrote as messages went to the app through a pipe; with the app gone,
  its next message (a warning about a file, a hook's output) failed and took the backup
  down with it, without a record in Activity, and a system backup with a hook stayed
  stuck holding its lock, so scheduled runs found it busy. Messages now go to a file.
- **Scheduled backups set up from the AppImage never ran once the app was closed**: their
  timers pointed at `flux` inside the AppImage's temporary mount, which is gone when the
  app exits and has a new name at each start. The AppImage now keeps a copy of `flux` in
  `~/.local/share/flux-backup/bin` and uses that for timers and for backups it starts.
  (Save a schedule again from a 1.0 AppImage if you set one up with an earlier build.)
- **`flux profile create --template`, `flux profile import` and `flux profile backintime`
  created backups with a schedule but no timer**, so they never ran until the profile was
  edited. The timer is now set up when the profile is created.
- **Opening a backup's settings changed its interval.** "Every 90 minutes" is stored as
  `1h 30m`; the settings read only the first part, showed "1 hours", and saved that with
  the next change of anything. Every 45 days became every minute. The whole interval is
  read now, and it is only written when you change it; an interval that isn't a whole
  number of minutes is shown as text.
- **A restore's result could vanish.** The restore dialog was closed by unrelated events
  (any backup finishing, a Vault locking itself after its idle time, Esc or the close
  button), and a restore that then failed, or couldn't restore some files, said nothing.
  The snapshot list now refreshes in place, the dialog can't be closed while a restore
  runs, and if it is gone when the restore ends, the outcome is shown as a message.
- **A backup that never succeeded counted as protected** on *All backups* and in the
  sidebar ("Your backup is protecting you") when its first run was stopped. With no
  finished backup it now says *Not backed up yet* and counts as needing attention.

### Security
- An encrypted Vault no longer accepts an unencrypted object in place of an encrypted one.
  Before, someone able to write to the drive could replace a snapshot record (or another
  object) with one Flux would trust without the key.
- **Recovering a backup trusted the settings saved next to it, commands included.** Flux
  never saves hooks or password commands there, but it ran them if someone with the
  drive or server had added them (for Mirror, rsync and Borg the file is plain text): at
  the first backup after *Recover an existing backup*, as you, or as root for a system
  backup. Hooks, webhooks, password commands, ssh options and extra rsync/borg arguments
  are now dropped from settings read off a drive.
- **`flux-vault-recover` could be made to write outside the folder it restores into**, by
  a snapshot (in a Vault without encryption, which whoever has the drive can rewrite)
  that restores a link and then a file "inside" it. Every folder on the way to a
  restored file must now be a real folder, as in Flux's own restore.
- **A backup run as root could be made to store another user's file under your name**: a
  file (or a folder on the way to it) swapped for a link between the moment Flux looked
  at it and the moment it read it was followed. Source files are now opened without
  following any link below the backed-up folder, and only if they are still regular
  files, which also stops a named pipe put there from hanging the backup.
- **An empty password is refused.** A password file with nothing in it, or a password
  command that printed nothing (a secret manager that wasn't unlocked), silently set up
  a Vault or Borg repository "encrypted" with an empty password. Creating one now fails
  and says where the empty password came from. If you created an encrypted backup with a
  password file or command, check that it wasn't empty: `flux key add` gives a Vault a
  real password (then remove the empty one with `flux key remove`).
- ***Copy diagnostics* included hook commands and webhook addresses it said were hidden**:
  the profile was cleaned, the last run's events below it were not, and they contain
  each hook's command line and output and, when a webhook fails, its address. They are
  hidden there too now.
- **A backup set to be encrypted never writes to a Vault that says it isn't.** Whether a
  Vault is encrypted is recorded in the Vault, in a file whoever holds the drive or server
  can rewrite; Flux believed it, and would have gone on backing up in the clear without a
  word. Now the backup stops and says so. (A backup recovered from an unencrypted Vault is
  set up as unencrypted, as found.)
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

[1.2.1]: https://github.com/neuralpunk/flux-backup/compare/v1.2.0...v1.2.1
[1.2.0]: https://github.com/neuralpunk/flux-backup/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/neuralpunk/flux-backup/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/neuralpunk/flux-backup/compare/v0.3.0...v1.0.0
[0.3.0]: https://github.com/neuralpunk/flux-backup/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/neuralpunk/flux-backup/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/neuralpunk/flux-backup/releases/tag/v0.1.0
