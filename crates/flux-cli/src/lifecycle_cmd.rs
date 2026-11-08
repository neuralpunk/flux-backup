//! `flux prune`, `name`, `pin`, `unpin`, `delete`, `diff`, `find`, `history`, `log`.

use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, bail};
use clap::Args;

use flux_core::ByteSize;
use flux_core::progress::{Event, Progress, silent};
use flux_fs::diff::{ChangeKind, diff};
use flux_fs::entry::Kind;
use flux_fs::history;
use flux_fs::snapshot::SnapshotMeta;
use flux_runner::runlog;

use crate::backup_cmd::{absolutize, duration, group, load_profile, repo_for, when};
use crate::{Ctx, output};

#[derive(Args)]
pub struct PruneArgs {
    pub profile: String,
    /// Only show what would be removed.
    #[arg(long, short = 'n')]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct NameArgs {
    pub profile: String,
    pub snapshot: String,
    /// New name; omit to remove the name.
    pub name: Option<String>,
}

#[derive(Args)]
pub struct PinArgs {
    pub profile: String,
    pub snapshot: String,
}

#[derive(Args)]
pub struct DeleteArgs {
    pub profile: String,
    #[arg(required = true)]
    pub snapshots: Vec<String>,
    /// Also delete pinned snapshots.
    #[arg(long)]
    pub force: bool,
    #[arg(long, short)]
    pub yes: bool,
}

#[derive(Args)]
#[command(after_help = "Examples:\n  \
    flux diff home                    latest snapshot vs. the one before\n  \
    flux diff home 2026-11-01         that snapshot vs. latest\n  \
    flux diff home --live             latest snapshot vs. files on disk now\n  \
    flux diff home A B --path ~/Docs  only below ~/Docs")]
pub struct DiffArgs {
    pub profile: String,
    /// Older snapshot (default: the one before NEW).
    pub old: Option<String>,
    /// Newer snapshot (default: latest).
    pub new: Option<String>,
    /// Compare OLD (default: latest) with the files on disk now.
    #[arg(long, conflicts_with = "new")]
    pub live: bool,
    /// Only show changes below this path.
    #[arg(long)]
    pub path: Option<PathBuf>,
    /// Only print the totals.
    #[arg(long)]
    pub stat: bool,
}

#[derive(Args)]
pub struct FindArgs {
    pub profile: String,
    /// Plain text matches part of a name, in any case (`report`); `*`, `?` and `[…]` make
    /// a glob. Without '/', names (`*.odt`); with '/', full paths
    /// (`/home/*/Documents/**/*.pdf`).
    pub pattern: String,
    /// Case-insensitive (always, for plain text).
    #[arg(short = 'i')]
    pub ignore_case: bool,
    /// Search only this snapshot.
    #[arg(long, short)]
    pub snapshot: Option<String>,
}

#[derive(Args)]
pub struct HistoryArgs {
    pub profile: String,
    pub path: PathBuf,
}

#[derive(Args)]
pub struct LogArgs {
    pub profile: String,
    /// Show the events of one run (`last` for the latest).
    #[arg(long)]
    pub run: Option<String>,
    /// Number of runs to list.
    #[arg(short = 'n', default_value_t = 20)]
    pub count: usize,
}

pub fn run_prune(ctx: &Ctx, args: PruneArgs) -> anyhow::Result<ExitCode> {
    let (_, profile, repo) = repo_for(ctx, &args.profile)?;
    let r = repo.prune(&profile.retention, args.dry_run, &silent)?;
    if ctx.json {
        output::print_json(&r)?;
        return Ok(ExitCode::SUCCESS);
    }
    let rows: Vec<Vec<String>> = r
        .decisions
        .iter()
        .map(|d| {
            vec![
                d.id.clone(),
                when(d.time),
                if d.keep {
                    "keep".into()
                } else if args.dry_run {
                    "would remove".into()
                } else {
                    "removed".into()
                },
                if d.expired {
                    "older than max_age".into()
                } else {
                    d.reasons
                        .iter()
                        .map(|x| x.label())
                        .collect::<Vec<_>>()
                        .join(", ")
                },
            ]
        })
        .collect();
    output::table(&["SNAPSHOT", "WHEN", "ACTION", "WHY"], &rows);
    let verb = if args.dry_run {
        "Would remove"
    } else {
        "Removed"
    };
    println!(
        "\n{verb} {} of {} snapshots.",
        r.removed.len(),
        r.decisions.len()
    );
    if !r.removed_for_space.is_empty() {
        println!(
            "Also removed for free space: {}",
            r.removed_for_space.join(", ")
        );
    }
    if let Some(freed) = r.bytes_freed {
        println!("Reclaimed {} of storage.", ByteSize(freed).display_approx());
    }
    if r.space_floor_unmet {
        println!("Warning: the target is below the profile's free-space minimum.");
    }
    if !profile.retention.enabled {
        println!("Retention is disabled for this profile; nothing is pruned automatically.");
    }
    Ok(ExitCode::SUCCESS)
}

