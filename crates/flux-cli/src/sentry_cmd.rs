//! `flux sentry`: what Flux Sentry knows of a backup, whether it holds it, and the two
//! ways to settle unusual changes.

use std::process::ExitCode;

use clap::Args;

use flux_core::profile::Sensitivity;
use flux_runner::repo::AnyRepo;
use flux_runner::{runlog, sentry};

use crate::backup_cmd::{ASK, load_profile};
use crate::{Ctx, output};

#[derive(Args)]
#[command(
    after_help = "While unusual changes aren't settled, no backup of the profile is made (`flux backup` refuses, scheduled ones are skipped). \
    Settle them with --this-was-me or --dealt-with."
)]
pub struct SentryArgs {
    pub profile: String,
    /// The unusual changes were yours: backups go on, Flux Sentry takes that backup as
    /// normal from now on, and the snapshot kept from before is no longer pinned.
    #[arg(long, conflicts_with = "dealt_with")]
    pub this_was_me: bool,
    /// The unusual changes weren't yours and you've seen to them (restored your files,
    /// cleaned the computer): backups go on. The snapshot from before stays pinned.
    #[arg(long)]
    pub dealt_with: bool,
}

pub fn run(ctx: &Ctx, args: SentryArgs) -> anyhow::Result<ExitCode> {
    let (id, p) = load_profile(ctx, &args.profile)?;
    // With Flux Sentry off nothing is held, whatever was found before it was turned off.
    let held = (p.unusual_changes != Sensitivity::Off)
        .then(|| flux_runner::sentry_hold(&ctx.paths, &id))
        .flatten();
    if args.this_was_me || args.dealt_with {
        let Some(u) = held else {
            println!("Nothing to settle: Flux Sentry isn't holding {id}.");
            return Ok(ExitCode::SUCCESS);
        };
        // "This was me" also takes the tag and the pin off the snapshots.
        let repo = args
            .this_was_me
            .then(|| AnyRepo::open(&ctx.paths, &id, &p, Some(ASK)).ok())
            .flatten();
        let settled = flux_runner::settle_unusual(
            &ctx.paths,
            &id,
            &p,
            &u.snapshot,
            args.this_was_me,
            repo.as_ref(),
        )?;
        if settled.marks_left {
            println!(
                "The backups couldn't be reached to take the \"unusual changes\" tag and the pin off the snapshots: they stay until you remove them (flux snapshots {id})."
            );
        }
        println!(
            "{}",
            if args.this_was_me {
                "Noted: those changes were yours. Backups go on, and Flux Sentry counts them as normal for this backup."
            } else {
                "Noted: it's dealt with. Backups go on; the snapshot from before the changes stays pinned."
            }
        );
        return Ok(ExitCode::SUCCESS);
    }

    let runs = runlog::recent_runs(&ctx.paths, &id, 300);
    let learned = sentry::limits(p.unusual_changes).and_then(|l| {
        let history = flux_runner::sentry_history(&ctx.paths, &id, runs.clone());
        flux_runner::unusual::learn(&history).map(|b| (b.usual(), b.alarm_share(&l), b.runs()))
    });
    if ctx.json {
        output::print_json(&serde_json::json!({
            "watching": p.unusual_changes,
            "learned": learned.map(|(usual, warns_at, runs)| serde_json::json!({ "usual": usual, "warns_at": warns_at, "runs": runs })),
            "held": held,
        }))?;
        return Ok(if held.is_some() {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        });
    }
    match p.unusual_changes {
        Sensitivity::Off => {
            println!("Flux Sentry is off for {id}: no warnings about unusual changes.");
            return Ok(ExitCode::SUCCESS);
        }
        Sensitivity::Normal => println!("Flux Sentry watches {id}."),
        Sensitivity::High => println!("Flux Sentry watches {id} closely."),
    }
    match learned {
        Some((usual, Some(warns_at), n)) => println!(
            "It knows this backup from {n} backups: {} of its files usually change; it would warn at about {}, or at the marks of ransomware.",
            sentry::share_words(usual),
            sentry::share_words(warns_at)
        ),
        // No amount of change alone would warn (it changes too much and too widely for
        // that, or has too few files): only what ransomware leaves behind does.
        Some((usual, None, n)) => println!(
            "It knows this backup from {n} backups: {} of its files usually change; it would warn at the marks of ransomware (no amount of change alone stands out here).",
            sentry::share_words(usual)
        ),
        None => println!(
            "It is still learning what's normal here (it needs {} backups); fixed limits stand in until then.",
            sentry::WARM_UP
        ),
    }
    if p.tripwires {
        let wires = flux_runner::tripwire::load(&ctx.paths, &id);
        match wires.len() {
            // A system backup's folders (/etc…) aren't written to.
            0 if ctx.paths.scope == flux_core::Scope::System => {
                println!("Tripwire files: not planted for system backups.")
            }
            0 => println!("Tripwire files: none yet; the next backup plants them."),
            n => println!(
                "Tripwire files: {n} ({}), looked at before every backup.",
                flux_runner::tripwire::NAME
            ),
        }
    }
    let Some(u) = held else {
        println!("✓ Nothing unusual: backups run as scheduled.");
        return Ok(ExitCode::SUCCESS);
    };
    println!("✗ On hold: no backup is made until this is settled.");
    if u.stopped {
        println!(
            "  Unusual changes in the files; the last backup was stopped before it was written:"
        );
    } else {
        println!("  Unusual changes in snapshot {}:", u.snapshot);
    }
    for s in &u.signals {
        println!("    {s}");
    }
    if let Some(b) = &u.before {
        println!(
            "  The snapshot before them, {b}, is kept (pinned): restore from it with `flux restore {id} -s {b} …`."
        );
    }
    println!("  If the changes were yours:        flux sentry {id} --this-was-me");
    println!("  If not, once you've seen to them: flux sentry {id} --dealt-with");
    Ok(ExitCode::FAILURE)
}
