//! `flux backup`, `flux snapshots`, `flux info`, `flux ls`, `flux restore`.

use std::io::IsTerminal;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::bail;
use clap::Args;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use jiff::Timestamp;

use flux_core::profile::has_errors;
use flux_core::progress::{Event, Phase, Progress};
use flux_core::{ByteSize, Profile, ProfileId, ProfileStore};
use flux_fs::entry::{Entry, Kind, is_within};
use flux_fs::fsops::XattrPolicy;
use flux_fs::outcome::BackupOutcome;
use flux_fs::restore::RestoreRequest;
use flux_fs::snapshot::{SnapshotMeta, SnapshotStatus};
use flux_runner::Trigger;
use flux_runner::password;
use flux_runner::repo::{AnyRepo, RepoError};

use crate::{Ctx, output};

/// Exit status when a backup finished but some files could not be backed up.
pub const EXIT_PARTIAL: u8 = 4;

#[derive(Args)]
pub struct BackupArgs {
    /// Profile to back up.
    pub profile: String,
    /// Name the snapshot (named snapshots are never pruned automatically).
    #[arg(long)]
    pub name: Option<String>,
    /// Show what would be copied without writing anything.
    #[arg(long, short = 'n')]
    pub dry_run: bool,
    /// Don't apply the retention policy after this backup.
    #[arg(long)]
    pub no_prune: bool,
    /// Run as a scheduled backup (honours skip-on-battery, lowers priority).
    #[arg(long, hide = true)]
    pub scheduled: bool,
    /// With --scheduled: only run if the profile's interval has passed since the last
    /// successful backup.
    #[arg(long, hide = true, requires = "scheduled")]
    pub if_due: bool,
    /// Shut the computer down a minute after the backup ends (not if it's stopped);
    /// `shutdown -c` cancels it.
    #[arg(long)]
    pub shutdown: bool,
}

#[derive(Args)]
pub struct SnapshotsArgs {
    pub profile: String,
}

#[derive(Args)]
pub struct InfoArgs {
    pub profile: String,
    /// Snapshot id, unique id prefix, name, or "latest".
    #[arg(default_value = "latest")]
    pub snapshot: String,
}

#[derive(Args)]
pub struct LsArgs {
    pub profile: String,
    /// Directory or file to list (original absolute path).
    #[arg(default_value = "/")]
    pub path: PathBuf,
    #[arg(long, short, default_value = "latest")]
    pub snapshot: String,
    /// Long listing: mode, owner, size, modification time.
    #[arg(short)]
    pub long: bool,
    /// List everything below PATH.
    #[arg(short = 'R', long)]
    pub recursive: bool,
}

#[derive(Args)]
pub struct RestoreArgs {
    pub profile: String,
    /// Files or folders to restore, by their original path.
    #[arg(required = true)]
    pub paths: Vec<PathBuf>,
    #[arg(long, short, default_value = "latest")]
    pub snapshot: String,
    /// Restore into this folder instead of the original location.
    #[arg(long)]
    pub to: Option<PathBuf>,
    /// Overwrite differing files instead of keeping them as NAME.flux-<snapshot>.
    #[arg(long)]
    pub overwrite: bool,
    /// Make restored folders exactly as in the snapshot: also remove what's in them now
    /// but not in the snapshot (kept as NAME.flux-<snapshot> unless --overwrite). What the
    /// backup never takes (exclusions, the size limit, caches) is left alone.
    #[arg(long)]
    pub exact: bool,
    /// Show what would be restored.
    #[arg(long, short = 'n')]
    pub dry_run: bool,
    /// Don't ask before restoring to the original location.
    #[arg(long, short)]
    pub yes: bool,
}

pub fn load_profile(ctx: &Ctx, id: &str) -> anyhow::Result<(ProfileId, Profile)> {
    let id = ProfileId::new(id)?;
    let p = ProfileStore::new(&ctx.paths).load(&id)?;
    let issues = p.validate_in(ctx.paths.scope);
    if has_errors(&issues) {
        for i in &issues {
            eprintln!("{id}: {i}");
        }
        bail!("profile {id} has errors; fix them with `flux profile edit {id}`");
    }
    Ok((id, p))
}

pub fn absolutize(p: PathBuf) -> anyhow::Result<PathBuf> {
    Ok(if p.is_absolute() {
        p
    } else {
        std::env::current_dir()?.join(p)
    })
}

