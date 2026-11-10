//! `flux restore-test`: restore a sample of the newest snapshot into a temporary folder
//! and check it.

use std::process::ExitCode;
use std::sync::Arc;

use clap::Args;

use flux_core::profile::TestMode;
use flux_core::progress::{Event, Progress};
use flux_runner::restore_test::{self, TestResult};

use crate::backup_cmd::{EXIT_PARTIAL, LiveProgress, repo_for, when};
use crate::{Ctx, output};

#[derive(Args)]
#[command(
    after_help = "Without a choice of files, the profile's own settings (`[restore_test]`, by default 10 random files).\n\
    Exit status: 0 passed, 4 passed with notes, 1 failed."
)]
pub struct RestoreTestArgs {
    pub profile: String,
    /// Test this many random files.
    #[arg(long, conflicts_with_all = ["percent", "full", "rolling"])]
    pub files: Option<u32>,
    /// Test random files making up this percentage of the snapshot.
    #[arg(long, conflicts_with_all = ["full", "rolling"])]
    pub percent: Option<u8>,
    /// Test every file.
    #[arg(long, conflicts_with = "rolling")]
    pub full: bool,
    /// The next share of every file (all of them once per window, 30 days by default).
    #[arg(long)]
    pub rolling: bool,
    /// Also compare each restored file with the file on disk now (differences are notes).
    #[arg(long)]
    pub compare_source: bool,
    /// Show the latest tests instead of running one.
    #[arg(long)]
    pub history: bool,
}

pub fn run(ctx: &Ctx, args: RestoreTestArgs) -> anyhow::Result<ExitCode> {
    if args.history {
        let (id, _) = crate::backup_cmd::load_profile(ctx, &args.profile)?;
        let st = restore_test::load_state(&ctx.paths, &id);
        if ctx.json {
            output::print_json(&st.history)?;
        } else if st.history.is_empty() {
            println!("No restore tests yet. Run one with `flux restore-test {id}`.");
        } else {
            let rows: Vec<Vec<String>> = st
                .history
                .iter()
                .rev()
                .map(|r| {
                    vec![
                        when(r.started),
                        format!("{:?}", r.result).to_lowercase(),
                        r.snapshot.clone(),
                        r.tested.to_string(),
                        flux_core::ByteSize(r.bytes).display_approx(),
                        r.problems_total.to_string(),
                    ]
                })
                .collect();
            output::table(
                &["WHEN", "RESULT", "SNAPSHOT", "FILES", "READ", "PROBLEMS"],
                &rows,
            );
        }
        return Ok(ExitCode::SUCCESS);
    }
    let (id, p, repo) = repo_for(ctx, &args.profile)?;
    let mut t = p.restore_test.clone();
    if let Some(n) = args.files {
        t.mode = TestMode::RandomFiles;
        t.files = n;
    } else if let Some(pc) = args.percent {
        t.mode = TestMode::Percent;
        t.percent = pc.clamp(1, 100);
    } else if args.full {
        t.mode = TestMode::Full;
    } else if args.rolling {
        t.mode = TestMode::Rolling;
    }
    t.compare_with_source |= args.compare_source;
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
    let r = restore_test::run(&ctx.paths, &id, &p, &t, &repo, &progress, &report);
    drop(live);
    if ctx.json {
        output::print_json(&r)?;
    } else {
        let mark = match r.result {
            TestResult::Passed => "✓",
            TestResult::Warning => "⚠",
            TestResult::Failed => "✗",
        };
        println!(
            "{mark} {} (snapshot {})",
            restore_test::summary(&r),
            r.snapshot
        );
        for pr in &r.problems {
            println!(
                "  {}: {}{}",
                pr.path,
                pr.problem.words(),
                pr.detail
                    .as_deref()
                    .map(|d| format!(" ({d})"))
                    .unwrap_or_default()
            );
        }
        if r.problems_total > r.problems.len() as u64 {
            println!(
                "  … and {} more",
                r.problems_total - r.problems.len() as u64
            );
        }
    }
    Ok(match r.result {
        TestResult::Passed => ExitCode::SUCCESS,
        TestResult::Warning => ExitCode::from(EXIT_PARTIAL),
        TestResult::Failed => ExitCode::FAILURE,
    })
}
