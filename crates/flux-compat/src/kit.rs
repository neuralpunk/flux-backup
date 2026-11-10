//! The recovery kit kept next to every compatibility-mode repository: a plain-text guide
//! with the exact commands for this profile and target, and a script that runs the same
//! backup with the standard tool only.

use flux_core::profile::{Profile, Target};
use flux_core::{ProfileId, sys};
use flux_transport::shell_quote;

use crate::filter::RuleSet;
use crate::rsync::{self, ArgSpec, LATEST, PARTIAL, RULES_FILE, SNAPSHOTS};

pub const RECOVERY_FILE: &str = "FLUX-RECOVERY.txt";
pub const SCRIPT_FILE: &str = "flux-backup.sh";

/// The kit's files.
pub struct Kit {
    pub recovery: String,
    pub script: String,
}

/// Where the repository is, in the forms the commands need.
struct Place {
    /// Repository path on the machine that holds it.
    repo: String,
    /// `user@host` for SSH targets.
    remote: Option<String>,
    /// The ssh command for rsync's `-e` and for remote shell commands.
    rsh: Option<String>,
}

impl Place {
    fn new(id: &ProfileId, p: &Profile) -> Self {
        let repo = rsync::repo_location(id, p);
        match &p.target {
            Target::Local { .. } => Self {
                repo,
                remote: None,
                rsh: None,
            },
            Target::Ssh { host, user, .. } => Self {
                repo,
                remote: Some(match user {
                    Some(u) => format!("{u}@{host}"),
                    None => host.clone(),
                }),
                rsh: rsync::plain_rsh(p),
            },
        }
    }

    /// A path inside the repository as an rsync/scp argument.
    fn arg(&self, rel: &str) -> String {
        let path = if rel.is_empty() {
            self.repo.clone()
        } else {
            format!("{}/{rel}", self.repo)
        };
        match &self.remote {
            Some(r) => format!("{r}:{}", shell_quote(&path)),
            None => shell_quote(&path),
        }
    }

    /// A shell command run where the repository is.
    fn there(&self, cmd: &str) -> String {
        match (&self.remote, &self.rsh) {
            (Some(r), Some(rsh)) => format!("{rsh} {r} {}", shell_quote(cmd)),
            _ => cmd.to_owned(),
        }
    }

    fn rsync_e(&self) -> String {
        match &self.rsh {
            Some(rsh) => format!(" -e {}", shell_quote(rsh)),
            None => String::new(),
        }
    }
}

/// rsync's short flags for restoring: archive and hard links, plus ACLs and extended
/// attributes when the profile keeps them.
fn restore_flags(p: &Profile) -> String {
    let mut f = String::from("-aH");
    if p.options.preserve_acl {
        f.push('A');
    }
    if p.options.preserve_xattr {
        f.push('X');
    }
    f
}

/// Options for sftp (port is `-P` there), ending in a space when not empty.
fn sftp_opts(p: &Profile) -> String {
    let Target::Ssh {
        port,
        identity_file,
        ..
    } = &p.target
    else {
        return String::new();
    };
    let mut o = String::new();
    if let Some(port) = port {
        o.push_str(&format!("-P {port} "));
    }
    if let Some(i) = identity_file {
        o.push_str(&format!("-i {} ", shell_quote(&i.display().to_string())));
    }
    o
}

/// Options for sshfs, ending in a space when not empty.
fn sshfs_opts(p: &Profile) -> String {
    let Target::Ssh {
        port,
        identity_file,
        ..
    } = &p.target
    else {
        return String::new();
    };
    let mut o = String::new();
    if let Some(port) = port {
        o.push_str(&format!("-p {port} "));
    }
    if let Some(i) = identity_file {
        o.push_str(&format!(
            "-o IdentityFile={} ",
            shell_quote(&i.display().to_string())
        ));
    }
    o
}

/// An example path inside the first source, for the examples.
fn example(p: &Profile) -> (String, String) {
    let src = flux_fs::scan::normalize_sources(&p.sources.paths)
        .into_iter()
        .next()
        .map(|s| s.display().to_string())
        .unwrap_or_else(|| "/home/me".into());
    let src = src.trim_end_matches('/').to_owned();
    (format!("{src}/path/to/file"), src)
}

/// The recovery kit for a compatibility-mode profile, as Flux writes it next to the
/// backups (`None` for other engines).
pub fn for_profile(id: &ProfileId, p: &Profile) -> Option<Kit> {
    match p.engine {
        flux_core::Engine::Rsync => {
            let version = crate::tools::rsync_status()
                .version
                .unwrap_or_else(|| "(not installed)".into());
            Some(rsync_kit(id, p, &version, None))
        }
        flux_core::Engine::Borg => {
            let version = crate::tools::borg_status()
                .version
                .unwrap_or_else(|| "(not installed)".into());
            Some(borg_kit(id, p, &version, None))
        }
        _ => None,
    }
}

/// Where the kit is kept: the repository folder (a local path, or `user@host:path`).
pub fn location(id: &ProfileId, p: &Profile) -> String {
    if p.engine == flux_core::Engine::Borg {
        let folder = crate::borg::folder_location(id, p);
        return match &p.target {
            Target::Ssh { host, user, .. } => format!(
                "{}{host}:{folder}",
                user.as_deref().map(|u| format!("{u}@")).unwrap_or_default()
            ),
            Target::Local { .. } => folder,
        };
    }
    let place = Place::new(id, p);
    match place.remote {
        Some(r) => format!("{r}:{}", place.repo),
        None => place.repo,
    }
}

/// `newest`: the newest snapshot's id, used in the examples when known.
pub fn rsync_kit(id: &ProfileId, p: &Profile, rsync_version: &str, newest: Option<&str>) -> Kit {
    let p = &crate::script_safe(p);
    Kit {
        recovery: rsync_recovery(
            id,
            p,
            rsync_version,
            newest.unwrap_or("2026-01-31_18-00-00"),
        ),
        script: rsync_script(id, p),
    }
}

