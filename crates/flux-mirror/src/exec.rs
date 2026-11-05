//! Carry out a [`Plan`] into the partial snapshot's data directory.

use std::collections::HashMap;
use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

use rayon::prelude::*;

use flux_core::progress::{Event, Progress, Reporter};

use crate::MirrorError;
use crate::manifest::{Entry, Kind};
use crate::plan::{Action, Plan};
use crate::repo::FileError;
use flux_fs::fsops::{
    ApplyPolicy, Copier, apply_metadata, make_special, make_tree_writable, remove_force,
};

pub struct ExecCtx<'a> {
    pub plan: &'a Plan,
    /// Data directory of the snapshot being built.
    pub dest: &'a Path,
    /// Data directory of the previous snapshot, for hard links.
    pub prev_data: Option<&'a Path>,
    /// `dest` may hold files from an interrupted run; reuse what's still valid.
    pub resume: bool,
    pub policy: &'a ApplyPolicy,
    pub copier: &'a Copier,
    pub progress: &'a Progress,
    pub report: Reporter<'a>,
    pub continue_on_errors: bool,
}

#[derive(Debug, Default)]
pub struct ExecResult {
    /// Per entry: true if it could not be backed up (it is left out of the manifest).
    pub failed: Vec<bool>,
    pub errors: Vec<FileError>,
    pub copied_files: u64,
    pub copied_bytes: u64,
    pub linked_files: u64,
}

struct State<'a> {
    ctx: &'a ExecCtx<'a>,
    failed: Vec<AtomicBool>,
    errors: Mutex<Vec<FileError>>,
    abort: AtomicBool,
    copied_files: AtomicU64,
    copied_bytes: AtomicU64,
    linked_files: AtomicU64,
}

impl State<'_> {
    fn dest(&self, e: &Entry) -> PathBuf {
        self.ctx.dest.join(e.rel_path())
    }

    fn fail(&self, idx: usize, e: &Entry, err: impl std::fmt::Display) {
        self.failed[idx].store(true, Relaxed);
        if self.ctx.progress.is_cancelled() {
            return; // interrupted by the user, not a problem with this file
        }
        let fe = FileError {
            path: e.abs_path().display().to_string(),
            message: err.to_string(),
        };
        (self.ctx.report)(Event::Warning {
            path: fe.path.clone(),
            message: fe.message.clone(),
        });
        self.ctx.progress.warnings.fetch_add(1, Relaxed);
        self.errors.lock().unwrap().push(fe);
        if !self.ctx.continue_on_errors {
            self.abort.store(true, Relaxed);
        }
    }

    fn stopped(&self) -> bool {
        self.abort.load(Relaxed) || self.ctx.progress.is_cancelled()
    }
}

pub fn execute(ctx: &ExecCtx<'_>) -> Result<ExecResult, MirrorError> {
    let plan = ctx.plan;
    let n = plan.entries.len();
    let st = State {
        ctx,
        failed: (0..n).map(|_| AtomicBool::new(false)).collect(),
        errors: Mutex::new(Vec::new()),
        abort: AtomicBool::new(false),
        copied_files: AtomicU64::new(0),
        copied_bytes: AtomicU64::new(0),
        linked_files: AtomicU64::new(0),
    };
    ctx.progress.entries_total.store(n as u64, Relaxed);
    ctx.progress
        .bytes_total
        .store(plan.stats.copy_bytes, Relaxed);

    if ctx.resume {
        cleanup_partial(ctx.dest, plan).map_err(|e| MirrorError::io(ctx.dest, e))?;
    }

    // 1. Directories, parents first (path order guarantees that). Private until finished.
    let mut builder = DirBuilder::new();
    builder.mode(0o700);
    for (idx, e) in plan.entries.iter().enumerate() {
        if plan.actions[idx] != Action::Mkdir {
            continue;
        }
        let d = st.dest(e);
        match builder.create(&d) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists && d.is_dir() => {}
            Err(err) => st.fail(idx, e, err),
        }
        ctx.progress.entries_done.fetch_add(1, Relaxed);
    }

    // 2. Everything except hard-link followers, in parallel.
    (0..n).into_par_iter().for_each(|idx| {
        if st.stopped() {
            return;
        }
        let e = &plan.entries[idx];
        let r = match plan.actions[idx] {
            Action::Mkdir | Action::LinkGroup(_) => return,
            Action::LinkPrev => link_prev(&st, e),
            Action::Copy => copy_from_source(&st, e, ctx.resume),
            Action::Symlink => make_symlink(&st, e),
            Action::Special => make_special_file(&st, e),
        };
        if let Err(err) = r {
            st.fail(idx, e, err);
        }
        ctx.progress.entries_done.fetch_add(1, Relaxed);
    });

    // 3. Hard-link followers: link to their leader's new copy.
    (0..n).into_par_iter().for_each(|idx| {
        let Action::LinkGroup(leader) = plan.actions[idx] else {
            return;
        };
        if st.stopped() {
            return;
        }
        let e = &plan.entries[idx];
        let r = if st.failed[leader].load(Relaxed) {
            copy_from_source(&st, e, ctx.resume)
        } else {
            let dst = st.dest(e);
            remove_force(&dst).and_then(|()| fs::hard_link(st.dest(&plan.entries[leader]), &dst))
        };
        if let Err(err) = r {
            st.fail(idx, e, err);
        }
        ctx.progress.entries_done.fetch_add(1, Relaxed);
    });

    if ctx.progress.is_cancelled() {
        return Err(MirrorError::Cancelled);
    }
    if st.abort.load(Relaxed) {
        let first = st.errors.lock().unwrap().first().cloned();
        return Err(MirrorError::Aborted(
            first
                .map(|f| format!("{}: {}", f.path, f.message))
                .unwrap_or_default(),
        ));
    }

    // 4. Directory metadata, children before parents, so restrictive modes and mtimes
    // aren't disturbed by later writes.
    for (idx, e) in plan.entries.iter().enumerate().rev() {
        if plan.actions[idx] == Action::Mkdir
            && !st.failed[idx].load(Relaxed)
            && let Err(err) = apply_metadata(&st.dest(e), e, ctx.policy)
        {
            st.fail(idx, e, err);
        }
    }

    Ok(ExecResult {
        failed: st.failed.into_iter().map(AtomicBool::into_inner).collect(),
        errors: st.errors.into_inner().unwrap(),
        copied_files: st.copied_files.into_inner(),
        copied_bytes: st.copied_bytes.into_inner(),
        linked_files: st.linked_files.into_inner(),
    })
}

