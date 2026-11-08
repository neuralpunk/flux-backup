//! Compare two sorted entry streams: two snapshots, or a snapshot and the live system.

use std::cmp::Ordering;

use serde::{Serialize, Serializer};

use crate::entry::{Entry, Kind, is_within};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Removed,
    Modified,
    /// Became a different kind of thing (file → directory, …).
    TypeChanged,
}

impl ChangeKind {
    pub fn marker(self) -> char {
        match self {
            ChangeKind::Added => '+',
            ChangeKind::Removed => '-',
            ChangeKind::Modified => 'M',
            ChangeKind::TypeChanged => 'T',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Content,
    Mtime,
    Mode,
    Owner,
    Xattrs,
    Target,
}

#[derive(Debug, Clone, Serialize)]
pub struct Change {
    #[serde(serialize_with = "lossy_abs")]
    pub path: Vec<u8>,
    pub change: ChangeKind,
    /// Kind after the change (before, for removals).
    pub kind: Kind,
    /// What differs, for `Modified`.
    pub fields: Vec<Field>,
    pub old_size: Option<u64>,
    pub new_size: Option<u64>,
}

fn lossy_abs<S: Serializer>(p: &[u8], s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&format!("/{}", String::from_utf8_lossy(p)))
}

impl Change {
    pub fn display_path(&self) -> String {
        format!("/{}", String::from_utf8_lossy(&self.path))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DiffSummary {
    pub added: u64,
    pub removed: u64,
    pub modified: u64,
    pub type_changed: u64,
    /// Net change in total file bytes.
    pub bytes_delta: i64,
}

impl DiffSummary {
    fn add(&mut self, c: &Change) {
        match c.change {
            ChangeKind::Added => self.added += 1,
            ChangeKind::Removed => self.removed += 1,
            ChangeKind::Modified => self.modified += 1,
            ChangeKind::TypeChanged => self.type_changed += 1,
        }
        self.bytes_delta += c.new_size.unwrap_or(0) as i64 - c.old_size.unwrap_or(0) as i64;
    }
}

fn fields(old: &Entry, new: &Entry) -> Vec<Field> {
    let mut f = Vec::new();
    if old.kind == Kind::File
        && (old.size != new.size || matches!((&old.hash, &new.hash), (Some(a), Some(b)) if a != b))
    {
        f.push(Field::Content);
    }
    if old.kind != Kind::Dir && (old.mtime, old.mtime_nsec) != (new.mtime, new.mtime_nsec) {
        f.push(Field::Mtime);
    }
    if old.mode != new.mode {
        f.push(Field::Mode);
    }
    if (old.uid, old.gid) != (new.uid, new.gid) {
        f.push(Field::Owner);
    }
    if old.xattrs != new.xattrs {
        f.push(Field::Xattrs);
    }
    if old.link != new.link || old.rdev != new.rdev {
        f.push(Field::Target);
    }
    f
}

fn size_of(e: &Entry) -> Option<u64> {
    (e.kind == Kind::File).then_some(e.size)
}

/// Diff `old` against `new`, restricted to paths within `prefix` (`b""` = everything).
pub fn diff<A, B, E>(old: A, new: B, prefix: &[u8]) -> Result<(Vec<Change>, DiffSummary), E>
where
    A: Iterator<Item = Result<Entry, E>>,
    B: Iterator<Item = Result<Entry, E>>,
{
    let mut old = old
        .filter(|r| r.as_ref().map_or(true, |e| is_within(&e.path, prefix)))
        .peekable();
    let mut new = new
        .filter(|r| r.as_ref().map_or(true, |e| is_within(&e.path, prefix)))
        .peekable();
    let mut out = Vec::new();
    let mut summary = DiffSummary::default();
    loop {
        let ord = match (old.peek(), new.peek()) {
            (None, None) => break,
            (Some(Err(_)), _) => return Err(old.next().unwrap().unwrap_err()),
            (_, Some(Err(_))) => return Err(new.next().unwrap().unwrap_err()),
            (Some(Ok(_)), None) => Ordering::Less,
            (None, Some(Ok(_))) => Ordering::Greater,
            (Some(Ok(a)), Some(Ok(b))) => a.path.cmp(&b.path),
        };
        let change = match ord {
            Ordering::Less => {
                let o = old.next().unwrap()?;
                Some(Change {
                    change: ChangeKind::Removed,
                    kind: o.kind,
                    fields: vec![],
                    old_size: size_of(&o),
                    new_size: None,
                    path: o.path,
                })
            }
            Ordering::Greater => {
                let n = new.next().unwrap()?;
                Some(Change {
                    change: ChangeKind::Added,
                    kind: n.kind,
                    fields: vec![],
                    old_size: None,
                    new_size: size_of(&n),
                    path: n.path,
                })
            }
            Ordering::Equal => {
                let o = old.next().unwrap()?;
                let n = new.next().unwrap()?;
                if o.kind != n.kind {
                    Some(Change {
                        change: ChangeKind::TypeChanged,
                        kind: n.kind,
                        fields: vec![],
                        old_size: size_of(&o),
                        new_size: size_of(&n),
                        path: n.path,
                    })
                } else {
                    let f = fields(&o, &n);
                    (!f.is_empty()).then(|| Change {
                        change: ChangeKind::Modified,
                        kind: n.kind,
                        fields: f,
                        old_size: size_of(&o),
                        new_size: size_of(&n),
                        path: n.path,
                    })
                }
            }
        };
        if let Some(c) = change {
            summary.add(&c);
            out.push(c);
        }
    }
    Ok((out, summary))
}

/// The [`DiffSummary`] of `old` against `new` (both sorted by path), without copying any
/// entry or collecting the changes: what a backup needs for its counts. Same rules as
/// [`diff`].
pub fn summarize<'a>(
    old: impl Iterator<Item = &'a Entry>,
    new: impl Iterator<Item = &'a Entry>,
) -> DiffSummary {
    let (mut old, mut new) = (old.peekable(), new.peekable());
    let mut summary = DiffSummary::default();
    // Only the kind of change and the sizes count: no path is copied.
    let mut count = |change, o: Option<&Entry>, n: Option<&Entry>, fields| {
        summary.add(&Change {
            change,
            kind: n.or(o).map_or(Kind::File, |e| e.kind),
            fields,
            old_size: o.and_then(size_of),
            new_size: n.and_then(size_of),
            path: Vec::new(),
        });
    };
    loop {
        match (old.peek(), new.peek()) {
            (None, None) => break,
            (Some(_), None) => count(ChangeKind::Removed, old.next(), None, vec![]),
            (None, Some(_)) => count(ChangeKind::Added, None, new.next(), vec![]),
            (Some(a), Some(b)) => match a.path.cmp(&b.path) {
                Ordering::Less => count(ChangeKind::Removed, old.next(), None, vec![]),
                Ordering::Greater => count(ChangeKind::Added, None, new.next(), vec![]),
                Ordering::Equal => {
                    let (o, n) = (old.next().unwrap(), new.next().unwrap());
                    if o.kind != n.kind {
                        count(ChangeKind::TypeChanged, Some(o), Some(n), vec![]);
                    } else {
                        let f = fields(o, n);
                        if !f.is_empty() {
                            count(ChangeKind::Modified, Some(o), Some(n), f);
                        }
                    }
                }
            },
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::test_entry as entry;

    fn ok(v: Vec<Entry>) -> impl Iterator<Item = Result<Entry, std::io::Error>> {
        v.into_iter().map(Ok)
    }

    #[test]
    fn summarize_matches_diff() {
        let mut f = entry("a/f", Kind::File);
        f.size = 10;
        let old = vec![
            entry("a", Kind::Dir),
            f.clone(),
            entry("a/gone", Kind::File),
            entry("a/x", Kind::File),
        ];
        let mut f2 = f.clone();
        f2.size = 15;
        f2.mtime += 1;
        let new = vec![
            entry("a", Kind::Dir),
            f2,
            entry("a/new", Kind::File),
            entry("a/x", Kind::Dir),
        ];
        let (_, by_diff) = diff(ok(old.clone()), ok(new.clone()), b"").unwrap();
        assert_eq!(summarize(old.iter(), new.iter()), by_diff);
        assert_eq!(summarize([].iter(), new.iter()).added, 4);
    }

    #[test]
    fn all_kinds_of_change() {
        let mut f = entry("a/f", Kind::File);
        f.size = 10;
        let old = vec![
            entry("a", Kind::Dir),
            f.clone(),
            entry("a/gone", Kind::File),
            entry("a/x", Kind::File),
        ];
        let mut f2 = f.clone();
        f2.size = 15;
        f2.mtime += 1;
        let mut dir_touched = entry("a", Kind::Dir);
        dir_touched.mtime += 100; // directory mtimes don't count
        let new = vec![
            dir_touched,
            f2,
            entry("a/new", Kind::File),
            entry("a/x", Kind::Dir),
        ];
        let (changes, sum) = diff(ok(old), ok(new), b"").unwrap();
        let got: Vec<_> = changes
            .iter()
            .map(|c| (c.display_path(), c.change.marker()))
            .collect();
        assert_eq!(
            got,
            [
                ("/a/f".into(), 'M'),
                ("/a/gone".into(), '-'),
                ("/a/new".into(), '+'),
                ("/a/x".into(), 'T')
            ]
        );
        assert_eq!(changes[0].fields, [Field::Content, Field::Mtime]);
        assert_eq!(
            (
                sum.added,
                sum.removed,
                sum.modified,
                sum.type_changed,
                sum.bytes_delta
            ),
            (1, 1, 1, 1, 5)
        );
    }

    #[test]
    fn prefix_filter() {
        let old = vec![entry("a/1", Kind::File), entry("b/1", Kind::File)];
        let (c, _) = diff(ok(old), ok(vec![]), b"b").unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].display_path(), "/b/1");
    }
}
