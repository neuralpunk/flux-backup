//! `flux recovery`: the recovery kit of a compatibility-mode profile.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, bail};
use clap::Args;

use crate::backup_cmd::{absolutize, load_profile};
use crate::{Ctx, output};

#[derive(Args)]
#[command(
    after_help = "Compatibility-mode profiles (engine rsync) keep this kit next to \
    their backups too, as FLUX-RECOVERY.txt and flux-backup.sh."
)]
pub struct RecoveryArgs {
    pub profile: String,
    /// Print the backup script (flux-backup.sh) instead of the guide.
    #[arg(long)]
    pub script: bool,
    /// Also save both files into this folder (a USB stick, your notes…).
    #[arg(long, value_name = "DIR")]
    pub save: Option<PathBuf>,
}

pub fn run(ctx: &Ctx, args: RecoveryArgs) -> anyhow::Result<ExitCode> {
    let (id, p) = load_profile(ctx, &args.profile)?;
    let Some(kit) = flux_compat::kit::for_profile(&id, &p) else {
        bail!(
            "profile {id} uses the {} engine; recovery kits come with compatibility mode \
             (engine = \"rsync\" or \"borg\"), whose backups are usable without Flux",
            p.engine.label()
        );
    };
    let location = flux_compat::kit::location(&id, &p);
    if let Some(dir) = &args.save {
        let dir = absolutize(dir.clone())?;
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("cannot create {}", dir.display()))?;
        for (name, text, mode) in [
            (flux_compat::kit::RECOVERY_FILE, &kit.recovery, 0o644),
            (flux_compat::kit::SCRIPT_FILE, &kit.script, 0o755),
        ] {
            let path = dir.join(format!("{id}-{name}"));
            flux_core::store::write_atomic(&path, text.as_bytes(), mode)
                .with_context(|| format!("cannot write {}", path.display()))?;
            if !ctx.json {
                eprintln!("Saved {}", path.display());
            }
        }
    }
    if ctx.json {
        output::print_json(&serde_json::json!({
            "profile": id,
            "engine": p.engine,
            "location": location,
            "recovery": kit.recovery,
            "script": kit.script,
        }))?;
    } else if args.script {
        print!("{}", kit.script);
    } else if args.save.is_none() {
        print!("{}", kit.recovery);
    }
    Ok(ExitCode::SUCCESS)
}

#[derive(Args)]
#[command(
    after_help = "Without --pick, lists the backups found. With it, sets that backup \
    up on this computer: the saved settings (folders, exclusions, rules) if they were kept \
    with it, reaching the backups where they were found.\n\nAn encrypted Vault's backups \
    are listed once its password is given (FLUX_PASSWORD, or typed)."
)]
pub struct RecoverArgs {
    /// Where the backups are: a drive's folder, `[user@]host:/path` or `ssh://…`.
    pub target: String,
    /// The backup to recover: its number in the list.
    #[arg(long, value_name = "N")]
    pub pick: Option<usize>,
    /// Carry on backing up from this computer, on the saved schedule (otherwise nothing
    /// is scheduled: restore what you need, then decide).
    #[arg(long)]
    pub carry_on: bool,
    /// The id of the profile made here (default: the one it had).
    #[arg(long)]
    pub id: Option<String>,
}

pub fn recover(ctx: &Ctx, args: RecoverArgs) -> anyhow::Result<ExitCode> {
    use flux_runner::recover::{self as rc, Then};
    let mut target = crate::target_spec::parse(&args.target)?;
    if let flux_core::Target::Local { path, .. } = &mut target {
        *path = absolutize(path.clone())?;
    }
    let env_pw = std::env::var("FLUX_PASSWORD")
        .ok()
        .filter(|v| !v.is_empty());
    let mut password = env_pw.map(flux_runner::password::Secret::new);
    let mut search = rc::find(
        &ctx.paths,
        &target,
        password.as_ref().map(|s| s.as_str()),
        false,
    )
    .map_err(|e| anyhow::anyhow!(e))?;
    if search.vault_locked
        && password.is_none()
        && !ctx.json
        && let Some(pw) =
            flux_runner::password::terminal_ask("Password of the encrypted Vault there: ", false)
    {
        password = Some(pw);
        search = rc::find(
            &ctx.paths,
            &target,
            password.as_ref().map(|s| s.as_str()),
            false,
        )
        .map_err(|e| anyhow::anyhow!(e))?;
    }
    if search.wrong_password {
        eprintln!("warning: that password doesn't open the Vault there; its backups aren't listed");
    } else if search.vault_locked {
        eprintln!(
            "note: there's an encrypted Vault there; give its password (FLUX_PASSWORD) to list its backups"
        );
    }
    let Some(n) = args.pick else {
        if ctx.json {
            output::print_json(&search)?;
            return Ok(ExitCode::SUCCESS);
        }
        if search.found.is_empty() {
            bail!("no Flux backups found in {}", args.target);
        }
        for (i, f) in search.found.iter().enumerate() {
            println!(
                "{:>3}. {} ({}), made on {} by {}{}",
                i + 1,
                f.name,
                f.engine.label(),
                f.kept.host,
                f.kept.user,
                if f.here { ", this computer" } else { "" }
            );
            if !f.sources.is_empty() {
                let s: Vec<_> = f.sources.iter().map(|p| p.display().to_string()).collect();
                println!("     {}", s.join(", "));
            }
            let mut facts = Vec::new();
            if let Some(c) = f.snapshots {
                facts.push(format!("{c} snapshot{}", if c == 1 { "" } else { "s" }));
            }
            if let Some(t) = f.newest {
                facts.push(format!("newest {}", t.strftime("%Y-%m-%d %H:%M")));
            }
            facts.push(if f.saved {
                "settings kept with it".into()
            } else {
                "no settings kept (made before Flux 1.0?)".into()
            });
            println!("     {}", facts.join(" · "));
        }
        println!(
            "\nRecover one with: flux recover {} --pick N [--carry-on]",
            args.target
        );
        return Ok(ExitCode::SUCCESS);
    };
    let found = search
        .found
        .get(n.wrapping_sub(1))
        .with_context(|| format!("there's no backup {n} in the list"))?;
    let then = if args.carry_on {
        Then::CarryOn
    } else {
        Then::Restore
    };
    let (id, p) = rc::recover(&ctx.paths, found, &target, then, args.id.as_deref())
        .map_err(|e| anyhow::anyhow!(e))?;
    // The password opened it: keep it the way the profile says, so backups and restores
    // don't ask again.
    if let Some(pw) = &password
        && matches!(
            found.engine,
            flux_core::Engine::Vault | flux_core::Engine::Borg
        )
        && p.password_source().is_some()
    {
        let service = flux_service::Service::new(ctx.paths.clone());
        if let Err(e) =
            flux_service::views::set_password_impl(&service, id.to_string(), pw.to_string())
        {
            eprintln!("warning: the password couldn't be kept for {id}: {e}");
        }
    }
    if then == Then::CarryOn {
        crate::profile_cmd::sync_schedule(ctx, &id, &p);
    }
    if ctx.json {
        output::print_json(&serde_json::json!({ "id": id, "profile": p }))?;
    } else {
        println!("Recovered “{}” as {id}.", p.name);
        println!("  Restore:  flux restore {id} --help   (or open Flux Backup)");
        if then == Then::Restore {
            println!(
                "  Nothing is scheduled. To carry on backing up from this computer:\n            flux profile edit {id}   (set its schedule)"
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}
