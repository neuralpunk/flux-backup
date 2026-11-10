//! `flux` — the Flux Backup command-line interface.

mod backup_cmd;
mod lifecycle_cmd;
mod mount_cmd;
mod output;
mod profile_cmd;
mod recovery_cmd;
mod restore_test_cmd;
mod schedule_cmd;
mod ssh_cmd;
mod target_spec;
mod vault_cmd;

use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};
use flux_core::{Paths, Scope};

/// Exit status used when a profile fails validation.
pub const EXIT_INVALID: u8 = 3;

#[derive(Parser)]
#[command(
    name = "flux",
    version = flux_core::VERSION,
    about = "Fast, snapshot-based backups for Linux",
    propagate_version = true
)]
struct Cli {
    /// Operate on system-wide (root) profiles in /etc/flux-backup instead of your own.
    #[arg(long, global = true)]
    system: bool,

    /// Machine-readable JSON output.
    #[arg(long, global = true)]
    json: bool,

    /// More log output (-v info, -vv debug, -vvv trace).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Take a snapshot now.
    Backup(backup_cmd::BackupArgs),
    /// List a profile's snapshots.
    #[command(alias = "list")]
    Snapshots(backup_cmd::SnapshotsArgs),
    /// Show details and problems of one snapshot.
    Info(backup_cmd::InfoArgs),
    /// Browse the files in a snapshot.
    Ls(backup_cmd::LsArgs),
    /// Restore files or folders from a snapshot.
    Restore(backup_cmd::RestoreArgs),
    /// Is a file or folder backed up, and which rule decides? `--list`: each item of a
    /// folder, with what's in and out below it.
    Explain(backup_cmd::ExplainArgs),
    /// Save files of a snapshot as an archive: .tar.zst (keeps everything) or .zip.
    Export(backup_cmd::ExportArgs),
    /// Show snapshots as read-only folders (FUSE) until unmounted.
    Mount(mount_cmd::MountArgs),
    /// Show what changed between snapshots, or since a snapshot.
    Diff(lifecycle_cmd::DiffArgs),
    /// Search for files across snapshots.
    Find(lifecycle_cmd::FindArgs),
    /// List the versions of one file or folder across snapshots.
    History(lifecycle_cmd::HistoryArgs),
    /// Remove snapshots the retention policy no longer keeps.
    Prune(lifecycle_cmd::PruneArgs),
    /// Name a snapshot (named snapshots are kept by default), or remove its name.
    Name(lifecycle_cmd::NameArgs),
    /// Write a note on a snapshot ("before the upgrade"), or remove it. Notes never
    /// affect what retention keeps.
    Note(lifecycle_cmd::NoteArgs),
    /// Add tags to a snapshot (`taxes`, `release`), remove them, or list them. Tags never
    /// affect what retention keeps; `flux snapshots --tag` finds snapshots by tag.
    Tag(lifecycle_cmd::TagArgs),
    /// Keep a snapshot forever, whatever the retention policy says.
    Pin(lifecycle_cmd::PinArgs),
    /// Let retention manage a pinned snapshot again.
    Unpin(lifecycle_cmd::PinArgs),
    /// Delete snapshots by hand.
    Delete(lifecycle_cmd::DeleteArgs),
    /// Show recent backup runs, or the events of one run.
    Log(lifecycle_cmd::LogArgs),
    /// Verify a profile's backups.
    Check(vault_cmd::CheckArgs),
    /// Restore a sample of the newest snapshot into a temporary folder and check every
    /// restored file (also run after backups, as `[restore_test]` says).
    RestoreTest(restore_test_cmd::RestoreTestArgs),
    /// Print diagnostics for a bug report: versions, tools, the profile (secrets hidden)
    /// and its recent runs.
    Diagnostics { profile: String },
    /// Show the recovery kit of a compatibility-mode profile: how to use its backups
    /// without Flux.
    Recovery(recovery_cmd::RecoveryArgs),
    /// Find the backups on a drive or a server (from another computer, say) and set one up
    /// here again, with the settings kept with it.
    Recover(recovery_cmd::RecoverArgs),
    /// Manage the passwords of a Vault.
    #[command(subcommand)]
    Key(vault_cmd::KeyCmd),
    /// Vault maintenance: move to the newest format.
    #[command(subcommand)]
    Vault(vault_cmd::VaultCmd),
    /// Remove locks a backup job left behind when it died (Vault backups).
    Unlock(vault_cmd::UnlockArgs),
    /// SSH helpers: test a server, create and install keys.
    #[command(subcommand)]
    Ssh(ssh_cmd::SshCmd),
    /// Automatic backups with systemd timers.
    #[command(subcommand)]
    Schedule(schedule_cmd::ScheduleCmd),
    /// What's running, how the last backups went, and what's next.
    Status(schedule_cmd::StatusArgs),
    /// Pause scheduled backups (backing up by hand still works).
    Pause {
        /// For how long, e.g. 1h, 3d (default: until `flux resume`).
        #[arg(long = "for", value_name = "DURATION", conflicts_with = "tomorrow")]
        duration: Option<flux_core::HumanDuration>,
        /// Until 7:00 tomorrow.
        #[arg(long)]
        tomorrow: bool,
    },
    /// Let scheduled backups run again after `flux pause`.
    Resume,
    /// Follow a running backup's progress.
    Watch(schedule_cmd::ProfileArg),
    /// Stop a running backup (it resumes next time).
    Cancel(schedule_cmd::ProfileArg),
    /// Create, inspect and manage backup profiles.
    #[command(subcommand)]
    Profile(profile_cmd::ProfileCmd),
    /// Show Flux's configuration.
    #[command(subcommand)]
    Config(ConfigCmd),
    /// Print a shell completion script (bash, zsh, fish, elvish, powershell).
    Completions { shell: clap_complete::Shell },
    /// Write man pages for flux and every subcommand into a folder (for packagers).
    #[command(hide = true)]
    Manpages { dir: std::path::PathBuf },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Print the directories Flux uses.
    Path,
}