/// A progress bar on stderr that tracks a [`Progress`] until dropped.
pub struct LiveProgress {
    bar: ProgressBar,
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl LiveProgress {
    pub fn start(progress: Arc<Progress>, enabled: bool) -> Self {
        let bar = if enabled {
            ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr_with_hz(10))
        } else {
            ProgressBar::hidden()
        };
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread = enabled.then(|| {
            let (bar, stop) = (bar.clone(), stop.clone());
            std::thread::spawn(move || {
                let spin = ProgressStyle::with_template("{spinner} {msg}").unwrap();
                let bytes = ProgressStyle::with_template(
                    "{spinner} {prefix:<10} [{bar:30}] {bytes}/{total_bytes} {binary_bytes_per_sec} ETA {eta} {msg}",
                )
                .unwrap()
                .progress_chars("━╸ ");
                bar.set_style(spin.clone());
                let mut bytes_mode = false;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let s = progress.snapshot();
                    let want_bytes = s.phase == Phase::Transferring && s.bytes_total > 0;
                    if want_bytes != bytes_mode {
                        bar.set_style(if want_bytes { bytes.clone() } else { spin.clone() });
                        bar.set_prefix("Copying");
                        bytes_mode = want_bytes;
                    }
                    let items = format!("{}/{} items", group(s.entries_done), group(s.entries_total));
                    if want_bytes {
                        bar.set_length(s.bytes_total);
                        bar.set_position(s.bytes_done.min(s.bytes_total));
                        bar.set_message(items);
                    } else {
                        let detail = match s.phase {
                            Phase::Scanning => format!("{} items found", group(s.scanned)),
                            Phase::Transferring => items,
                            _ => String::new(),
                        };
                        bar.set_message(format!("{} {detail}", s.phase.label()));
                    }
                    bar.tick();
                    std::thread::sleep(Duration::from_millis(100));
                }
            })
        });
        Self { bar, stop, thread }
    }

    pub fn println(&self, msg: impl AsRef<str>) {
        if self.bar.is_hidden() {
            eprintln!("{}", msg.as_ref());
        } else {
            self.bar.println(msg);
        }
    }
}

impl Drop for LiveProgress {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.bar.finish_and_clear();
    }
}

pub fn install_ctrlc(progress: &Arc<Progress>) {
    let p = progress.clone();
    let _ = ctrlc::set_handler(move || {
        if p.is_cancelled() {
            eprintln!("\nflux: forced exit");
            std::process::exit(130);
        }
        eprintln!("\nflux: stopping after the current files… (press Ctrl-C again to force)");
        p.cancel();
    });
}

