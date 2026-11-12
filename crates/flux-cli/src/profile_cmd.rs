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
    Create {
        /// Profile id (lowercase letters, digits, '-' and '_').
        id: String,
        /// Display name (defaults to the id).
        #[arg(long)]
        name: Option<String>,
        /// Where backups go: /local/path, [user@]host:/path or ssh://[user@]host[:port]/path.
        #[arg(long, short)]
        target: String,
        /// File or folder to back up (repeatable; replaces the template's).
        #[arg(long = "source", short = 's', required_unless_present = "template")]
        sources: Vec<PathBuf>,
        /// Exclude pattern (repeatable; added to the template's).
        #[arg(long, short = 'x')]
        exclude: Vec<String>,
        #[arg(long, value_enum, default_value_t = EngineArg::Mirror)]
        engine: EngineArg,
        /// Start from a template: its folders, exclusions, schedule and what to keep
        /// (`flux profile templates` lists them).
        #[arg(long)]
        template: Option<String>,
    },
    /// The templates new profiles can start from.
    Templates,
    /// Save a profile's settings as a template (without where it backs up to).
    SaveTemplate { id: String, name: String },
    /// Remove one of your templates.
    DeleteTemplate { template: String },
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
    /// Export a profile: as a Flux profile, or for another backup tool.
    #[command(after_help = "Formats:\n  \
        flux       a Flux profile (TOML), for flux profile import\n  \
        borgmatic  a borgmatic configuration (YAML)\n  \
        restic     a restic backup script\n  \
        rsync      an rsync backup script\n  \
        borg       a borg backup script\n\n\
        Whatever a format can't express is listed as a note, in the file and on stderr.")]
    Export {
        id: String,
        /// flux, borgmatic, restic, rsync or borg.
        #[arg(long, short, default_value = "flux")]
        format: String,
        /// Write here instead of printing (a folder gets the suggested file name).
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
    /// Copy a profile under a new id (`<id>-copy`), with its schedule.
    Duplicate { id: String },
    /// Leave a file or folder out of the profiles that back it up (all of them, or those
    /// named with --only): adds an exclusion rule naming just it, or, when it's one of a
    /// profile's folders, stops taking that folder. Says what changes and asks first.
    LeaveOut {
        path: String,
        /// Only this profile (repeatable).
        #[arg(long = "only", value_name = "ID")]
        only: Vec<String>,
        /// Do not ask for confirmation.
        #[arg(long, short)]
        yes: bool,
    },
    /// Back up a folder in a profile: adds it to the profile's folders (in place of any of
    /// them inside it), or removes the exclusion rule that names just this folder. Says
    /// what changes and asks first.
    AddFolder {
        id: String,
        path: String,
        /// Do not ask for confirmation.
        #[arg(long, short)]
        yes: bool,
    },
    /// Import a profile exported with `flux profile export` (or written by hand).
    Import {
        file: PathBuf,
        /// The new profile's id (default: the one it was exported under, or its name).
        #[arg(long)]
        id: Option<String>,
    },
    /// List the profiles set up in Back In Time, or create a Flux profile from one of them
    /// (its folders, exclusions, schedule and what to keep). Back In Time's snapshots are
    /// left as they are; Flux makes its own.
    Backintime {
        /// The Back In Time profile to start from (its number in the list).
        number: Option<u32>,
        /// The new profile's id (default: from its name).
        #[arg(long)]
        id: Option<String>,
        /// How the new backups are stored (default: rsync, plain folders like Back In
        /// Time's; vault for profiles Back In Time encrypted).
        #[arg(long, value_enum)]
        engine: Option<EngineArg>,
    },
    /// More places a profile's backups go: each backup run goes to all of them, one after
    /// the other, each from the sources (independent copies). A destination's snapshots
    /// are those of `<id>+<name>`: `flux snapshots home+nas`.
    #[command(subcommand)]
    Destination(DestinationCmd),
}

