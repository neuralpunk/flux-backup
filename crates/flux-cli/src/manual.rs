//! The manual: what each command's man page says beyond its options (more about what it
//! does, examples of basic use, its exit status, related pages), and how the pages are
//! written. The same examples follow `flux <command> --help`.
//!
//! Every visible command has a page here, and every example is parsed by the real
//! command line in the tests, so neither can drift from what `flux` does.

use std::fmt::Write as _;
use std::io::Write as _;

/// What a man page adds to clap's options.
pub struct Page {
    /// More about the command, after its one-line summary. Paragraphs are separated by a
    /// blank line.
    pub about: &'static str,
    /// What it does, and the command line that does it.
    pub examples: &'static [(&'static str, &'static str)],
    /// The exit status, when there's more to say than 0 and 1.
    pub exit: &'static [(u8, &'static str)],
    /// Related pages, without the section ("flux-restore").
    pub see_also: &'static [&'static str],
}

const PARTIAL: (u8, &str) = (
    4,
    "It finished, but not everything went through: the message says what.",
);
const INVALID: (u8, &str) = (
    3,
    "The profile has errors; they're listed, each with the setting it's about.",
);

/// The page of `name` ("flux", "flux-backup", "flux-profile-create").
pub fn page(name: &str) -> Option<Page> {
    let p = |about, examples, exit, see_also| {
        Some(Page {
            about,
            examples,
            exit,
            see_also,
        })
    };
    match name {
        "flux" => p(
            "Flux Backup keeps snapshots of your folders on a drive or an SSH server: each one is your files as they were at that moment, and only what changed since the last one is copied. A profile (a backup) says what to back up, where to, how (plain folders, an encrypted Vault, or rsync and Borg so the backups work without Flux), when, and how long to keep snapshots. The desktop app and this command share the same profiles.\n\nMost commands take the profile's id first. A snapshot is named by its id, a unique start of an id, its name, or latest; for Mirror and rsync backups the id is the start time (2026-11-01_03-00-00), so a date picks that day's snapshot.\n\nWith --system, commands work on the computer's system backups (in /etc/flux-backup), which need root. --json prints machine-readable output for scripts. Each command has its own page: flux-backup(1), flux-restore(1) and so on.",
            &[
                (
                    "Set up a backup of your home folder to an external drive",
                    "flux profile create home --target /run/media/me/Backup --source ~",
                ),
                ("Back it up now", "flux backup home"),
                (
                    "See what's protected and when each backup runs next",
                    "flux status",
                ),
                ("List its snapshots", "flux snapshots home"),
                ("Find a file in every snapshot", "flux find home '*.odt'"),
                (
                    "Restore a folder as it was in the newest snapshot, into another folder",
                    "flux restore home ~/Documents --to ~/Restored",
                ),
                (
                    "Change what, where and when it backs up, in your editor",
                    "flux profile edit home",
                ),
                (
                    "Check that the backups can be read back",
                    "flux restore-test home",
                ),
            ],
            &[],
            &[
                "flux-profile",
                "flux-backup",
                "flux-restore",
                "flux-status",
                "flux-schedule",
            ],
        ),
        "flux-backup" => p(
            "Take a snapshot of the profile now. Flux looks through its folders for what's new or changed since the last snapshot (by size and time, or by checksum if the profile says so) and stores only that: a Mirror backup hard-links unchanged files, a Vault stores each piece of data once, rsync and Borg do the same in their own way. The snapshot is built aside and appears only once complete, so a backup that's stopped or crashes never leaves a half-made one. A stopped backup carries on where it was next time.\n\nAfterwards the retention rules remove snapshots that are no longer kept (not with --no-prune), the profile's other destinations get their copy, and a restore test runs if one is due. A backup of an encrypted Vault needs its password: from the keyring, a file or a command as the profile says, FLUX_PASSWORD, or typed here.",
            &[
                ("Back up the profile home now", "flux backup home"),
                (
                    "Back up, and name the snapshot so retention keeps it",
                    "flux backup home --name before-upgrade",
                ),
                (
                    "See what would be copied, without writing anything",
                    "flux backup home --dry-run",
                ),
                (
                    "Back up and keep every snapshot this time",
                    "flux backup home --no-prune",
                ),
                (
                    "Back up before leaving, then power off a minute after it ends",
                    "flux backup home --shutdown",
                ),
                (
                    "Back up a system backup (as root)",
                    "sudo flux --system backup etc",
                ),
            ],
            &[PARTIAL],
            &[
                "flux-status",
                "flux-watch",
                "flux-cancel",
                "flux-prune",
                "flux-log",
            ],
        ),
        "flux-snapshots" => p(
            "List a profile's snapshots, oldest first: when each was made, how long it took, how much was new, and whether it's complete, named, pinned, noted or tagged. A snapshot made while some files couldn't be read is marked; flux info says which.",
            &[
                ("List the snapshots of home", "flux snapshots home"),
                ("Only those tagged taxes", "flux snapshots home --tag taxes"),
                ("As JSON, for a script", "flux --json snapshots home"),
            ],
            &[],
            &["flux-info", "flux-ls", "flux-name", "flux-tag"],
        ),
        "flux-info" => p(
            "Show one snapshot in detail: when it was made and how long it took, its size and what was new, its name, pin, note and tags, and the files it couldn't read, if any. Without a snapshot, the newest.",
            &[
                ("The newest snapshot", "flux info home"),
                (
                    "The snapshot made on 1 November",
                    "flux info home 2026-11-01",
                ),
            ],
            &[],
            &["flux-snapshots", "flux-diff"],
        ),
        "flux-ls" => p(
            "List the files in a snapshot, like ls: a folder's contents with sizes, times and permissions, or every file below it with --recursive. Paths are as they are on your computer (~/Documents). Nothing is restored to list them.",
            &[
                ("The top of the newest snapshot", "flux ls home"),
                (
                    "A folder, in the snapshot of 1 November",
                    "flux ls home ~/Documents --snapshot 2026-11-01",
                ),
                (
                    "Everything below a folder",
                    "flux ls home ~/Pictures/2025 --recursive",
                ),
            ],
            &[],
            &["flux-find", "flux-restore", "flux-mount"],
        ),
        "flux-restore" => p(
            "Restore files or folders from a snapshot (the newest one that has them, unless --snapshot says which). With --to, they go into that folder, under their full paths; otherwise back where they were. Nothing is lost by restoring: a file that's there and differs is first moved aside as NAME.flux-<snapshot>, unless --overwrite. Identical files are skipped. Owners, permissions, times, links, extended attributes and ACLs are restored as recorded.\n\nWith --exact, a folder is made exactly as it was in the snapshot: what's in it now but wasn't then is taken out too (moved aside the same way, unless --overwrite). What the backup never takes (exclusions, the size limit, caches) is left alone. Restoring in place asks first unless --yes; --dry-run shows what would happen.",
            &[
                (
                    "Restore one file into another folder",
                    "flux restore home ~/Documents/report.odt --to ~/Restored",
                ),
                (
                    "Put a folder back where it was (asks first)",
                    "flux restore home ~/Documents",
                ),
                (
                    "The same, from the snapshot of 1 November",
                    "flux restore home ~/Documents --snapshot 2026-11-01",
                ),
                (
                    "Make a folder exactly as it was, removing what's new since",
                    "flux restore home ~/Projects/site --exact",
                ),
                (
                    "See what a restore would do",
                    "flux restore home ~/Documents --dry-run",
                ),
            ],
            &[PARTIAL],
            &[
                "flux-ls",
                "flux-find",
                "flux-history",
                "flux-export",
                "flux-mount",
            ],
        ),
        "flux-explain" => p(
            "Say whether a file or folder is backed up by the profile, and why: the exclusion that leaves it out (or the folder it's in that one leaves out), a !pattern that brings it back, the size limit, a cache folder or marker file, another disk, the backup's own storage, or that it isn't in any folder the backup takes. Paths needn't exist yet; one ending in / is judged as a folder. With --list, each item of a folder, with how much is in and out below it.",
            &[
                (
                    "Is this file backed up?",
                    "flux explain home ~/Downloads/film.mkv",
                ),
                (
                    "Several at once",
                    "flux explain home ~/Projects/app/node_modules ~/.cache",
                ),
                (
                    "Each item of a folder, with what's in and out",
                    "flux explain home ~/Projects --list",
                ),
            ],
            &[],
            &["flux-profile-edit", "flux-profile-show"],
        ),
        "flux-export" => p(
            "Save files of a snapshot as an archive, to hand to someone or keep apart from the backups. The archive's kind comes from its name: .tar.zst keeps everything (owners, permissions, times to the nanosecond, links, devices, extended attributes and ACLs, which GNU tar and bsdtar read); .zip opens anywhere and keeps files, folders, times and basic permissions only (what it can't hold is listed after). Without paths, the whole snapshot. --checksums writes a list of each file's checksum next to the archive.",
            &[
                (
                    "The whole newest snapshot as a .tar.zst",
                    "flux export home ~/home.tar.zst",
                ),
                (
                    "One folder of an older snapshot, as a zip",
                    "flux export home ~/photos-2025.zip ~/Pictures/2025 --snapshot 2026-11-01",
                ),
                (
                    "With a SHA-256 list for checking it later",
                    "flux export home /mnt/usb/docs.tar.zst ~/Documents --checksums sha256",
                ),
            ],
            &[PARTIAL],
            &["flux-restore", "flux-mount"],
        ),
        "flux-mount" => p(
            "Show a profile's snapshots as read-only folders (one per snapshot, or just one with --snapshot) at MOUNTPOINT, to browse with any program, until you press Ctrl+C or run fusermount3 -u MOUNTPOINT. Needs FUSE (the fuse3 package). Nothing is restored; files are read from the backups as programs open them.",
            &[
                (
                    "Every snapshot of home under ~/snapshots",
                    "flux mount home ~/snapshots",
                ),
                (
                    "Only the snapshot of 1 November",
                    "flux mount home ~/snapshots --snapshot 2026-11-01",
                ),
            ],
            &[],
            &["flux-ls", "flux-restore", "fusermount3"],
        ),
        "flux-diff" => p(
            "Show what changed between two snapshots: files added, removed and changed, with sizes. Without snapshots, the newest against the one before; with one, that one against the newest. With --live, the newest snapshot (or OLD) against the files on disk now: what a backup would take. --path limits it to one folder, --stat prints only the totals.",
            &[
                (
                    "The newest snapshot against the one before",
                    "flux diff home",
                ),
                (
                    "The snapshot of 1 November against the newest",
                    "flux diff home 2026-11-01",
                ),
                (
                    "What changed since the newest snapshot",
                    "flux diff home --live",
                ),
                (
                    "Two snapshots, only below a folder",
                    "flux diff home 2026-11-01 2026-11-08 --path ~/Documents",
                ),
                ("Only the totals", "flux diff home --stat"),
            ],
            &[],
            &["flux-history", "flux-snapshots"],
        ),
        "flux-find" => p(
            "Search every snapshot (or one, with --snapshot) for files whose name matches PATTERN, a shell-style pattern: *.odt matches anywhere, one with a / matches whole paths (Documents/*.odt). Each match says which snapshots hold it, and whether the file is still on disk.",
            &[
                (
                    "Every OpenDocument file in any snapshot",
                    "flux find home '*.odt'",
                ),
                (
                    "A file by name, in the newest snapshot only",
                    "flux find home report.pdf --snapshot latest",
                ),
                ("Below one folder", "flux find home 'Documents/Taxes/*'"),
            ],
            &[],
            &["flux-history", "flux-ls", "flux-restore"],
        ),
        "flux-history" => p(
            "List the versions of one file or folder across the snapshots: each different version once, with its date and size and the snapshots that hold it. Restore any of them with flux restore --snapshot.",
            &[
                ("Every version of a file", "flux history home ~/notes.md"),
                ("Of a folder", "flux history home ~/Documents/Taxes"),
            ],
            &[],
            &["flux-restore", "flux-find", "flux-diff"],
        ),
        "flux-prune" => p(
            "Remove the snapshots the retention rules no longer keep (keep_daily, keep_weekly and so on, in the profile's [retention]). Named and pinned snapshots stay, and so do at least safety_floor complete ones. Backups prune by themselves after each run; this is for doing it by hand, or seeing what would go with --dry-run. A Vault frees the data only the removed snapshots used.",
            &[
                ("See what would be removed", "flux prune home --dry-run"),
                ("Remove it", "flux prune home"),
            ],
            &[],
            &["flux-delete", "flux-pin", "flux-name"],
        ),
        "flux-name" => p(
            "Give a snapshot a name (before-upgrade), or remove its name by leaving NAME out. A named snapshot can be picked by its name, and retention keeps it while keep_named is on (the default).",
            &[
                (
                    "Name the newest snapshot",
                    "flux name home latest before-upgrade",
                ),
                ("Remove the name", "flux name home before-upgrade"),
            ],
            &[],
            &["flux-pin", "flux-note", "flux-tag"],
        ),
        "flux-note" => p(
            "Write a note on a snapshot (\"Before the upgrade to Fedora 46\"), shown with it in the app and flux info; --clear removes it. Notes travel with the backups and never change what retention keeps.",
            &[
                (
                    "Note why a snapshot matters",
                    "flux note home latest Before the upgrade to Fedora 46",
                ),
                ("Remove the note", "flux note home latest --clear"),
            ],
            &[],
            &["flux-tag", "flux-name", "flux-info"],
        ),
        "flux-tag" => p(
            "Add tags to a snapshot (taxes, release), remove them with --remove, or list them with no tags given. flux snapshots --tag finds snapshots by tag. Tags never change what retention keeps: pin or name a snapshot to keep it.",
            &[
                (
                    "Tag the newest snapshot",
                    "flux tag home latest taxes paperwork",
                ),
                ("Take a tag off", "flux tag home latest paperwork --remove"),
                ("List a snapshot's tags", "flux tag home latest"),
            ],
            &[],
            &["flux-snapshots", "flux-note"],
        ),
        "flux-pin" => p(
            "Keep a snapshot forever, whatever the retention rules say, until it's unpinned. A pinned snapshot can still be deleted by hand with flux delete --force.",
            &[(
                "Keep the snapshot of 1 November",
                "flux pin home 2026-11-01",
            )],
            &[],
            &["flux-unpin", "flux-name", "flux-prune"],
        ),
        "flux-unpin" => p(
            "Let retention manage a pinned snapshot again: it's removed when the rules no longer keep it.",
            &[("Unpin it", "flux unpin home 2026-11-01")],
            &[],
            &["flux-pin"],
        ),
        "flux-delete" => p(
            "Delete snapshots by hand, now. Asks first unless --yes. Named and pinned snapshots are refused unless --force. A Vault frees the data only the deleted snapshots used.",
            &[
                (
                    "Delete one snapshot (asks first)",
                    "flux delete home 2026-10-01",
                ),
                (
                    "Delete several without asking",
                    "flux delete home 2026-10-01 2026-10-02 --yes",
                ),
                (
                    "Delete a pinned one too",
                    "flux delete home before-upgrade --force",
                ),
            ],
            &[],
            &["flux-prune", "flux-pin"],
        ),
        "flux-log" => p(
            "Show the profile's recent backup runs (how each went, how long it took, how much it copied), or with --run, everything that happened in one run: its warnings, the files it couldn't read, hooks, pruning, other destinations and restore tests. Kept as the profile's [retention] keep_history_* says.",
            &[
                ("Recent runs", "flux log home"),
                (
                    "Everything that happened in the last run",
                    "flux log home --run last",
                ),
                ("As JSON, with timing details", "flux --json log home"),
            ],
            &[],
            &["flux-status", "flux-watch"],
        ),
        "flux-check" => p(
            "Verify a profile's backups: every snapshot complete and every file accounted for, a Vault's packs and index consistent. With --read-data, also read every stored byte and check it (slow, but it finds a damaged drive). Problems are listed; the backups aren't changed.\n\nA Vault can keep repair data ([vault] parity in the profile): Reed-Solomon parity that rebuilds parts a drive has damaged. A check then says which problems could be repaired, and --repair (which reads every byte, like --read-data) rebuilds them, checks each result against the same checksums, writes it back, and says what it repaired. It also adds repair data to what was stored before the setting was on.",
            &[
                ("Check the structure", "flux check home"),
                (
                    "Read and verify every byte too",
                    "flux check home --read-data",
                ),
                (
                    "And rebuild what is damaged from the Vault's repair data",
                    "flux check home --repair",
                ),
            ],
            &[(1, "Problems were found (or the check couldn't run).")],
            &["flux-restore-test", "flux-unlock"],
        ),
        "flux-restore-test" => p(
            "Read a sample of the newest snapshot back, the way a restore reads it, and check every file as it streams: its size and its checksum, and with --compare-source the file on disk now (a difference is only a note: you may have changed it since). Nothing is restored or written anywhere: each file passes through a small buffer that's wiped as it goes, so memory use is the same for ten files as for every file, and an encrypted backup stays encrypted at rest.\n\nWithout a choice of files, the profile's own [restore_test] settings (10 random files by default). --damage (with --certainty) sizes the sample by what it must prove: enough random files that damage to that share of the files or more would be caught with that certainty (459 files for 1% at 99%, almost whatever the size of the backup; fewer for a small one). A test of random files that passes says what it would have caught. --rolling tests the next share of every file, so all of them are tested once per window (30 days by default). The profile can run tests by itself after backups; --history shows the latest ones.",
            &[
                (
                    "Test as the profile says (10 random files)",
                    "flux restore-test home",
                ),
                (
                    "50 random files, also compared with your files now",
                    "flux restore-test home --files 50 --compare-source",
                ),
                (
                    "Enough files to catch damage to 1% of them, 99 times in 100",
                    "flux restore-test home --damage 1%",
                ),
                (
                    "The same for 0.1% of them, surer still",
                    "flux restore-test home --damage 0.1% --certainty 99.9%",
                ),
                (
                    "A share of the snapshot",
                    "flux restore-test home --percent 5",
                ),
                ("Every file", "flux restore-test home --full"),
                ("The latest tests", "flux restore-test home --history"),
            ],
            &[
                (
                    4,
                    "It passed with notes (files that differ from now, or it stopped at a limit).",
                ),
                (
                    1,
                    "It failed: a file couldn't be read back, or came back wrong.",
                ),
            ],
            &["flux-check", "flux-restore"],
        ),
        "flux-sentry" => p(
            "Flux Sentry watches each backup for unusual changes: far more files changed than is usual for that backup (it learns how much is, from the backup's own history), files renamed to a new extension, text that now looks encrypted, ransom notes. Without options, this says how it watches the profile, what it has learned, and whether it holds the backup.\n\nWhen a backup shows unusual changes, the snapshot before it is pinned, old snapshots stop being removed, the profile's other places aren't backed up to, and no further backup is made, by schedule or by hand, until the changes are settled: a later backup could only store more of the damage. Settle them with --this-was-me (they were yours: Flux Sentry takes them as normal for this backup from now on, and the pin comes off) or --dealt-with (they weren't, and you've restored what you need: the pin stays). Turning unusual_changes off in the profile also lets backups run.",
            &[
                (
                    "What Flux Sentry knows, and whether it holds the backup",
                    "flux sentry home",
                ),
                ("The big change was yours", "flux sentry home --this-was-me"),
                (
                    "Not yours: restore from the snapshot kept from before…",
                    "flux restore home ~/Documents -s 2026-11-09_20-00-00",
                ),
                ("…then let backups go on", "flux sentry home --dealt-with"),
            ],
            &[(1, "The backup is on hold (without an option).")],
            &["flux-restore", "flux-snapshots", "flux-backup"],
        ),
        "flux-diagnostics" => p(
            "Print what a bug report needs: the versions of Flux and the tools it uses, the system, the profile with its secrets hidden (passwords, password commands, webhook addresses, hooks), and its recent runs. Read it before you share it.",
            &[(
                "Save it to attach to a report",
                "flux diagnostics home > flux-diagnostics.txt",
            )],
            &[],
            &["flux-log"],
        ),
        "flux-recovery" => p(
            "Show the recovery kit of a compatibility-mode profile (rsync or Borg): how to get the backups back without Flux, with the exact commands. --script prints the script that makes a backup the same way; --save writes both into a folder. The kit is also kept next to the backups, as FLUX-RECOVERY.txt and flux-backup.sh.",
            &[
                ("Read the guide", "flux recovery home"),
                ("The backup script", "flux recovery home --script"),
                (
                    "Keep both on a USB stick",
                    "flux recovery home --save /run/media/me/USB",
                ),
            ],
            &[],
            &["flux-recover"],
        ),
        "flux-recover" => p(
            "Find the backups Flux made on a drive or a server (from another computer, say) and set one up here again. Without --pick, lists what's there. With it, makes a profile that reaches that backup where it was found, with the settings kept with it (folders, exclusions, rules); --carry-on lets it go on backing up into the same history, --id chooses the new profile's id. An encrypted Vault's backups are listed once its password is given (FLUX_PASSWORD, or typed).",
            &[
                ("What's on this drive?", "flux recover /run/media/me/Backup"),
                (
                    "Set the second one up here",
                    "flux recover me@nas:/srv/backup --pick 2",
                ),
                (
                    "And carry on backing up into it, as home",
                    "flux recover /run/media/me/Backup --pick 1 --carry-on --id home",
                ),
            ],
            &[],
            &["flux-recovery", "flux-profile-import"],
        ),
        "flux-unlock" => p(
            "Remove the locks a Vault backup left behind when it died (a crash, a power cut), so the next backup can run. Only locks whose job is clearly gone are removed. --force also removes those that still look in use (a job on another computer that died or lost its connection less than 30 minutes ago): only when you're sure no job is using the backup from anywhere. No password is needed.",
            &[
                ("Remove stale locks", "flux unlock home"),
                ("Remove every lock", "flux unlock home --force"),
            ],
            &[],
            &["flux-check"],
        ),
        "flux-key" => p(
            "Manage an encrypted Vault's passwords. A Vault can have several (each one opens it), its password can be kept in the desktop keyring, and its key can be made stronger or need an unlock file as well. Changing a password never re-encrypts the backups: only the small key file is rewritten.",
            &[
                ("List a Vault's passwords", "flux key list home"),
                ("Change the password", "flux key passwd home"),
            ],
            &[],
            &[
                "flux-key-init",
                "flux-key-add",
                "flux-key-passwd",
                "flux-key-store",
                "flux-key-strengthen",
                "flux-key-unlock-file",
            ],
        ),
        "flux-key-init" => p(
            "Create the encrypted Vault now with a new password (asked twice), instead of at the first backup, and keep the password where the profile says (the keyring, or a key file that doesn't exist yet). Then even the first, scheduled backup has nothing to ask.",
            &[("Set it up", "flux key init home")],
            &[],
            &["flux-key", "flux-key-store"],
        ),
        "flux-key-list" => p(
            "List the passwords (key slots) that open a Vault, with their labels, when each was added and how strongly it's hashed. Works while the Vault is locked.",
            &[("List them", "flux key list home")],
            &[],
            &["flux-key-add", "flux-key-remove"],
        ),
        "flux-key-add" => p(
            "Add another password that opens the Vault (for a second person, or a recovery password kept in a safe). Asks for a password that opens it now, then the new one twice.",
            &[(
                "Add a recovery password",
                "flux key add home --label recovery",
            )],
            &[],
            &["flux-key-list", "flux-key-remove"],
        ),
        "flux-key-remove" => p(
            "Remove one of a Vault's passwords, by its key id as flux key list shows it. The last one can't be removed.",
            &[(
                "Remove the password with key id 7c41e0a2",
                "flux key remove home 7c41e0a2",
            )],
            &[],
            &["flux-key-list"],
        ),
        "flux-key-passwd" => p(
            "Change the password in use: asks for it, then the new one twice (the new one is added, then the old one removed). If the profile keeps it in the keyring, the keyring is updated; a key file or a command is yours to update.",
            &[("Change it", "flux key passwd home")],
            &[],
            &["flux-key-store"],
        ),
        "flux-key-store" => p(
            "Ask for the Vault's password once and keep it in the desktop keyring, so scheduled backups can run without anyone typing it (the profile's password_source must be keyring).",
            &[("Keep it in the keyring", "flux key store home")],
            &[],
            &["flux-key-forget"],
        ),
        "flux-key-forget" => p(
            "Remove the Vault's password from the desktop keyring. Backups then need it from somewhere else.",
            &[("Forget it", "flux key forget home")],
            &[],
            &["flux-key-store"],
        ),
        "flux-key-strengthen" => p(
            "Hash the password in use again, more strongly, so a stolen drive's password resists guessing better: standard (64 MiB, 3 passes), strong (256 MiB, 4), maximum (1 GiB, 6), or your own --memory (in MiB), --passes and --threads. Unlocking takes longer by the same factor. It's instant: only the key changes, not the data. It's also the strength of the Vault's new passwords.",
            &[
                (
                    "The strong level",
                    "flux key strengthen home --level strong",
                ),
                (
                    "Your own settings: 512 MiB and 5 passes",
                    "flux key strengthen home --memory 512 --passes 5 --threads 4",
                ),
            ],
            &[],
            &["flux-key-list", "flux-key-unlock-file"],
        ),
        "flux-key-unlock-file" => p(
            "Make the password open the Vault only together with a small file (kept on a USB stick, say): create makes a new one at PATH, use takes an existing file, remove makes the password enough again. Without the file, backups say where they expect it. Keep a copy: losing it loses the backups.",
            &[
                (
                    "Require a new unlock file on a USB stick",
                    "flux key unlock-file home create /run/media/me/STICK/home.key",
                ),
                (
                    "Use one you have",
                    "flux key unlock-file home use /run/media/me/STICK/home.key",
                ),
                ("Stop requiring it", "flux key unlock-file home remove"),
            ],
            &[],
            &["flux-key-strengthen"],
        ),
        "flux-vault" => p(
            "Vault maintenance beyond backups and checks.",
            &[(
                "Move a Vault to the newest format",
                "flux vault upgrade home",
            )],
            &[],
            &["flux-vault-upgrade", "flux-check"],
        ),
        "flux-vault-upgrade" => p(
            "Move a Vault to the newest format (format 2 keeps sparse files, such as disk images, small). Snapshots already stored are untouched. Afterwards, Flux before 1.0 can't open it.",
            &[("Upgrade it", "flux vault upgrade home")],
            &[],
            &["flux-vault"],
        ),
        "flux-ssh" => p(
            "Helpers for backups to an SSH server: check that Flux can log in and write there, and make and install a key so it logs in without a password.",
            &[
                ("Test the server of profile nas", "flux ssh test nas"),
                ("Make a key and install it", "flux ssh keygen"),
            ],
            &[],
            &["flux-ssh-test", "flux-ssh-keygen", "flux-ssh-copy-id"],
        ),
        "flux-ssh-test" => p(
            "Log in to the profile's server as a backup would (its user, port, key and ssh options) and check that the folder can be written, with what's missing if not: the host key, the key, the folder, rsync or borg on the server.",
            &[("Test it", "flux ssh test nas")],
            &[],
            &["flux-ssh-copy-id"],
        ),
        "flux-ssh-keygen" => p(
            "Create an SSH key for backups, without a passphrase so scheduled backups can use it (~/.ssh/flux_backup_ed25519 unless --file says otherwise). Then install it with flux ssh copy-id.",
            &[
                ("Create the key", "flux ssh keygen"),
                ("Somewhere else", "flux ssh keygen --file ~/.ssh/nas_backup"),
            ],
            &[],
            &["flux-ssh-copy-id"],
        ),
        "flux-ssh-copy-id" => p(
            "Install a public key on the profile's server with ssh-copy-id (asks for the server's password once), so backups log in with the key.",
            &[
                ("Install the profile's key", "flux ssh copy-id nas"),
                (
                    "Install another key",
                    "flux ssh copy-id nas --key ~/.ssh/nas_backup.pub",
                ),
            ],
            &[],
            &["flux-ssh-keygen", "flux-ssh-test"],
        ),
        "flux-schedule" => p(
            "Automatic backups. A profile's schedule is set in the profile ([schedule]: at set times, every few hours, when its drive is plugged in); saving it writes a systemd user timer (or a crontab line, with cron), so backups run even with the app closed. These commands look at the timers and put them right.",
            &[
                ("Every profile's timer and next run", "flux schedule status"),
                ("Make the timers match the profiles", "flux schedule sync"),
            ],
            &[],
            &[
                "flux-schedule-status",
                "flux-schedule-sync",
                "flux-pause",
                "flux-profile-edit",
            ],
        ),
        "flux-schedule-status" => p(
            "Show each profile's schedule, whether its timer is installed and up to date, when it runs next and when it last ran.",
            &[("Show them", "flux schedule status")],
            &[(
                1,
                "A timer is missing or out of date: flux schedule sync fixes it.",
            )],
            &["flux-schedule-sync"],
        ),
        "flux-schedule-install" => p(
            "Write (or rewrite) the timer of one profile from its schedule, and start it.",
            &[("Install home's timer", "flux schedule install home")],
            &[],
            &["flux-schedule-remove", "flux-schedule-sync"],
        ),
        "flux-schedule-remove" => p(
            "Remove one profile's timer: it's then backed up only by hand, until its timer is installed again (saving the profile does that).",
            &[("Remove it", "flux schedule remove home")],
            &[],
            &["flux-schedule-install"],
        ),
        "flux-schedule-sync" => p(
            "Make every timer match its profile: install missing ones, rewrite those out of date, and remove those of profiles that are gone or manual.",
            &[("Sync them", "flux schedule sync")],
            &[],
            &["flux-schedule-status"],
        ),
        "flux-schedule-use" => p(
            "Choose what runs scheduled backups: systemd timers (the default) or cron, as Back In Time uses. Every schedule moves over; the rest of your crontab is left alone.",
            &[
                ("Use cron", "flux schedule use cron"),
                ("Back to systemd timers", "flux schedule use systemd"),
            ],
            &[],
            &["flux-schedule-status"],
        ),
        "flux-status" => p(
            "Show every profile (or one) at a glance: whether it's running, how its last backup went and when, when it runs next, and anything that needs you (a drive not seen for a while, failed backups, little space left).",
            &[
                ("Every profile", "flux status"),
                ("One, as JSON", "flux --json status home"),
            ],
            &[],
            &["flux-log", "flux-watch", "flux-schedule-status"],
        ),
        "flux-pause" => p(
            "Pause scheduled backups, for a presentation or a slow connection: for a while (--for), until 7:00 tomorrow (--tomorrow), or until flux resume. Backups that come due meanwhile are skipped, not queued; backing up by hand still works.",
            &[
                ("For an hour", "flux pause --for 1h"),
                ("Until tomorrow morning", "flux pause --tomorrow"),
                ("Until resumed", "flux pause"),
            ],
            &[],
            &["flux-resume"],
        ),
        "flux-resume" => p(
            "Let scheduled backups run again after flux pause.",
            &[("Resume them", "flux resume")],
            &[],
            &["flux-pause"],
        ),
        "flux-watch" => p(
            "Follow a running backup's progress (however it was started: by hand, its timer, or the app) until it ends.",
            &[("Watch home", "flux watch home")],
            &[],
            &["flux-status", "flux-cancel"],
        ),
        "flux-cancel" => p(
            "Stop a running backup. Nothing half-made is kept, and the next backup carries on where this one stopped.",
            &[("Stop it", "flux cancel home")],
            &[],
            &["flux-watch"],
        ),
        "flux-profile" => p(
            "Create, look at and change profiles: what to back up, where, how and when. Each profile is a TOML file in ~/.config/flux-backup/profiles (or /etc/flux-backup/profiles with --system); flux profile edit opens it in your editor and checks it before saving.",
            &[
                ("Your profiles", "flux profile list"),
                (
                    "Create one",
                    "flux profile create home --target /run/media/me/Backup --source ~",
                ),
                ("Change it in your editor", "flux profile edit home"),
            ],
            &[],
            &[
                "flux-profile-create",
                "flux-profile-edit",
                "flux-profile-show",
            ],
        ),
        "flux-profile-list" => p(
            "List your profiles: their ids, names, where they back up to and how.",
            &[("List them", "flux profile list")],
            &[],
            &["flux-profile-show"],
        ),
        "flux-profile-show" => p(
            "Print a profile's settings as TOML (every setting, defaults included), followed by any problems found in it.",
            &[
                ("Show it", "flux profile show home"),
                ("As JSON", "flux --json profile show home"),
            ],
            &[],
            &["flux-profile-edit", "flux-profile-validate"],
        ),
        "flux-profile-create" => p(
            "Create a profile. The target is a folder (a drive's mount point), [user@]host:/path or ssh://[user@]host[:port]/path; sources are the folders to back up (your home folder unless given); exclusions use .gitignore-style patterns. --engine chooses how backups are stored: mirror (plain folders, the default), vault (encrypted, deduplicated), rsync or borg (standard tools, so the backups work without Flux). --template starts from one of your templates. A schedule set in it gets its timer at once.",
            &[
                (
                    "Your home folder to an external drive",
                    "flux profile create home --target /run/media/me/Backup --source ~",
                ),
                (
                    "Documents to a server, encrypted",
                    "flux profile create docs --target me@nas:/srv/backup --source ~/Documents --engine vault",
                ),
                (
                    "Leaving some things out",
                    "flux profile create home --target /run/media/me/Backup --source ~ --exclude '~/Downloads/' --exclude '*.iso'",
                ),
                (
                    "Backups that rsync can read without Flux",
                    "flux profile create plain --target /run/media/me/Backup --source ~ --engine rsync",
                ),
                (
                    "A Borg repository",
                    "flux profile create borg --target me@nas:/srv/backup --source ~ --engine borg",
                ),
                (
                    "From a template",
                    "flux profile create work --target /run/media/me/Backup --template work-laptop",
                ),
            ],
            &[INVALID],
            &["flux-profile-edit", "flux-profile-templates", "flux-backup"],
        ),
        "flux-profile-edit" => p(
            "Open a profile in $VISUAL or $EDITOR, then check it: one with errors isn't saved (you're asked whether to edit it again; no leaves it as it was), and warnings are shown. Its timer follows the new schedule.",
            &[
                ("Change home", "flux profile edit home"),
                ("With another editor", "EDITOR=nano flux profile edit home"),
            ],
            &[],
            &["flux-profile-show", "flux-profile-validate"],
        ),
        "flux-profile-delete" => p(
            "Delete a profile and its timer (asks first unless --yes). Its backups stay where they are: flux recover finds them again.",
            &[("Delete it", "flux profile delete old-laptop")],
            &[],
            &["flux-recover"],
        ),
        "flux-profile-validate" => p(
            "Check profiles for errors and warnings (every profile when no id is given), as the app does while you edit: a target inside a source, a password that scheduled backups can't get, a size limit that leaves out nearly everything.",
            &[
                ("Check every profile", "flux profile validate"),
                ("Check two", "flux profile validate home docs"),
            ],
            &[INVALID],
            &["flux-profile-edit"],
        ),
        "flux-profile-duplicate" => p(
            "Copy a profile, with its schedule, under a new id (<id>-copy) and name (\"… (copy)\"), to back up the same folders to a second place, say. Change its target with flux profile edit.",
            &[("Copy it", "flux profile duplicate home")],
            &[],
            &["flux-profile-edit", "flux-profile-destination-add"],
        ),
        "flux-profile-export" => p(
            "Export a profile: as a Flux profile file to import on another computer (secrets left out), or as the configuration or script of another backup tool (borgmatic, restic, rsync, borg). Whatever a format can't express is listed as a note, in the file and on stderr.",
            &[
                ("As a Flux profile", "flux profile export home -o home.toml"),
                (
                    "As a borgmatic configuration",
                    "flux profile export home --format borgmatic -o config.yaml",
                ),
            ],
            &[],
            &["flux-profile-import"],
        ),
        "flux-profile-import" => p(
            "Import a profile exported by Flux (another computer's, say), under its own id or --id. Its secrets weren't exported: set the password source and check the target before the first backup.",
            &[
                ("Import it", "flux profile import home.toml"),
                (
                    "Under another id",
                    "flux profile import home.toml --id home-old",
                ),
            ],
            &[INVALID],
            &["flux-profile-export", "flux-recover"],
        ),
        "flux-profile-backintime" => p(
            "Import a Back In Time profile (its folders, exclusions, schedule and rules) as a Flux profile. Without a number, lists the Back In Time profiles found; --engine chooses how Flux stores the new backups (rsync keeps Back In Time's kind of folders).",
            &[
                ("What's there to import?", "flux profile backintime"),
                (
                    "Import the first",
                    "flux profile backintime 1 --id home --engine rsync",
                ),
            ],
            &[INVALID],
            &["flux-profile-create"],
        ),
        "flux-profile-templates" => p(
            "List the templates a new profile can start from: Flux's own (home folder, documents, a server, a system backup…) and yours.",
            &[("List them", "flux profile templates")],
            &[],
            &["flux-profile-save-template", "flux-profile-create"],
        ),
        "flux-profile-save-template" => p(
            "Save a profile's settings as a template (everything but where it backs up to), to start new profiles from.",
            &[("Save one", "flux profile save-template home work-laptop")],
            &[],
            &["flux-profile-templates", "flux-profile-delete-template"],
        ),
        "flux-profile-delete-template" => p(
            "Remove one of your templates. Profiles made from it don't change.",
            &[("Remove it", "flux profile delete-template work-laptop")],
            &[],
            &["flux-profile-templates"],
        ),
        "flux-profile-destination" => p(
            "A profile can keep copies in several places: each backup goes to the main destination and then to each other one, either from your files again (independent copies) or copied from the main one (--replicate). Mark a place that's kept away from home with --offsite.",
            &[
                (
                    "A profile's destinations",
                    "flux profile destination list home",
                ),
                (
                    "Add a server, encrypted, kept elsewhere",
                    "flux profile destination add home nas --target me@nas:/srv/backup --engine vault --offsite",
                ),
            ],
            &[],
            &["flux-profile-destination-add"],
        ),
        "flux-profile-destination-list" => p(
            "List a profile's destinations, the main one first, with how each is stored and whether it's kept elsewhere.",
            &[("List them", "flux profile destination list home")],
            &[],
            &["flux-profile-destination-add"],
        ),
        "flux-profile-destination-add" => p(
            "Add a destination to a profile. --engine says how it's stored (any engine, not only the main one's); --replicate copies the main destination's new snapshots there instead of reading your files again; --offsite marks it as kept elsewhere; --label gives it a name for people.",
            &[
                (
                    "A USB stick, encrypted",
                    "flux profile destination add home usb --target /run/media/me/STICK --engine vault --label 'USB stick'",
                ),
                (
                    "A server, copied from the main backup, kept elsewhere",
                    "flux profile destination add home nas --target me@nas:/srv/backup --replicate --offsite",
                ),
            ],
            &[INVALID],
            &["flux-profile-destination-remove"],
        ),
        "flux-profile-destination-remove" => p(
            "Remove a destination from a profile. Its backups stay where they are.",
            &[("Remove it", "flux profile destination remove home usb")],
            &[],
            &["flux-profile-destination-add"],
        ),
        "flux-config" => p(
            "Flux's own configuration.",
            &[("Where Flux keeps its files", "flux config path")],
            &[],
            &["flux-config-path"],
        ),
        "flux-config-path" => p(
            "Print where Flux keeps profiles, settings, state (run logs, test reports) and its cache, for you or (with --system) the system.",
            &[
                ("Yours", "flux config path"),
                ("The system's", "flux --system config path"),
            ],
            &[],
            &["flux"],
        ),
        "flux-completions" => p(
            "Print a shell completion script, so Tab completes commands, options and profile ids.",
            &[
                (
                    "For bash",
                    "flux completions bash > ~/.local/share/bash-completion/completions/flux",
                ),
                ("For zsh", "flux completions zsh > ~/.zfunc/_flux"),
                (
                    "For fish",
                    "flux completions fish > ~/.config/fish/completions/flux.fish",
                ),
            ],
            &[],
            &["flux"],
        ),
        _ => None,
    }
}

