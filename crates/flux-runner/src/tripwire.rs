//! Tripwire files: small hidden files Flux Sentry plants in the folders a backup covers,
//! and looks at before each backup. Nothing else has a reason to touch them, and
//! ransomware changes or renames every file it finds: one that has changed or gone is as
//! clear a sign as there is, however little else has happened yet.
//!
//! A tripwire is `.flux-sentry-tripwire.txt`: a note saying what it is, and a random
//! token (so each has its own contents). Flux remembers where it planted them and the
//! BLAKE3 of each (`sentry/<profile>.tripwires.json` in the state folder). They go in
//! each folder the profile backs up and in its first few visible subfolders. Only for a
//! person's own backups: a system backup's folders (`/etc`…) aren't written to.
//!
//! Flux only ever removes a tripwire that is still exactly as it planted it.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use flux_core::{Paths, Profile, ProfileId, Scope};

/// The name of a tripwire file.
pub const NAME: &str = ".flux-sentry-tripwire.txt";
/// Subfolders of a source that get one (the first, by name), besides the source itself.
const SUBFOLDERS: usize = 8;

/// A tripwire Flux planted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wire {
    pub path: PathBuf,
    /// BLAKE3 of its contents, in hex.
    pub hash: String,
}

/// What became of a tripwire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tripped {
    /// Its contents aren't what was planted.
    Changed(PathBuf),
    /// It is gone, though its folder is still there.
    Removed(PathBuf),
}

impl Tripped {
    pub fn path(&self) -> &Path {
        match self {
            Tripped::Changed(p) | Tripped::Removed(p) => p,
        }
    }
}

fn state_path(paths: &Paths, id: &ProfileId) -> PathBuf {
    paths
        .state_dir
        .join("sentry")
        .join(format!("{id}.tripwires.json"))
}

/// The tripwires planted for a profile.
pub fn load(paths: &Paths, id: &ProfileId) -> Vec<Wire> {
    std::fs::read(state_path(paths, id))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save(paths: &Paths, id: &ProfileId, wires: &[Wire]) {
    let path = state_path(paths, id);
    if wires.is_empty() {
        let _ = std::fs::remove_file(&path);
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec_pretty(wires) {
        let _ = flux_core::store::write_atomic(&path, &json, 0o600);
    }
}

fn hash_of(path: &Path) -> Option<String> {
    // A tripwire is a few hundred bytes; one that has grown past a megabyte has changed.
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_file() || meta.len() > 1 << 20 {
        return Some(String::new());
    }
    std::fs::read(path)
        .ok()
        .map(|b| blake3::hash(&b).to_hex().to_string())
}

fn contents(profile: &Profile) -> String {
    let mut token = [0u8; 16];
    let _ = getrandom::fill(&mut token);
    format!(
        "This file is a tripwire, put here by Flux Backup for the backup \u{201c}{}\u{201d}.\n\
         \n\
         Flux Sentry looks at it before every backup. Nothing has a reason to change this\n\
         file, and ransomware changes every file it can find: if this one is changed or\n\
         removed, Flux stops backing up at once and warns you, so your backups stay as\n\
         they were.\n\
         \n\
         Leave it as it is. To be rid of it, turn off \u{201c}Tripwire files\u{201d} in the backup's\n\
         settings (What to keep), and Flux removes them all.\n\
         \n\
         {}\n",
        profile.name,
        hex::encode(token)
    )
}

/// The folders of a profile that should hold a tripwire: each source that is a folder,
/// and its first visible subfolders.
fn places(profile: &Profile) -> Vec<PathBuf> {
    let is_dir = |p: &Path| std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_dir());
    let mut out = Vec::new();
    for root in flux_fs::scan::normalize_sources(&profile.sources.paths) {
        if !is_dir(&root) {
            continue;
        }
        let mut subs: Vec<PathBuf> = std::fs::read_dir(&root)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                is_dir(p)
                    && p.file_name()
                        .is_some_and(|n| !n.to_string_lossy().starts_with('.'))
            })
            .collect();
        subs.sort();
        subs.truncate(SUBFOLDERS);
        out.push(root);
        out.extend(subs);
    }
    out
}

fn write_new(path: &Path, text: &str) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(path)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()
}

/// Is the file at `path` a tripwire as Flux writes them, whole and unaltered in form:
/// the note, then a token? One planted for another backup of the same folder, or put
/// back by a restore of an older snapshot, is as good a tripwire as this profile's own:
/// what ransomware leaves of one is never this.
fn is_tripwire(path: &Path) -> bool {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => return false,
    };
    if !meta.file_type().is_file() || meta.len() > 4096 {
        return false;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let token = text.trim_end().rsplit('\n').next().unwrap_or_default();
    text.starts_with("This file is a tripwire, put here by Flux Backup for the backup ")
        && text.ends_with('\n')
        && token.len() == 32
        && token.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A tripwire whose folder can't be seen now (its drive isn't connected, most likely):
