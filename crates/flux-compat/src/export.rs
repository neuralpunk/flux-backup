//! Exporting a profile: as a Flux profile (importable again), or as the configuration of
//! another backup tool where a faithful translation exists. Whatever a format can't
//! express is listed, in the file as comments and in [`Export::notes`], never dropped
//! silently.

use serde::Serialize;

use flux_core::profile::{
    ChangeDetection, Engine, PasswordSource, Profile, Retention, SymlinkMode, Target,
};
use flux_core::{ProfileId, sys};
use flux_transport::shell_quote;

use crate::{borg, kit};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// A Flux profile (TOML), importable with `flux profile import`.
    Flux,
    /// A borgmatic configuration (YAML) for Borg.
    Borgmatic,
    /// A restic backup script with an exclude file.
    Restic,
    /// A shell script that runs the backup with rsync.
    Rsync,
    /// A shell script that runs the backup with borg.
    Borg,
}

impl Format {
    pub const ALL: [Format; 5] = [
        Format::Flux,
        Format::Borgmatic,
        Format::Restic,
        Format::Rsync,
        Format::Borg,
    ];

    pub fn parse(s: &str) -> Option<Format> {
        Format::ALL.into_iter().find(|f| f.info().id == s)
    }

    pub fn info(self) -> FormatInfo {
        let (id, label, description, works_with) = match self {
            Format::Flux => (
                "flux",
                "Flux Backup profile",
                "This profile as a file, to import on another computer or keep as a copy.",
                "Flux Backup (flux profile import)",
            ),
            Format::Borgmatic => (
                "borgmatic",
                "borgmatic configuration",
                "The same backup as a borgmatic config: Borg with schedules, retention and checks.",
                "borgmatic 1.8 or newer, and Borg",
            ),
            Format::Restic => (
                "restic",
                "restic script",
                "A script that runs the same backup with restic, including retention.",
                "restic 0.16 or newer",
            ),
            Format::Rsync => (
                "rsync",
                "rsync script",
                "A script that runs the same backup with rsync: plain-folder snapshots.",
                "rsync 3.1 or newer",
            ),
            Format::Borg => (
                "borg",
                "Borg script",
                "A script that runs the same backup with borg create.",
                "Borg 1.2 to 1.4",
            ),
        };
        FormatInfo {
            id,
            label,
            description,
            works_with,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FormatInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub works_with: &'static str,
}

/// An exported profile.
#[derive(Debug, Clone, Serialize)]
pub struct Export {
    pub format: Format,
    /// A good file name for it.
    pub filename: String,
    pub contents: String,
    /// The file is a script to run.
    pub executable: bool,
    /// What couldn't be carried over.
    pub notes: Vec<String>,
}

pub fn export(id: &ProfileId, p: &Profile, format: Format) -> Export {
    match format {
        Format::Flux => flux(id, p),
        Format::Borgmatic => borgmatic(id, p),
        Format::Restic => restic(id, p),
        Format::Rsync => {
            let mut q = p.clone();
            q.engine = Engine::Rsync;
            q.vault = None;
            q.borg = None;
            let version = crate::tools::rsync_status()
                .version
                .unwrap_or_else(|| "3.x".into());
            let k = kit::rsync_kit(id, &q, &version, None);
            Export {
                format,
                filename: format!("{id}-rsync-backup.sh"),
                contents: k.script,
                executable: true,
                notes: common_notes(p),
            }
        }
        Format::Borg => {
            let mut q = p.clone();
            q.engine = Engine::Borg;
            q.vault = None;
            q.rsync = None;
            if q.borg.is_none() {
                q.borg = Some(borg_options_from(p));
            }
            let version = crate::tools::borg_status()
                .version
                .unwrap_or_else(|| "1.x".into());
            let k = kit::borg_kit(id, &q, &version, None);
            let mut notes = common_notes(p);
            if p.engine != Engine::Borg {
                notes.push(format!(
                    "The repository {} doesn't exist yet: create it once with borg init --make-parent-dirs --encryption={} (the script doesn't).",
                    borg::repo_url(id, &q, true),
                    q.borg.as_ref().map_or("repokey-blake2", |b| b.encryption.as_arg())
                ));
            }
            notes.extend(borg_limits(p));
            Export {
                format,
                filename: format!("{id}-borg-backup.sh"),
                contents: k.script,
                executable: true,
                notes,
            }
        }
    }
}

/// Borg settings for a profile that isn't a Borg profile: encrypted unless it was a
/// plain-folder one, with its password source.
fn borg_options_from(p: &Profile) -> flux_core::profile::BorgOptions {
    let mut b = flux_core::profile::BorgOptions::default();
    if let Some(src) = p.password_source() {
        b.password_source = src;
    }
    b
}

fn header(id: &ProfileId, p: &Profile, comment: &str, what: &str) -> String {
    format!(
        "{comment} {what} of the Flux Backup profile \u{201c}{}\u{201d} ({id}),\n{comment} exported by Flux Backup {} on {} for user {}.\n",
        p.name,
        flux_core::VERSION,
        sys::hostname(),
        sys::username()
    )
}

fn flux(id: &ProfileId, p: &Profile) -> Export {
    // Webhooks stay behind: their addresses and headers can hold secrets.
    let mut q = p.clone();
    let mut notes = Vec::new();
    if !q.webhooks.is_empty() {
        q.webhooks.clear();
        notes.push("Webhooks aren't exported (their addresses and headers can hold secrets); add them again after importing.".into());
    }
    let body = q
        .to_toml()
        .unwrap_or_else(|e| format!("# could not serialize: {e}\n"));
    Export {
        format: Format::Flux,
        filename: format!("{id}.flux-profile.toml"),
        contents: format!(
            "{}# Import it with:  flux profile import {id}.flux-profile.toml\n# (or in the app: + next to \u{201c}Your backups\u{201d} › Import a profile…)\n# flux-profile-id = \"{id}\"\n\n{body}",
            header(id, p, "#", "Export")
        ),
        executable: false,
        notes,
    }
}

/// What every non-Flux format loses.
fn common_notes(p: &Profile) -> Vec<String> {
    let mut n = Vec::new();
    n.push(
        "Pinned and named snapshots are a Flux feature; other tools' retention may remove them."
            .into(),
    );
    if !p.destinations.is_empty() {
        let names: Vec<&str> = p.destinations.iter().map(|d| d.label()).collect();
        n.push(format!(
            "Only the main destination is exported; the other places backups go ({}) aren't.",
            names.join(", ")
        ));
    }
    if p.retention.max_age.is_some() {
        n.push("\u{201c}Never keep longer than\u{201d} (max_age) has no equivalent.".into());
    }
    if p.retention.min_free_space.is_some() || p.retention.min_free_inodes_percent.is_some() {
        n.push("Free-space floors (min_free_space) have no equivalent.".into());
    }
    if !matches!(p.schedule.trigger, flux_core::profile::Trigger::Manual) || p.schedule.on_connect {
        n.push(format!(
            "The schedule ({}) isn't part of the export: run it from cron or a systemd timer.",
            schedule_words(p)
        ));
    }
    if p.schedule.skip_on_battery {
        n.push("\u{201c}Wait for mains power\u{201d} isn't carried over.".into());
    }
    n
}

fn schedule_words(p: &Profile) -> String {
    let s = &p.schedule;
    let mut w = match s.trigger {
        flux_core::profile::Trigger::Manual => "manual".to_owned(),
        flux_core::profile::Trigger::Calendar => {
            format!("at {}", s.on_calendar.as_deref().unwrap_or("?"))
        }
        flux_core::profile::Trigger::Interval => format!(
            "every {}",
            s.every.map(|e| e.to_string()).unwrap_or_default()
        ),
    };
    if s.on_connect {
        w.push_str(", and when the drive is connected");
    }
    w
}

fn borg_limits(p: &Profile) -> Vec<String> {
    let mut n = Vec::new();
    if p.sources.exclude_larger_than.is_some() {
        n.push("Borg can't skip files by size (exclude_larger_than).".into());
    }
    if p.options.symlinks != SymlinkMode::Preserve {
        n.push("Borg always stores symbolic links as links.".into());
    }
    n
}

/// borgmatic's `encryption_passcommand` (or restic's `RESTIC_PASSWORD_COMMAND`) for a
/// password source.
fn passcommand(id: &ProfileId, src: &PasswordSource) -> Option<String> {
    match src {
        PasswordSource::Keyring => Some(format!(
            "secret-tool lookup service flux-backup profile {id}"
        )),
        PasswordSource::File { path } => Some(format!(
            "head -n 1 {}",
            shell_quote(&path.display().to_string())
        )),
        PasswordSource::Command { command } => Some(format!("{command} | head -n 1")),
        PasswordSource::Prompt => None,
    }
}

/// A YAML double-quoted string.
fn yq(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// `--keep-within` style durations (borg and restic): whole days, else hours.
fn within(r: &Retention) -> Option<String> {
    r.keep_within.map(|w| {
        let secs = w.0.as_secs();
        if secs % 86_400 == 0 {
            format!("{}d", secs / 86_400)
        } else {
            format!("{}h", secs.div_ceil(3600))
        }
    })
}

fn borgmatic(id: &ProfileId, p: &Profile) -> Export {
    let mut notes = common_notes(p);
    notes.extend(borg_limits(p));
    let b = p.borg.clone().unwrap_or_else(|| borg_options_from(p));
    let mut q = p.clone();
    q.engine = Engine::Borg;
    q.borg = Some(b.clone());
    let repo = borg::repo_url(id, &q, true);
    if p.engine != Engine::Borg {
        notes.push(format!(
            "The repository doesn't exist yet: create it once with borgmatic repo-create --encryption {} (borgmatic init before 1.9).",
            b.encryption.as_arg()
        ));
    }
    if !p.options.preserve_acl || !p.options.preserve_xattr {
        notes.push("ACLs/extended attributes are turned off with extra_borg_options.".into());
    }

    let mut y = header(id, p, "#", "borgmatic configuration");
    y.push_str("# Check it with:  borgmatic config validate --config THIS-FILE\n");
    for n in &notes {
        y.push_str(&format!("# Note: {n}\n"));
    }
    y.push('\n');
    y.push_str("source_directories:\n");
    for s in flux_fs::scan::normalize_sources(&p.sources.paths) {
        y.push_str(&format!("    - {}\n", yq(&s.display().to_string())));
    }
    y.push_str(&format!(
        "\nrepositories:\n    - path: {}\n      label: {}\n",
        yq(&repo),
        yq(&format!("flux-{id}"))
    ));

    // Exclusions: the same Borg patterns Flux uses (first match wins), each with its
    // style spelled out (borgmatic doesn't document the "P sh" default-style line).
    let patterns: Vec<String> = borg::patterns(p, "", &borg::never_paths(p))
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#') && !l.starts_with("P "))
        .map(|l| match l.split_once(' ') {
            Some((sign, rest))
                if !["pp:", "sh:", "fm:", "re:", "pf:"]
                    .iter()
                    .any(|x| rest.starts_with(x)) =>
            {
                format!("{sign} sh:{rest}")
            }
            _ => l.to_owned(),
        })
        .collect();
    if !patterns.is_empty() {
        y.push_str("\n# Exclusions (Borg patterns; the first matching pattern wins).\npatterns:\n");
        for l in &patterns {
            y.push_str(&format!("    - {}\n", yq(l)));
        }
    }
    if p.sources.exclude_caches {
        y.push_str("exclude_caches: true\n");
    }
    if !p.sources.exclude_if_present.is_empty() {
        y.push_str("exclude_if_present:\n");
        for m in &p.sources.exclude_if_present {
            y.push_str(&format!("    - {}\n", yq(m)));
        }
    }
    if p.sources.one_file_system {
        y.push_str("one_file_system: true\n");
    }

    y.push_str(&format!("\ncompression: {}\n", yq(&b.compression)));
    if b.encryption != flux_core::profile::BorgEncryption::None
        && let Some(pc) = passcommand(id, &b.password_source)
    {
        y.push_str(&format!("encryption_passcommand: {}\n", yq(&pc)));
    }
    if let Target::Ssh {
        identity_file,
        ssh_options,
        ..
    } = &p.target
        && (identity_file.is_some() || !ssh_options.is_empty())
    {
        let mut a = vec!["ssh".to_owned()];
        if let Some(i) = identity_file {
            a.extend(["-i".into(), i.display().to_string()]);
        }
        for o in ssh_options {
            a.extend(["-o".into(), o.clone()]);
        }
        y.push_str(&format!(
            "ssh_command: {}\n",
            yq(&a
                .iter()
                .map(|s| shell_quote(s))
                .collect::<Vec<_>>()
                .join(" "))
        ));
    }
    if let Some(r) = &b.remote_path {
        y.push_str(&format!("remote_path: {}\n", yq(r)));
    }
    let mut create_extra: Vec<String> = Vec::new();
    if !p.options.preserve_acl {
        create_extra.push("--noacls".into());
    }
    if !p.options.preserve_xattr {
        create_extra.push("--noxattrs".into());
    }
    if p.options.change_detection == ChangeDetection::Checksum {
        create_extra.push("--files-cache=disabled".into());
    }
    create_extra.extend(b.extra_args.iter().cloned());
    if !create_extra.is_empty() {
        y.push_str(&format!(
            "extra_borg_options:\n    create: {}\n",
            yq(&create_extra.join(" "))
        ));
    }
    y.push_str("archive_name_format: \"{hostname}-{now:%Y-%m-%d_%H-%M-%S}\"\n");

    let r = &p.retention;
    if r.enabled {
        y.push_str("\n# Retention (Flux's pins and names aren't known to borgmatic).\n");
        if r.keep_last > 0 {
            y.push_str(&format!("keep_secondly: {}\n", r.keep_last));
        }
        if let Some(w) = within(r) {
            y.push_str(&format!("keep_within: {w}\n"));
        }
        for (n, what) in [
            (r.keep_hourly, "hourly"),
            (r.keep_daily, "daily"),
            (r.keep_weekly, "weekly"),
            (r.keep_monthly, "monthly"),
            (r.keep_yearly, "yearly"),
        ] {
            if n > 0 {
                y.push_str(&format!("keep_{what}: {n}\n"));
            }
        }
    }
    y.push_str("\nchecks:\n    - name: repository\n    - name: archives\n");

    let hooks = &p.hooks;
    if !hooks.pre_backup.is_empty() || !hooks.post_backup.is_empty() || !hooks.on_error.is_empty() {
        y.push_str(
            "\n# Hooks (the FLUX_* variables Flux sets aren't available here). These keys work in\n\
             # borgmatic 1.8 and later; borgmatic 2.0 prefers the equivalent `commands:` form:\n\
             #   commands:\n\
             #       - before: action\n\
             #         when: [create]\n\
             #         run: [\"...\"]\n",
        );
        for (key, list) in [
            ("before_backup", &hooks.pre_backup),
            ("after_backup", &hooks.post_backup),
            ("on_error", &hooks.on_error),
        ] {
            if !list.is_empty() {
                y.push_str(&format!("{key}:\n"));
                for c in list {
                    y.push_str(&format!("    - {}\n", yq(c)));
                }
            }
        }
    }
    Export {
        format: Format::Borgmatic,
        filename: format!("{id}-borgmatic.yaml"),
        contents: y,
        executable: false,
        notes,
    }
}

/// One gitignore-style pattern for restic's exclude file (also last-match-wins, `!` to
/// re-include; patterns without a slash match a name at any depth).
pub fn restic_pattern(pattern: &str, home: Option<&std::path::Path>) -> Option<String> {
    let pattern = pattern.trim();
    let (neg, body) = match pattern.strip_prefix('!') {
        Some(rest) => ("!", rest),
        None => ("", pattern),
    };
    let body = match (body.strip_prefix('~'), home) {
        (Some(rest), Some(h)) if rest.is_empty() || rest.starts_with('/') => {
            format!("{}{rest}", h.display().to_string().trim_end_matches('/'))
        }
        _ => body.to_owned(),
    };
    let body = body.trim_end_matches('/');
    if body.is_empty() || body.starts_with('#') {
        return None;
    }
    let pat = if let Some(rest) = body.strip_prefix("**/") {
        rest.to_owned()
    } else if body.starts_with('/') || !body.contains('/') {
        body.to_owned()
    } else {
        // gitignore anchors a pattern with an inner slash at the root.
        format!("/{body}")
    };
    Some(format!("{neg}{pat}"))
}

fn restic(id: &ProfileId, p: &Profile) -> Export {
    let mut notes = common_notes(p);
    if p.options.symlinks != SymlinkMode::Preserve {
        notes.push("restic always stores symbolic links as links.".into());
    }
    if !p.options.preserve_acl || !p.options.preserve_xattr {
        notes.push(
            "restic always keeps extended attributes (and ACLs, which are stored as attributes)."
                .into(),
        );
    }
    let repo = match &p.target {
        Target::Local { path, .. } => path.join(format!("restic-{id}")).display().to_string(),
        Target::Ssh {
            host,
            user,
            port,
            path,
            ..
        } => {
            if port.is_some() {
                notes.push(
                    "restic's sftp backend takes the port from your ssh config (add a Host entry with Port).".into(),
                );
            }
            format!(
                "sftp:{}{host}:{}/restic-{id}",
                user.as_deref().map(|u| format!("{u}@")).unwrap_or_default(),
                path.display().to_string().trim_end_matches('/')
            )
        }
    };
    notes.push(format!(
        "The repository doesn't exist yet: the script creates it with restic init on its first run ({repo})."
    ));
    let home = sys::home_dir();
    let mut excludes: Vec<String> = p
        .sources
        .exclude
        .iter()
        .filter_map(|x| restic_pattern(x, home.as_deref()))
        .collect();
    if let Target::Local { path, .. } = &p.target {
        excludes.insert(0, path.join(format!("restic-{id}")).display().to_string());
    }
    let mut flags = Vec::new();
    if p.sources.one_file_system {
        flags.push("--one-file-system".to_owned());
    }
    if p.sources.exclude_caches {
        flags.push("--exclude-caches".to_owned());
    }
    for m in &p.sources.exclude_if_present {
        flags.push(format!("--exclude-if-present={}", shell_quote(m)));
    }
    if let Some(max) = p.sources.exclude_larger_than {
        flags.push(format!("--exclude-larger-than={}", max.bytes()));
    }
    if p.options.change_detection == ChangeDetection::Checksum {
        flags.push("--force".to_owned());
    }
    flags.push("--exclude-file=\"$EXCLUDES\"".to_owned());
    let sources = flux_fs::scan::normalize_sources(&p.sources.paths)
        .iter()
        .map(|s| shell_quote(&s.display().to_string()))
        .collect::<Vec<_>>()
        .join(" ");
    let r = &p.retention;
    let mut keep = Vec::new();
    if r.keep_last > 0 {
        keep.push(format!("--keep-last {}", r.keep_last));
    }
    if let Some(w) = within(r) {
        keep.push(format!("--keep-within {w}"));
    }
    for (n, what) in [
        (r.keep_hourly, "hourly"),
        (r.keep_daily, "daily"),
        (r.keep_weekly, "weekly"),
        (r.keep_monthly, "monthly"),
        (r.keep_yearly, "yearly"),
    ] {
        if n > 0 {
            keep.push(format!("--keep-{what} {n}"));
        }
    }
    let pw = match p
        .password_source()
        .as_ref()
        .and_then(|s| passcommand(id, s))
    {
        Some(pc) => format!(
            "[ -n \"${{RESTIC_PASSWORD:-}}${{RESTIC_PASSWORD_COMMAND:-}}${{RESTIC_PASSWORD_FILE:-}}\" ] || export RESTIC_PASSWORD_COMMAND={}\n",
            shell_quote(&pc)
        ),
        None => "# restic asks for the repository password (or set RESTIC_PASSWORD_FILE).\n".into(),
    };
    let notes_text = notes
        .iter()
        .map(|n| format!("# Note: {n}\n"))
        .collect::<String>();
    let hooks = |list: &[String]| -> String { list.iter().map(|c| format!("{c}\n")).collect() };
    let pre = hooks(&p.hooks.pre_backup);
    let post = hooks(&p.hooks.post_backup);
    let script = format!(
        r#"#!/bin/sh
{header}#
#   sh {file}             back up (creates the repository the first time)
#   sh {file} --dry-run   only show what would be backed up
{notes_text}set -eu

export RESTIC_REPOSITORY={repo}
{pw}
DRY=
if [ "${{1:-}}" = "--dry-run" ]; then DRY=1; fi

# Create the repository on the first run.
restic cat config >/dev/null 2>&1 || {{ [ -n "$DRY" ] || restic init; }}

# Exclusions (restic's exclude-file syntax; the last matching pattern wins).
EXCLUDES=$(mktemp)
trap 'rm -f "$EXCLUDES"' EXIT
cat > "$EXCLUDES" <<'FLUX_EXCLUDES'
{excludes}FLUX_EXCLUDES

{pre}restic backup {flags} ${{DRY:+--dry-run}} \
    {sources}
[ -n "$DRY" ] && exit 0
{post}{forget}"#,
        header = header(id, p, "#", "restic equivalent"),
        file = format!("{id}-restic-backup.sh"),
        excludes = excludes
            .iter()
            .map(|x| format!("{x}\n"))
            .collect::<String>(),
        flags = flags.join(" "),
        forget = if r.enabled && !keep.is_empty() {
            format!(
                "\n# Retention, then free the space.\nrestic forget --prune {}\n",
                keep.join(" ")
            )
        } else {
            String::new()
        },
    );
    Export {
        format: Format::Restic,
        filename: format!("{id}-restic-backup.sh"),
        contents: script,
        executable: true,
        notes,
    }
}

/// Name of the copy of a profile kept with its backups.
pub const SAVED_PROFILE: &str = "flux-profile.toml";

/// The copy of a profile kept with its backups, for recovering them on another computer:
/// the Flux profile without webhooks, hooks, or password commands (a command can hold the
/// password itself), with where its backups are kept (`made_as` lines) so a recovery can
/// find them again.
pub fn recovery_copy(id: &ProfileId, p: &Profile) -> String {
    let mut q = p.clone();
    q.webhooks.clear();
    q.hooks = Default::default();
    for source in [
        q.vault.as_mut().map(|v| &mut v.password_source),
        q.borg.as_mut().map(|b| &mut b.password_source),
    ]
    .into_iter()
    .flatten()
    {
        if matches!(source, flux_core::profile::PasswordSource::Command { .. }) {
            *source = flux_core::profile::PasswordSource::Keyring;
        }
    }
    let k = p.kept_as(id);
    // The copy describes the backup, not this computer's way into it.
    q.origin = None;
    let body = q
        .to_toml()
        .unwrap_or_else(|e| format!("# could not serialize: {e}\n"));
    format!(
        "# The settings of the Flux Backup profile “{name}”, kept with its backups so\n\
         # they can be recovered on another computer (Flux Backup: Recover an existing backup,\n\
         # or flux recover). Saved by Flux Backup {version}.\n\
         # Without passwords, hooks and webhooks.\n\
         # flux-profile-id = \"{id}\"\n\
         # made-on = \"{host}\"\n\
         # made-by = \"{user}\"\n\
         # kept-as = \"{kept}\"\n\n{body}",
        name = p.name,
        version = flux_core::VERSION,
        host = k.host,
        user = k.user,
        kept = k.profile,
    )
}

/// A profile read from next to its backups, as what it is there: a description of the
/// backup, written by whoever could write to that drive or server. Everything in it that
/// would run a program on this computer is dropped: hooks, webhooks, a password command,
/// ssh options (`ProxyCommand` runs a command), and the extra arguments and remote program
/// passed to rsync and borg. [`recovery_copy`] never writes the first three; none of them
/// is ever taken from a file found on a drive.
fn description_only(mut p: Profile) -> Profile {
    use flux_core::profile::{PasswordSource, Target};
    fn plain(t: &mut Target) {
        if let Target::Ssh { ssh_options, .. } = t {
            ssh_options.clear();
        }
    }
    p.hooks = Default::default();
    p.webhooks.clear();
    for source in [
        p.vault.as_mut().map(|v| &mut v.password_source),
        p.borg.as_mut().map(|b| &mut b.password_source),
    ]
    .into_iter()
    .flatten()
    {
        if matches!(source, PasswordSource::Command { .. }) {
            *source = PasswordSource::Keyring;
        }
    }
    plain(&mut p.target);
    // Other places it was copied to are set up again by hand (they may be elsewhere now).
    p.destinations.clear();
    if let Some(r) = p.rsync.as_mut() {
        r.extra_args.clear();
    }
    if let Some(b) = p.borg.as_mut() {
        b.extra_args.clear();
        b.remote_path = None;
    }
    p
}

/// A [`recovery_copy`] read back: the profile, its id and the names its backups are kept
/// under.
pub fn parse_recovery_copy(
    text: &str,
) -> Result<(Profile, Option<flux_core::profile::KeptAs>), String> {
    let field = |name: &str| {
        text.lines().find_map(|l| {
            l.strip_prefix(&format!("# {name} = "))
                .map(|v| v.trim().trim_matches('"').to_owned())
        })
    };
    let p = description_only(Profile::from_toml(text).map_err(|e| e.to_string())?);
    let kept = match (field("made-on"), field("made-by"), field("kept-as")) {
        (Some(host), Some(user), Some(profile)) => Some(flux_core::profile::KeptAs {
            host,
            user,
            profile,
        }),
        _ => None,
    };
    Ok((p, kept))
}

/// Read an exported (or hand-written) Flux profile. Returns the id it was exported
/// under, if the file says so, and the profile.
pub fn parse_flux(text: &str) -> Result<(Option<String>, Profile), String> {
    let id = text.lines().find_map(|l| {
        l.strip_prefix("# flux-profile-id = ")
            .map(|v| v.trim().trim_matches('"').to_owned())
    });
    let p = Profile::from_toml(text).map_err(|e| e.to_string())?;
    Ok((id, p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> (ProfileId, Profile) {
        let mut p = Profile::new(
            "Home \"folder\"",
            Target::Local {
                path: "/mnt/backup".into(),
                fs_uuid: None,
            },
            vec!["/home/me".into()],
        );
        p.sources.exclude = vec!["*.tmp".into(), "build/out".into(), "!keep.tmp".into()];
        p.sources.exclude_if_present = vec![".nobackup".into()];
        p.hooks.pre_backup = vec!["echo 'start'".into()];
        (ProfileId::new("home").unwrap(), p)
    }

    #[test]
    fn flux_round_trips() {
        let (id, p) = profile();
        let e = export(&id, &p, Format::Flux);
        let (back_id, back) = parse_flux(&e.contents).unwrap();
        assert_eq!(back_id.as_deref(), Some("home"));
        assert_eq!(back, p);
    }

    #[test]
    fn borgmatic_has_the_essentials() {
        let (id, p) = profile();
        let e = export(&id, &p, Format::Borgmatic);
        let y = &e.contents;
        assert!(y.contains("source_directories:\n    - \"/home/me\""), "{y}");
        assert!(y.contains("label: \"flux-home\""));
        assert!(y.contains("    - \"! sh:**/*.tmp\""));
        assert!(y.contains("    - \"+ sh:**/keep.tmp\""));
        assert!(y.contains("    - \"! pp:mnt/backup/flux-borg\""));
        assert!(!y.contains("P sh"));
        assert!(y.contains("exclude_caches: true"));
        assert!(y.contains("keep_daily: 7"));
        assert!(y.contains("keep_within: 1d"));
        assert!(y.contains(
            "encryption_passcommand: \"secret-tool lookup service flux-backup profile home\""
        ));
        assert!(y.contains("before_backup:\n    - \"echo 'start'\""));
        assert!(e.notes.iter().any(|n| n.contains("doesn't exist yet")));
    }

    #[test]
    fn restic_patterns_and_script() {
        let h = Some(std::path::Path::new("/home/me"));
        assert_eq!(restic_pattern("*.tmp", h).unwrap(), "*.tmp");
        assert_eq!(restic_pattern("node_modules/", h).unwrap(), "node_modules");
        assert_eq!(restic_pattern("~/Videos/", h).unwrap(), "/home/me/Videos");
        assert_eq!(restic_pattern("a/b", h).unwrap(), "/a/b");
        assert_eq!(restic_pattern("**/x/y", h).unwrap(), "x/y");
        assert_eq!(restic_pattern("!keep.tmp", h).unwrap(), "!keep.tmp");
        let (id, p) = profile();
        let e = export(&id, &p, Format::Restic);
        assert!(e.executable);
        assert!(
            e.contents
                .contains("export RESTIC_REPOSITORY=/mnt/backup/restic-home")
        );
        assert!(e.contents.contains("--exclude-caches"));
        assert!(
            e.contents
                .contains("restic forget --prune --keep-last 1 --keep-within 1d --keep-daily 7")
        );
        assert!(e.contents.contains("echo 'start'\nrestic backup"));
    }

    #[test]
    fn scripts_and_formats() {
        let (id, p) = profile();
        assert!(
            export(&id, &p, Format::Rsync)
                .contents
                .contains("--link-dest")
        );
        let b = export(&id, &p, Format::Borg);
        assert!(b.contents.contains("borg create"));
        assert!(b.notes.iter().any(|n| n.contains("borg init")));
        for f in Format::ALL {
            assert_eq!(Format::parse(f.info().id), Some(f));
        }
    }
}