/// `flux <command> --help` ends with its examples (`flux help backup` too).
pub fn with_examples(cmd: clap::Command) -> clap::Command {
    fn walk(cmd: clap::Command, name: &str) -> clap::Command {
        let subs: Vec<String> = cmd
            .get_subcommands()
            .filter(|s| !s.is_hide_set())
            .map(|s| s.get_name().to_owned())
            .collect();
        let mut cmd = cmd;
        for s in subs {
            let page_name = format!("{name}-{s}");
            cmd = cmd.mut_subcommand(&s, |sub| walk(sub, &page_name));
        }
        let Some(page) = page(name).filter(|p| !p.examples.is_empty()) else {
            return cmd;
        };
        let mut text = cmd
            .get_after_help()
            .map(|h| format!("{h}\n\n"))
            .unwrap_or_default();
        text.push_str("Examples:\n");
        for (what, line) in page.examples {
            let _ = write!(text, "  {line}\n      {what}\n");
        }
        cmd.after_long_help(text.trim_end().to_owned())
    }
    walk(cmd, "flux")
}

/// Text for roff: backslashes and leading dots escaped, hyphens as ASCII minus.
fn esc(s: &str) -> String {
    let s = s.replace('\\', "\\e").replace('-', "\\-");
    if s.starts_with('.') || s.starts_with('\'') {
        format!("\\&{s}")
    } else {
        s
    }
}