/// nothing can be said or done about it until it can.
fn out_of_reach(w: &Wire) -> bool {
    !w.path.parent().is_some_and(|d| d.is_dir())
}

/// The tripwires other profiles have planted (the same folder can be in two backups).
fn of_others(paths: &Paths, id: &ProfileId) -> Vec<PathBuf> {
    let own = state_path(paths, id);
    let Some(dir) = own.parent() else {
        return Vec::new();
    };
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            *p != own
                && p.file_name()
                    .is_some_and(|n| n.to_string_lossy().ends_with(".tripwires.json"))
        })
        .filter_map(|p| std::fs::read(p).ok())
        .filter_map(|b| serde_json::from_slice::<Vec<Wire>>(&b).ok())
        .flatten()
        .map(|w| w.path)
        .collect()
}

/// Plant tripwires where the profile should have them and has none yet. Returns how many
/// were planted. A file of that name that Flux didn't plant is left alone; a tripwire
/// already there (another backup of the same folder planted it) is watched as it is.
/// Those in folders the profile no longer backs up are removed.
pub fn plant(paths: &Paths, id: &ProfileId, profile: &Profile) -> usize {
    if paths.scope != Scope::User {
        return 0;
    }
    let before = load(paths, id);
    // Under a folder this profile no longer backs up: not looked at any more, so not
    // left lying there (if still as planted, and no other backup watches it).
    let sources = flux_fs::scan::normalize_sources(&profile.sources.paths);
    let (mut wires, strays): (Vec<Wire>, Vec<Wire>) = before
        .iter()
        .cloned()
        .partition(|w| sources.iter().any(|s| w.path.starts_with(s)));
    if !strays.is_empty() {
        let others = of_others(paths, id);
        for w in strays.iter().filter(|w| !others.contains(&w.path)) {
            if hash_of(&w.path).is_some_and(|h| h == w.hash) {
                let _ = std::fs::remove_file(&w.path);
            }
        }
    }
    let mut planted = 0;
    for dir in places(profile) {
        let path = dir.join(NAME);
        if wires.iter().any(|w| w.path == path) {
            continue;
        }
        if path.exists() {
            if is_tripwire(&path)
                && let Some(hash) = hash_of(&path)
            {
                wires.push(Wire { path, hash });
            }
            continue;
        }
        let text = contents(profile);
        if write_new(&path, &text).is_ok() {
            wires.push(Wire {
                path,
                hash: blake3::hash(text.as_bytes()).to_hex().to_string(),
            });
            planted += 1;
        }
    }
    if wires != before {
        save(paths, id, &wires);
    }
    planted
}

/// Look at every tripwire under one of `present` (the sources whose drive is there).
/// Returns those that were changed or removed. One whose folder is gone is forgotten:
/// folders are moved and deleted by their owner, files inside them aren't touched alone.
/// One that is a different tripwire now, whole (another backup of the folder set it
/// again, or a restore put an older one back), is watched as it is.
pub fn check(paths: &Paths, id: &ProfileId, present: &[PathBuf]) -> Vec<Tripped> {
    let wires = load(paths, id);
    let mut kept = Vec::with_capacity(wires.len());
    let mut tripped = Vec::new();
    for mut w in wires {
        if !present.iter().any(|root| w.path.starts_with(root)) {
            kept.push(w); // its drive isn't connected: nothing to say
            continue;
        }
        if out_of_reach(&w) {
            continue;
        }
        match hash_of(&w.path) {
            Some(h) if h == w.hash => {}
            Some(h) if is_tripwire(&w.path) => w.hash = h,
            Some(_) => tripped.push(Tripped::Changed(w.path.clone())),
            None => tripped.push(Tripped::Removed(w.path.clone())),
        }
        kept.push(w);
    }
    save(paths, id, &kept);
    tripped
}

/// Set again the tripwires that were changed or removed (after their owner has settled
/// what tripped them): a changed one is moved aside and a new one planted in its place;
/// a removed one is planted again. One that can't be seen now (its drive is away) is
/// kept as it is, to be looked at when it can be.
pub fn rearm(paths: &Paths, id: &ProfileId, profile: &Profile) {
    let mut kept = Vec::new();
    for mut w in load(paths, id) {
        if out_of_reach(&w) {
            kept.push(w);
            continue;
        }
        match hash_of(&w.path) {
            Some(h) if h == w.hash => kept.push(w),
            // Another backup of this folder has set it again already.
            Some(h) if is_tripwire(&w.path) => {
                w.hash = h;
                kept.push(w);
            }
            // Changed: not Flux's file any more. Out of the way under another name
            // (never deleted), so a fresh tripwire can take its place.
            Some(_) => {
                let mut aside = w.path.clone().into_os_string();
                aside.push(".tripped");
                let _ = std::fs::rename(&w.path, aside);
            }
            None => {}
        }
    }
    save(paths, id, &kept);
    plant(paths, id, profile);
}