pub fn run_name(ctx: &Ctx, args: NameArgs) -> anyhow::Result<ExitCode> {
    let (_, _, repo) = repo_for(ctx, &args.profile)?;
    let s = repo.find(&args.snapshot)?;
    if let Some(n) = &args.name
        && (n.trim().is_empty()
            || repo
                .snapshots()?
                .iter()
                .any(|o| o.id != s.id && o.name.as_deref() == Some(n)))
    {
        bail!("name {n:?} is empty or already used by another snapshot");
    }
    let m = repo.update(&s.id, Some(args.name.clone()), None)?;
    match &m.name {
        Some(n) => println!("Snapshot {} is now named {n:?}.", m.id),
        None => println!("Removed the name of snapshot {}.", m.id),
    }
    Ok(ExitCode::SUCCESS)
}

pub fn run_pin(ctx: &Ctx, args: PinArgs, pinned: bool) -> anyhow::Result<ExitCode> {
    let (_, _, repo) = repo_for(ctx, &args.profile)?;
    let s = repo.find(&args.snapshot)?;
    repo.update(&s.id, None, Some(pinned))?;
    println!(
        "Snapshot {} {}.",
        s.id,
        if pinned {
            "is pinned and will never be pruned"
        } else {
            "is no longer pinned"
        }
    );
    Ok(ExitCode::SUCCESS)
}

pub fn run_delete(ctx: &Ctx, args: DeleteArgs) -> anyhow::Result<ExitCode> {
    let (_, _, repo) = repo_for(ctx, &args.profile)?;
    let snaps = args
        .snapshots
        .iter()
        .map(|s| repo.find(s))
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(p) = snaps.iter().find(|s| s.pinned && !args.force) {
        bail!("snapshot {} is pinned; unpin it or use --force", p.id);
    }
    let ids: Vec<&str> = snaps.iter().map(|s| s.id.as_str()).collect();
    if !args.yes && !output::confirm(&format!("Permanently delete {}?", ids.join(", ")))? {
        bail!("not confirmed (use --yes)");
    }
    let freed = repo.delete(&ids, &flux_core::progress::silent)?;
    for id in &ids {
        println!("Deleted {id}");
    }
    if let Some(b) = freed {
        println!("Freed {}.", flux_core::ByteSize(b).display_approx());
    }
    Ok(ExitCode::SUCCESS)
}

fn rel_bytes(p: &std::path::Path) -> Vec<u8> {
    let b = p.as_os_str().as_bytes();
    let b = b.strip_prefix(b"/").unwrap_or(b);
    b.strip_suffix(b"/").unwrap_or(b).to_vec()
}

