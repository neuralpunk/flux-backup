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
    their backups too, as FLUX-RECOVERY.txt and flux-backup.sh.\n\nExamples:\n  \
    flux recovery home\n  \
    flux recovery home --script > flux-backup.sh\n  \
    flux recovery home --save /run/media/me/USB"
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
