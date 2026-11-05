//! `flux mount`: snapshots as a read-only folder tree (FUSE).

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::Duration;

use anyhow::{Context, bail};
use clap::Args;

use crate::Ctx;
use crate::backup_cmd::{absolutize, repo_for};

#[derive(Args)]
pub struct MountArgs {
    /// Profile whose snapshots to show.
    pub profile: String,
    /// An empty folder to show them in.
    pub mountpoint: PathBuf,
    /// Show only this snapshot (id, name, `latest`, …), directly at the mount point.
    #[arg(long, short)]
    pub snapshot: Option<String>,
}

pub fn run_mount(ctx: &Ctx, args: MountArgs) -> anyhow::Result<ExitCode> {
    let mountpoint = absolutize(args.mountpoint)?;
    if !mountpoint.is_dir() {
        bail!(
            "{} is not a folder; create an empty one to mount on",
            mountpoint.display()
        );
    }
    let (id, _, repo) = repo_for(ctx, &args.profile)?;
    let snaps = repo.snapshots()?;
    if snaps.is_empty() {
        bail!("profile {id} has no snapshots yet");
    }
    let only = match &args.snapshot {
        Some(spec) => {
            let s = repo.find(spec)?;
            snaps.iter().position(|x| x.id == s.id)
        }
        None => None,
    };
    let count = snaps.len();
    let fs = flux_mount::SnapshotFs::new(Arc::new(repo), snaps, only);
    let session = flux_mount::mount(fs, &mountpoint).with_context(|| {
        format!(
            "cannot mount on {} (is FUSE available? `fusermount3` must be installed)",
            mountpoint.display()
        )
    })?;
    let what = match only {
        Some(_) => "One snapshot".to_owned(),
        None if count == 1 => "1 snapshot".to_owned(),
        None => format!("{count} snapshots"),
    };
    eprintln!(
        "{what} of {id} mounted read-only on {}.\nPress Ctrl-C, or run `fusermount3 -u {}`, to unmount.",
        mountpoint.display(),
        mountpoint.display()
    );

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        let _ = ctrlc::set_handler(move || stop.store(true, Relaxed));
    }
    while !stop.load(Relaxed) && !session.guard.is_finished() {
        std::thread::sleep(Duration::from_millis(200));
    }
    if session.guard.is_finished() {
        // Unmounted from outside.
        session.join().ok();
    } else {
        session
            .umount_and_join()
            .with_context(|| format!("unmounting {}", mountpoint.display()))?;
    }
    eprintln!("Unmounted {}.", mountpoint.display());
    Ok(ExitCode::SUCCESS)
}