pub fn run_backup(ctx: &Ctx, args: BackupArgs) -> anyhow::Result<ExitCode> {
    let (id, profile) = load_profile(ctx, &args.profile)?;
    let progress = Arc::new(Progress::new());
    install_ctrlc(&progress);
    let live = LiveProgress::start(
        progress.clone(),
        !ctx.json && std::io::stderr().is_terminal(),
    );
    let report = |ev: Event| match ev {
        Event::Warning { path, message } if path.is_empty() => {
            live.println(format!("warning: {message}"))
        }
        Event::Warning { path, message } => live.println(format!("warning: {path}: {message}")),
        Event::Info { message } => live.println(message),
        Event::Phase { .. } => {}
    };
    let started = std::time::Instant::now();
    let result = flux_runner::run_backup(
        &flux_runner::BackupJob {
            paths: &ctx.paths,
            id: &id,
            profile: &profile,
            name: args.name,
            dry_run: args.dry_run,
            trigger: if args.scheduled {
                Trigger::Scheduled
            } else {
                Trigger::Manual
            },
            prune: !args.no_prune,
            only_if_due: args.if_due,
            ask: (!args.scheduled).then_some(ASK),
            shutdown_after: args.shutdown,
        },
        &progress,
        &report,
    );
    drop(live);
    let secs = started.elapsed().as_secs_f64();
    let record = result.record;
    let out = match result.outcome {
        Ok(o) => o,
        Err(flux_runner::RunError::Busy(_)) if args.scheduled => {
            println!("Skipped: a backup of {id} is already running.");
            return Ok(ExitCode::SUCCESS);
        }
        Err(flux_runner::RunError::Skipped(why)) => {
            if ctx.json {
                output::print_json(&record)?;
            } else {
                println!("Skipped: {why}.");
            }
            return Ok(ExitCode::SUCCESS);
        }
        Err(e) => {
            if ctx.json {
                output::print_json(&record)?;
            }
            return Err(e.into());
        }
    };

    if ctx.json {
        output::print_json(&serde_json::json!({ "run": record, "result": out }))?;
    } else {
        match &out {
            BackupOutcome::Created { snapshot: s, .. } => {
                let st = &s.stats;
                let mark = if s.status == SnapshotStatus::Complete {
                    "✓"
                } else {
                    "⚠"
                };
                println!("{mark} Snapshot {} created in {}", s.id, duration(secs));
                println!(
                    "  {} files, {} · items: {} new, {} changed, {} deleted",
                    group(st.files),
                    ByteSize(st.total_bytes).display_approx(),
                    group(st.new),
                    group(st.changed),
                    group(st.deleted)
                );
                let (verb, reuse) = match profile.engine {
                    flux_core::Engine::Mirror | flux_core::Engine::Rsync => {
                        ("copied", "hard-linked")
                    }
                    flux_core::Engine::Vault | flux_core::Engine::Borg => ("read", "reused"),
                };
                println!(
                    "  {verb} {} in {} files, {reuse} {} unchanged files",
                    ByteSize(st.copied_bytes).display_approx(),
                    group(st.copied_files),
                    group(st.linked_files)
                );
                if matches!(
                    profile.engine,
                    flux_core::Engine::Vault | flux_core::Engine::Borg
                ) {
                    println!(
                        "  stored {} after deduplication and compression",
                        ByteSize(st.stored_bytes).display_approx()
                    );
                }
                if s.errors_total > 0 {
                    println!(
                        "  {} items could not be backed up; see `flux info {id} {}`",
                        s.errors_total, s.id
                    );
                }
            }
            BackupOutcome::Unchanged { previous, .. } => {
                println!("✓ Nothing changed since snapshot {previous}; no new snapshot needed.");
            }
            BackupOutcome::DryRun { stats, errors } => {
                println!("Dry run: nothing was written.");
                println!(
                    "  {} files, {} · items: {} new, {} changed, {} unchanged, {} deleted",
                    group(stats.files),
                    ByteSize(stats.total_bytes).display_approx(),
                    group(stats.new),
                    group(stats.changed),
                    group(stats.unchanged),
                    group(stats.deleted)
                );
                println!(
                    "  would copy {} in {} files and hard-link {} files",
                    ByteSize(stats.copy_bytes).display_approx(),
                    group(stats.copy_files),
                    group(stats.link_files)
                );
                if !errors.is_empty() {
                    println!("  {} items could not be read", errors.len());
                }
            }
        }
        if !record.pruned.is_empty() {
            println!(
                "  pruned {} old snapshots: {}",
                record.pruned.len(),
                record.pruned.join(", ")
            );
        }
        for h in record.hooks.iter().filter(|h| !h.success) {
            println!("  hook failed: {}", h.command);
        }
    }
    Ok(match out {
        BackupOutcome::Created { snapshot, .. } if snapshot.status != SnapshotStatus::Complete => {
            ExitCode::from(EXIT_PARTIAL)
        }
        _ => ExitCode::SUCCESS,
    })
}

/// How the CLI asks for a Vault password: on the terminal, if there is one.
pub const ASK: password::Ask<'static> = &password::terminal_ask;

pub fn repo_for(ctx: &Ctx, profile: &str) -> anyhow::Result<(ProfileId, Profile, AnyRepo)> {
    let (id, p) = load_profile(ctx, profile)?;
    let repo = AnyRepo::open(&ctx.paths, &id, &p, Some(ASK))?;
    Ok((id, p, repo))
}

pub fn run_snapshots(ctx: &Ctx, args: SnapshotsArgs) -> anyhow::Result<ExitCode> {
    let (_, _, repo) = repo_for(ctx, &args.profile)?;
    let snaps = repo.snapshots()?;
    if ctx.json {
        output::print_json(&snaps)?;
        return Ok(ExitCode::SUCCESS);
    }
    if snaps.is_empty() {
        println!("No snapshots yet.");
        return Ok(ExitCode::SUCCESS);
    }
    let rows: Vec<Vec<String>> = snaps
        .iter()
        .rev()
        .map(|s| {
            vec![
                s.id.clone(),
                when(s.started),
                status(s),
                group(s.stats.files),
                ByteSize(s.stats.total_bytes).display_approx(),
                format!("+{} ~{} -{}", s.stats.new, s.stats.changed, s.stats.deleted),
                ByteSize(s.stats.copied_bytes).display_approx(),
                match (&s.name, s.pinned) {
                    (Some(n), true) => format!("{n} (pinned)"),
                    (Some(n), false) => n.clone(),
                    (None, true) => "(pinned)".into(),
                    (None, false) => String::new(),
                },
            ]
        })
        .collect();
    output::table(
        &[
            "ID", "WHEN", "STATUS", "FILES", "SIZE", "CHANGES", "COPIED", "NAME",
        ],
        &rows,
    );
    Ok(ExitCode::SUCCESS)
}

