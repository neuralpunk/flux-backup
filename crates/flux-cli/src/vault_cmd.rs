//! `flux check` and `flux key …`.

use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, bail};
use clap::{Args, Subcommand};

use flux_core::progress::{Event, Progress};
use flux_core::{ByteSize, Engine};
use flux_runner::password;
use flux_runner::repo::AnyRepo;

use crate::backup_cmd::{LiveProgress, group, load_profile, repo_for, when};
use crate::{Ctx, output};

#[derive(Args)]
pub struct CheckArgs {
    pub profile: String,
    /// Also read every stored byte and verify it (slow; reads the whole backup).
    #[arg(long)]
    pub read_data: bool,
}

pub fn run_check(ctx: &Ctx, args: CheckArgs) -> anyhow::Result<ExitCode> {
    let (_, _, repo) = repo_for(ctx, &args.profile)?;
    let progress = Arc::new(Progress::new());
    crate::backup_cmd::install_ctrlc(&progress);
    let live = LiveProgress::start(
        progress.clone(),
        !ctx.json && std::io::IsTerminal::is_terminal(&std::io::stderr()),
    );
    let report = |ev: Event| {
        if let Event::Warning { path, message } = ev {
            live.println(format!("warning: {path}: {message}"));
        }
    };
    let r = repo.check(args.read_data, &progress, &report);
    drop(live);
    let r = r?;
    if ctx.json {
        output::print_json(&r)?;
    } else {
        for e in &r.errors {
            println!("error: {e}");
        }
        for w in &r.warnings {
            println!("note: {w}");
        }
        let what = if repo.engine() == Engine::Vault {
            "blobs"
        } else {
            "items"
        };
        let read = if args.read_data {
            format!(
                ", read {} and verified it",
                ByteSize(r.bytes_read).display_approx()
            )
        } else {
            String::new()
        };
        let mark = if r.errors.is_empty() { "✓" } else { "✗" };
        println!(
            "{mark} {} snapshots, {} {what} checked{read}: {} problems",
            group(r.snapshots),
            group(r.checked),
            r.errors.len()
        );
        if !args.read_data && r.errors.is_empty() {
            println!("  (structure only; add --read-data to verify every byte)");
        }
    }
    Ok(if r.errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[derive(Subcommand)]
pub enum KeyCmd {
    /// List the passwords (key slots) that open a profile's Vault.
    List { profile: String },
    /// Add another password (the current one keeps working).
    Add {
        profile: String,
        /// Label shown in `flux key list`.
        #[arg(long, default_value = "added")]
        label: String,
    },
    /// Remove a password by its key id.
    Remove { profile: String, key: String },
    /// Change the password: add the new one, then remove the one used to open.
    Passwd { profile: String },
    /// Set up encryption before the first backup: choose the password, create the
    /// encrypted Vault or Borg repository now, and keep the password where the profile
    /// says it comes from (keyring, or a key file that doesn't exist yet), so the first
    /// scheduled backup needs nobody to type it.
    Init { profile: String },
    /// Save the profile's password in the desktop keyring (needed for scheduled backups).
    Store { profile: String },
    /// Remove the profile's password from the desktop keyring.
    Forget { profile: String },
}

fn vault_of<'a>(repo: &'a AnyRepo, profile: &str) -> anyhow::Result<&'a flux_vault::VaultRepo> {
    repo.vault().with_context(|| {
        format!(
            "profile {profile} uses the {} engine, which has no Vault passwords",
            repo.engine().label()
        )
    })
}

fn ask_new(what: &str) -> anyhow::Result<password::Secret> {
    password::terminal_ask(what, true).context("a new password must be typed on a terminal")
}

