//! `flux schedule …`, `flux status`, `flux watch`, `flux cancel`.

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;

use anyhow::{Context, bail};
use clap::{Args, Subcommand};

use flux_core::profile::has_errors;
use flux_core::progress::{Event, Progress};
use flux_core::{ProfileId, ProfileStore};
use flux_runner::jobs::{self, Client, Message};
use flux_runner::runlog;
use flux_runner::schedule::{self, Systemd, UnitEnv};

use crate::backup_cmd::{LiveProgress, load_profile, when};
use crate::{Ctx, output};

#[derive(Subcommand)]
pub enum ScheduleCmd {
    /// Install or update the systemd timer for a profile, per its [schedule] settings.
    Install { profile: String },
    /// Remove a profile's timer (the profile itself is kept).
    Remove { profile: String },
    /// Make installed timers match all profiles (install, update, remove).
    Sync,
    /// Show timers, next and last runs.
    Status,
}

#[derive(Args)]
pub struct ProfileArg {
    pub profile: String,
}

#[derive(Args)]
pub struct StatusArgs {
    /// Only this profile.
    pub profile: Option<String>,
}

fn print_applied(id: &ProfileId, a: &schedule::Applied) {
    for n in &a.written {
        println!("{id}: wrote {n}");
    }
    for n in &a.removed {
        println!("{id}: removed {n}");
    }
    if !a.changed() {
        println!("{id}: up to date");
    }
}