/// `flux.1`, plus `flux-<sub>.1` (and `flux-<sub>-<subsub>.1`) for every visible
/// subcommand: clap's synopsis and options, and the manual's description, examples,
/// exit status and related pages.
pub fn write_pages(mut cmd: clap::Command, dir: &std::path::Path) -> anyhow::Result<usize> {
    cmd.build();
    std::fs::create_dir_all(dir)?;
    fn write(cmd: clap::Command, name: &str, dir: &std::path::Path) -> anyhow::Result<usize> {
        let cmd = cmd.name(name.to_owned());
        let mut out = Vec::new();
        render(&cmd, name, &mut out)?;
        std::fs::write(dir.join(format!("{name}.1")), out)?;
        let mut n = 1;
        for sub in cmd
            .get_subcommands()
            .filter(|s| !s.is_hide_set() && s.get_name() != "help")
        {
            n += write(sub.clone(), &format!("{name}-{}", sub.get_name()), dir)?;
        }
        Ok(n)
    }
    write(cmd, "flux", dir)
}

fn render(cmd: &clap::Command, name: &str, w: &mut Vec<u8>) -> anyhow::Result<()> {
    let man = clap_mangen::Man::new(cmd.clone());
    let page = page(name);
    man.render_title(w)?;
    man.render_name_section(w)?;
    man.render_synopsis_section(w)?;

    // DESCRIPTION: the manual's paragraphs (NAME already has the one-line summary), or
    // clap's own text for a command the manual doesn't know.
    let mut r = String::from(".SH DESCRIPTION\n");
    match &page {
        Some(p) => {
            for para in p.about.split("\n\n") {
                let _ = writeln!(r, ".PP\n{}", esc(para));
            }
        }
        None => {
            if let Some(about) = cmd.get_long_about().or_else(|| cmd.get_about()) {
                let _ = writeln!(r, "{}", esc(&about.to_string()));
            }
        }
    }
    w.write_all(r.as_bytes())?;

    if cmd.get_arguments().any(|a| !a.is_hide_set()) {
        man.render_options_section(w)?;
    }
    if cmd.has_subcommands() {
        man.render_subcommands_section(w)?;
    }

    let mut r = String::new();
    // Notes written for --help (formats, what an archive keeps): kept as written.
    if let Some(notes) = cmd.get_after_help() {
        let _ = writeln!(r, ".SH NOTES\n.nf");
        for line in notes.to_string().lines() {
            let _ = writeln!(r, "{}", esc(line));
        }
        let _ = writeln!(r, ".fi");
    }
    if let Some(p) = &page {
        if !p.examples.is_empty() {
            let _ = writeln!(r, ".SH EXAMPLES");
            for (what, line) in p.examples {
                let _ = writeln!(
                    r,
                    ".PP\n{}:\n.RS 4\n.EX\n{}\n.EE\n.RE",
                    esc(what),
                    esc(line)
                );
            }
        }
        let _ = writeln!(r, ".SH \"EXIT STATUS\"");
        let mut statuses: Vec<(u8, &str)> = vec![
            (0, "It worked."),
            (1, "It failed; the message says why."),
            (
                2,
                "The command line was wrong (an unknown option, a missing argument).",
            ),
        ];
        for &(code, text) in p.exit {
            match statuses.iter_mut().find(|(c, _)| *c == code) {
                Some(s) => s.1 = text,
                None => statuses.push((code, text)),
            }
        }
        statuses.sort_by_key(|(c, _)| *c);
        for (code, text) in statuses {
            let _ = writeln!(r, ".TP\n.B {code}\n{}", esc(text));
        }
        if name == "flux" {
            r.push_str(&esc_block(ENVIRONMENT));
            r.push_str(&esc_block(FILES));
        }
        if !p.see_also.is_empty() {
            let refs: Vec<String> = p
                .see_also
                .iter()
                .map(|s| format!("\\fB{}\\fR(1)", esc(s)))
                .collect();
            let _ = writeln!(r, ".SH \"SEE ALSO\"\n{}", refs.join(", "));
        }
    }
    w.write_all(r.as_bytes())?;
    man.render_version_section(w)?;
    Ok(())
}