#[derive(Subcommand)]
pub enum DestinationCmd {
    /// List a profile's destinations, the main one first.
    List { id: String },
    /// Add a destination.
    Add {
        id: String,
        /// Its name in the profile (`nas`): lowercase letters, digits, '-' and '_'.
        name: String,
        /// Where: /local/path, [user@]host:/path or ssh://[user@]host[:port]/path.
        #[arg(long, short)]
        target: String,
        /// How it's stored (default: mirror; with --replicate, the main destination's).
        #[arg(long, value_enum)]
        engine: Option<EngineArg>,
        /// Copy the main destination's backup after each run instead of backing up from
        /// the sources again (Vault anywhere; Mirror and rsync between local drives).
        #[arg(long)]
        replicate: bool,
        /// What the app calls it ("NAS in the office").
        #[arg(long)]
        label: Option<String>,
        /// Kept away from this computer's building.
        #[arg(long)]
        offsite: bool,
    },
    /// Remove a destination. Its backups stay where they are.
    #[command(alias = "rm")]
    Remove { id: String, name: String },
}

/// An engine and its default options.
fn engine_options(
    e: EngineArg,
) -> (
    Engine,
    Option<VaultOptions>,
    Option<flux_core::profile::BorgOptions>,
) {
    match e {
        EngineArg::Mirror => (Engine::Mirror, None, None),
        EngineArg::Vault => (Engine::Vault, Some(VaultOptions::default()), None),
        EngineArg::Rsync => (Engine::Rsync, None, None),
        EngineArg::Borg => (
            Engine::Borg,
            None,
            Some(flux_core::profile::BorgOptions::default()),
        ),
    }
}