pub fn run_schedule(ctx: &Ctx, cmd: ScheduleCmd) -> anyhow::Result<ExitCode> {
    let sd = Systemd::new(&ctx.paths);
    let env = UnitEnv::current(ctx.paths.scope);
    match cmd {
        ScheduleCmd::Install { profile } => {
            let (id, p) = load_profile(ctx, &profile)?;
            let a = schedule::apply(&sd, &id, &p, &env)?;
            if ctx.json {
                output::print_json(&a)?;
                return Ok(ExitCode::SUCCESS);
            }
            if p.schedule.trigger == flux_core::profile::Trigger::Manual && !p.schedule.on_connect {
                println!("{id} is set to manual backups; there is nothing to schedule.");
                println!(
                    "Set `trigger = \"calendar\"` (with on_calendar) or `\"interval\"` (with every) under [schedule]."
                );
                if a.changed() {
                    print_applied(&id, &a);
                }
                return Ok(ExitCode::SUCCESS);
            }
            print_applied(&id, &a);
            let st = schedule::status(&sd, &id, &p, &env);
            if let Some(next) = st.next_run {
                println!("{id}: next run {next}");
            }
            linger_hint(&sd);
        }
        ScheduleCmd::Remove { profile } => {
            let id = ProfileId::new(profile)?;
            let a = schedule::remove(&sd, &id)?;
            if ctx.json {
                output::print_json(&a)?;
            } else if a.removed.is_empty() {
                println!("{id} has no timer installed.");
            } else {
                print_applied(&id, &a);
            }
        }
        ScheduleCmd::Sync => {
            let store = ProfileStore::new(&ctx.paths);
            let mut results = Vec::new();
            let mut known = Vec::new();
            for l in store.list()? {
                known.push(l.id.clone());
                match l.profile {
                    Ok(p) if !has_errors(&p.validate()) => {
                        let a = schedule::apply(&sd, &l.id, &p, &env)?;
                        results.push((l.id, a));
                    }
                    _ => eprintln!("{}: skipped (profile has errors)", l.id),
                }
            }
            for id in schedule::installed_ids(&sd) {
                if !known.contains(&id) {
                    let a = schedule::remove(&sd, &id)?;
                    results.push((id, a));
                }
            }
            if ctx.json {
                let v: Vec<_> = results
                    .iter()
                    .map(|(id, a)| serde_json::json!({ "profile": id, "applied": a }))
                    .collect();
                output::print_json(&v)?;
            } else {
                for (id, a) in &results {
                    print_applied(id, a);
                }
            }
        }
        ScheduleCmd::Status => {
            let store = ProfileStore::new(&ctx.paths);
            let mut rows = Vec::new();
            let mut all = Vec::new();
            for l in store.list()? {
                let Ok(p) = l.profile else { continue };
                let st = schedule::status(&sd, &l.id, &p, &env);
                let last = runlog::recent_runs(&ctx.paths, &l.id, 1).into_iter().next();
                rows.push(vec![
                    l.id.to_string(),
                    st.schedule.clone(),
                    if st.out_of_date && st.installed {
                        "out of date (run `flux schedule sync`)".into()
                    } else if st.out_of_date {
                        "not installed".into()
                    } else if !st.installed {
                        "—".into()
                    } else if st.timer_active {
                        "active".into()
                    } else {
                        "installed".into()
                    },
                    st.next_run.clone().unwrap_or_default(),
                    last.as_ref()
                        .map(|r| format!("{} ({})", when(r.started), r.status.label()))
                        .unwrap_or_default(),
                ]);
                all.push(serde_json::json!({ "status": st, "last_run": last }));
            }
            if ctx.json {
                output::print_json(&all)?;
            } else {
                output::table(
                    &["PROFILE", "SCHEDULE", "TIMER", "NEXT RUN", "LAST RUN"],
                    &rows,
                );
                linger_hint(&sd);
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn linger_hint(sd: &Systemd) {
    if sd.scope == flux_core::Scope::User && schedule::linger_enabled() == Some(false) {
        println!(
            "\nNote: your timers run only while you are logged in. To back up while logged out, run\n  loginctl enable-linger"
        );
    }
}

/// `flux status`: what's running, how the last runs went, what's next.
pub fn run_status(ctx: &Ctx, args: StatusArgs) -> anyhow::Result<ExitCode> {
    let store = ProfileStore::new(&ctx.paths);
    let sd = Systemd::new(&ctx.paths);
    let env = UnitEnv::current(ctx.paths.scope);
    let running = jobs::running(&ctx.paths);
    let mut rows = Vec::new();
    let mut json = Vec::new();
    for l in store.list()? {
        if args.profile.as_deref().is_some_and(|p| p != l.id.as_str()) {
            continue;
        }
        let Ok(p) = l.profile else { continue };
        let job = running.iter().find(|j| j.profile == l.id.as_str());
        let live = job.and_then(|_| first_progress(ctx, &l.id));
        let last = runlog::recent_runs(&ctx.paths, &l.id, 1).into_iter().next();
        let st = schedule::status(&sd, &l.id, &p, &env);
        let now = match (&job, &live) {
            (Some(_), Some(s)) if s.bytes_total > 0 => format!(
                "running: {} {:.0}%",
                s.phase.label(),
                s.bytes_done as f64 * 100.0 / s.bytes_total.max(1) as f64
            ),
            (Some(_), Some(s)) => format!("running: {}", s.phase.label()),
            (Some(_), None) => "running".into(),
            _ => "idle".into(),
        };
        rows.push(vec![
            l.id.to_string(),
            now,
            last.as_ref()
                .map(|r| format!("{} ({})", when(r.started), r.status.label()))
                .unwrap_or_else(|| "never".into()),
            st.next_run.clone().unwrap_or_else(|| {
                if st.schedule == "manual" {
                    "manual".into()
                } else {
                    String::new()
                }
            }),
        ]);
        json.push(serde_json::json!({ "profile": l.id, "running": job, "progress": live, "last_run": last, "schedule": st }));
    }
    if ctx.json {
        output::print_json(&json)?;
    } else if rows.is_empty() {
        println!(
            "No profiles yet. Create one with `{}`.",
            if ctx.paths.scope == flux_core::Scope::System {
                "sudo flux --system profile create"
            } else {
                "flux profile create"
            }
        );
    } else {
        output::table(&["PROFILE", "NOW", "LAST RUN", "NEXT RUN"], &rows);
    }
    Ok(ExitCode::SUCCESS)
}

fn first_progress(ctx: &Ctx, id: &ProfileId) -> Option<flux_core::progress::ProgressSnapshot> {
    let mut c = Client::connect(&ctx.paths, id).ok()?;
    for _ in 0..1000 {
        match c.next_message().ok()?? {
            Message::Progress { progress } => return Some(progress),
            Message::Finished { .. } => return None,
            _ => {}
        }
    }
    None
}

/// `flux watch`: follow a running backup until it ends.
pub fn run_watch(ctx: &Ctx, args: ProfileArg) -> anyhow::Result<ExitCode> {
    let id = ProfileId::new(args.profile)?;
    let mut c = Client::connect(&ctx.paths, &id)
        .with_context(|| format!("no backup of {id} is running"))?;
    let local = Arc::new(Progress::new());
    let live = LiveProgress::start(
        local.clone(),
        !ctx.json && std::io::IsTerminal::is_terminal(&std::io::stderr()),
    );
    let _ = ctrlc::set_handler(|| {
        eprintln!("\nDetached. The backup keeps running; stop it with `flux cancel`.");
        std::process::exit(0);
    });
    let mut result = None;
    while let Some(m) = c.next_message()? {
        if ctx.json {
            output::print_json_line(&m)?;
        }
        match m {
            Message::Hello { info } if !ctx.json => live.println(format!(
                "Watching {} (run {}, started {})",
                info.profile,
                info.run_id,
                when(info.started)
            )),
            Message::Progress { progress: s } => {
                if local.phase() != s.phase {
                    local.set_phase(s.phase);
                }
                local.scanned.store(s.scanned, Relaxed);
                local.entries_total.store(s.entries_total, Relaxed);
                local.entries_done.store(s.entries_done, Relaxed);
                local.bytes_total.store(s.bytes_total, Relaxed);
                local.bytes_done.store(s.bytes_done, Relaxed);
            }
            Message::Event {
                event: Event::Warning { path, message },
            } if !ctx.json => live.println(format!("warning: {path}: {message}")),
            Message::Event {
                event: Event::Info { message },
            } if !ctx.json => live.println(message),
            Message::Finished { status, error } => {
                result = Some((status, error));
                break;
            }
            _ => {}
        }
    }
    drop(live);
    match result {
        Some((status, error)) if !ctx.json => {
            println!(
                "Backup of {id} {}{}",
                status.label(),
                error.map(|e| format!(": {e}")).unwrap_or_default()
            );
            Ok(if status.is_success() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        Some(_) => Ok(ExitCode::SUCCESS),
        None => bail!("lost connection to the backup of {id}"),
    }
}

/// `flux cancel`: ask a running backup to stop.
pub fn run_cancel(ctx: &Ctx, args: ProfileArg) -> anyhow::Result<ExitCode> {
    let id = ProfileId::new(args.profile)?;
    let mut c = Client::connect(&ctx.paths, &id)
        .with_context(|| format!("no backup of {id} is running"))?;
    c.cancel()?;
    if !ctx.json {
        eprintln!("Asked the backup of {id} to stop; waiting for it to finish the current files…");
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    while std::time::Instant::now() < deadline {
        match c.next_message()? {
            Some(Message::Finished { status, .. }) => {
                if ctx.json {
                    output::print_json(&serde_json::json!({ "profile": id, "status": status }))?;
                } else {
                    println!(
                        "Backup of {id} {}. The partial snapshot is kept and resumed next time.",
                        status.label()
                    );
                }
                return Ok(ExitCode::SUCCESS);
            }
            Some(_) => {}
            None => break,
        }
    }
    println!("The backup of {id} has stopped.");
    Ok(ExitCode::SUCCESS)
}
