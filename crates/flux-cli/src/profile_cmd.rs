//! `flux profile …`

use std::path::PathBuf;
use std::process::{Command, ExitCode};

use anyhow::{Context, bail};
use clap::{Subcommand, ValueEnum};
use flux_core::profile::{VaultOptions, has_errors};
use flux_core::{Engine, Issue, Profile, ProfileId, ProfileStore, Target};
use serde::Serialize;

use crate::{Ctx, EXIT_INVALID, output, target_spec};

#[derive(Subcommand)]
pub enum ProfileCmd {
    /// List all profiles.
    #[command(alias = "ls")]
    List,
    /// Show a profile's settings.
    Show { id: String },
    /// Create a profile.
    #[command(after_help = "Examples:\n  \
        flux profile create home --target /run/media/me/Backup --source ~\n  \
        flux profile create docs --target me@nas:/srv/backup --source ~/Documents --engine vault")]
    Create {
        /// Profile id (lowercase letters, digits, '-' and '_').
        id: String,
        /// Display name (defaults to the id).
        #[arg(long)]
        name: Option<String>,
        /// Where backups go: /local/path, [user@]host:/path or ssh://[user@]host[:port]/path.
        #[arg(long, short)]
        target: String,
        /// File or folder to back up (repeatable).
        #[arg(long = "source", short = 's', required = true)]
        sources: Vec<PathBuf>,
        /// Exclude pattern (repeatable).
        #[arg(long, short = 'x')]
        exclude: Vec<String>,
        #[arg(long, value_enum, default_value_t = EngineArg::Mirror)]
        engine: EngineArg,
    },
    /// Open a profile in $EDITOR, then validate it.
    Edit { id: String },
    /// Delete a profile (existing backups are left untouched).
    #[command(alias = "rm")]
    Delete {
        id: String,
        /// Do not ask for confirmation.
        #[arg(long, short)]
        yes: bool,
    },
    /// Check profiles for errors (all profiles when no id is given).
    Validate { ids: Vec<String> },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum EngineArg {
    /// Hard-link snapshots as plain folders.
    Mirror,
    /// Deduplicated, compressed, encrypted repository.
    Vault,
}

pub fn run(ctx: &Ctx, cmd: ProfileCmd) -> anyhow::Result<ExitCode> {
    let store = ProfileStore::new(&ctx.paths);
    match cmd {
        ProfileCmd::List => list(ctx, &store),
        ProfileCmd::Show { id } => show(ctx, &store, &parse_id(&id)?),
        ProfileCmd::Create {
            id,
            name,
            target,
            sources,
            exclude,
            engine,
        } => {
            let id = parse_id(&id)?;
            let target = target_spec::parse(&target)?;
            let sources = sources
                .into_iter()
                .map(absolutize)
                .collect::<anyhow::Result<_>>()?;
            let mut p = Profile::new(name.unwrap_or_else(|| id.to_string()), target, sources);
            p.sources.exclude = exclude;
            if let EngineArg::Vault = engine {
                p.engine = Engine::Vault;
                p.vault = Some(VaultOptions::default());
            }
            let issues = p.validate_in(ctx.paths.scope);
            report_issues(&id, &issues);
            if has_errors(&issues) {
                return Ok(ExitCode::from(EXIT_INVALID));
            }
            let path = store.create(&id, &p)?;
            if ctx.json {
                output::print_json(&serde_json::json!({ "id": id, "path": path }))?;
            } else {
                println!("Created profile {id} ({})", path.display());
            }
            Ok(ExitCode::SUCCESS)
        }
        ProfileCmd::Edit { id } => edit(ctx, &store, &parse_id(&id)?),
        ProfileCmd::Delete { id, yes } => {
            let id = parse_id(&id)?;
            if !store.exists(&id) {
                bail!("profile {id:?} not found");
            }
            if !yes
                && !output::confirm(&format!(
                    "Delete profile {id}? Backups already made are kept."
                ))?
            {
                bail!("not confirmed (use --yes to delete without asking)");
            }
            store.delete(&id)?;
            let sd = flux_runner::schedule::Systemd::new(&ctx.paths);
            match flux_runner::schedule::remove(&sd, &id) {
                Ok(a) if !a.removed.is_empty() && !ctx.json => println!("Removed its timer."),
                Err(e) => eprintln!("warning: could not remove the timer: {e}"),
                _ => {}
            }
            if !ctx.json {
                println!("Deleted profile {id}");
            }
            Ok(ExitCode::SUCCESS)
        }
        ProfileCmd::Validate { ids } => validate(ctx, &store, ids),
    }
}

fn parse_id(s: &str) -> anyhow::Result<ProfileId> {
    Ok(ProfileId::new(s)?)
}

fn absolutize(p: PathBuf) -> anyhow::Result<PathBuf> {
    let p = if p.is_absolute() {
        p
    } else {
        std::env::current_dir()?.join(p)
    };
    // Normalise `.`/`..` lexically; the path need not exist yet.
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    Ok(out)
}

fn target_summary(t: &Target) -> String {
    match t {
        Target::Local { path, .. } => path.display().to_string(),
        Target::Ssh {
            host,
            user,
            port,
            path,
            ..
        } => {
            let user = user.as_deref().map(|u| format!("{u}@")).unwrap_or_default();
            let port = port.map(|p| format!(":{p}")).unwrap_or_default();
            format!(
                "ssh://{user}{host}{port}{}{}",
                if path.is_absolute() { "" } else { "/~/" },
                path.display()
            )
        }
    }
}

#[derive(Serialize)]
struct ListRow<'a> {
    id: &'a ProfileId,
    path: &'a std::path::Path,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile: Option<&'a Profile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn list(ctx: &Ctx, store: &ProfileStore) -> anyhow::Result<ExitCode> {
    let profiles = store.list()?;
    if ctx.json {
        let rows: Vec<_> = profiles
            .iter()
            .map(|l| ListRow {
                id: &l.id,
                path: &l.path,
                profile: l.profile.as_ref().ok(),
                error: l.profile.as_ref().err().map(|e| format!("{e:#}")),
            })
            .collect();
        output::print_json(&rows)?;
        return Ok(ExitCode::SUCCESS);
    }
    if profiles.is_empty() {
        println!(
            "No profiles yet. Create one with `{}`.",
            if ctx.paths.scope == flux_core::Scope::System {
                "sudo flux --system profile create"
            } else {
                "flux profile create"
            }
        );
        return Ok(ExitCode::SUCCESS);
    }
    let rows: Vec<Vec<String>> = profiles
        .iter()
        .map(|l| match &l.profile {
            Ok(p) => vec![
                l.id.to_string(),
                p.name.clone(),
                p.engine.to_string(),
                target_summary(&p.target),
                schedule_summary(p),
            ],
            Err(e) => vec![
                l.id.to_string(),
                format!("<invalid: {e}>"),
                String::new(),
                String::new(),
                String::new(),
            ],
        })
        .collect();
    output::table(&["ID", "NAME", "ENGINE", "TARGET", "SCHEDULE"], &rows);
    Ok(ExitCode::SUCCESS)
}

fn schedule_summary(p: &Profile) -> String {
    use flux_core::profile::Trigger;
    let s = &p.schedule;
    let mut out = match s.trigger {
        Trigger::Manual => "manual".to_owned(),
        Trigger::Calendar => s.on_calendar.clone().unwrap_or_default(),
        Trigger::Interval => s.every.map(|e| format!("every {e}")).unwrap_or_default(),
    };
    if s.on_connect {
        out.push_str(" + on connect");
    }
    out
}

fn show(ctx: &Ctx, store: &ProfileStore, id: &ProfileId) -> anyhow::Result<ExitCode> {
    let p = store.load(id)?;
    if ctx.json {
        output::print_json(
            &serde_json::json!({ "id": id, "profile": p, "issues": p.validate_in(ctx.paths.scope) }),
        )?;
    } else {
        print!("{}", p.to_toml()?);
        report_issues(id, &p.validate_in(ctx.paths.scope));
    }
    Ok(ExitCode::SUCCESS)
}

fn report_issues(id: &ProfileId, issues: &[Issue]) {
    for i in issues {
        eprintln!("{id}: {i}");
    }
}

fn validate(ctx: &Ctx, store: &ProfileStore, ids: Vec<String>) -> anyhow::Result<ExitCode> {
    #[derive(Serialize)]
    struct Res {
        id: String,
        ok: bool,
        issues: Vec<Issue>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    }
    let mut results = Vec::new();
    let entries: Vec<(ProfileId, Result<Profile, String>)> = if ids.is_empty() {
        store
            .list()?
            .into_iter()
            .map(|l| (l.id, l.profile.map_err(|e| format!("{e:#}"))))
            .collect()
    } else {
        ids.iter()
            .map(|s| {
                let id = parse_id(s)?;
                let p = store.load(&id).map_err(|e| format!("{e:#}"));
                Ok((id, p))
            })
            .collect::<anyhow::Result<_>>()?
    };
    for (id, p) in entries {
        match p {
            Ok(p) => {
                let issues = p.validate_in(ctx.paths.scope);
                results.push(Res {
                    id: id.to_string(),
                    ok: !has_errors(&issues),
                    issues,
                    error: None,
                })
            }
            Err(e) => results.push(Res {
                id: id.to_string(),
                ok: false,
                issues: vec![],
                error: Some(e),
            }),
        }
    }
    let all_ok = results.iter().all(|r| r.ok);
    if ctx.json {
        output::print_json(&results)?;
    } else {
        for r in &results {
            if let Some(e) = &r.error {
                println!("✗ {}: {e}", r.id);
                continue;
            }
            println!("{} {}", if r.ok { "✓" } else { "✗" }, r.id);
            for i in &r.issues {
                println!("    {i}");
            }
        }
    }
    Ok(if all_ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_INVALID)
    })
}