pub struct Ctx {
    pub paths: Paths,
    pub json: bool,
}

fn main() -> ExitCode {
    quiet_broken_pipe();
    let cli = Cli::parse();
    init_logging(cli.verbose);

    let scope = if cli.system {
        Scope::System
    } else {
        Scope::User
    };
    let paths = match Paths::resolve(scope) {
        Ok(p) => p,
        Err(e) => return fail(&e.into(), scope),
    };
    let ctx = Ctx {
        paths,
        json: cli.json,
    };

    let result = match cli.command {
        Command::Backup(a) => backup_cmd::run_backup(&ctx, a),
        Command::Snapshots(a) => backup_cmd::run_snapshots(&ctx, a),
        Command::Info(a) => backup_cmd::run_info(&ctx, a),
        Command::Ls(a) => backup_cmd::run_ls(&ctx, a),
        Command::Restore(a) => backup_cmd::run_restore(&ctx, a),
        Command::Export(a) => backup_cmd::run_export(&ctx, a),
        Command::Explain(a) => backup_cmd::run_explain(&ctx, a),
        Command::Mount(a) => mount_cmd::run_mount(&ctx, a),
        Command::Diff(a) => lifecycle_cmd::run_diff(&ctx, a),
        Command::Find(a) => lifecycle_cmd::run_find(&ctx, a),
        Command::History(a) => lifecycle_cmd::run_history(&ctx, a),
        Command::Prune(a) => lifecycle_cmd::run_prune(&ctx, a),
        Command::Name(a) => lifecycle_cmd::run_name(&ctx, a),
        Command::Note(a) => lifecycle_cmd::run_note(&ctx, a),
        Command::Tag(a) => lifecycle_cmd::run_tag(&ctx, a),
        Command::Pin(a) => lifecycle_cmd::run_pin(&ctx, a, true),
        Command::Unpin(a) => lifecycle_cmd::run_pin(&ctx, a, false),
        Command::Delete(a) => lifecycle_cmd::run_delete(&ctx, a),
        Command::Log(a) => lifecycle_cmd::run_log(&ctx, a),
        Command::Check(a) => vault_cmd::run_check(&ctx, a),
        Command::RestoreTest(a) => restore_test_cmd::run(&ctx, a),
        Command::Recovery(a) => recovery_cmd::run(&ctx, a),
        Command::Recover(a) => recovery_cmd::recover(&ctx, a),
        Command::Diagnostics { profile } => {
            let service = flux_service::Service::new(ctx.paths.clone());
            flux_service::extras::diagnostics_impl(&service, profile)
                .map(|text| {
                    print!("{text}");
                    ExitCode::SUCCESS
                })
                .map_err(|e| anyhow::anyhow!(e.to_string()))
        }
        Command::Key(cmd) => vault_cmd::run_key(&ctx, cmd),
        Command::Vault(cmd) => vault_cmd::run_vault(&ctx, cmd),
        Command::Unlock(a) => vault_cmd::run_unlock(&ctx, a),
        Command::Ssh(cmd) => ssh_cmd::run_ssh(&ctx, cmd),
        Command::Schedule(cmd) => schedule_cmd::run_schedule(&ctx, cmd),
        Command::Status(a) => schedule_cmd::run_status(&ctx, a),
        Command::Pause { duration, tomorrow } => schedule_cmd::run_pause(&ctx, duration, tomorrow),
        Command::Resume => schedule_cmd::run_resume(&ctx),
        Command::Watch(a) => schedule_cmd::run_watch(&ctx, a),
        Command::Cancel(a) => schedule_cmd::run_cancel(&ctx, a),
        Command::Profile(cmd) => profile_cmd::run(&ctx, cmd),
        Command::Config(ConfigCmd::Path) => config_path(&ctx),
        Command::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "flux", &mut std::io::stdout());
            Ok(ExitCode::SUCCESS)
        }
        Command::Manpages { dir } => manpages(&dir),
    };
    match result {
        Ok(code) => code,
        Err(e) => fail(&e, scope),
    }
}