/// A section given as `.SH TITLE` then `term\tdescription` lines.
fn esc_block(block: &str) -> String {
    let mut out = String::new();
    for line in block.lines() {
        if let Some(title) = line.strip_prefix(".SH ") {
            let _ = writeln!(out, ".SH {title}");
        } else if let Some((term, text)) = line.split_once('\t') {
            let _ = writeln!(out, ".TP\n.B {}\n{}", esc(term), esc(text));
        }
    }
    out
}

const ENVIRONMENT: &str = ".SH ENVIRONMENT
FLUX_PASSWORD\tAn encrypted Vault's password (or Borg passphrase), over the profile's own source.
FLUX_HOME\tKeep all configuration and state under this one folder (portable setups, tests).
FLUX_LOG\tWhat to log, e.g. debug or flux_vault=trace (like -v).
FLUX_SSH, FLUX_RSYNC\tThe ssh and rsync programs to use.
FLUX_SECRET_TOOL\tThe secret-tool program, for the desktop keyring.
FLUX_SYSTEMCTL, FLUX_SYSTEMD_UNIT_DIR\tHow systemd is called, and where timers are written.
FLUX_CRONTAB, FLUX_CRON_DIR\tThe crontab program and the /etc/cron.d folder, with cron.
FLUX_CURL\tThe curl program webhooks are sent with.
VISUAL, EDITOR\tThe editor flux profile edit opens.";

