//! A file manager's *Leave out of backups* and *Back up this folder*: what each of your
//! backups would change for a path ([`folder_plan_impl`]), and making those changes once
//! they're confirmed ([`apply_folder_change_impl`]). The changes are worked out again when
//! applied, so the page only says which backups, never what to write.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use flux_core::{Profile, ProfileId, ProfileStore, sys};
use flux_fs::coverage::{self, Why};

use crate::{ApiError, Result, Service};

/// What the person asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Want {
    /// *Leave out of backups*.
    Exclude,
    /// *Back up this folder*.
    Include,
}

/// The change to one backup's settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Change {
    /// Add an exclusion rule (`~/Downloads/big.iso`).
    AddRule { rule: String },
    /// Stop taking one of its folders: the path is one of them.
    RemoveFolder { folder: String },
    /// Take the folder; `replaces`: folders it took inside it, which it now covers.
    AddFolder {
        folder: String,
        replaces: Vec<String>,
    },
    /// Remove the rule that leaves out this path, and only it.
    RemoveRule { rule: String },
}

/// One backup, and what would change in it.
#[derive(Debug, Serialize)]
pub struct BackupChange {
    pub id: String,
    pub name: String,
    /// It takes the path now.
    pub backed_up: bool,
    /// Why it's in or out now, in words.
    pub why: String,
    pub change: Option<Change>,
    /// Why no change is offered, when none is.
    pub blocked: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FolderPlan {
    /// Absolute.
    pub path: String,
    /// `~/Downloads` for one in the home folder.
    pub shown: String,
    pub is_dir: bool,
    /// Disk space it takes, if it could be counted (`partial`: at least that).
    pub size: Option<u64>,
    pub partial: bool,
    /// For *Leave out*: the backups that take it. For *Back up*: every backup.
    pub backups: Vec<BackupChange>,
}

#[derive(Debug, Serialize)]
pub struct Applied {
    /// The backups changed: id and name.
    pub changed: Vec<(String, String)>,
    /// Backups that couldn't be changed, and why.
    pub failed: Vec<(String, String)>,
}

/// `path` as `~/…` when it's in the home folder.
pub fn shown(path: &Path) -> String {
    match sys::home_dir().and_then(|h| path.strip_prefix(&h).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// Absolute, without `.` or `..`, without a trailing `/`.
fn clean(path: &str) -> Result<PathBuf> {
    let p = PathBuf::from(path.trim());
    let p = std::path::absolute(&p).unwrap_or(p);
    if !p.is_absolute() {
        return Err(ApiError::new("invalid", "Give a full path."));
    }
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            c => out.push(c),
        }
    }
    Ok(out)
}

/// `path` spelled the way backup `p` spells its folders: on Fedora Atomic a file manager
/// says `/var/home/me/x` where the backup takes `/home/me` (a link to it). `None`: no folder
/// of the backup holds it, however spelled.
fn as_backup_spells(p: &Profile, path: &Path) -> Option<PathBuf> {
    if p.sources.paths.iter().any(|s| path.starts_with(s)) {
        return Some(path.to_path_buf());
    }
    let real = std::fs::canonicalize(path).ok()?;
    p.sources
        .paths
        .iter()
        .filter_map(|s| {
            let rs = std::fs::canonicalize(s).ok()?;
            let rest = real.strip_prefix(&rs).ok()?;
            Some(s.join(rest))
        })
        .max_by_key(|x| x.as_os_str().len())
}

/// Characters that mean something in a gitignore pattern, escaped so the rule names this
/// one path. `None` for a name that isn't valid text (a rule can't be written for it).
fn rule_for(path: &Path, is_dir: bool) -> Option<String> {
    let (prefix, rest) = match sys::home_dir().and_then(|h| path.strip_prefix(&h).ok()) {
        Some(r) if !r.as_os_str().is_empty() => ("~/", r.to_path_buf()),
        _ => ("/", path.strip_prefix("/").ok()?.to_path_buf()),
    };
    let text = rest.to_str()?;
    let mut s = String::from(prefix);
    for c in text.chars() {
        if matches!(c, '\\' | '*' | '?' | '[' | ']') {
            s.push('\\');
        }
        s.push(c);
    }
    // A trailing space would be dropped from the pattern.
    let spaces = s.len() - s.trim_end_matches(' ').len();
    if spaces > 0 {
        s.truncate(s.len() - spaces);
        for _ in 0..spaces {
            s.push_str("\\ ");
        }
    }
    if is_dir {
        s.push('/');
    }
    Some(s)
}

/// Is `rule` (as written) exactly `path`, nothing else: no wildcards, `~/` or `/` then the
/// path?
fn names_only(rule: &str, path: &Path) -> bool {
    let r = rule.trim().trim_end_matches('/');
    if r.contains(['*', '?', '[', '\\']) || r.starts_with('!') {
        return false;
    }
    let expanded = match (r.strip_prefix("~/"), sys::home_dir()) {
        (Some(rest), Some(h)) => h.join(rest),
        _ if r.starts_with('/') => PathBuf::from(r),
        _ => return false,
    };
    expanded == path
}

fn plan_one(
    p: &Profile,
    want: Want,
    path: &Path,
    is_dir: bool,
) -> (bool, String, Option<Change>, Option<String>) {
    let rules = match coverage::Rules::of(p) {
        Ok(r) => r,
        Err(e) => {
            return (
                false,
                e.to_string(),
                None,
                Some("Its exclusions have a mistake: fix them in its settings first.".into()),
            );
        }
    };
    let spelled = as_backup_spells(p, path).unwrap_or_else(|| path.to_path_buf());
    let why = coverage::explain(&rules, &spelled);
    let text = why.text(&spelled);
    let backed_up = why.included();
    let (change, blocked) = match want {
        Want::Exclude if !backed_up => (None, None),
        Want::Exclude => {
            if p.sources.paths.contains(&spelled) {
                if p.sources.paths.len() == 1 {
                    (
                        None,
                        Some("It's all this backup takes. Delete the backup instead, or choose what it takes in its settings.".into()),
                    )
                } else {
                    (
                        Some(Change::RemoveFolder {
                            folder: spelled.display().to_string(),
                        }),
                        None,
                    )
                }
            } else {
                match rule_for(&spelled, is_dir) {
                    Some(rule) => (Some(Change::AddRule { rule }), None),
                    None => (None, Some("Its name isn't plain text, so Flux can't write a rule for it: leave it out in the backup's settings.".into())),
                }
            }
        }
        Want::Include => match &why {
            Why::Included { .. } => (None, None),
            Why::NotInSources => {
                let replaces: Vec<String> = p
                    .sources
                    .paths
                    .iter()
                    .filter(|s| s.starts_with(path) && s.as_path() != path)
                    .map(|s| s.display().to_string())
                    .collect();
                (
                    Some(Change::AddFolder {
                        folder: path.display().to_string(),
                        replaces,
                    }),
                    None,
                )
            }
            Why::Excluded { rule, at } if at == &spelled && names_only(rule, &spelled) => {
                (Some(Change::RemoveRule { rule: rule.clone() }), None)
            }
            Why::Excluded { .. } | Why::GitIgnored { .. } => (
                None,
                Some("Change what it leaves out in its settings to take it.".into()),
            ),
            Why::Storage { .. } => (None, Some("It's where this backup keeps its copies.".into())),
            Why::TooLarge { .. } => (None, Some("Raise the size limit in its settings to take it.".into())),
            Why::Marker { .. } => (None, Some("Remove the marker file, or change what it leaves out in its settings, to take it.".into())),
            Why::OtherDisk { .. } => (None, Some("Turn off *Stay on one disk* in its settings to take it.".into())),
        },
    };
    (backed_up, text, change, blocked)
}

/// What each of your backups would change for `path`.
pub fn folder_plan_impl(state: &Service, path: String, want: Want) -> Result<FolderPlan> {
    let path = clean(&path)?;
    let meta = std::fs::metadata(&path).ok();
    let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
    let store = ProfileStore::new(&state.paths);
    let mut backups = Vec::new();
    for listed in store.list().unwrap_or_default() {
        let Ok(p) = listed.profile else { continue };
        let (backed_up, why, change, blocked) = plan_one(&p, want, &path, is_dir);
        if want == Want::Exclude && !backed_up {
            continue;
        }
        backups.push(BackupChange {
            id: listed.id.to_string(),
            name: p.name.clone(),
            backed_up,
            why,
            change,
            blocked,
        });
    }
    let (size, partial) = match meta {
        Some(_) => match crate::edit::du_partial(&path, Instant::now() + Duration::from_secs(3)) {
            Some((s, done)) => (Some(s), !done),
            None => (None, false),
        },
        None => (None, false),
    };
    Ok(FolderPlan {
        shown: shown(&path),
        path: path.display().to_string(),
        is_dir,
        size,
        partial,
        backups,
    })
}

fn apply(p: &mut Profile, change: &Change) {
    let s = &mut p.sources;
    match change {
        Change::AddRule { rule } => {
            if !s.exclude.contains(rule) {
                s.exclude.push(rule.clone());
            }
        }
        Change::RemoveFolder { folder } => s.paths.retain(|x| x.as_path() != Path::new(folder)),
        Change::AddFolder { folder, replaces } => {
            s.paths
                .retain(|x| !replaces.iter().any(|r| x.as_path() == Path::new(r)));
            s.paths.push(PathBuf::from(folder));
        }
        Change::RemoveRule { rule } => s.exclude.retain(|x| x != rule),
    }
}

/// Make the change worked out for `path` in each of backups `ids`.
pub fn apply_folder_change_impl(
    state: &Service,
    path: String,
    want: Want,
    ids: Vec<String>,
) -> Result<Applied> {
    let plan = folder_plan_impl(state, path, want)?;
    let store = ProfileStore::new(&state.paths);
    let mut out = Applied {
        changed: Vec::new(),
        failed: Vec::new(),
    };
    for id in ids {
        let Some(b) = plan.backups.iter().find(|b| b.id == id) else {
            out.failed.push((id, "It doesn't take this.".into()));
            continue;
        };
        let Some(change) = &b.change else {
            let why = b
                .blocked
                .clone()
                .unwrap_or_else(|| "Nothing to change.".into());
            out.failed.push((b.name.clone(), why));
            continue;
        };
        let pid = ProfileId::new(&id).map_err(|e| ApiError::new("error", e.to_string()))?;
        let mut p = store.load(&pid)?;
        apply(&mut p, change);
        let value = serde_json::to_value(&p).map_err(|e| ApiError::new("error", e.to_string()))?;
        let saved = crate::edit::save_profile_impl(state, id.clone(), value, false)?;
        if saved.saved {
            out.changed.push((id, b.name.clone()));
        } else {
            let why = saved
                .issues
                .iter()
                .find(|i| i.severity == flux_core::Severity::Error)
                .map_or_else(
                    || "Its settings wouldn't be valid.".into(),
                    |i| i.message.clone(),
                );
            out.failed.push((b.name.clone(), why));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_core::profile::Target;
    use flux_core::{Paths, Scope};

    fn setup(sources: &[(&str, &[&str])]) -> (tempfile::TempDir, Service) {
        let tmp = tempfile::tempdir().unwrap();
        let t = tmp.path();
        for d in ["home/docs/work", "home/films", "other/stuff", "tgt"] {
            std::fs::create_dir_all(t.join(d)).unwrap();
        }
        std::fs::write(t.join("home/docs/big file [1].iso"), vec![0u8; 8192]).unwrap();
        let paths = Paths::under(t.join("flux"), Scope::User);
        let store = ProfileStore::new(&paths);
        for (id, srcs) in sources {
            let p = Profile::new(
                *id,
                Target::Local {
                    path: t.join("tgt"),
                    fs_uuid: None,
                },
                srcs.iter().map(|s| t.join(s)).collect(),
            );
            store.create(&ProfileId::new(*id).unwrap(), &p).unwrap();
        }
        (tmp, Service::new(paths))
    }

    fn taken(state: &Service, id: &str, path: &Path) -> bool {
        let p = ProfileStore::new(&state.paths)
            .load(&ProfileId::new(id).unwrap())
            .unwrap();
        coverage::explain(&coverage::Rules::of(&p).unwrap(), path).included()
    }

    #[test]
    fn leaving_out_adds_a_rule_to_every_backup_that_takes_it() {
        let (tmp, state) = setup(&[
            ("home", &["home"]),
            ("docs", &["home/docs"]),
            ("other", &["other"]),
        ]);
        let t = tmp.path();
        let file = t.join("home/docs/big file [1].iso");
        let plan = folder_plan_impl(&state, file.display().to_string(), Want::Exclude).unwrap();
        let ids: Vec<_> = plan.backups.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, ["docs", "home"], "only the backups that take it");
        assert_eq!(plan.size, Some(8192));
        let Some(Change::AddRule { rule }) = &plan.backups[0].change else {
            panic!()
        };
        assert!(rule.ends_with("big file \\[1\\].iso"), "{rule}");
        let done = apply_folder_change_impl(
            &state,
            file.display().to_string(),
            Want::Exclude,
            vec!["docs".into(), "home".into()],
        )
        .unwrap();
        assert_eq!(done.changed.len(), 2, "{:?}", done.failed);
        assert!(!taken(&state, "docs", &file));
        assert!(!taken(&state, "home", &file));
        // Its neighbours are still taken.
        assert!(taken(&state, "home", &t.join("home/docs/work")));
        // Asking again changes nothing.
        let again = folder_plan_impl(&state, file.display().to_string(), Want::Exclude).unwrap();
        assert!(again.backups.is_empty());
    }

    #[test]
    fn leaving_out_one_of_its_folders_stops_taking_it_but_not_the_only_one() {
        let (tmp, state) = setup(&[
            ("two", &["home/docs", "home/films"]),
            ("one", &["home/films"]),
        ]);
        let films = tmp.path().join("home/films");
        let plan = folder_plan_impl(&state, films.display().to_string(), Want::Exclude).unwrap();
        let one = plan.backups.iter().find(|b| b.id == "one").unwrap();
        assert!(one.change.is_none() && one.blocked.is_some());
        let two = plan.backups.iter().find(|b| b.id == "two").unwrap();
        assert!(matches!(two.change, Some(Change::RemoveFolder { .. })));
        let done = apply_folder_change_impl(
            &state,
            films.display().to_string(),
            Want::Exclude,
            vec!["two".into(), "one".into()],
        )
        .unwrap();
        assert_eq!(done.changed.len(), 1);
        assert_eq!(done.failed.len(), 1);
        let p = ProfileStore::new(&state.paths)
            .load(&ProfileId::new("two").unwrap())
            .unwrap();
        assert_eq!(p.sources.paths, vec![tmp.path().join("home/docs")]);
    }

    #[test]
    fn backing_up_a_folder_adds_it_and_replaces_what_it_covers() {
        let (tmp, state) = setup(&[("docs", &["home/docs/work"])]);
        let t = tmp.path();
        let plan = folder_plan_impl(
            &state,
            t.join("home/docs").display().to_string(),
            Want::Include,
        )
        .unwrap();
        let Some(Change::AddFolder { replaces, .. }) = &plan.backups[0].change else {
            panic!()
        };
        assert_eq!(replaces, &[t.join("home/docs/work").display().to_string()]);
        apply_folder_change_impl(
            &state,
            t.join("home/docs").display().to_string(),
            Want::Include,
            vec!["docs".into()],
        )
        .unwrap();
        let p = ProfileStore::new(&state.paths)
            .load(&ProfileId::new("docs").unwrap())
            .unwrap();
        assert_eq!(p.sources.paths, vec![t.join("home/docs")]);
    }

    #[test]
    fn backing_up_a_folder_a_rule_left_out_removes_that_rule_only_when_it_names_it() {
        let (tmp, state) = setup(&[("home", &["home"])]);
        let t = tmp.path();
        let store = ProfileStore::new(&state.paths);
        let id = ProfileId::new("home").unwrap();
        let mut p = store.load(&id).unwrap();
        p.sources.exclude = vec![
            format!("{}/", t.join("home/films").display()),
            "work/".into(),
        ];
        store.save(&id, &p).unwrap();
        let films = folder_plan_impl(
            &state,
            t.join("home/films").display().to_string(),
            Want::Include,
        )
        .unwrap();
        assert!(matches!(
            films.backups[0].change,
            Some(Change::RemoveRule { .. })
        ));
        // `work/` leaves out every folder of that name: not removed for this one.
        let work = folder_plan_impl(
            &state,
            t.join("home/docs/work").display().to_string(),
            Want::Include,
        )
        .unwrap();
        assert!(work.backups[0].change.is_none() && work.backups[0].blocked.is_some());
        apply_folder_change_impl(
            &state,
            t.join("home/films").display().to_string(),
            Want::Include,
            vec!["home".into()],
        )
        .unwrap();
        assert!(taken(&state, "home", &t.join("home/films")));
        assert_eq!(
            store.load(&id).unwrap().sources.exclude,
            vec!["work/".to_string()]
        );
    }

    #[test]
    fn rules_name_one_path() {
        assert_eq!(
            rule_for(Path::new("/srv/a*b?/c[d]"), true).unwrap(),
            "/srv/a\\*b\\?/c\\[d\\]/"
        );
        assert_eq!(
            rule_for(Path::new("/srv/trailing  "), false).unwrap(),
            "/srv/trailing\\ \\ "
        );
        assert!(names_only("/srv/x/", Path::new("/srv/x")));
        assert!(!names_only("/srv/*/", Path::new("/srv/x")));
        assert!(!names_only("x/", Path::new("/srv/x")));
    }
}
