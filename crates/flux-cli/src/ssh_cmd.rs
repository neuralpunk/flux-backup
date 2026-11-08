//! `flux ssh test|keygen|copy-id`.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

use anyhow::{Context, bail};
use clap::Subcommand;

use flux_core::{ByteSize, Engine, Target};
use flux_transport::SshTarget;

use crate::Ctx;
use crate::backup_cmd::load_profile;

#[derive(Subcommand)]
pub enum SshCmd {
    /// Connect to a profile's server and check everything a backup needs.
    Test { profile: String },
    /// Create an SSH key for backups (ed25519, no passphrase, so timers can use it).
    Keygen {
        /// Key file (default ~/.ssh/flux_backup_ed25519).
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Install a public key on a profile's server (runs ssh-copy-id; asks for the password once).
    CopyId {
        profile: String,
        /// Public key to install (default: the profile's identity_file, else ~/.ssh/flux_backup_ed25519).
        #[arg(long)]
        key: Option<PathBuf>,
    },
}

fn target_of(ctx: &Ctx, profile: &str) -> anyhow::Result<(flux_core::Profile, SshTarget)> {
    let (_, p) = load_profile(ctx, profile)?;
    let t = SshTarget::from_target(&p.target, &ctx.paths.runtime_dir, false)
        .with_context(|| format!("profile {profile} has a local target, not an SSH one"))?;
    Ok((p, t))
}

fn default_key() -> PathBuf {
    flux_core::sys::home_dir()
        .unwrap_or_default()
        .join(".ssh/flux_backup_ed25519")
}

pub fn run_ssh(ctx: &Ctx, cmd: SshCmd) -> anyhow::Result<ExitCode> {
    match cmd {
        SshCmd::Test { profile } => test(ctx, &profile),
        SshCmd::Keygen { file } => {
            let file = file.unwrap_or_else(default_key);
            if file.exists() {
                println!("{} already exists; using it.", file.display());
            } else {
                if let Some(dir) = file.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let comment = format!("flux-backup@{}", flux_core::sys::hostname());
                let status = Command::new("ssh-keygen")
                    .args(["-q", "-t", "ed25519", "-N", "", "-C", &comment, "-f"])
                    .arg(&file)
                    .status()
                    .context("cannot run ssh-keygen")?;
                if !status.success() {
                    bail!("ssh-keygen failed");
                }
                println!("Created {} (and .pub).", file.display());
            }
            println!(
                "Install it on the server with: flux ssh copy-id <profile> --key {}.pub",
                file.display()
            );
            Ok(ExitCode::SUCCESS)
        }
        SshCmd::CopyId { profile, key } => {
            let (p, t) = target_of(ctx, &profile)?;
            let private = key
                .map(|k| k.with_extension(""))
                .or_else(|| match &p.target {
                    Target::Ssh { identity_file, .. } => identity_file.clone(),
                    Target::Local { .. } => None,
                })
                .unwrap_or_else(default_key);
            let public = PathBuf::from(format!("{}.pub", private.display()));
            if !public.exists() {
                bail!(
                    "{} doesn't exist; create a key with `flux ssh keygen`",
                    public.display()
                );
            }
            let mut c = Command::new("ssh-copy-id");
            c.arg("-i").arg(&public);
            if let Some(port) = t.port {
                c.args(["-p", &port.to_string()]);
            }
            for o in &t.options {
                c.args(["-o", o]);
            }
            let status = c
                .arg(t.destination())
                .status()
                .context("cannot run ssh-copy-id")?;
            if !status.success() {
                bail!("ssh-copy-id failed");
            }
            println!(
                "Key installed. If the profile doesn't use it yet, add under [target]:\n  identity_file = \"{}\"",
                private.display()
            );
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn test(ctx: &Ctx, profile: &str) -> anyhow::Result<ExitCode> {
    let (p, t) = target_of(ctx, profile)?;
    let Target::Ssh { path, .. } = &p.target else {
        unreachable!()
    };
    let ok = |m: &str| println!("✓ {m}");
    println!("Connecting to {} …", t.destination());
    // First connection with the terminal attached, so host-key and password prompts work.
    let status = t.command("true").status().context("cannot run ssh")?;
    if !status.success() {
        bail!("ssh to {} failed (see the message above)", t.destination());
    }
    let host = t.run("uname -n").unwrap_or_default();
    ok(&format!("connected ({})", host.trim()));

    if matches!(p.engine, Engine::Mirror | Engine::Rsync) {
        match t.run("rsync --version | head -n 1") {
            Ok(v) => ok(&format!("rsync on the server: {}", v.trim())),
            Err(_) => bail!(
                "rsync is not installed on the server; the {} engine over SSH needs it",
                p.engine.label()
            ),
        }
    }
    if p.engine == Engine::Borg {
        let remote = p
            .borg
            .as_ref()
            .and_then(|b| b.remote_path.clone())
            .unwrap_or_else(|| "borg".into());
        match t.run(&format!(
            "{} --version",
            flux_transport::shell_quote(&remote)
        )) {
            Ok(v) => ok(&format!("borg on the server: {}", v.trim())),
            Err(_) => bail!(
                "borg is not installed on the server; the Borg engine over SSH needs it \
                 (or set borg.remote_path to where it is)"
            ),
        }
    }
    let mut sftp = t.sftp().context("SFTP doesn't work on the server")?;
    ok("SFTP works");
    let dir = path.display().to_string();
    sftp.mkdir_p(&dir, 0o700)
        .with_context(|| format!("cannot create {dir} on the server"))?;
    let probe = format!("{dir}/.flux-write-test");
    sftp.write_new(&probe, b"ok", 0o600)
        .with_context(|| format!("cannot write in {dir}"))?;
    sftp.remove(&probe)?;
    let full = sftp.realpath(&dir).unwrap_or(dir);
    ok(&format!("{full} is writable"));
    match sftp.statvfs(&full) {
        Ok(v) => ok(&format!(
            "{} free on the server",
            ByteSize(v.free_bytes()).display_approx()
        )),
        Err(_) => println!("? free space unknown (server doesn't report it)"),
    }
    if t.batch {
        println!("note: running in batch mode");
    }
    println!(
        "\nReady. Scheduled backups can't type passwords: make sure a key or agent logs in without prompting."
    );
    Ok(ExitCode::SUCCESS)
}