pub fn run_key(ctx: &Ctx, cmd: KeyCmd) -> anyhow::Result<ExitCode> {
    match cmd {
        KeyCmd::List { profile } => {
            let (_, _, repo) = repo_for(ctx, &profile)?;
            let keys = vault_of(&repo, &profile)?.keys()?;
            if ctx.json {
                output::print_json(&keys)?;
            } else {
                let rows: Vec<Vec<String>> = keys
                    .iter()
                    .map(|k| {
                        vec![
                            k.id.clone(),
                            k.label.clone(),
                            when(k.created),
                            if k.current {
                                "← used now".into()
                            } else {
                                String::new()
                            },
                        ]
                    })
                    .collect();
                output::table(&["KEY", "LABEL", "CREATED", ""], &rows);
            }
        }
        KeyCmd::Add { profile, label } => {
            let (_, _, repo) = repo_for(ctx, &profile)?;
            let v = vault_of(&repo, &profile)?;
            let pw = ask_new("New additional password: ")?;
            let _l = v.lock(true)?;
            let id = v.add_key(&pw, &label)?;
            println!("Added key {id}. Both passwords now open this Vault.");
        }
        KeyCmd::Remove { profile, key } => {
            let (_, _, repo) = repo_for(ctx, &profile)?;
            let v = vault_of(&repo, &profile)?;
            let _l = v.lock(true)?;
            v.remove_key(&key)?;
            println!("Removed key {key}.");
        }
        KeyCmd::Passwd { profile } => {
            let (id, p, repo) = repo_for(ctx, &profile)?;
            let v = vault_of(&repo, &profile)?;
            let old = v.slot.clone().context("this Vault is not encrypted")?;
            let pw = ask_new("New password: ")?;
            let _l = v.lock(true)?;
            let new = v.add_key(&pw, "changed")?;
            v.remove_key(&old)?;
            if matches!(
                p.vault.as_ref().map(|v| &v.password_source),
                Some(flux_core::profile::PasswordSource::Keyring) | None
            ) {
                password::keyring_store(&id, &p.name, &pw)?;
                println!("Password changed (key {new}) and saved in the keyring.");
            } else {
                println!(
                    "Password changed (key {new}). Update the profile's password source if it stores the old one."
                );
            }
        }
        KeyCmd::Init { profile } => {
            use flux_core::profile::PasswordSource;
            let (id, p) = load_profile(ctx, &profile)?;
            let Some(source) = p.password_source() else {
                bail!(
                    "profile {id} uses the {} engine without encryption, so it has no password",
                    p.engine.label()
                );
            };
            let what = if p.engine == Engine::Borg {
                "Borg passphrase"
            } else {
                "Vault password"
            };
            // FLUX_PASSWORD (scripts), else a command or an existing key file supplies it,
            // else it's typed (twice).
            let from_env = std::env::var("FLUX_PASSWORD")
                .ok()
                .filter(|v| !v.is_empty());
            let pw = match &source {
                _ if from_env.is_some() => password::Secret::new(from_env.unwrap_or_default()),
                PasswordSource::Command { .. } => password::Secret::default(),
                PasswordSource::File { path } if path.exists() => password::Secret::default(),
                _ => ask_new(&format!("{what} for “{}”: ", p.name))?,
            };
            let service = flux_service::Service::new(ctx.paths.clone());
            flux_service::views::set_password_impl(&service, id.to_string(), pw.to_string())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            if source == PasswordSource::Prompt {
                println!(
                    "Encryption is set up for {id}. Its password comes from “prompt”, so every backup asks for it; use the keyring or a key file to let scheduled backups run on their own."
                );
            } else {
                println!(
                    "Encryption is set up for {id}; its first backup, scheduled or not, won't need to ask."
                );
            }
        }
        KeyCmd::Store { profile } => {
            let (id, p) = load_profile(ctx, &profile)?;
            if p.password_source().is_none() {
                bail!(
                    "profile {id} uses the {} engine without encryption, so it has no password",
                    p.engine.label()
                );
            }
            let (what, repo) = if p.engine == Engine::Borg {
                ("Borg passphrase", "repository")
            } else {
                ("Vault password", "Vault")
            };
            let pw = password::terminal_ask(&format!("{what} for “{}”: ", p.name), false)
                .context("type the password on a terminal")?;
            // Make sure it's the right one before storing it.
            if p.engine == Engine::Borg {
                let r = flux_compat::borg::BorgRepo::for_profile(
                    &ctx.paths,
                    &id,
                    &p,
                    Some(pw.clone()),
                    false,
                )?;
                if r.exists() {
                    r.verify().with_context(|| {
                        format!("that passphrase doesn't open the repository of {id}")
                    })?;
                }
            } else {
                verify_password(ctx, &id, &p, &pw)?;
            }
            password::keyring_store(&id, &p.name, &pw)?;
            println!("Saved in the keyring; scheduled backups of {id} can now open the {repo}.");
        }
        KeyCmd::Forget { profile } => {
            let (id, _) = load_profile(ctx, &profile)?;
            password::keyring_clear(&id)?;
            println!("Removed the password of {id} from the keyring.");
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Open the Vault with `pw` to verify it (when the Vault exists yet).
fn verify_password(
    ctx: &Ctx,
    id: &flux_core::ProfileId,
    p: &flux_core::Profile,
    pw: &str,
) -> anyhow::Result<()> {
    use flux_runner::repo::{sftp_pool, vault_backend};
    let pool = sftp_pool(&ctx.paths, p, false);
    if flux_vault::VaultRepo::exists(vault_backend(p, pool.clone()).as_ref())? {
        flux_vault::VaultRepo::open(vault_backend(p, pool), Some(pw))
            .with_context(|| format!("that password doesn't open the Vault of {id}"))?;
    }
    Ok(())
}
