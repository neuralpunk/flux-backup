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
    /// Vaults with repair data: read every stored byte, rebuild what is damaged from its
    /// repair data, and add repair data to what lacks it (implies --read-data).
    #[arg(long)]
    pub repair: bool,
}

pub fn run_check(ctx: &Ctx, mut args: CheckArgs) -> anyhow::Result<ExitCode> {
    args.read_data |= args.repair;
    // What a Vault can't be opened without, first.
    let mut repaired_first = Vec::new();
    if args.repair {
        let (_, p) = load_profile(ctx, &args.profile)?;
        repaired_first = flux_runner::repo::repair_vault_basics(&ctx.paths, &p);
    }
    let (_, profile, repo) = repo_for(ctx, &args.profile)?;
    if args.repair && repo.engine() != Engine::Vault {
        bail!(
            "--repair is for Vault backups, which can keep repair data (this one is {})",
            repo.engine()
        );
    }
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
    let r = repo.check_with(args.read_data, args.repair, &progress, &report);
    drop(live);
    let mut r = r?;
    r.repaired.splice(0..0, repaired_first);
    // Plain folders on a FAT32, exFAT or NTFS drive: worth knowing, not a problem found.
    if let Some(fit) = flux_core::target_check::filesystem_fit_of(&profile) {
        r.warnings.push(fit.sentence());
    }
    // What the backup drives say about their own health (asked now; nothing is kept).
    let now = jiff::Timestamp::now();
    for (key, path) in flux_runner::drive_health::places(args.profile.as_str(), &profile) {
        let Some(reading) = flux_runner::drive_health::read(&path) else {
            continue;
        };
        let drive = if reading.model.is_empty() {
            "the backup drive".to_owned()
        } else {
            format!("the backup drive ({})", reading.model)
        };
        let known = flux_runner::drive_health::load(&ctx.paths, &key)
            .unwrap_or_default()
            .update(&reading, now);
        r.warnings.push(match known.said(now) {
            Some(s) => format!("{} {}", s.text, s.hint),
            None if !known.reports => format!(
                "{drive} doesn't report its health (most USB sticks and many USB enclosures don't), so Flux can't tell how it is"
            ),
            None => format!("{drive} reports no problems with its health"),
        });
    }
    if ctx.json {
        output::print_json(&r)?;
    } else {
        for e in &r.errors {
            println!("error: {e}");
        }
        for w in &r.warnings {
            println!("note: {w}");
        }
        for fixed in &r.repaired {
            println!("repaired: {fixed}");
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
        let repaired = match r.repaired.len() {
            0 => String::new(),
            n => format!(", {n} repaired"),
        };
        println!(
            "{mark} {} snapshots, {} {what} checked{read}: {} problems{repaired}",
            group(r.snapshots),
            group(r.checked),
            r.errors.len()
        );
        if r.parity_added > 0 {
            println!(
                "  repair data added to {} stored files that had none",
                group(r.parity_added)
            );
        }
        if r.repairable > 0 {
            println!(
                "  {} of them can be rebuilt from the repair data: flux check {} --repair",
                group(r.repairable),
                args.profile
            );
        }
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
    /// Make a recovery code: a long random code, shown once, that opens the Vault like a
    /// password when every password is lost (and without the unlock file). Print it or keep
    /// it somewhere safe, away from the backups. A new one replaces the old.
    Recovery {
        profile: String,
        /// Remove the recovery code instead.
        #[arg(long)]
        remove: bool,
    },
    /// Change the password: add the new one, then remove the one used to open.
    Passwd { profile: String },
    /// Hash the password in use harder (or less hard): makes guessing a stolen drive's
    /// password slower, and unlocking slower by the same factor. Instant: only the key
    /// changes. Also the strength for the Vault's new passwords.
    Strengthen {
        profile: String,
        /// standard (64 MiB, 3 passes), strong (256 MiB, 4) or maximum (1 GiB, 6);
        /// default: the profile's setting.
        #[arg(long, value_parser = ["standard", "strong", "maximum"], conflicts_with = "memory")]
        level: Option<String>,
        /// Custom: memory in MiB (with --passes and --threads).
        #[arg(long, requires_all = ["passes", "threads"])]
        memory: Option<u32>,
        #[arg(long)]
        passes: Option<u32>,
        #[arg(long)]
        threads: Option<u32>,
    },
    /// An unlock file: the password in use then opens the Vault only together with it
    /// (keep it on a USB stick). `create PATH` makes a new one, `use PATH` takes an
    /// existing file, `remove` makes the password enough again.
    UnlockFile {
        profile: String,
        #[arg(value_parser = ["create", "use", "remove"])]
        action: String,
        path: Option<std::path::PathBuf>,
    },
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
                            hashing_words(k.hashing),
                            if k.unlock_file { "yes" } else { "" }.into(),
                            if k.current {
                                "← used now".into()
                            } else {
                                String::new()
                            },
                        ]
                    })
                    .collect();
                output::table(
                    &["KEY", "LABEL", "CREATED", "HASHING", "UNLOCK FILE", ""],
                    &rows,
                );
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
        KeyCmd::Recovery { profile, remove } => {
            let (id, _, repo) = repo_for(ctx, &profile)?;
            let v = vault_of(&repo, &profile)?;
            let _l = v.lock(true)?;
            if remove {
                if v.remove_recovery_code()? {
                    println!("Removed the recovery code: it no longer opens this Vault.");
                } else {
                    println!("This Vault has no recovery code.");
                }
                let _ = flux_runner::recover::set_answer(&ctx.paths, &id, "recovery_code", false);
            } else {
                let code = v.set_recovery_code()?;
                println!(
                    "Recovery code for {profile}:\n\n    {}\n\nIt opens this Vault like a password, also without the unlock file. Print it or\nkeep it somewhere safe, away from the backups: it's shown only now. Any recovery\ncode made before no longer works.",
                    code.as_str()
                );
                let _ = flux_runner::recover::set_answer(&ctx.paths, &id, "recovery_code", true);
            }
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
            // Opened with the recovery code (the password was lost): the code stays.
            let by_code = v
                .keys()?
                .iter()
                .any(|k| k.id == old && k.label == flux_vault::crypto::RECOVERY_LABEL);
            let new = v.add_key(&pw, "changed")?;
            if by_code {
                println!(
                    "Opened with the recovery code, which still works; `flux key recovery {profile}` makes a new one."
                );
            } else {
                v.remove_key(&old)?;
            }
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
        KeyCmd::Strengthen {
            profile,
            level,
            memory,
            passes,
            threads,
        } => {
            use flux_core::profile::PasswordHashing as H;
            let (id, p) = load_profile(ctx, &profile)?;
            let hashing = match (level.as_deref(), memory) {
                (Some("strong"), _) => H::Strong,
                (Some("maximum"), _) => H::Maximum,
                (Some(_), _) => H::Standard,
                (None, Some(m)) => H::Custom {
                    memory_mib: m,
                    passes: passes.unwrap_or(3),
                    threads: threads.unwrap_or(4),
                },
                (None, None) => p.vault.as_ref().map(|v| v.hashing).unwrap_or_default(),
            };
            let mut check = p.clone();
            check.vault.get_or_insert_with(Default::default).hashing = hashing;
            if let Some(i) = check
                .validate()
                .into_iter()
                .find(|i| i.field == "vault.hashing")
            {
                bail!("{i}");
            }
            let pw = cli_password(&id, &p)?;
            let service = flux_service::Service::new(ctx.paths.clone());
            flux_service::security::strengthen(&service, profile, hashing, Some(&pw))
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            let ms = flux_service::security::time_hashing_impl(hashing)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            println!(
                "The password is now hashed with {} settings. Unlocking takes about {:.1} s on this computer.",
                hashing_words(hashing),
                ms as f64 / 1000.0
            );
        }
        KeyCmd::UnlockFile {
            profile,
            action,
            path,
        } => {
            let (id, p) = load_profile(ctx, &profile)?;
            let path = match (action.as_str(), path) {
                ("remove", _) => None,
                (_, Some(path)) => Some(crate::backup_cmd::absolutize(path)?),
                (_, None) => bail!(
                    "say where the unlock file is: `flux key unlock-file {profile} {action} PATH`"
                ),
            };
            let pw = cli_password(&id, &p)?;
            let service = flux_service::Service::new(ctx.paths.clone());
            let r = flux_service::security::set_unlock_file(
                &service,
                profile.clone(),
                path.map(|p| p.display().to_string()),
                action == "create",
                Some(&pw),
            )
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            match &r.unlock_file {
                Some(f) => {
                    println!(
                        "The password now opens this Vault only together with {}. Keep a copy of that file somewhere safe: without it the password doesn't work.",
                        f.display()
                    );
                    for k in &r.without_file {
                        println!(
                            "Note: password {} (“{}”) still opens it without the file; `flux key remove {profile} {}` removes it.",
                            k.id, k.label, k.id
                        );
                    }
                }
                None => println!("The password alone opens this Vault again."),
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
        flux_runner::repo::open_vault_with(vault_backend(p, pool), p, pw)
            .with_context(|| format!("that password doesn't open the Vault of {id}"))?;
    }
    Ok(())
}

#[derive(Args)]
pub struct UnlockArgs {
    pub profile: String,
    /// Also remove locks that still look in use (a job on another computer that died or
    /// lost its connection less than 30 minutes ago). Only when you're sure no job is
    /// using the backup from anywhere.
    #[arg(long)]
    pub force: bool,
}

#[derive(Subcommand)]
pub enum VaultCmd {
    /// Move a Vault to the newest format (format 2 keeps holes in sparse files). The
    /// snapshots already there stay readable. Flux Backup before 1.0 can't open it
    /// afterwards, so upgrade only once every computer using it runs 1.0.
    Upgrade { profile: String },
}

fn vault_backend_of(ctx: &Ctx, profile: &str) -> anyhow::Result<Box<dyn flux_vault::Backend>> {
    let (_, p) = load_profile(ctx, profile)?;
    if p.engine != Engine::Vault {
        bail!(
            "profile {profile} uses the {} engine; only Vault backups use lock files that can be left behind",
            p.engine.label()
        );
    }
    let pool = flux_runner::repo::sftp_pool(&ctx.paths, &p, false);
    Ok(flux_runner::repo::vault_backend(&p, pool))
}

/// Remove locks left by jobs that died (a computer switched off mid-backup, a lost
/// connection). Needs no password.
pub fn run_unlock(ctx: &Ctx, args: UnlockArgs) -> anyhow::Result<ExitCode> {
    let backend = vault_backend_of(ctx, &args.profile)?;
    let removed = backend
        .clear_locks(args.force)
        .with_context(|| format!("removing locks at {}", backend.describe()))?;
    let held = backend.lock_holders().unwrap_or_default();
    if ctx.json {
        let describe = |v: &[flux_transport::lock::LockInfo]| {
            v.iter().map(|l| l.describe()).collect::<Vec<_>>()
        };
        output::print_json(&serde_json::json!({
            "removed": describe(&removed),
            "still_held": describe(&held),
        }))?;
    } else {
        if removed.is_empty() {
            println!("No locks to remove.");
        }
        for l in &removed {
            println!("Removed the lock of {}.", l.describe());
        }
        for l in &held {
            println!(
                "In use by {}. If that job is no longer running, `flux unlock {} --force` removes its lock.",
                l.describe(),
                args.profile
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub fn run_vault(ctx: &Ctx, cmd: VaultCmd) -> anyhow::Result<ExitCode> {
    match cmd {
        VaultCmd::Upgrade { profile } => {
            let backend = vault_backend_of(ctx, &profile)?;
            let u = flux_vault::VaultRepo::upgrade_format(backend.as_ref())?;
            if ctx.json {
                output::print_json(&u)?;
            } else if u.from == u.to {
                println!("The Vault is already in format {}.", u.to);
            } else {
                println!(
                    "Upgraded the Vault from format {} to {}. Its snapshots stay as they were; new ones keep holes in sparse files.",
                    u.from, u.to
                );
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn hashing_words(h: flux_core::profile::PasswordHashing) -> String {
    match h {
        flux_core::profile::PasswordHashing::Custom {
            memory_mib,
            passes,
            threads,
        } => format!("custom ({memory_mib} MiB, {passes} passes, {threads} threads)"),
        h => h.label().to_owned(),
    }
}

/// The Vault password for a key change: `FLUX_PASSWORD`, the profile's source, or typed.
fn cli_password(
    id: &flux_core::ProfileId,
    p: &flux_core::Profile,
) -> anyhow::Result<password::Secret> {
    password::obtain(id, p, password::Purpose::Open, Some(crate::backup_cmd::ASK))
        .map_err(|e| anyhow::anyhow!(e.to_string()))
}