fn destinations(ctx: &Ctx, store: &ProfileStore, cmd: DestinationCmd) -> anyhow::Result<ExitCode> {
    match cmd {
        DestinationCmd::List { id } => {
            let pid = parse_id(&id)?;
            let p = store.load(&pid)?;
            if ctx.json {
                crate::output::print_json(&serde_json::json!({
                    "main": { "id": pid.as_str(), "engine": p.engine.label(), "target": target_summary(&p.target), "offsite": p.offsite },
                    "destinations": p.destinations.iter().map(|d| serde_json::json!({
                        "id": pid.with_destination(&d.name).map(|i| i.to_string()).unwrap_or_default(),
                        "name": d.name, "label": d.label(), "engine": d.engine.label(),
                        "target": target_summary(&d.target), "offsite": d.offsite,
                    })).collect::<Vec<_>>(),
                }))?;
                return Ok(ExitCode::SUCCESS);
            }
            let mut rows = vec![vec![
                pid.to_string(),
                "(main)".into(),
                p.engine.label().into(),
                target_summary(&p.target),
                if p.offsite { "offsite" } else { "" }.into(),
            ]];
            for d in &p.destinations {
                rows.push(vec![
                    pid.with_destination(&d.name)?.to_string(),
                    d.label().into(),
                    d.engine.label().into(),
                    target_summary(&d.target),
                    [
                        (d.mode == flux_core::profile::CopyMode::Replicate)
                            .then_some("copy of main"),
                        d.offsite.then_some("offsite"),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(", "),
                ]);
            }
            crate::output::table(&["ID", "NAME", "ENGINE", "WHERE", ""], &rows);
            Ok(ExitCode::SUCCESS)
        }
        DestinationCmd::Add {
            id,
            name,
            target,
            engine,
            replicate,
            label,
            offsite,
        } => {
            let pid = parse_id(&id)?;
            let mut p = store.load(&pid)?;
            if p.destinations.iter().any(|d| d.name == name) {
                anyhow::bail!("{id} already has a destination named {name:?}");
            }
            let (engine, vault, borg) = match (engine, replicate) {
                (Some(e), _) => engine_options(e),
                // A copy of the main backup: stored the same way (its options come along).
                (None, true) => (p.engine, None, None),
                (None, false) => engine_options(EngineArg::Mirror),
            };
            p.destinations.push(flux_core::profile::Destination {
                name: name.clone(),
                label,
                engine,
                target: crate::target_spec::parse(&target)?,
                offsite,
                mode: if replicate {
                    flux_core::profile::CopyMode::Replicate
                } else {
                    flux_core::profile::CopyMode::Independent
                },
                vault,
                rsync: None,
                borg,
            });
            let issues = p.validate_in(ctx.paths.scope);
            let errors: Vec<_> = issues
                .iter()
                .filter(|i| i.severity == flux_core::profile::Severity::Error)
                .collect();
            if !errors.is_empty() {
                for i in &errors {
                    eprintln!("{id}: {i}");
                }
                anyhow::bail!("the destination wasn't added");
            }
            store.save(&pid, &p)?;
            println!(
                "Backups of {id} also go to {target} now (snapshots: flux snapshots {}).",
                pid.with_destination(&name)?
            );
            Ok(ExitCode::SUCCESS)
        }
        DestinationCmd::Remove { id, name } => {
            let pid = parse_id(&id)?;
            let mut p = store.load(&pid)?;
            let before = p.destinations.len();
            p.destinations.retain(|d| d.name != name);
            if p.destinations.len() == before {
                anyhow::bail!("{id} has no destination named {name:?}");
            }
            store.save(&pid, &p)?;
            println!(
                "Removed destination {name} from {id}. The backups made there stay where they are."
            );
            Ok(ExitCode::SUCCESS)
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub enum EngineArg {
    /// Hard-link snapshots as plain folders.
    Mirror,
    /// Deduplicated, compressed, encrypted repository.
    Vault,
    /// Compatibility mode: plain folders written by the system rsync, usable without Flux.
    Rsync,
    /// Compatibility mode: a standard Borg repository written by the system borg.
    Borg,
}

pub fn run(ctx: &Ctx, cmd: ProfileCmd) -> anyhow::Result<ExitCode> {
    let store = ProfileStore::new(&ctx.paths);
    match cmd {
        ProfileCmd::List => list(ctx, &store),
        ProfileCmd::Destination(cmd) => destinations(ctx, &store, cmd),
        ProfileCmd::Show { id } => show(ctx, &store, &parse_id(&id)?),
        ProfileCmd::Create {
            id,
            name,
            target,
            sources,
            exclude,
            engine,
            template,
        } => {
            let id = parse_id(&id)?;
            let target = target_spec::parse(&target)?;
            let sources: Vec<PathBuf> = sources
                .into_iter()
                .map(absolutize)
                .collect::<anyhow::Result<_>>()?;
            let mut p = match &template {
                Some(t) => {
                    let service = flux_service::Service::new(ctx.paths.clone());
                    let t = own_template(&ctx.paths, t);
                    let mut p = flux_service::edit::new_from_template_impl(&service, t)
                        .map_err(|e| anyhow::anyhow!("{e} (see flux profile templates)"))?
                        .profile;
                    p.target = target;
                    if !sources.is_empty() {
                        p.sources.paths = sources;
                    }
                    p.sources.exclude.extend(exclude);
                    if let Some(n) = name {
                        p.name = n;
                    } else if p.name.is_empty() {
                        p.name = id.to_string();
                    }
                    p
                }
                None => {
                    let mut p =
                        Profile::new(name.unwrap_or_else(|| id.to_string()), target, sources);
                    p.sources.exclude = exclude;
                    p
                }
            };
            match engine {
                EngineArg::Mirror => {}
                EngineArg::Vault => {
                    p.engine = Engine::Vault;
                    p.vault = Some(VaultOptions::default());
                }
                EngineArg::Rsync => p.engine = Engine::Rsync,
                EngineArg::Borg => {
                    p.engine = Engine::Borg;
                    p.borg = Some(flux_core::profile::BorgOptions::default());
                }
            }
            let issues = p.validate_in(ctx.paths.scope);
            report_issues(&id, &issues);
            if has_errors(&issues) {
                return Ok(ExitCode::from(EXIT_INVALID));
            }
            let path = store.create(&id, &p)?;
            sync_schedule(ctx, &id, &p);
            if ctx.json {
                output::print_json(&serde_json::json!({ "id": id, "path": path }))?;
            } else {
                println!("Created profile {id} ({})", path.display());
            }
            Ok(ExitCode::SUCCESS)
        }
        ProfileCmd::Templates => {
            let list = flux_runner::templates::list(&ctx.paths);
            if ctx.json {
                output::print_json(&list)?;
            } else {
                for t in list {
                    let id = t.id.strip_prefix("own:").unwrap_or(&t.id);
                    println!("{id:<14} {}{}", t.name, if t.own { " (yours)" } else { "" });
                    println!("{:<14} {}", "", t.line);
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        ProfileCmd::SaveTemplate { id, name } => {
            let service = flux_service::Service::new(ctx.paths.clone());
            flux_service::edit::save_template_impl(&service, id, name.clone())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!(
                "Saved the template “{name}”. Use it with: flux profile create … --template \"{name}\""
            );
            Ok(ExitCode::SUCCESS)
        }
        ProfileCmd::DeleteTemplate { template } => {
            let id = own_template(&ctx.paths, &template);
            flux_runner::templates::delete(&ctx.paths, &id).map_err(|e| anyhow::anyhow!(e))?;
            println!("Removed the template {template}.");
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
            // Flux Sentry's tripwire files for it go with it (the app's delete did
            // this; this one left them in your folders), and an open alarm is closed.
            flux_runner::forget_profile(&ctx.paths, &id);
            let sd = flux_runner::schedule::Scheduler::new(&ctx.paths);
            match sd.remove(&id) {
                Ok(a) if !a.removed.is_empty() && !ctx.json => println!("Removed its schedule."),
                Err(e) => eprintln!("warning: could not remove the schedule: {e}"),
                _ => {}
            }
            if !ctx.json {
                println!("Deleted profile {id}");
            }
            Ok(ExitCode::SUCCESS)
        }
        ProfileCmd::Validate { ids } => validate(ctx, &store, ids),
        ProfileCmd::LeaveOut { path, only, yes } => {
            folder_change(ctx, path, flux_service::folders::Want::Exclude, only, yes)
        }
        ProfileCmd::AddFolder { id, path, yes } => {
            parse_id(&id)?;
            folder_change(
                ctx,
                path,
                flux_service::folders::Want::Include,
                vec![id],
                yes,
            )
        }
        ProfileCmd::Export { id, format, output } => {
            let id = parse_id(&id)?;
            let p = store.load(&id)?;
            let Some(f) = flux_compat::export::Format::parse(&format) else {
                bail!("unknown format {format:?}; use flux, borgmatic, restic, rsync or borg");
            };
            let e = flux_compat::export::export(&id, &p, f);
            for n in &e.notes {
                eprintln!("note: {n}");
            }
            match output {
                None => print!("{}", e.contents),
                Some(out) => {
                    let out = absolutize(out)?;
                    let path = if out.is_dir() {
                        out.join(&e.filename)
                    } else {
                        out
                    };
                    flux_core::store::write_atomic(
                        &path,
                        e.contents.as_bytes(),
                        if e.executable { 0o755 } else { 0o644 },
                    )?;
                    if ctx.json {
                        output::print_json(&serde_json::json!({ "path": path, "notes": e.notes }))?;
                    } else {
                        println!("Exported {id} as {} to {}", f.info().label, path.display());
                    }
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        ProfileCmd::Duplicate { id } => {
            let id = parse_id(&id)?;
            if !store.exists(&id) {
                bail!("profile {id:?} not found");
            }
            let service = flux_service::Service::new(ctx.paths.clone());
            let new = flux_service::extras::duplicate_profile_impl(&service, id.to_string())
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            if ctx.json {
                output::print_json(&serde_json::json!({ "id": new }))?;
            } else {
                println!(
                    "Copied {id} to {new}. Change its destination with `flux profile edit {new}`."
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        ProfileCmd::Backintime { number, id, engine } => {
            backintime(ctx, &store, number, id, engine)
        }
        ProfileCmd::Import { file, id } => {
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("cannot read {}", file.display()))?;
            let (exported_id, p) = flux_compat::export::parse_flux(&text)
                .map_err(|e| anyhow::anyhow!("{} is not a Flux profile: {e}", file.display()))?;
            let id = match id.or(exported_id) {
                Some(i) => parse_id(&i)?,
                None => ProfileId::slugify(&p.name).context("give the profile an id with --id")?,
            };
            if store.exists(&id) {
                bail!("a profile “{id}” exists already; choose another id with --id");
            }
            let issues = p.validate_in(ctx.paths.scope);
            report_issues(&id, &issues);
            if has_errors(&issues) {
                return Ok(ExitCode::from(EXIT_INVALID));
            }
            let path = store.create(&id, &p)?;
            sync_schedule(ctx, &id, &p);
            if ctx.json {
                output::print_json(&serde_json::json!({ "id": id, "path": path }))?;
            } else {
                println!(
                    "Imported {id} ({}). Check its destination with `flux profile show {id}`.",
                    path.display()
                );
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn backintime(
    ctx: &Ctx,
    store: &ProfileStore,
    number: Option<u32>,
    id: Option<String>,
    engine: Option<EngineArg>,
) -> anyhow::Result<ExitCode> {
    use flux_compat::backintime;
    let home = flux_core::sys::home_dir().context("no home folder")?;
    let config = home.join(backintime::CONFIG);
    let base = Profile::new(
        "",
        Target::Local {
            path: PathBuf::new(),
            fs_uuid: None,
        },
        Vec::new(),
    );
    let found = backintime::read(&config, &base)
        .with_context(|| format!("cannot read {}", config.display()))?;
    let Some(number) = number else {
        if ctx.json {
            output::print_json(&found)?;
        } else if found.is_empty() {
            println!(
                "No Back In Time profiles ({} doesn't exist).",
                config.display()
            );
        } else {
            for b in &found {
                let to = match &b.profile.target {
                    Target::Local { path, .. } => path.display().to_string(),
                    Target::Ssh { host, path, .. } => format!("{host}:{}", path.display()),
                };
                println!(
                    "{}  {}  ({} {}, to {to}, {} snapshots)",
                    b.number,
                    b.name,
                    b.profile.sources.paths.len(),
                    if b.profile.sources.paths.len() == 1 {
                        "folder"
                    } else {
                        "folders"
                    },
                    b.snapshot_count
                );
            }
            println!("\nCreate a Flux profile from one with `flux profile backintime NUMBER`.");
        }
        return Ok(ExitCode::SUCCESS);
    };
    let b = found
        .into_iter()
        .find(|b| b.number == number)
        .with_context(|| format!("Back In Time has no profile {number}"))?;
    let mut p = b.profile;
    match engine {
        None => {}
        Some(EngineArg::Mirror) => (p.engine, p.vault) = (Engine::Mirror, None),
        Some(EngineArg::Rsync) => (p.engine, p.vault) = (Engine::Rsync, None),
        Some(EngineArg::Vault) => {
            p.engine = Engine::Vault;
            p.vault.get_or_insert_with(VaultOptions::default);
        }
        Some(EngineArg::Borg) => {
            (p.engine, p.vault) = (Engine::Borg, None);
            p.borg = Some(flux_core::profile::BorgOptions::default());
        }
    }
    let id = match id {
        Some(i) => parse_id(&i)?,
        None => ProfileId::slugify(&b.name).context("give the profile an id with --id")?,
    };
    if store.exists(&id) {
        bail!("a profile “{id}” exists already; choose another id with --id");
    }
    let issues = p.validate_in(ctx.paths.scope);
    report_issues(&id, &issues);
    if has_errors(&issues) {
        return Ok(ExitCode::from(EXIT_INVALID));
    }
    let path = store.create(&id, &p)?;
    sync_schedule(ctx, &id, &p);
    if ctx.json {
        output::print_json(&serde_json::json!({ "id": id, "path": path, "notes": b.notes }))?;
    } else {
        println!(
            "Created {id} from Back In Time's “{}” ({}).",
            b.name,
            path.display()
        );
        for n in &b.notes {
            println!("  note: {n}");
        }
        println!(
            "Back In Time's snapshots are left as they are. Turn off its schedule so both \
             don't back up at once."
        );
    }
    Ok(ExitCode::SUCCESS)
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

/// Keep the profile's timer in step after it is created or changed (only if it has or
/// needs one). A profile with a schedule and no timer never runs, and nothing says so.
pub(crate) fn sync_schedule(ctx: &Ctx, id: &ProfileId, p: &Profile) {
    use flux_runner::schedule;
    // With --json, standard output is the answer and nothing else.
    let say = |m: String| {
        if ctx.json {
            eprintln!("{m}");
        } else {
            println!("{m}");
        }
    };
    let sd = schedule::Scheduler::new(&ctx.paths);
    let needs = p.schedule.trigger != flux_core::profile::Trigger::Manual || p.schedule.on_connect;
    if !needs && !sd.installed_ids().contains(id) {
        return;
    }
    match sd.apply(id, p, &schedule::UnitEnv::current(ctx.paths.scope)) {
        Ok(a) if a.changed() => say(format!("Updated the schedule ({}).", schedule::summary(p))),
        Ok(_) => {}
        Err(e) => eprintln!("warning: profile saved, but the schedule could not be updated: {e}"),
    }
    if let Some(w) = sd.unsupported(p) {
        say(format!("Note: {w}"));
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

/// A template by id or by name: a built-in id (`development`), yours by its name
/// (`"Work laptop"`) or id (`work-laptop`, `own:work-laptop`).
fn own_template(paths: &flux_core::Paths, t: &str) -> String {
    let list = flux_runner::templates::list(paths);
    list.iter()
        .find(|x| x.id == t || x.id == format!("own:{t}") || (x.own && x.name == t))
        .map_or_else(|| t.to_owned(), |x| x.id.clone())
}

/// `profile leave-out` and `profile add-folder`: the changes the app's *Leave out of
/// backups* and *Back up this folder* make, said first.
fn folder_change(
    ctx: &Ctx,
    path: String,
    want: flux_service::folders::Want,
    only: Vec<String>,
    yes: bool,
) -> anyhow::Result<ExitCode> {
    use flux_service::folders::{Change, Want, apply_folder_change_impl, folder_plan_impl};
    let service = flux_service::Service::new(ctx.paths.clone());
    let plan = folder_plan_impl(&service, path, want)?;
    let picked: Vec<_> = plan
        .backups
        .iter()
        .filter(|b| only.is_empty() || only.contains(&b.id))
        .collect();
    for id in &only {
        if !plan.backups.iter().any(|b| &b.id == id) {
            if want == Want::Include {
                bail!("profile {id:?} not found");
            }
            bail!("{id} doesn't back up {}", plan.shown);
        }
    }
    if picked.is_empty() {
        bail!("none of your profiles backs up {}", plan.shown);
    }
    let mut ids = Vec::new();
    for b in &picked {
        let line = match (&b.change, &b.blocked) {
            (Some(Change::AddRule { rule }), _) => format!("adds the exclusion rule {rule}"),
            (Some(Change::RemoveFolder { folder }), _) => format!("stops backing up {folder}"),
            (Some(Change::AddFolder { folder, replaces }), _) if replaces.is_empty() => {
                format!("adds {folder} to its folders")
            }
            (Some(Change::AddFolder { folder, replaces }), _) => {
                format!(
                    "adds {folder} to its folders, in place of {}",
                    replaces.join(", ")
                )
            }
            (Some(Change::RemoveRule { rule }), _) => {
                format!("removes the exclusion rule {rule}")
            }
            (None, Some(why)) => format!("can't: {why}"),
            (None, None) if b.backed_up => "already backs it up".into(),
            (None, None) => "nothing to change".into(),
        };
        if !ctx.json {
            println!("{} ({}): {line}", b.name, b.id);
        }
        if b.change.is_some() {
            ids.push(b.id.clone());
        }
    }
    if ids.is_empty() {
        if ctx.json {
            output::print_json(&plan)?;
        }
        return Ok(ExitCode::SUCCESS);
    }
    if !yes && !output::confirm("Make these changes?")? {
        bail!("not confirmed (use --yes to change without asking)");
    }
    let done = apply_folder_change_impl(&service, plan.path.clone(), want, ids)?;
    if ctx.json {
        output::print_json(&done)?;
    } else {
        for (name, why) in &done.failed {
            eprintln!("{name}: {why}");
        }
        if !done.changed.is_empty() {
            println!("Changed from the next backup on. Snapshots already made are as they were.");
        }
    }
    Ok(if done.failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
