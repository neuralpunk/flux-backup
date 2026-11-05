//! Decide what to do with every scanned entry by merge-joining the (sorted) scan against
//! the (sorted) previous manifest. The previous manifest is streamed, never fully loaded.

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::manifest::{Entry, Kind, ManifestError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Create a directory (metadata applied after its contents).
    Mkdir,
    /// Unchanged regular file: hard-link the previous snapshot's copy.
    LinkPrev,
    /// New or changed regular file: copy from the source.
    Copy,
    /// Same inode as an earlier entry in this snapshot: hard-link to that entry's copy.
    LinkGroup(usize),
    Symlink,
    Special,
}

pub use flux_fs::outcome::PlanStats;

pub struct Plan {
    pub entries: Vec<Entry>,
    pub actions: Vec<Action>,
    pub stats: PlanStats,
}

/// Build the plan. `prev` yields the previous snapshot's entries in path order.
pub fn plan<I>(mut entries: Vec<Entry>, prev: Option<I>) -> Result<Plan, ManifestError>
where
    I: Iterator<Item = Result<Entry, ManifestError>>,
{
    let mut stats = PlanStats::default();
    let mut actions = Vec::with_capacity(entries.len());
    let mut prev = prev.map(|i| i.peekable());
    let mut groups: HashMap<(u64, u64), usize> = HashMap::new();

    for (idx, e) in entries.iter_mut().enumerate() {
        // Advance the previous manifest up to this path; anything skipped was deleted.
        let mut matched: Option<Entry> = None;
        if let Some(p) = prev.as_mut() {
            loop {
                let ord = match p.peek() {
                    None => break,
                    Some(Err(_)) => return Err(p.next().unwrap().unwrap_err()),
                    Some(Ok(pe)) => pe.path.as_slice().cmp(&e.path),
                };
                match ord {
                    Ordering::Less => {
                        p.next();
                        stats.deleted += 1;
                    }
                    Ordering::Equal => {
                        matched = p.next().map(Result::unwrap);
                        break;
                    }
                    Ordering::Greater => break,
                }
            }
        }

        stats.entries += 1;
        let action = match e.kind {
            Kind::Dir => {
                stats.dirs += 1;
                count(&mut stats, matched.as_ref().map(|m| e.same_meta_as(m)));
                Action::Mkdir
            }
            Kind::Symlink => {
                stats.symlinks += 1;
                count(&mut stats, matched.as_ref().map(|m| e.same_meta_as(m)));
                Action::Symlink
            }
            Kind::File => {
                stats.files += 1;
                stats.total_bytes += e.size;
                let same = matched.as_ref().map(|m| e.same_file_as(m));
                count(&mut stats, same);
                if same == Some(true)
                    && let Some(m) = &matched
                    && e.hash.is_none()
                {
                    e.hash = m.hash;
                }
                let leader = (e.nlink > 1)
                    .then(|| *groups.entry((e.dev, e.ino)).or_insert(idx))
                    .filter(|&l| l != idx);
                match (leader, same) {
                    (Some(l), _) => Action::LinkGroup(l),
                    (None, Some(true)) => {
                        stats.link_files += 1;
                        Action::LinkPrev
                    }
                    (None, _) => {
                        stats.copy_files += 1;
                        stats.copy_bytes += e.size;
                        Action::Copy
                    }
                }
            }
            _ => {
                stats.specials += 1;
                count(&mut stats, matched.as_ref().map(|m| e.same_meta_as(m)));
                Action::Special
            }
        };
        actions.push(action);
    }
    if let Some(p) = prev {
        for r in p {
            r?;
            stats.deleted += 1;
        }
    }
    Ok(Plan {
        entries,
        actions,
        stats,
    })
}

fn count(stats: &mut PlanStats, same: Option<bool>) {
    match same {
        None => stats.new += 1,
        Some(true) => stats.unchanged += 1,
        Some(false) => stats.changed += 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_fs::entry::test_entry as entry;

    fn file(path: &str, size: u64, mtime: i64) -> Entry {
        let mut e = entry(path, Kind::File);
        e.size = size;
        e.mtime = mtime;
        e
    }

    #[test]
    fn first_backup_copies_everything() {
        let cur = vec![entry("a", Kind::Dir), file("a/x", 10, 1), file("a/y", 5, 1)];
        let p = plan(cur, None::<std::iter::Empty<_>>).unwrap();
        assert_eq!(p.actions, [Action::Mkdir, Action::Copy, Action::Copy]);
        assert_eq!((p.stats.new, p.stats.copy_bytes), (3, 15));
        assert!(p.stats.has_changes());
    }

    #[test]
    fn incremental() {
        let prev = vec![
            entry("a", Kind::Dir),
            file("a/gone", 1, 1),
            file("a/same", 10, 1),
            file("a/touched", 10, 1),
            file("z/gone-at-end", 1, 1),
        ];
        let mut cur = vec![
            entry("a", Kind::Dir),
            file("a/new", 3, 2),
            file("a/same", 10, 1),
            file("a/touched", 10, 2),
        ];
        cur[0].mtime = 99; // dir mtime changes are not changes
        let p = plan(cur, Some(prev.into_iter().map(Ok))).unwrap();
        assert_eq!(
            p.actions,
            [Action::Mkdir, Action::Copy, Action::LinkPrev, Action::Copy]
        );
        let s = &p.stats;
        assert_eq!((s.new, s.changed, s.unchanged, s.deleted), (1, 1, 2, 2));
        assert_eq!((s.copy_files, s.copy_bytes, s.link_files), (2, 13, 1));
    }

    #[test]
    fn no_changes_detected() {
        let prev = vec![entry("a", Kind::Dir), file("a/x", 1, 1)];
        let p = plan(prev.clone(), Some(prev.into_iter().map(Ok))).unwrap();
        assert!(!p.stats.has_changes());
    }

    #[test]
    fn metadata_change_forces_copy() {
        let prev = vec![file("x", 1, 1)];
        let mut cur = prev.clone();
        cur[0].mode = 0o600;
        let p = plan(cur, Some(prev.into_iter().map(Ok))).unwrap();
        assert_eq!(p.actions, [Action::Copy]);
    }

    #[test]
    fn hard_link_groups() {
        let mut a = file("a", 4, 1);
        let mut b = file("b", 4, 1);
        a.nlink = 2;
        b.nlink = 2;
        a.ino = 42;
        b.ino = 42;
        let p = plan(vec![a, b], None::<std::iter::Empty<_>>).unwrap();
        assert_eq!(p.actions, [Action::Copy, Action::LinkGroup(0)]);
        assert_eq!(p.stats.copy_bytes, 4);
    }
}