const FILES: &str = ".SH FILES
~/.config/flux-backup/profiles/*.toml\tYour profiles, one file each (/etc/flux-backup/profiles with --system).
~/.config/flux-backup/settings.toml\tPreferences for every backup: notifications, webhooks, the scheduler.
~/.local/state/flux-backup\tRun logs, restore test reports and what's remembered between runs (/var/lib/flux-backup for the system).
~/.cache/flux-backup\tCached file listings, to browse and search quickly (/var/cache/flux-backup); safe to delete.
~/.config/systemd/user/flux-backup-*.timer\tThe timers of scheduled backups (with cron: lines in your crontab).";

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn pages(cmd: &clap::Command, name: &str, out: &mut Vec<String>) {
        out.push(name.to_owned());
        for s in cmd
            .get_subcommands()
            .filter(|s| !s.is_hide_set() && s.get_name() != "help")
        {
            pages(s, &format!("{name}-{}", s.get_name()), out);
        }
    }

    /// Every command has a page with something to say and an example; every example and
    /// every page it points to are real.
    #[test]
    fn every_command_has_a_page_with_examples_that_parse() {
        let cmd = crate::Cli::command();
        let mut names = Vec::new();
        pages(&cmd, "flux", &mut names);
        assert!(names.len() > 70, "{}", names.len());
        for name in &names {
            let p = page(name).unwrap_or_else(|| panic!("no manual page for {name}"));
            assert!(!p.about.is_empty() && !p.examples.is_empty(), "{name}");
            for (what, line) in p.examples {
                assert!(
                    !what.ends_with('.') && !what.ends_with(':'),
                    "{name}: {what:?}"
                );
                // What a shell would pass: words, with '…' and "…" kept together.
                let mut words = Vec::new();
                let mut cur = String::new();
                let mut quote = None;
                for ch in line.chars() {
                    match (quote, ch) {
                        (None, '\'' | '"') => quote = Some(ch),
                        (Some(q), c) if c == q => quote = None,
                        (None, ' ') => {
                            if !cur.is_empty() {
                                words.push(std::mem::take(&mut cur));
                            }
                        }
                        (_, c) => cur.push(c),
                    }
                }
                if !cur.is_empty() {
                    words.push(cur);
                }
                // Only the flux command itself (after `sudo`, an `X=y`, before `>`).
                let start = words
                    .iter()
                    .position(|w| w == "flux")
                    .unwrap_or_else(|| panic!("{name}: {line}"));
                let end = words.iter().position(|w| w == ">").unwrap_or(words.len());
                let argv = &words[start..end];
                if let Err(e) = <crate::Cli as clap::Parser>::try_parse_from(argv) {
                    panic!("{name}: `{line}` doesn't parse: {e}");
                }
            }
            for s in p.see_also {
                assert!(
                    names.iter().any(|n| n == s) || ["fusermount3"].contains(s),
                    "{name} points to {s}, which has no page"
                );
            }
        }
    }

    #[test]
    fn pages_render_with_their_sections() {
        let dir = tempfile::tempdir().unwrap();
        let n = write_pages(crate::Cli::command(), dir.path()).unwrap();
        assert!(n > 70);
        let backup = std::fs::read_to_string(dir.path().join("flux-backup.1")).unwrap();
        for section in [
            ".SH DESCRIPTION",
            ".SH EXAMPLES",
            ".SH \"EXIT STATUS\"",
            ".SH \"SEE ALSO\"",
        ] {
            assert!(backup.contains(section), "{section}");
        }
        assert!(
            backup.contains("flux backup home \\-\\-dry\\-run"),
            "examples escaped"
        );
        assert!(backup.contains(".B 4"), "its own exit status");
        let top = std::fs::read_to_string(dir.path().join("flux.1")).unwrap();
        assert!(top.contains(".SH ENVIRONMENT") && top.contains(".SH FILES"));
        let export = std::fs::read_to_string(dir.path().join("flux-profile-export.1")).unwrap();
        assert!(export.contains(".SH NOTES\n.nf"), "notes kept as written");

        let help = with_examples(crate::Cli::command())
            .find_subcommand_mut("backup")
            .unwrap()
            .render_long_help()
            .to_string();
        assert!(help.contains("Examples:\n  flux backup home\n"), "{help}");
    }
}