/// Keep the profile's timer in step after an edit (only if it has or needs one).
fn sync_schedule(ctx: &Ctx, id: &ProfileId, p: &Profile) {
    use flux_runner::schedule;
    let sd = schedule::Systemd::new(&ctx.paths);
    let needs = p.schedule.trigger != flux_core::profile::Trigger::Manual || p.schedule.on_connect;
    if !needs && !schedule::installed_ids(&sd).contains(id) {
        return;
    }
    match schedule::apply(&sd, id, p, &schedule::UnitEnv::current(ctx.paths.scope)) {
        Ok(a) if a.changed() => println!("Updated the timer ({}).", schedule::summary(p)),
        Ok(_) => {}
        Err(e) => eprintln!("warning: profile saved, but the timer could not be updated: {e}"),
    }
}

fn edit(ctx: &Ctx, store: &ProfileStore, id: &ProfileId) -> anyhow::Result<ExitCode> {
    let path = store.path_of(id);
    let original = std::fs::read_to_string(&path)
        .with_context(|| format!("profile {id:?} not found at {}", path.display()))?;
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".into());

    // Private (0700) temp dir: profiles may contain password commands or hook secrets.
    let dir = tempfile::Builder::new().prefix("flux-edit-").tempdir()?;
    let tmp = dir.path().join(format!("{id}.toml"));
    std::fs::write(&tmp, &original)?;
    loop {
        let status = Command::new("sh")
            .arg("-c")
            .arg(format!("{editor} \"$1\""))
            .arg("flux-edit")
            .arg(&tmp)
            .status()
            .with_context(|| format!("cannot start editor {editor:?}"))?;
        if !status.success() {
            bail!("editor exited with {status}; profile unchanged");
        }
        let text = std::fs::read_to_string(&tmp)?;
        if text == original {
            println!("No changes.");
            break;
        }
        let problem = match Profile::from_toml(&text) {
            Ok(p) => {
                let issues = p.validate_in(ctx.paths.scope);
                report_issues(id, &issues);
                if has_errors(&issues) {
                    Some("the profile has errors".to_owned())
                } else {
                    store.save(id, &p)?;
                    sync_schedule(ctx, id, &p);
                    println!("Saved {}", path.display());
                    None
                }
            }
            Err(e) => Some(e.to_string()),
        };
        match problem {
            None => break,
            Some(msg) => {
                eprintln!("{msg}");
                if !output::confirm("Edit again? (No discards your changes)")? {
                    println!("Profile unchanged.");
                    break;
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
