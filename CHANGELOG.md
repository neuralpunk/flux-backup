# Changelog

All notable changes to Flux Backup. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

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

[1.0.0]: https://github.com/neuralpunk/flux-backup/compare/v0.3.0...v1.0.0
[0.3.0]: https://github.com/neuralpunk/flux-backup/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/neuralpunk/flux-backup/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/neuralpunk/flux-backup/releases/tag/v0.1.0