pub fn run_diff(ctx: &Ctx, args: DiffArgs) -> anyhow::Result<ExitCode> {
    let (id, profile, repo) = repo_for(ctx, &args.profile)?;
    let snaps = repo.snapshots()?;
    let prefix = args
        .path
        .map(absolutize)
        .transpose()?
        .map(|p| rel_bytes(&p))
        .unwrap_or_default();

    let (old_label, new_label, changes, summary);
    if args.live {
        let old = repo.find(args.old.as_deref().unwrap_or("latest"))?;
        let progress = Arc::new(Progress::new());
        crate::backup_cmd::install_ctrlc(&progress);
        if !ctx.json {
            eprintln!("Scanning current files…");
        }
        let live = flux_runner::repo::scan_live(&id, &profile, &progress, &|ev: Event| {
            if let Event::Warning { path, message } = ev {
                eprintln!("warning: {path}: {message}");
            }
        })?;
        (changes, summary) = diff(repo.entries(&old)?, live.into_iter().map(Ok), &prefix)?;
        old_label = old.id;
        new_label = "now".to_owned();
    } else {
        let new = repo.find(args.new.as_deref().unwrap_or("latest"))?;
        let old = match &args.old {
            Some(o) => repo.find(o)?,
            // The one before in the list (oldest first). Not by comparing times: some
            // tools record start times to the second only, so two snapshots can tie.
            None => snaps
                .iter()
                .position(|s| s.id == new.id)
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| snaps.get(i))
                .cloned()
                .context("there is no snapshot before the latest one to compare with")?,
        };
        (changes, summary) = diff(repo.entries(&old)?, repo.entries(&new)?, &prefix)?;
        old_label = old.id;
        new_label = new.id;
    }

    if ctx.json {
        output::print_json(
            &serde_json::json!({ "old": old_label, "new": new_label, "summary": summary, "changes": changes }),
        )?;
        return Ok(ExitCode::SUCCESS);
    }
    if !args.stat {
        for c in &changes {
            let detail = match c.change {
                ChangeKind::Modified => {
                    let f: Vec<String> = c
                        .fields
                        .iter()
                        .map(|f| format!("{f:?}").to_lowercase())
                        .collect();
                    let size = match (c.old_size, c.new_size) {
                        (Some(a), Some(b)) if a != b => {
                            format!(
                                ", {} → {}",
                                ByteSize(a).display_approx(),
                                ByteSize(b).display_approx()
                            )
                        }
                        _ => String::new(),
                    };
                    format!("  ({}{size})", f.join(", "))
                }
                _ => String::new(),
            };
            let slash = if c.kind == Kind::Dir { "/" } else { "" };
            println!("{} {}{slash}{detail}", c.change.marker(), c.display_path());
        }
        if !changes.is_empty() {
            println!();
        }
    }
    let delta = summary.bytes_delta;
    println!(
        "{old_label} → {new_label}: {} added, {} removed, {} modified, {} changed type, {}{}",
        group(summary.added),
        group(summary.removed),
        group(summary.modified),
        group(summary.type_changed),
        if delta < 0 { "−" } else { "+" },
        ByteSize(delta.unsigned_abs()).display_approx()
    );
    Ok(ExitCode::SUCCESS)
}

pub fn run_find(ctx: &Ctx, args: FindArgs) -> anyhow::Result<ExitCode> {
    let (_, _, repo) = repo_for(ctx, &args.profile)?;
    let snaps = match &args.snapshot {
        Some(s) => vec![repo.find(s)?],
        None => repo.snapshots()?,
    };
    let full_path = args.pattern.contains('/');
    // Plain text: part of a name, like the app's search box.
    let plain = !args.pattern.contains(['*', '?', '[']);
    let pattern = if plain {
        format!("*{}*", args.pattern.trim_start_matches('/'))
    } else {
        args.pattern.trim_start_matches('/').to_owned()
    };
    let glob = globset::GlobBuilder::new(&pattern)
        .literal_separator(full_path && !plain)
        .case_insensitive(args.ignore_case || plain)
        .build()
        .with_context(|| format!("invalid pattern {:?}", args.pattern))?
        .compile_matcher();
    let matcher = move |path: &[u8]| {
        let candidate = if full_path {
            path
        } else {
            path.rsplit(|&b| b == b'/').next().unwrap_or(path)
        };
        glob.is_match(std::ffi::OsStr::from_bytes(candidate))
    };
    let open = |s: &SnapshotMeta| repo.entries(s);
    let found = history::find(&open, &snaps, &matcher)?;
    if ctx.json {
        output::print_json(&found)?;
        return Ok(ExitCode::SUCCESS);
    }
    if found.is_empty() {
        println!("No matches in {} snapshots.", snaps.len());
        return Ok(ExitCode::SUCCESS);
    }
    let rows: Vec<Vec<String>> = found
        .iter()
        .map(|f| {
            vec![
                f.path.clone(),
                if f.kind == Kind::File {
                    ByteSize(f.latest_size).display_approx()
                } else {
                    String::new()
                },
                group(u64::from(f.versions)),
                group(u64::from(f.snapshots)),
                f.latest.clone(),
            ]
        })
        .collect();
    output::table(
        &[
            "PATH",
            "SIZE",
            "VERSIONS",
            "IN SNAPSHOTS",
            "NEWEST SNAPSHOT",
        ],
        &rows,
    );
    Ok(ExitCode::SUCCESS)
}