pub fn run_info(ctx: &Ctx, args: InfoArgs) -> anyhow::Result<ExitCode> {
    let (_, _, repo) = repo_for(ctx, &args.profile)?;
    let s = repo.find(&args.snapshot)?;
    if ctx.json {
        output::print_json(&s)?;
        return Ok(ExitCode::SUCCESS);
    }
    let st = &s.stats;
    let secs = (s.finished.as_millisecond() - s.started.as_millisecond()) as f64 / 1000.0;
    output::kv(&[
        ("snapshot", s.id.clone()),
        ("name", s.name.clone().unwrap_or_else(|| "—".into())),
        ("pinned", if s.pinned { "yes" } else { "no" }.into()),
        ("started", when(s.started)),
        ("duration", duration(secs)),
        ("status", status(&s)),
        ("host / user", format!("{} / {}", s.host, s.user)),
        (
            "sources",
            s.sources
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
        ),
        (
            "contents",
            format!(
                "{} files, {} dirs, {} symlinks, {}",
                group(st.files),
                group(st.dirs),
                group(st.symlinks),
                ByteSize(st.total_bytes).display_approx()
            ),
        ),
        (
            "changes",
            format!(
                "{} new, {} changed, {} unchanged, {} deleted",
                group(st.new),
                group(st.changed),
                group(st.unchanged),
                group(st.deleted)
            ),
        ),
        (
            "copied",
            format!(
                "{} in {} files ({} hard-linked)",
                ByteSize(st.copied_bytes).display_approx(),
                group(st.copied_files),
                group(st.linked_files)
            ),
        ),
        (
            "stored",
            if matches!(
                repo.engine(),
                flux_core::Engine::Vault | flux_core::Engine::Borg
            ) {
                format!(
                    "{} new data after deduplication and compression",
                    ByteSize(st.stored_bytes).display_approx()
                )
            } else {
                "—".into()
            },
        ),
        ("location", repo.location(&s)),
        ("flux", s.flux_version.clone()),
    ]);
    if s.errors_total > 0 {
        println!(
            "\n{} problems{}:",
            s.errors_total,
            if (s.errors.len() as u64) < s.errors_total {
                " (first shown)"
            } else {
                ""
            }
        );
        for e in &s.errors {
            println!("  {}: {}", e.path, e.message);
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub fn run_ls(ctx: &Ctx, args: LsArgs) -> anyhow::Result<ExitCode> {
    let (_, _, repo) = repo_for(ctx, &args.profile)?;
    let snap = repo.find(&args.snapshot)?;
    let path = absolutize(args.path)?;
    let want = path.as_os_str().as_bytes();
    let want = want.strip_prefix(b"/").unwrap_or(want);
    let want = want.strip_suffix(b"/").unwrap_or(want).to_vec();

    let mut shown: Vec<Entry> = Vec::new();
    let mut found_self = want.is_empty();
    for e in repo.entries(&snap)? {
        let e = e?;
        if e.path == want {
            found_self = true;
            if e.kind != Kind::Dir {
                shown.push(e);
            }
            continue;
        }
        if !is_within(&e.path, &want) {
            continue;
        }
        let rest = &e.path[if want.is_empty() { 0 } else { want.len() + 1 }..];
        if args.recursive || !rest.contains(&b'/') {
            shown.push(e);
        }
    }
    if !found_self {
        bail!("{} is not in snapshot {}", path.display(), snap.id);
    }
    if ctx.json {
        let rows: Vec<_> = shown
            .iter()
            .map(|e| {
                serde_json::json!({
                    "path": e.abs_path(),
                    "kind": e.kind,
                    "mode": e.mode,
                    "uid": e.uid,
                    "gid": e.gid,
                    "size": e.size,
                    "mtime": e.mtime,
                    "link": e.link.as_ref().map(|l| String::from_utf8_lossy(l).into_owned()),
                })
            })
            .collect();
        output::print_json(&rows)?;
        return Ok(ExitCode::SUCCESS);
    }
    let base_len = if want.is_empty() { 0 } else { want.len() + 1 };
    for e in &shown {
        let name = if e.path == want {
            e.abs_path().display().to_string()
        } else {
            String::from_utf8_lossy(&e.path[base_len..]).into_owned()
        };
        let suffix = match (e.kind, &e.link) {
            (Kind::Dir, _) => "/".to_owned(),
            (Kind::Symlink, Some(l)) => format!(" -> {}", String::from_utf8_lossy(l)),
            _ => String::new(),
        };
        if args.long {
            let t = Timestamp::from_second(e.mtime)
                .map(|t| {
                    t.to_zoned(jiff::tz::TimeZone::system())
                        .strftime("%Y-%m-%d %H:%M")
                        .to_string()
                })
                .unwrap_or_default();
            println!(
                "{}{} {:>5} {:>5} {:>10} {t}  {name}{suffix}",
                e.kind.letter(),
                mode_string(e.mode),
                e.uid,
                e.gid,
                if e.kind == Kind::File {
                    ByteSize(e.size).display_approx()
                } else {
                    String::new()
                },
            );
        } else {
            println!("{name}{suffix}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub fn run_restore(ctx: &Ctx, args: RestoreArgs) -> anyhow::Result<ExitCode> {
    let (id, profile) = load_profile(ctx, &args.profile)?;
    let repo = AnyRepo::open(&ctx.paths, &id, &profile, Some(ASK))?;
    let snap = repo.find(&args.snapshot)?;
    let paths = args
        .paths
        .into_iter()
        .map(absolutize)
        .collect::<anyhow::Result<Vec<_>>>()?;
    let to = args.to.map(absolutize).transpose()?;
    let opts = &profile.options;
    let keep_existing = opts.backup_before_restore && !args.overwrite;
    let filter = args
        .exact
        .then(|| flux_fs::scan::exact_filter(&profile))
        .transpose()?;
    let request = |dry_run: bool, paths: Vec<std::path::PathBuf>| RestoreRequest {
        snapshot: &snap,
        paths,
        to: to.clone(),
        keep_existing,
        dry_run,
        xattrs: XattrPolicy {
            acl: opts.preserve_acl,
            other: opts.preserve_xattr,
        },
        links: flux_fs::safe::LinkPolicy::for_process(),
        exact: filter.as_ref(),
    };
    // What an exact restore would remove, to ask about it (and list it in a dry run).
    let removing = match &filter {
        Some(_) => {
            let entries = repo
                .entries(&snap)?
                .map(|e| e.map_err(|e| std::io::Error::other(e.to_string())));
            Some(flux_fs::restore::preview(
                entries,
                &request(true, paths.clone()),
            )?)
        }
        None => None,
    };
    let remove_n = removing.as_ref().map_or(0, |p| p.remove);
    if remove_n > 0 && !args.dry_run && !args.yes {
        let p = removing.as_ref().expect("counted");
        for path in &p.remove_paths {
            println!("  {}", path.display());
        }
        if p.remove > p.remove_paths.len() as u64 {
            println!("  … and {} more", p.remove - p.remove_paths.len() as u64);
        }
        let how = if keep_existing {
            "kept as NAME.flux-<snapshot>"
        } else {
            "deleted"
        };
        if !output::confirm(&format!(
            "These {} items aren't in snapshot {} and will be removed ({how}). Continue?",
            group(remove_n),
            snap.id
        ))? {
            bail!("not confirmed (use --yes to restore without asking)");
        }
    }

    if to.is_none() && !args.dry_run && !args.yes {
        let list = paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let how = if args.overwrite {
            "overwriting"
        } else {
            "keeping"
        };
        let question = format!(
            "Restore {list} from snapshot {} to the original location ({how} files that differ)?",
            snap.id
        );
        if !output::confirm(&question)? {
            bail!("not confirmed (use --yes, or --to DIR to restore elsewhere)");
        }
    }

    let progress = Arc::new(Progress::new());
    install_ctrlc(&progress);
    let live = LiveProgress::start(
        progress.clone(),
        !ctx.json && std::io::stderr().is_terminal(),
    );
    let report = |ev: Event| {
        if let Event::Warning { path, message } = ev {
            live.println(format!("warning: {path}: {message}"));
        }
    };
    let out = repo.restore(&request(args.dry_run, paths), &progress, &report);
    drop(live);
    let out = match out {
        Err(RepoError::NotInSnapshot(path, sid)) => {
            let hint = match newest_containing(&repo, &path) {
                Some(other) => format!("; it is in snapshot {other} (use --snapshot {other})"),
                None => "; no snapshot contains it".to_owned(),
            };
            bail!("{} is not in snapshot {sid}{hint}", path.display());
        }
        r => r?,
    };
    if ctx.json {
        output::print_json(&out)?;
    } else if args.dry_run {
        for p in &out.would_write {
            println!("{}", p.display());
        }
        println!(
            "Dry run: {} items would be restored from {}.",
            out.would_write.len(),
            snap.id
        );
        if let Some(p) = &removing {
            for path in &p.remove_paths {
                println!("remove {}", path.display());
            }
            println!(
                "{} items not in the snapshot would be removed{}.",
                group(p.remove),
                if keep_existing {
                    " (kept as NAME.flux-<snapshot>)"
                } else {
                    ""
                }
            );
        }
    } else {
        println!(
            "✓ Restored {} items ({}) from {}; {} already identical.",
            group(out.restored),
            ByteSize(out.bytes).display_approx(),
            snap.id,
            group(out.skipped_identical)
        );
        if out.removed > 0 {
            println!(
                "  removed {} {} that weren't in the snapshot",
                group(out.removed),
                if out.removed == 1 { "item" } else { "items" }
            );
        }
        for p in &out.moved_aside {
            println!("  kept existing file as {}", p.display());
        }
        if !out.errors.is_empty() {
            println!("  {} items could not be restored", out.errors.len());
        }
    }
    Ok(if out.errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_PARTIAL)
    })
}

/// Newest snapshot that contains `path`.
fn newest_containing(repo: &AnyRepo, path: &std::path::Path) -> Option<String> {
    let want = path.as_os_str().as_bytes();
    let want = want.strip_prefix(b"/").unwrap_or(want);
    let snaps = repo.snapshots().ok()?;
    snaps.iter().rev().find_map(|s| {
        let mut m = repo.entries(s).ok()?;
        m.any(|e| e.is_ok_and(|e| e.path == want))
            .then(|| s.id.clone())
    })
}

fn status(s: &SnapshotMeta) -> String {
    match s.status {
        SnapshotStatus::Complete => "ok".into(),
        SnapshotStatus::CompleteWithErrors => format!("{} errors", s.errors_total),
    }
}

pub fn when(t: Timestamp) -> String {
    let local = t
        .to_zoned(jiff::tz::TimeZone::system())
        .strftime("%Y-%m-%d %H:%M")
        .to_string();
    let ago = Timestamp::now().as_second() - t.as_second();
    let rel = match ago {
        i64::MIN..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", ago / 60),
        3600..86_400 => format!("{} h ago", ago / 3600),
        86_400..2_592_000 => format!("{} days ago", ago / 86_400),
        _ => return local,
    };
    format!("{local} ({rel})")
}

pub fn duration(secs: f64) -> String {
    if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        let s = secs as u64;
        if s < 3600 {
            format!("{}m {:02}s", s / 60, s % 60)
        } else {
            format!("{}h {:02}m", s / 3600, (s % 3600) / 60)
        }
    }
}

/// 1234567 → "1,234,567".
pub fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn mode_string(mode: u32) -> String {
    let mut s = String::with_capacity(9);
    for (shift, special, ch) in [(6, 0o4000, 's'), (3, 0o2000, 's'), (0, 0o1000, 't')] {
        let bits = (mode >> shift) & 7;
        s.push(if bits & 4 != 0 { 'r' } else { '-' });
        s.push(if bits & 2 != 0 { 'w' } else { '-' });
        let x = bits & 1 != 0;
        s.push(match (mode & special != 0, x) {
            (true, true) => ch,
            (true, false) => ch.to_ascii_uppercase(),
            (false, true) => 'x',
            (false, false) => '-',
        });
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting() {
        assert_eq!(group(0), "0");
        assert_eq!(group(1234567), "1,234,567");
        assert_eq!(mode_string(0o755), "rwxr-xr-x");
        assert_eq!(mode_string(0o4755), "rwsr-xr-x");
        assert_eq!(mode_string(0o1777), "rwxrwxrwt");
        assert_eq!(duration(5.25), "5.2s");
        assert_eq!(duration(125.0), "2m 05s");
    }
}