fn link_prev(st: &State<'_>, e: &Entry) -> io::Result<()> {
    let dst = st.dest(e);
    let prev = st.ctx.prev_data.map(|p| p.join(e.rel_path()));
    if let Some(prev) = &prev {
        if st.ctx.resume
            && let (Ok(d), Ok(p)) = (fs::symlink_metadata(&dst), fs::symlink_metadata(prev))
        {
            if d.ino() == p.ino() && d.dev() == p.dev() {
                st.linked_files.fetch_add(1, Relaxed);
                return Ok(());
            }
            remove_force(&dst)?;
        }
        match fs::hard_link(prev, &dst) {
            Ok(()) => {
                st.linked_files.fetch_add(1, Relaxed);
                return Ok(());
            }
            // Previous copy missing, too many links (EMLINK), or the like: copy instead.
            Err(err) => {
                tracing::debug!("hard link from {} failed ({err}); copying", prev.display())
            }
        }
    }
    st.ctx.progress.bytes_total.fetch_add(e.size, Relaxed);
    copy_from_source(st, e, st.ctx.resume)
}

fn copy_from_source(st: &State<'_>, e: &Entry, resume: bool) -> io::Result<()> {
    let dst = st.dest(e);
    if resume && let Ok(m) = fs::symlink_metadata(&dst) {
        if m.is_file()
            && m.len() == e.size
            && m.mtime() == e.mtime
            && m.mtime_nsec() as u32 == e.mtime_nsec
        {
            st.ctx.progress.bytes_done.fetch_add(e.size, Relaxed);
            st.copied_files.fetch_add(1, Relaxed);
            return Ok(());
        }
        remove_force(&dst)?;
    }
    let src = e.abs_path();
    let n = st.ctx.copier.copy(&src, &dst, st.ctx.progress)?;
    if n != e.size {
        (st.ctx.report)(Event::Warning {
            path: src.display().to_string(),
            message: format!(
                "file changed while being backed up ({} → {n} bytes)",
                e.size
            ),
        });
        // Account for the difference so the progress bar still ends at 100%.
        if n < e.size {
            st.ctx.progress.bytes_done.fetch_add(e.size - n, Relaxed);
        }
    }
    st.copied_files.fetch_add(1, Relaxed);
    st.copied_bytes.fetch_add(n, Relaxed);
    apply_metadata(&dst, e, st.ctx.policy)
}

fn make_symlink(st: &State<'_>, e: &Entry) -> io::Result<()> {
    let dst = st.dest(e);
    if st.ctx.resume {
        remove_force(&dst)?;
    }
    let target = std::ffi::OsStr::from_bytes(e.link.as_deref().unwrap_or_default());
    std::os::unix::fs::symlink(target, &dst)?;
    apply_metadata(&dst, e, st.ctx.policy)
}

fn make_special_file(st: &State<'_>, e: &Entry) -> io::Result<()> {
    let creatable = match e.kind {
        Kind::Fifo => true,
        Kind::CharDevice | Kind::BlockDevice => st.ctx.policy.privileged,
        _ => false, // sockets are recreated by their programs, never copied
    };
    if !creatable {
        return Ok(()); // recorded in the manifest only
    }
    let dst = st.dest(e);
    if st.ctx.resume {
        remove_force(&dst)?;
    }
    make_special(&dst, e)?;
    apply_metadata(&dst, e, st.ctx.policy)
}

/// Remove whatever an interrupted run left in `dest` that the new plan doesn't contain
/// (or contains as a different kind), and make directories writable again.
fn cleanup_partial(dest: &Path, plan: &Plan) -> io::Result<()> {
    if !dest.exists() {
        return Ok(());
    }
    make_tree_writable(dest)?;
    let index: HashMap<&[u8], &Entry> = plan
        .entries
        .iter()
        .map(|e| (e.path.as_slice(), e))
        .collect();
    let mut stack = vec![dest.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for ent in fs::read_dir(&dir)? {
            let ent = ent?;
            let path = ent.path();
            let rel = path
                .strip_prefix(dest)
                .unwrap_or(&path)
                .as_os_str()
                .as_bytes();
            let is_dir = ent.file_type()?.is_dir();
            match index.get(rel) {
                Some(e) if (e.kind == Kind::Dir) == is_dir => {
                    if is_dir {
                        stack.push(path);
                    }
                }
                _ => remove_force(&path)?,
            }
        }
    }
    Ok(())
}