pub fn run_history(ctx: &Ctx, args: HistoryArgs) -> anyhow::Result<ExitCode> {
    let (_, _, repo) = repo_for(ctx, &args.profile)?;
    let path = absolutize(args.path)?;
    let open = |s: &SnapshotMeta| repo.entries(s);
    let versions = history::history(&open, &repo.snapshots()?, &rel_bytes(&path))?;
    if ctx.json {
        output::print_json(&versions)?;
        return Ok(ExitCode::SUCCESS);
    }
    if versions.is_empty() {
        bail!("{} is not in any snapshot", path.display());
    }
    let rows: Vec<Vec<String>> = versions
        .iter()
        .rev()
        .map(|v| {
            let mtime = jiff::Timestamp::from_second(v.mtime)
                .map(|t| {
                    t.to_zoned(jiff::tz::TimeZone::system())
                        .strftime("%Y-%m-%d %H:%M")
                        .to_string()
                })
                .unwrap_or_default();
            vec![
                mtime,
                ByteSize(v.size).display_approx(),
                if v.first == v.last {
                    v.first.clone()
                } else {
                    format!("{} … {}", v.first, v.last)
                },
                group(u64::from(v.snapshots)),
            ]
        })
        .collect();
    println!("{} — {} versions\n", path.display(), versions.len());
    output::table(&["MODIFIED", "SIZE", "SNAPSHOTS", "COUNT"], &rows);
    println!(
        "\nRestore a version with: flux restore {} {} --snapshot <SNAPSHOT>",
        args.profile,
        path.display()
    );
    Ok(ExitCode::SUCCESS)
}

pub fn run_log(ctx: &Ctx, args: LogArgs) -> anyhow::Result<ExitCode> {
    let (id, _) = load_profile(ctx, &args.profile)?;
    if let Some(run) = &args.run {
        let run_id = if run == "last" {
            runlog::recent_runs(&ctx.paths, &id, 1)
                .into_iter()
                .next()
                .map(|r| r.run_id)
                .context("no runs recorded yet")?
        } else {
            run.clone()
        };
        let events = runlog::run_events(&ctx.paths, &id, &run_id);
        if ctx.json {
            output::print_json(&events)?;
            return Ok(ExitCode::SUCCESS);
        }
        if events.is_empty() {
            bail!("no event log for run {run_id} (only the last 50 runs keep one)");
        }
        for e in events {
            let t =
                e.t.to_zoned(jiff::tz::TimeZone::system())
                    .strftime("%H:%M:%S");
            match e.event {
                Event::Phase { phase } => println!("{t}  ── {}", phase.label()),
                Event::Warning { path, message } => println!("{t}  warning: {path}: {message}"),
                Event::Info { message } => println!("{t}  {message}"),
            }
        }
        return Ok(ExitCode::SUCCESS);
    }
    let runs = runlog::recent_runs(&ctx.paths, &id, args.count);
    if ctx.json {
        output::print_json(&runs)?;
        return Ok(ExitCode::SUCCESS);
    }
    if runs.is_empty() {
        println!("No runs recorded yet.");
        return Ok(ExitCode::SUCCESS);
    }
    let rows: Vec<Vec<String>> = runs
        .iter()
        .map(|r| {
            let secs = (r.finished.as_millisecond() - r.started.as_millisecond()) as f64 / 1000.0;
            let note = r.error.clone().unwrap_or_else(|| {
                let mut n = Vec::new();
                if r.warnings > 0 {
                    n.push(format!("{} warnings", r.warnings));
                }
                if !r.pruned.is_empty() {
                    n.push(format!("pruned {}", r.pruned.len()));
                }
                n.join(", ")
            });
            vec![
                r.run_id.clone(),
                when(r.started),
                duration(secs),
                format!("{:?}", r.trigger).to_lowercase(),
                r.status.label().into(),
                r.snapshot.clone().unwrap_or_default(),
                note,
            ]
        })
        .collect();
    output::table(
        &[
            "RUN", "STARTED", "TOOK", "TRIGGER", "STATUS", "SNAPSHOT", "NOTE",
        ],
        &rows,
    );
    Ok(ExitCode::SUCCESS)
}