/// The options part of the backup command, as the script runs it (`$RULES`, `$LINK`,
/// `$DRY` and `$DEST` are shell variables).
fn script_rsync_line(p: &Profile) -> String {
    // ACLs and extended attributes go through $KEEP, which dry runs leave empty.
    let mut plain = p.clone();
    plain.options.preserve_acl = false;
    plain.options.preserve_xattr = false;
    let mut args = rsync::rsync_args(&ArgSpec {
        profile: &plain,
        rules_file: "@RULES@".into(),
        link_dest: None,
        rsh: rsync::plain_rsh(p),
        sources: Vec::new(),
        dest: String::new(),
        dry_run: false,
        live: false,
    });
    args.pop();
    let sources: Vec<String> = rsync::source_args(&p.sources.paths)
        .iter()
        .map(|s| shell_quote(s))
        .collect();
    format!(
        "{} ${{LINK:+\"--link-dest=$LINK\"}} ${{DRY:+--dry-run}} \\\n    {} \\\n    \"$DEST\"",
        rsync::shell_line("rsync", &args)
            .replacen("rsync ", "rsync $KEEP ", 1)
            .replace("@RULES@", "\"$RULES\""),
        sources.join(" ")
    )
}

fn rsync_recovery(id: &ProfileId, p: &Profile, rsync_version: &str, example_id: &str) -> String {
    let place = Place::new(id, p);
    let flags = restore_flags(p);
    let e = place.rsync_e();
    let (file, folder) = example(p);
    let file_rel = file.trim_start_matches('/');
    let folder_rel = folder.trim_start_matches('/');
    let file_dir = std::path::Path::new(&file)
        .parent()
        .map(|d| d.display().to_string())
        .unwrap_or_else(|| "/".into());
    let sources = flux_fs::scan::normalize_sources(&p.sources.paths)
        .iter()
        .map(|s| format!("  {}", s.display()))
        .collect::<Vec<_>>()
        .join("\n");
    let target = match &place.remote {
        Some(r) => format!("{} on the server {r}", place.repo),
        None => place.repo.clone(),
    };
    let list = place.there(&format!(
        "ls -1 {}",
        shell_quote(&format!("{}/{SNAPSHOTS}", place.repo))
    ));
    let browse = match &place.remote {
        Some(r) => format!(
            "  sftp {}{r}:{}\n  (or mount it: sshfs {}{r}:{} ~/flux-backups)",
            sftp_opts(p),
            shell_quote(&format!("{}/{LATEST}/data", place.repo)),
            sshfs_opts(p),
            shell_quote(&place.repo),
        ),
        None => format!("  Open {}/{LATEST}/data in any file manager.", place.repo),
    };
    let one_file = format!(
        "  rsync {flags}{e} {} {}",
        place.arg(&format!("{LATEST}/data/{file_rel}")),
        shell_quote(&format!("{file_dir}/"))
    );
    let one_folder = format!(
        "  rsync {flags}{e} --numeric-ids {} {}",
        place.arg(&format!("{LATEST}/data/{folder_rel}/")),
        shell_quote(&format!("{folder}/"))
    );
    // Each top folder of the backup (`data/home`, `data/etc`) into `/`, never `data/`
    // itself: rsync gives the destination folder the mode and owner of a source named
    // with a trailing slash, and `data` is a private folder of whoever made the backup.
    // With sudo, `rsync … data/ /` would make `/` that too, and lock everyone else out.
    let tops: Option<Vec<String>> = {
        let mut seen = Vec::new();
        flux_fs::scan::normalize_sources(&p.sources.paths)
            .iter()
            .map(|s| match s.components().nth(1) {
                Some(std::path::Component::Normal(c)) => Some(c.to_string_lossy().into_owned()),
                _ => None, // the backup is of `/` itself
            })
            .collect::<Option<Vec<String>>>()
            .map(|v| {
                for t in v {
                    if !seen.contains(&t) {
                        seen.push(t);
                    }
                }
                seen
            })
            .filter(|v| !v.is_empty())
    };
    let everything = match &tops {
        Some(tops) => {
            let from = tops
                .iter()
                .map(|t| place.arg(&format!("{LATEST}/data/{t}")))
                .collect::<Vec<_>>()
                .join(" ");
            format!(
                "  rsync {flags}{e} --numeric-ids --dry-run {from} /\n  rsync {flags}{e} --numeric-ids {from} /"
            )
        }
        // A backup of `/` itself has no top folders to name. `data/` it is, with the one
        // thing that must be put right afterwards.
        None => format!(
            "  rsync {flags}{e} --numeric-ids --dry-run {from} /\n  rsync {flags}{e} --numeric-ids {from} /\n  \
             Then give / its own permissions and owner back (rsync sets them from the\n  \
             backup's data folder, which is private to whoever made the backup):\n  \
             chmod 755 / && chown root:root /",
            from = place.arg(&format!("{LATEST}/data/"))
        ),
    };
    let elsewhere = format!(
        "  rsync {flags}{e} {} ~/restored/",
        place.arg(&format!("{SNAPSHOTS}/{example_id}/data/{folder_rel}"))
    );
    let delete = place.there(&format!(
        "chmod -R u+w {d} && rm -rf {d}",
        d = shell_quote(&format!("{}/{SNAPSHOTS}/2025-01-31_18-00-00", place.repo))
    ));
    let script_run = match &place.remote {
        Some(r) => format!(
            "  The script works from the computer being backed up. Copy it there first:\n\
             \x20   rsync{e} {} ./\n\
             \x20 then run:\n\
             \x20   sh {SCRIPT_FILE}            (or: sh {SCRIPT_FILE} --dry-run)\n\
             \x20 It needs rsync and ssh on this computer, and rsync on {r}. It does what\n\
             \x20 Flux does: unchanged files are shared with the newest snapshot, an\n\
             \x20 interrupted backup continues next time, and the new snapshot is named\n\
             \x20 after the date and time.",
            place.arg(SCRIPT_FILE)
        ),
        None => format!(
            "  sh {}            (or add --dry-run to only show what would change)\n\
             \x20 It needs nothing but rsync. It does what Flux does: unchanged files are\n\
             \x20 shared with the newest snapshot, an interrupted backup continues next\n\
             \x20 time, and the new snapshot is named after the date and time.",
            shell_quote(&format!("{}/{SCRIPT_FILE}", place.repo))
        ),
    };
    let caches = if p.sources.exclude_caches || !p.sources.exclude_if_present.is_empty() {
        let mut what = Vec::new();
        if p.sources.exclude_caches {
            what.push("CACHEDIR.TAG".to_owned());
        }
        what.extend(p.sources.exclude_if_present.iter().cloned());
        format!(
            "  Folders containing {} are left out (their contents; the folder stays).\n\
             \x20 rsync can't do this by itself, so Flux (and the script, with find) looks for\n\
             \x20 them first and adds them to the rules.\n",
            what.join(" or ")
        )
    } else {
        String::new()
    };
    let exclusions = if p.sources.exclude.is_empty() {
        "  (none)".to_owned()
    } else {
        p.sources
            .exclude
            .iter()
            .map(|x| format!("  {x}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let login = match &place.remote {
        Some(r) => format!(
            "\n  The backups are on another computer ({r}). The commands below reach it with\n\
             \x20 ssh, so you need the same login Flux used (an ssh key, or the password).\n"
        ),
        None => String::new(),
    };
    let easy = match &place.remote {
        Some(_) => format!(
            "  Your files are ordinary files and folders on the server. The easiest way to\n\
             \x20 get one back is to browse there (see BROWSE below), or to copy it with the\n\
             \x20 RESTORE ONE FILE command. The newest backup is in\n\
             \x20   {}/{LATEST}/data\n\
             \x20 where every file sits under its usual path.",
            place.repo
        ),
        None => format!(
            "  Your files are ordinary files and folders. To get one back, open\n\
             \x20   {}/{LATEST}/data\n\
             \x20 in your file manager, go to the file's usual place (for example\n\
             \x20 {file_dir}), and copy it out. That's all: no program, no password.\n\
             \x20 The commands below do the same from a terminal, for many files at once.",
            place.repo
        ),
    };
    let expert_cmd = rsync::shell_line(
        "rsync",
        &rsync::rsync_args(&ArgSpec {
            profile: p,
            rules_file: match &place.remote {
                // rsync reads the rules on this computer; Flux uses a local copy.
                Some(_) => RULES_FILE.into(),
                None => format!("{}/{SNAPSHOTS}/{PARTIAL}/{RULES_FILE}", place.repo),
            },
            link_dest: Some("../../<previous snapshot>/data".into()),
            rsh: place.rsh.clone(),
            sources: rsync::source_args(&p.sources.paths),
            dest: match &place.remote {
                Some(r) => format!("{r}:{}/{SNAPSHOTS}/{PARTIAL}/data/", place.repo),
                None => format!("{}/{SNAPSHOTS}/{PARTIAL}/data/", place.repo),
            },
            dry_run: false,
            live: false,
        }),
    );
    format!(
        "FLUX BACKUP \u{2014} RECOVERY GUIDE (rsync compatibility mode)
==============================================================

These are backups of \u{201c}{name}\u{201d}, made by Flux Backup in rsync compatibility mode.
Every snapshot is a plain folder written by rsync, a standard Linux copying tool.
You don't need Flux to get your files back or to keep making backups.

  Profile:   {name} ({id})
  Computer:  {host}, user {user}
  Stored in: {target}
  Written by Flux Backup {flux} with rsync {rsync_version}
  Backed up:
{sources}

Never used rsync or a terminal? Read IN SHORT and NEW TO RSYNC, then use the
commands below as they are. Know rsync well? Skip to FOR RSYNC USERS at the end.

IN SHORT
--------
{easy}

NEW TO RSYNC
------------
  rsync copies files and folders. Check that it's installed by typing
  rsync --version in a terminal; if it isn't, install the \u{201c}rsync\u{201d} package
  (sudo apt install rsync, sudo dnf install rsync, sudo pacman -S rsync).

  Each command below is one line: copy the whole line into a terminal and press
  Enter. Three things are worth knowing:
    * A slash at the end of a folder (folder/) means \u{201c}what's inside this folder\u{201d}.
      The commands below already have the slashes they need.
    * {flags} keeps everything about the files: permissions, dates, links and
      attributes.
    * --dry-run only shows what would happen. Add it first when you're unsure.
{login}
WHERE YOUR FILES ARE
--------------------
  snapshots/<date and time>/data/<the file's usual path>
  for example
    snapshots/{example_id}/data{file}

  latest points to the newest snapshot. Every snapshot is complete on its own.
  Files that didn't change are shared with the snapshot before (hard links), so
  they take space only once, and deleting one snapshot never damages another.

LIST THE SNAPSHOTS
------------------
  {list}

BROWSE
------
{browse}

RESTORE ONE FILE
----------------
{one_file}

RESTORE A FOLDER (to where it was)
----------------------------------
{one_folder}

  rsync replaces files that differ. To keep your current versions too, add
  --backup --suffix=.before-restore (they're renamed instead of replaced).
  rsync never deletes files that aren't in the backup unless you add --delete.

RESTORE EVERYTHING (every backed-up path to where it was)
--------------------------------------------------------
  Look first with --dry-run, then run it for real:
{everything}
  Put sudo in front to restore other users' files and file owners.

RESTORE AN OLDER VERSION, OR INTO ANOTHER FOLDER
------------------------------------------------
  Use snapshots/<date and time> instead of latest, and any destination:
{elsewhere}

MAKE A NEW BACKUP WITHOUT FLUX
------------------------------
{script_run}

WHAT IS LEFT OUT
----------------
  Exclusions from the profile:
{exclusions}
{caches}
DELETE OLD SNAPSHOTS
--------------------
  {delete}
  (Snapshot folders can be read-only; chmod makes them deletable.) If you delete
  the newest snapshot, latest points nowhere until the next backup.

THE FILES HERE
--------------
  {RECOVERY_FILE}   this guide (Flux rewrites it after every backup)
  {SCRIPT_FILE}       the backup, with rsync only
  flux-repo.toml       marks this folder as Flux's
  snapshots/           the backups; latest points to the newest
  In each snapshot: data/ (your files), rsync-command.txt (the exact command that
  wrote it), {RULES_FILE} (the exclusion rules it used) and meta.json (Flux's
  notes: statistics, names, pins; never needed to restore).

FOR RSYNC USERS
---------------
  Each backup is one rsync run into snapshots/{PARTIAL}, renamed to the date and time
  (local time, %Y-%m-%d_%H-%M-%S, _2, _3 within one second) when rsync exits 0, 23
  or 24, after which latest is repointed. An interrupted run leaves {PARTIAL} in place
  and the next run continues into it. The command (Flux adds --info=progress2 --stats
  --no-human-readable to follow it):

  {expert_cmd}

  * --relative with absolute sources: data/ mirrors /, so restores map 1:1.
  * {RULES_FILE} is first-match-wins. Flux translates the profile's gitignore-style
    patterns (last match wins) by reversing them; anchored patterns stay anchored
    to /, patterns with an inner slash are anchored, **/x becomes x.
  * Marker folders (CACHEDIR.TAG, exclude_if_present) are found before each run and
    written as \u{201c}- /path/*\u{201d} rules (the folder itself is kept).
  * Nothing else is Flux-specific: meta.json is optional, snapshots without it
    (made by the script or by hand) are listed by Flux, which adds a meta.json the
    next time it backs up. Flux restores from these folders with its own restore
    (identical files skipped, differing ones kept aside); the rsync commands above
    are the equivalent.
",
        name = p.name,
        host = p.kept_as(id).host,
        user = p.kept_as(id).user,
        flux = flux_core::VERSION,
    )
}

fn rsync_script(id: &ProfileId, p: &Profile) -> String {
    let place = Place::new(id, p);
    let rules = RuleSet::new(
        &p.sources,
        sys::home_dir().as_deref(),
        rsync::never_paths(p),
    )
    .render(&format!(
        "Flux Backup: exclusion rules for \u{201c}{}\u{201d} ({id})",
        p.name
    ));
    let marker_names: Vec<String> = p
        .sources
        .exclude_caches
        .then(|| "CACHEDIR.TAG".to_owned())
        .into_iter()
        .chain(p.sources.exclude_if_present.iter().cloned())
        .collect();
    let markers = if marker_names.is_empty() {
        String::new()
    } else {
        let names = marker_names
            .iter()
            .map(|n| format!("-name {}", shell_quote(n)))
            .collect::<Vec<_>>()
            .join(" -o ");
        let xdev = if p.sources.one_file_system {
            " -xdev"
        } else {
            ""
        };
        let roots = flux_fs::scan::normalize_sources(&p.sources.paths)
            .iter()
            .map(|s| shell_quote(&s.display().to_string()))
            .collect::<Vec<_>>()
            .join(" ");
        format!(
            r#"
# Folders marked as caches (CACHEDIR.TAG) or with a no-backup marker: leave their
# contents out, as Flux does. rsync can't do this by itself.
find {roots}{xdev} -type f \( {names} \) 2>/dev/null | while IFS= read -r f; do
    if [ "$(basename "$f")" = CACHEDIR.TAG ]; then
        head -c 43 "$f" 2>/dev/null | grep -q '^Signature: 8a477f597d28d172789f06886806bc55' || continue
    fi
    printf -- '- %s/*\n' "$(dirname "$f" | sed 's/[][*?\\]/\\&/g')"
done > "$RULES.markers"
cat "$RULES.markers" "$RULES.profile" > "$RULES"
rm -f "$RULES.markers"
"#
        )
    };
    let no_markers = if marker_names.is_empty() {
        "cp \"$RULES.profile\" \"$RULES\"\n"
    } else {
        ""
    };
    let (setup, prev, finish) = match &place.remote {
        None => (
            format!(
                "REPO={}\nmkdir -p \"$REPO/{SNAPSHOTS}/{PARTIAL}/data\"\nRULES=\"$REPO/{SNAPSHOTS}/{PARTIAL}/{RULES_FILE}\"\nDEST=\"$REPO/{SNAPSHOTS}/{PARTIAL}/data/\"\n",
                shell_quote(&place.repo)
            ),
            format!("PREV=$(ls -1 \"$REPO/{SNAPSHOTS}\" | grep -v '^\\.' | sort | tail -n 1)\n"),
            format!(
                "ID=$BASE\nN=2\nwhile [ -e \"$REPO/{SNAPSHOTS}/$ID\" ]; do ID=\"${{BASE}}_$N\"; N=$((N + 1)); done\nmv \"$REPO/{SNAPSHOTS}/{PARTIAL}\" \"$REPO/{SNAPSHOTS}/$ID\"\nln -sfn \"{SNAPSHOTS}/$ID\" \"$REPO/{LATEST}\"\necho \"Created snapshot $REPO/{SNAPSHOTS}/$ID\"\n"
            ),
        ),
        Some(remote) => {
            let rsh = place.rsh.clone().unwrap_or_else(|| "ssh".into());
            (
                format!(
                    "SERVER={}\nREPO={}\nSSH={}\nWORK=$(mktemp -d)\ntrap 'rm -rf \"$WORK\"' EXIT\nRULES=\"$WORK/{RULES_FILE}\"\nDEST=\"$SERVER:$REPO/{SNAPSHOTS}/{PARTIAL}/data/\"\n$SSH \"$SERVER\" \"mkdir -p '$REPO/{SNAPSHOTS}/{PARTIAL}/data'\"\n",
                    shell_quote(remote),
                    shell_quote(&place.repo),
                    shell_quote(&rsh),
                ),
                format!(
                    "PREV=$($SSH \"$SERVER\" \"ls -1 '$REPO/{SNAPSHOTS}'\" | grep -v '^\\.' | sort | tail -n 1)\n"
                ),
                format!(
                    "rsync -e \"$SSH\" \"$RULES\" \"$SERVER:$REPO/{SNAPSHOTS}/{PARTIAL}/{RULES_FILE}\"\nID=$($SSH \"$SERVER\" \"cd '$REPO/{SNAPSHOTS}' && I=$BASE N=2 && while [ -e \\\"\\$I\\\" ]; do I=${{BASE}}_\\$N; N=\\$((N + 1)); done && mv {PARTIAL} \\\"\\$I\\\" && ln -sfn \\\"{SNAPSHOTS}/\\$I\\\" ../{LATEST} && echo \\\"\\$I\\\"\")\necho \"Created snapshot $SERVER:$REPO/{SNAPSHOTS}/$ID\"\n"
                ),
            )
        }
    };
    let rsync_line = script_rsync_line(p);
    let rules_tag = crate::heredoc_tag("FLUX_RULES", &rules);
    format!(
        r#"#!/bin/sh
# Flux Backup recovery kit: back up "{name}" ({id}) with plain rsync, without Flux.
#
#   sh {SCRIPT_FILE}             make a new snapshot
#   sh {SCRIPT_FILE} --dry-run   only show what would change
#
# It does what Flux does: hard-link unchanged files against the newest snapshot
# (--link-dest), write into snapshots/{PARTIAL} (continued next time if interrupted),
# then rename it to the date and time and point "latest" at it.
# Generated by Flux Backup {flux}. FLUX-RECOVERY.txt explains everything else.
set -eu

DRY=
KEEP={keep}
if [ "${{1:-}}" = "--dry-run" ]; then DRY=1; KEEP=; fi
{setup}
# Exclusion rules (rsync filter syntax; the first matching rule wins).
cat > "$RULES.profile" <<'{rules_tag}'
{rules}{rules_tag}
{markers}{no_markers}rm -f "$RULES.profile"

# The newest snapshot: unchanged files become hard links to it.
{prev}LINK=
[ -n "$PREV" ] && LINK="../../$PREV/data"

set +e
{rsync_line}
RC=$?
set -e
case $RC in
    0) ;;
    23|24) echo "Some files could not be copied (rsync exit $RC); the snapshot is still usable." >&2 ;;
    *) echo "rsync failed (exit $RC). The unfinished snapshot is kept in snapshots/{PARTIAL}; run this again to continue it." >&2
       exit "$RC" ;;
esac
[ -n "$DRY" ] && exit 0

# A new name from the date and time, made unique like Flux does (_2, _3, …).
BASE=$(date +%Y-%m-%d_%H-%M-%S)
{finish}"#,
        name = p.name.replace('"', "'"),
        flux = flux_core::VERSION,
        keep = shell_quote(
            &[
                p.options.preserve_acl.then_some("--acls"),
                p.options.preserve_xattr.then_some("--xattrs"),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ")
        ),
    )
}

/// `BORG_PASSCOMMAND` for the profile's password source (so borg reads the passphrase
/// where Flux does), or `None` when borg should ask (or no passphrase is needed).
fn borg_passcommand(id: &ProfileId, p: &Profile) -> Option<String> {
    use flux_core::profile::PasswordSource;
    match p.password_source()? {
        PasswordSource::Keyring => Some(format!(
            "secret-tool lookup service flux-backup profile {}",
            id
        )),
        PasswordSource::File { path } => Some(format!(
            "head -n 1 {}",
            shell_quote(&path.display().to_string())
        )),
        PasswordSource::Command { command } => Some(format!("{command} | head -n 1")),
        PasswordSource::Prompt => None,
    }
}

/// `BORG_RSH` when ssh needs more than the URL says (a key file, options).
fn borg_rsh(p: &Profile) -> Option<String> {
    let Target::Ssh {
        identity_file,
        ssh_options,
        ..
    } = &p.target
    else {
        return None;
    };
    if identity_file.is_none() && ssh_options.is_empty() {
        return None;
    }
    let mut a = vec!["ssh".to_owned()];
    if let Some(i) = identity_file {
        a.extend(["-i".into(), i.display().to_string()]);
    }
    for o in ssh_options {
        a.extend(["-o".into(), o.clone()]);
    }
    Some(
        a.iter()
            .map(|s| shell_quote(s))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// `borg prune` options with the same effect as the profile's retention (as far as Borg
/// can express it).
fn borg_prune_args(p: &Profile) -> String {
    let r = &p.retention;
    let mut a = Vec::new();
    if r.keep_last > 0 {
        a.push(format!("--keep-last {}", r.keep_last));
    }
    if let Some(w) = r.keep_within {
        let secs = w.0.as_secs();
        a.push(if secs % 86_400 == 0 {
            format!("--keep-within {}d", secs / 86_400)
        } else {
            format!("--keep-within {}H", secs.div_ceil(3600))
        });
    }
    for (n, what) in [
        (r.keep_hourly, "hourly"),
        (r.keep_daily, "daily"),
        (r.keep_weekly, "weekly"),
        (r.keep_monthly, "monthly"),
        (r.keep_yearly, "yearly"),
    ] {
        if n > 0 {
            a.push(format!("--keep-{what} {n}"));
        }
    }
    if a.is_empty() {
        a.push("--keep-last 1".into());
    }
    a.join(" ")
}

/// `newest`: the newest archive's name, used in the examples when known.
pub fn borg_kit(id: &ProfileId, p: &Profile, borg_version: &str, newest: Option<&str>) -> Kit {
    use crate::borg;
    let p = &crate::script_safe(p);
    let opts = p.borg.clone().unwrap_or_default();
    let url = borg::repo_url(id, p, true);
    let folder = borg::folder_location(id, p);
    let example_id = newest.unwrap_or("2026-01-31_18-00-00");
    let (file, src) = example(p);
    let file_rel = file.trim_start_matches('/');
    let src_rel = src.trim_start_matches('/');
    let encrypted = opts.encryption != flux_core::profile::BorgEncryption::None;
    let passcommand = borg_passcommand(id, p);
    let rsh = borg_rsh(p);
    let remote = match &p.target {
        Target::Ssh { host, user, .. } => Some(match user {
            Some(u) => format!("{u}@{host}"),
            None => host.clone(),
        }),
        Target::Local { .. } => None,
    };

    let mut setup = vec![format!("export BORG_REPO={}", shell_quote(&url))];
    if let Some(pc) = &passcommand {
        setup.push(format!("export BORG_PASSCOMMAND={}", shell_quote(pc)));
    }
    if let Some(r) = &rsh {
        setup.push(format!("export BORG_RSH={}", shell_quote(r)));
    }
    if let Some(r) = &opts.remote_path {
        setup.push(format!("export BORG_REMOTE_PATH={}", shell_quote(r)));
    }
    let setup_lines = setup
        .iter()
        .map(|l| format!("  {l}"))
        .collect::<Vec<_>>()
        .join("\n");
    let passphrase_note = if !encrypted {
        "  This repository isn't encrypted: no passphrase is needed.".to_owned()
    } else {
        match p.password_source() {
            Some(flux_core::profile::PasswordSource::Keyring) => format!(
                "  The passphrase is the one you chose in Flux. Flux keeps it in your desktop\n\
                 \x20 keyring; the BORG_PASSCOMMAND line above lets borg read it from there\n\
                 \x20 (secret-tool, from libsecret). Without it, leave that line out and borg\n\
                 \x20 asks you to type the passphrase. Show it with:\n\
                 \x20   secret-tool lookup service flux-backup profile {id}"
            ),
            Some(flux_core::profile::PasswordSource::Prompt) | None => {
                "  The passphrase is the one you chose in Flux; borg asks for it.".to_owned()
            }
            _ => "  The passphrase is the one you chose in Flux. The BORG_PASSCOMMAND line above\n\
                  \x20 reads it the same way Flux does; leave it out and borg asks you instead."
                .to_owned(),
        }
    };
    let server_note = match &remote {
        Some(r) => format!(
            "\n  The repository is on another computer ({r}). borg reaches it with ssh, so you\n\
             \x20 need the same login Flux used (an ssh key, or the password), and borg must be\n\
             \x20 installed on {r} too.\n"
        ),
        None => String::new(),
    };
    let sources = flux_fs::scan::normalize_sources(&p.sources.paths)
        .iter()
        .map(|s| format!("  {}", s.display()))
        .collect::<Vec<_>>()
        .join("\n");
    let exclusions = if p.sources.exclude.is_empty() {
        "  (none)".to_owned()
    } else {
        p.sources
            .exclude
            .iter()
            .map(|x| format!("  {x}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let mut markers = Vec::new();
    if p.sources.exclude_caches {
        markers.push("folders with a CACHEDIR.TAG (--exclude-caches)".to_owned());
    }
    for m in &p.sources.exclude_if_present {
        markers.push(format!("folders containing {m} (--exclude-if-present)"));
    }
    let markers = if markers.is_empty() {
        String::new()
    } else {
        format!("  Also left out: {}.\n", markers.join(", "))
    };
    let patterns_path = match &remote {
        Some(_) => PATTERNS_REF.to_owned(),
        None => format!("{folder}/{}", borg::PATTERNS_FILE),
    };
    let mut create = vec!["create".to_owned(), "--stats".into()];
    create.extend(borg::create_options(p, &opts, &patterns_path));
    create.push("::{now:%Y-%m-%d_%H-%M-%S}".into());
    create.extend(
        flux_fs::scan::normalize_sources(&p.sources.paths)
            .iter()
            .map(|s| s.display().to_string()),
    );
    let create_line = rsync::shell_line("borg", &create);
    let script_where = match &remote {
        Some(_) => format!(
            "  The script works from the computer being backed up. Copy it there first:\n\
             \x20   scp {}:{} ./\n\
             \x20 then run:\n\
             \x20   sh {SCRIPT_FILE}            (or: sh {SCRIPT_FILE} --dry-run)",
            remote.as_deref().unwrap_or_default(),
            shell_quote(&format!("{folder}/{SCRIPT_FILE}"))
        ),
        None => format!(
            "  sh {}            (or add --dry-run to only show what would change)",
            shell_quote(&format!("{folder}/{SCRIPT_FILE}"))
        ),
    };
    let key_export = if encrypted {
        format!(
            "KEEP A COPY OF THE KEY
----------------------
  The encryption key is stored inside the repository, locked by your passphrase.
  If the repository's key were ever damaged, nothing could be decrypted, so keep a
  copy somewhere else (it's useless without the passphrase):
    borg key export :: ~/flux-borg-key-{id}.txt
    borg key export --paper :: > ~/flux-borg-key-{id}-paper.txt    (to print)

"
        )
    } else {
        String::new()
    };
    let recovery = format!(
        "FLUX BACKUP \u{2014} RECOVERY GUIDE (Borg compatibility mode)
=============================================================

These are backups of \u{201c}{name}\u{201d}, made by Flux Backup in Borg compatibility mode.
They're stored in a standard Borg repository, written by borg itself. You don't
need Flux to get your files back or to keep making backups: borg is enough.

  Profile:     {name} ({id})
  Computer:    {host}, user {user}
  Repository:  {url}
  Encryption:  {enc}        Compression: {comp}
  Written by Flux Backup {flux} with Borg {borg_version}
  Backed up:
{sources}

Never used Borg? Read IN SHORT and NEW TO BORG, then use the commands below as
they are. Know Borg well? Skip to FOR BORG USERS at the end.

IN SHORT
--------
  Your files are inside the repository folder above, in Borg's own format
  ({enc_short}), so you can't open them there directly. To get them back you
  need the program borg{and_pass}, then:
    1. open a terminal and paste the lines under SET UP,
    2. type   borg list                    to see your backups (archives),
    3. mount them like a folder (BROWSE) or extract what you need (RESTORE).

NEW TO BORG
-----------
  borg is a free backup program in every Linux distribution. Check that it's
  installed by typing  borg --version ; if it isn't, install the \u{201c}borgbackup\u{201d}
  package (sudo apt install borgbackup, sudo dnf install borgbackup,
  sudo pacman -S borg).

  * A repository is the folder that holds all your backups.
  * Each backup is an \u{201c}archive\u{201d}, named after the date and time it was made.
    In commands, ::NAME means \u{201c}archive NAME in the repository\u{201d}.
  * borg extract writes files into the folder you're in, under their full path
    (home/me/...), so cd to the right place first.
  * --dry-run --list shows what would happen without changing anything.

  Each command below is one line: copy the whole line into a terminal and press
  Enter.
{server_note}
SET UP (once per terminal window)
-------------------------------
{setup_lines}

{passphrase_note}

LIST THE BACKUPS
----------------
  borg list                              every archive
  borg list --last 1 --short             the newest one's name
  borg list ::{example_id}     the files in one archive

BROWSE
------
  mkdir -p ~/flux-backups && borg mount :: ~/flux-backups
  Every archive shows up as a folder; copy what you need with any file manager.
  When you're done:  borg umount ~/flux-backups
  (Mounting needs borg's FUSE support, e.g. the python3-llfuse or python3-pyfuse3
  package. If it's missing, extract instead.)

RESTORE ONE FILE
----------------
  mkdir -p ~/restored && cd ~/restored && borg extract ::{example_id} {file_rel}
  The file is now in ~/restored/{file_rel}; copy it wherever you like.

RESTORE A FOLDER (to where it was)
----------------------------------
  cd / && borg extract --dry-run --list ::{example_id} {src_rel}
  cd / && borg extract ::{example_id} {src_rel}
  borg replaces files that differ and doesn't delete anything else. To keep your
  current versions, restore into another folder instead (as above) and compare.

RESTORE EVERYTHING (every backed-up path to where it was)
--------------------------------------------------------
  cd / && sudo --preserve-env=BORG_REPO,BORG_RSH borg extract --list ::{example_id}
  sudo is needed for other users' files and file owners; borg asks for the
  passphrase.

  To use the newest archive whatever its name:
    A=$(borg list --last 1 --short) && borg list \"::$A\"

{key_export}MAKE A NEW BACKUP WITHOUT FLUX
------------------------------
{script_where}
  It runs the same borg create as Flux, with the same exclusions, and names the
  archive after the date and time. It needs nothing but borg.

WHAT IS LEFT OUT
----------------
  Exclusions from the profile:
{exclusions}
{markers}
DELETE OLD BACKUPS
------------------
  borg delete ::2025-01-31_18-00-00      one archive
  borg compact                           then free the space
  Or let borg choose, with rules like Flux's for this profile:
    borg prune --list --dry-run {prune}
    borg prune --list {prune} && borg compact
  borg prune doesn't know Flux's pinned and named snapshots; check with --dry-run.

CHECK THE BACKUPS
-----------------
  borg check                             structure (quick)
  borg check --verify-data               read and verify every byte (slow)

THE FILES HERE ({folder})
------------------------
  {RECOVERY_FILE}   this guide (Flux rewrites it after every backup)
  {SCRIPT_FILE}       the backup, with borg only
  {patterns}    the exclusion patterns, in borg's syntax
  repo/                the Borg repository

FOR BORG USERS
--------------
  Standard Borg 1.x repository: borg init --encryption={enc} --make-parent-dirs.
  One archive per backup (also when nothing changed), named %Y-%m-%d_%H-%M-%S in
  local time (_2 within one second). Flux's backup command (it adds --json --progress --list
  --filter=AME --log-json and a --comment to follow the run):

  {create_line}

  * Patterns: flux-patterns.txt, \"P sh\", first match wins. The profile's gitignore
    patterns are reversed; exclusions use \"!\" (don't recurse) because gitignore
    never looks inside excluded folders; name patterns become **/name; borg can't
    tell folders from files, so a trailing / is dropped.
  * Archive comments hold Flux's notes as JSON: {{\"flux\":1,\"name\":\"\u{2026}\",\"pinned\":true}}
    (set with borg recreate --comment). Archives without it are listed by Flux too.
  * Flux runs borg with BORG_RELOCATED_REPO_ACCESS_IS_OK=yes,
    BORG_UNKNOWN_UNENCRYPTED_REPO_ACCESS_IS_OK=yes, BORG_PASSPHRASE from its own
    password source, and BORG_RSH for ssh options. Your ~/.config/borg and
    ~/.cache/borg are shared with Flux.
  * Restores from Flux extract into a temporary folder, then go through Flux's own
    restore (identical files skipped, differing ones kept aside); borg extract is
    the equivalent. Retention: Flux's policy, applied with borg delete + compact.
",
        name = p.name,
        host = p.kept_as(id).host,
        user = p.kept_as(id).user,
        enc = opts.encryption.as_arg(),
        enc_short = if encrypted {
            "compressed, deduplicated and encrypted"
        } else {
            "compressed and deduplicated"
        },
        and_pass = if encrypted {
            " and your passphrase"
        } else {
            ""
        },
        comp = opts.compression,
        flux = flux_core::VERSION,
        prune = borg_prune_args(p),
        patterns = borg::PATTERNS_FILE,
    );

    // The script.
    let mut env = vec![format!("export BORG_REPO={}", shell_quote(&url))];
    if let Some(pc) = &passcommand {
        env.push(format!(
            "[ -n \"${{BORG_PASSPHRASE:-}}${{BORG_PASSCOMMAND:-}}\" ] || export BORG_PASSCOMMAND={}",
            shell_quote(pc)
        ));
    }
    if let Some(r) = &rsh {
        env.push(format!("export BORG_RSH={}", shell_quote(r)));
    }
    if let Some(r) = &opts.remote_path {
        env.push(format!("export BORG_REMOTE_PATH={}", shell_quote(r)));
    }
    let pattern_text = borg::patterns(
        p,
        &format!(
            "Flux Backup: exclusion patterns for \u{201c}{}\u{201d} ({id})",
            p.name
        ),
        &borg::never_paths(p),
    );
    let patterns_tag = crate::heredoc_tag("FLUX_PATTERNS", &pattern_text);
    let mut opts_only = borg::create_options(p, &opts, "@PATTERNS@");
    opts_only.insert(0, "create".into());
    let script_create = format!(
        "{} $MODE \\\n    '::{{now:%Y-%m-%d_%H-%M-%S}}' \\\n    {}",
        rsync::shell_line("borg", &opts_only).replace(
            "--patterns-from=@PATTERNS@",
            "--patterns-from=\"$PATTERNS\""
        ),
        flux_fs::scan::normalize_sources(&p.sources.paths)
            .iter()
            .map(|s| shell_quote(&s.display().to_string()))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let script = format!(
        r#"#!/bin/sh
# Flux Backup recovery kit: back up "{name}" ({id}) with plain borg, without Flux.
#
#   sh {SCRIPT_FILE}             make a new archive
#   sh {SCRIPT_FILE} --dry-run   only show what would be backed up
#
# Generated by Flux Backup {flux}. FLUX-RECOVERY.txt explains everything else.
set -eu

{env}

# --dry-run only shows what would be backed up; otherwise borg prints statistics.
MODE=--stats
if [ "${{1:-}}" = "--dry-run" ]; then MODE="--dry-run --list"; fi

# Exclusion patterns (borg --patterns-from syntax; the first matching pattern wins).
PATTERNS=$(mktemp)
trap 'rm -f "$PATTERNS"' EXIT
cat > "$PATTERNS" <<'{patterns_tag}'
{pattern_text}{patterns_tag}

set +e
{script_create}
RC=$?
set -e
case $RC in
    0) ;;
    1) echo "borg finished with warnings (some files could not be read); the archive is usable." >&2 ;;
    *) echo "borg failed (exit $RC)." >&2
       exit "$RC" ;;
esac
"#,
        name = p.name.replace('"', "'"),
        flux = flux_core::VERSION,
        env = env.join("\n"),
    );
    Kit { recovery, script }
}

/// How the guide refers to the patterns file when it's on a server.
const PATTERNS_REF: &str = "flux-patterns.txt";

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(target: Target) -> Profile {
        let mut p = Profile::new("Home", target, vec!["/home/me".into()]);
        p.engine = flux_core::Engine::Rsync;
        p.sources.exclude = vec!["*.tmp".into()];
        p
    }

    #[test]
    fn local_kit_mentions_the_right_commands() {
        let id = ProfileId::new("home").unwrap();
        let p = profile(Target::Local {
            path: "/mnt/backup".into(),
            fs_uuid: None,
        });
        let k = rsync_kit(&id, &p, "3.4.1", None);
        let repo = rsync::repo_location(&id, &p);
        assert!(k.recovery.contains(&format!("{repo}/latest/data/home/me/")));
        assert!(k.recovery.contains("rsync -aHAX"));
        // "Restore everything" names the backup's top folders, never `data/` itself:
        // with sudo that would give `/` the mode and owner of the private data folder.
        assert!(
            k.recovery
                .contains(&format!("--numeric-ids {repo}/latest/data/home /\n")),
            "{}",
            k.recovery
        );
        assert!(!k.recovery.contains("latest/data/ /"));
        let mut whole = p.clone();
        whole.sources.paths = vec!["/".into()];
        let w = rsync_kit(&id, &whole, "3.4.1", None).recovery;
        assert!(w.contains("latest/data/ /") && w.contains("chmod 755 / && chown root:root /"));
        assert!(k.recovery.contains("rsync 3.4.1"));
        assert!(k.script.contains("--link-dest=$LINK"));
        assert!(k.script.contains("- *.tmp\n"));
        assert!(k.script.contains("CACHEDIR.TAG"));
        assert!(k.script.contains(&format!("REPO={repo}")));
    }

    #[test]
    fn borg_kit_maps_settings() {
        let id = ProfileId::new("docs").unwrap();
        let mut p = profile(Target::Local {
            path: "/mnt/backup".into(),
            fs_uuid: None,
        });
        p.engine = flux_core::Engine::Borg;
        p.borg = Some(flux_core::profile::BorgOptions::default());
        let k = borg_kit(&id, &p, "1.4.5", Some("2026-11-07_10-00-00"));
        assert!(
            k.recovery
                .contains("export BORG_REPO=/mnt/backup/flux-borg/")
        );
        assert!(
            k.recovery
                .contains("BORG_PASSCOMMAND='secret-tool lookup service flux-backup profile docs'")
        );
        assert!(k.recovery.contains("--keep-within 1d --keep-daily 7"));
        assert!(k.recovery.contains("borg extract ::2026-11-07_10-00-00"));
        assert!(k.recovery.contains("borg key export"));
        assert!(k.script.contains("--patterns-from=\"$PATTERNS\""));
        assert!(k.script.contains("! **/*.tmp"));
        assert!(k.script.contains("--exclude-caches"));
    }

    #[test]
    fn ssh_kit_uses_the_server() {
        let id = ProfileId::new("home").unwrap();
        let p = profile(Target::Ssh {
            host: "nas".into(),
            user: Some("me".into()),
            port: Some(2222),
            path: "/srv/backup".into(),
            identity_file: None,
            ssh_options: vec![],
            bandwidth_limit: None,
        });
        let k = rsync_kit(&id, &p, "3.4.1", None);
        assert!(k.recovery.contains("-e 'ssh -p 2222' me@nas:"));
        assert!(k.recovery.contains("sftp -P 2222 me@nas:"));
        assert!(k.script.contains("SERVER=me@nas"));
        assert!(k.script.contains("SSH='ssh -p 2222'"));
    }
}