/// Remove a profile's tripwires (the setting was turned off, or the profile deleted):
/// each one that is still exactly as planted, and that no other backup of the same
/// folder watches. Returns how many were removed. Those that can't be seen now (their
/// drive is away) are remembered, and removed by a later call.
pub fn remove(paths: &Paths, id: &ProfileId) -> usize {
    let wires = load(paths, id);
    let others = of_others(paths, id);
    let mut removed = 0;
    let mut later = Vec::new();
    for w in wires {
        if out_of_reach(&w) {
            later.push(w);
        } else if !others.contains(&w.path)
            && hash_of(&w.path).is_some_and(|h| h == w.hash)
            && std::fs::remove_file(&w.path).is_ok()
        {
            removed += 1;
        }
    }
    save(paths, id, &later);
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_core::profile::Target;

    fn setup() -> (tempfile::TempDir, Paths, ProfileId, Profile) {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("home");
        for d in ["Documents", "Pictures", ".cache", "Music/albums"] {
            std::fs::create_dir_all(src.join(d)).unwrap();
        }
        std::fs::write(src.join("notes.txt"), "mine").unwrap();
        let paths = Paths::under(tmp.path().join("flux"), Scope::User);
        let profile = Profile::new(
            "Home",
            Target::Local {
                path: tmp.path().join("drive"),
                fs_uuid: None,
            },
            vec![src],
        );
        (tmp, paths, ProfileId::new("home").unwrap(), profile)
    }

    #[test]
    fn planted_checked_tripped_and_removed() {
        let (tmp, paths, id, profile) = setup();
        let src = tmp.path().join("home");
        // The source and its visible subfolders, not hidden ones or deeper ones.
        assert_eq!(plant(&paths, &id, &profile), 4);
        for d in ["", "Documents", "Pictures", "Music"] {
            assert!(src.join(d).join(NAME).is_file(), "{d}");
        }
        assert!(!src.join(".cache").join(NAME).exists());
        assert!(!src.join("Music/albums").join(NAME).exists());
        let text = std::fs::read_to_string(src.join(NAME)).unwrap();
        assert!(text.contains("tripwire") && text.contains("\u{201c}Home\u{201d}"));
        // Planting again adds nothing; a new folder gets one.
        assert_eq!(plant(&paths, &id, &profile), 0);
        std::fs::create_dir(src.join("Videos")).unwrap();
        assert_eq!(plant(&paths, &id, &profile), 1);
        let present = [src.clone()];
        assert_eq!(check(&paths, &id, &present), []);

        // Ransomware: one encrypted in place, one renamed away.
        std::fs::write(src.join("Documents").join(NAME), [0x9c; 400]).unwrap();
        std::fs::rename(
            src.join("Pictures").join(NAME),
            src.join("Pictures").join(format!("{NAME}.locked")),
        )
        .unwrap();
        // A folder its owner deleted takes its tripwire along: no alarm.
        std::fs::remove_dir_all(src.join("Music")).unwrap();
        // Touching one without changing it is nothing either.
        let untouched = std::fs::read(src.join(NAME)).unwrap();
        std::fs::write(src.join(NAME), untouched).unwrap();
        let mut tripped = check(&paths, &id, &present);
        tripped.sort_by(|a, b| a.path().cmp(b.path()));
        assert_eq!(
            tripped,
            [
                Tripped::Changed(src.join("Documents").join(NAME)),
                Tripped::Removed(src.join("Pictures").join(NAME)),
            ]
        );
        // A drive that isn't connected says nothing.
        assert_eq!(check(&paths, &id, &[]), []);

        // Settled: the changed one is moved aside, both are planted afresh.
        rearm(&paths, &id, &profile);
        assert_eq!(check(&paths, &id, &present), []);
        assert!(
            src.join("Documents")
                .join(format!("{NAME}.tripped"))
                .is_file()
        );
        assert!(src.join("Pictures").join(NAME).is_file());

        // Turned off: every one still as planted is removed, nothing else.
        std::fs::write(src.join("Videos").join(NAME), "someone's own file now").unwrap();
        assert_eq!(remove(&paths, &id), 3);
        assert!(!src.join(NAME).exists() && !src.join("Documents").join(NAME).exists());
        assert!(
            src.join("Videos").join(NAME).is_file(),
            "changed: not ours to delete"
        );
        assert!(src.join("notes.txt").is_file());
        assert!(load(&paths, &id).is_empty());
    }

    #[test]
    fn someone_elses_file_and_system_backups_are_left_alone() {
        let (tmp, paths, id, profile) = setup();
        let src = tmp.path().join("home");
        std::fs::write(src.join(NAME), "not Flux's").unwrap();
        assert_eq!(plant(&paths, &id, &profile), 3);
        assert_eq!(
            std::fs::read_to_string(src.join(NAME)).unwrap(),
            "not Flux's"
        );
        assert_eq!(check(&paths, &id, std::slice::from_ref(&src)), []);
        remove(&paths, &id);
        assert!(src.join(NAME).is_file());

        let mut system = paths.clone();
        system.scope = Scope::System;
        assert_eq!(plant(&system, &id, &profile), 0);
    }

    /// Two backups of the same folder watch the same tripwires: the second takes the
    /// first's as they are, either may set them again without tripping the other, and
    /// neither removes a file the other still watches.
    #[test]
    fn two_backups_of_one_folder_share_their_tripwires() {
        let (tmp, paths, a, profile) = setup();
        let b = ProfileId::new("home-nas").unwrap();
        let src = tmp.path().join("home");
        let present = [src.clone()];
        assert_eq!(plant(&paths, &a, &profile), 4);
        assert_eq!(plant(&paths, &b, &profile), 0, "none written twice");
        assert_eq!(load(&paths, &b).len(), 4, "but all four are watched");

        // Ransomware changes one: both backups are stopped.
        std::fs::write(src.join("Documents").join(NAME), [0x9c; 400]).unwrap();
        assert_eq!(check(&paths, &a, &present).len(), 1);
        assert_eq!(check(&paths, &b, &present).len(), 1);
        // Settled for one, then the other: the new tripwire stands for both.
        rearm(&paths, &a, &profile);
        rearm(&paths, &b, &profile);
        assert_eq!(check(&paths, &a, &present), []);
        assert_eq!(check(&paths, &b, &present), []);
        let sorted = |id: &ProfileId| {
            let mut w = load(&paths, id);
            w.sort_by(|x, y| x.path.cmp(&y.path));
            w
        };
        assert_eq!(sorted(&a), sorted(&b));

        // One backup stops using them: the files stay for the other.
        assert_eq!(remove(&paths, &a), 0);
        assert!(src.join(NAME).is_file());
        assert_eq!(check(&paths, &b, &present), []);
        assert_eq!(remove(&paths, &b), 4);
        assert!(!src.join(NAME).exists());
    }

    /// A restore of an older snapshot puts an older tripwire back (they are backed up
    /// like any file): a whole tripwire with another token isn't an alarm.
    #[test]
    fn an_older_tripwire_put_back_by_a_restore_is_no_alarm() {
        let (tmp, paths, id, profile) = setup();
        let src = tmp.path().join("home");
        let present = [src.clone()];
        plant(&paths, &id, &profile);
        std::fs::write(src.join(NAME), contents(&profile)).unwrap();
        assert_eq!(check(&paths, &id, &present), []);
        // And it is the one watched from now on.
        std::fs::write(src.join(NAME), "not a tripwire any more").unwrap();
        assert_eq!(check(&paths, &id, &present).len(), 1);
    }

    /// A tripwire on a drive that is away when an alarm is settled, or when the setting
    /// is turned off, is neither forgotten nor orphaned.
    #[test]
    fn tripwires_out_of_reach_are_remembered() {
        let (tmp, paths, id, mut profile) = setup();
        let drive = tmp.path().join("drive-of-its-own");
        std::fs::create_dir_all(drive.join("Photos")).unwrap();
        profile.sources.paths.push(drive.clone());
        assert_eq!(plant(&paths, &id, &profile), 6);
        // Unplugged: the folder is gone from view.
        let away = tmp.path().join("unplugged");
        std::fs::rename(&drive, &away).unwrap();
        rearm(&paths, &id, &profile);
        assert_eq!(load(&paths, &id).len(), 6, "all still known");
        assert_eq!(remove(&paths, &id), 4);
        assert_eq!(load(&paths, &id).len(), 2, "the two on the drive wait");
        // Back again: they go too.
        std::fs::rename(&away, &drive).unwrap();
        assert_eq!(remove(&paths, &id), 2);
        assert!(load(&paths, &id).is_empty());
        assert!(!drive.join(NAME).exists());
    }

    /// A folder taken out of the backup loses its tripwires at the next backup.
    #[test]
    fn a_folder_no_longer_backed_up_loses_its_tripwires() {
        let (tmp, paths, id, mut profile) = setup();
        let other = tmp.path().join("work");
        std::fs::create_dir_all(&other).unwrap();
        profile.sources.paths.push(other.clone());
        assert_eq!(plant(&paths, &id, &profile), 5);
        profile.sources.paths.pop();
        assert_eq!(plant(&paths, &id, &profile), 0);
        assert!(!other.join(NAME).exists());
        assert_eq!(load(&paths, &id).len(), 4);
    }
}