/// `flux ls … | head` closes stdout early; exit quietly like other Unix tools instead of
/// panicking. (Rust ignores SIGPIPE, so a write to the closed pipe fails and `println!`
/// panics; this hook catches that one case.)
fn quiet_broken_pipe() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info
            .payload()
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| info.payload().downcast_ref::<&str>().copied())
            .unwrap_or("");
        if msg.contains("Broken pipe") {
            std::process::exit(141);
        }
        default(info);
    }));
}

fn fail(e: &anyhow::Error, scope: Scope) -> ExitCode {
    // Reader went away (`flux … | head`): exit quietly like other Unix tools.
    let broken_pipe = e.chain().any(|c| {
        c.downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe)
            || c.downcast_ref::<serde_json::Error>()
                .is_some_and(|j| j.io_error_kind() == Some(std::io::ErrorKind::BrokenPipe))
    });
    if broken_pipe {
        return ExitCode::from(141);
    }
    eprintln!("flux: error: {e:#}");
    let denied = e.chain().any(|c| {
        c.downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::PermissionDenied)
    }) || format!("{e:#}").contains("ermission denied");
    if scope == Scope::System && denied && !flux_core::sys::is_root() {
        eprintln!("hint: system profiles belong to root; run it as `sudo flux --system …`");
    }
    ExitCode::FAILURE
}

fn init_logging(verbose: u8) {
    let default = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_env("FLUX_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();
}

fn config_path(ctx: &Ctx) -> anyhow::Result<ExitCode> {
    let p = &ctx.paths;
    if ctx.json {
        let v = serde_json::json!({
            "scope": p.scope,
            "config_dir": p.config_dir,
            "profiles_dir": p.profiles_dir(),
            "state_dir": p.state_dir,
            "cache_dir": p.cache_dir,
            "runtime_dir": p.runtime_dir,
        });
        output::print_json(&v)?;
    } else {
        output::kv(&[
            ("config", p.config_dir.display().to_string()),
            ("profiles", p.profiles_dir().display().to_string()),
            ("state", p.state_dir.display().to_string()),
            ("cache", p.cache_dir.display().to_string()),
            ("runtime", p.runtime_dir.display().to_string()),
        ]);
    }
    Ok(ExitCode::SUCCESS)
}

/// `flux.1`, plus `flux-<sub>.1` (and `flux-<sub>-<subsub>.1`) for every visible subcommand.
fn manpages(dir: &std::path::Path) -> anyhow::Result<ExitCode> {
    std::fs::create_dir_all(dir)?;
    fn write(cmd: clap::Command, name: &str, dir: &std::path::Path) -> anyhow::Result<usize> {
        let cmd = cmd.name(name.to_owned());
        let mut buf = Vec::new();
        clap_mangen::Man::new(cmd.clone()).render(&mut buf)?;
        std::fs::write(dir.join(format!("{name}.1")), buf)?;
        let mut n = 1;
        for sub in cmd
            .get_subcommands()
            .filter(|s| !s.is_hide_set() && s.get_name() != "help")
        {
            n += write(sub.clone(), &format!("{name}-{}", sub.get_name()), dir)?;
        }
        Ok(n)
    }
    let mut cmd = Cli::command();
    cmd.build();
    let n = write(cmd, "flux", dir)?;
    eprintln!("wrote {n} man pages to {}", dir.display());
    Ok(ExitCode::SUCCESS)
}
