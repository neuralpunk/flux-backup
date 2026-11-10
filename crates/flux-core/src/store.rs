//! Loading and saving profiles under `<config_dir>/profiles/<id>.toml`.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::paths::Paths;
use crate::profile::{Profile, ProfileId};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("profile {0:?} not found")]
    NotFound(ProfileId),
    #[error("profile {0:?} already exists")]
    AlreadyExists(ProfileId),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },
    #[error("cannot serialise profile: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("{0} is a destination of profile {1}: change it in that profile")]
    Destination(ProfileId, ProfileId),
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> StoreError + '_ {
    move |source| StoreError::Io {
        path: path.to_owned(),
        source,
    }
}

/// A profile file that exists on disk, whether or not it parsed.
pub struct ListedProfile {
    pub id: ProfileId,
    pub path: PathBuf,
    pub profile: Result<Profile, StoreError>,
}

/// Profile persistence for one [`Paths`] scope.
#[derive(Debug, Clone)]
pub struct ProfileStore {
    dir: PathBuf,
}

impl ProfileStore {
    pub fn new(paths: &Paths) -> Self {
        Self {
            dir: paths.profiles_dir(),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path_of(&self, id: &ProfileId) -> PathBuf {
        self.dir.join(format!("{id}.toml"))
    }

    pub fn exists(&self, id: &ProfileId) -> bool {
        self.path_of(id).is_file()
    }

    /// All profiles, sorted by id. Files that fail to parse are returned with their error so
    /// one broken profile doesn't hide the others. Files whose names aren't valid ids are
    /// skipped.
    pub fn list(&self) -> Result<Vec<ListedProfile>, StoreError> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io_err(&self.dir)(e)),
        };
        let mut out = Vec::new();
        for entry in entries {
            let entry = entry.map_err(io_err(&self.dir))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let Some(id) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| ProfileId::new(s).ok())
            else {
                tracing::warn!(
                    "ignoring profile file with invalid name: {}",
                    path.display()
                );
                continue;
            };
            let profile = read_profile(&path);
            out.push(ListedProfile { id, path, profile });
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    /// A profile, or (for `home+nas`) the profile as its destination `nas` sees it.
    pub fn load(&self, id: &ProfileId) -> Result<Profile, StoreError> {
        if let Some((base, dest)) = id.split_destination() {
            return self
                .load(&base)?
                .for_destination(dest)
                .ok_or_else(|| StoreError::NotFound(id.clone()));
        }
        let path = self.path_of(id);
        match read_profile(&path) {
            Err(StoreError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
                Err(StoreError::NotFound(id.clone()))
            }
            r => r,
        }
    }

    /// The line that says which schema a profile file Flux wrote follows (from 1.0 on).
    const SCHEMA_LINE: &str = "# schema";

    /// Create a new profile; fails if one with this id exists.
    pub fn create(&self, id: &ProfileId, profile: &Profile) -> Result<PathBuf, StoreError> {
        if let Some((base, _)) = id.split_destination() {
            return Err(StoreError::Destination(id.clone(), base));
        }
        if self.exists(id) {
            return Err(StoreError::AlreadyExists(id.clone()));
        }
        self.save(id, profile)
    }

    /// Write (create or replace) a profile atomically. Sources on a drive of their own get
    /// that drive recorded (`sources.volumes`), so a missing drive is recognised later.
    pub fn save(&self, id: &ProfileId, profile: &Profile) -> Result<PathBuf, StoreError> {
        if let Some((base, _)) = id.split_destination() {
            return Err(StoreError::Destination(id.clone(), base));
        }
        let mut profile = profile.clone();
        crate::source_check::record_volumes(&mut profile);
        let mut text = format!(
            "# Flux Backup profile. Reference: https://github.com/neuralpunk/flux-backup\n{} {}, written by Flux Backup {}\n",
            Self::SCHEMA_LINE,
            crate::profile::PROFILE_VERSION,
            crate::VERSION
        );
        text.push_str(&profile.to_toml()?);
        let path = self.path_of(id);
        create_private_dir(&self.dir)?;
        // A profile from before 1.0 (or written by hand): kept as it was, the first time
        // Flux rewrites it.
        if let Ok(old) = fs::read_to_string(&path)
            && !old.contains(Self::SCHEMA_LINE)
        {
            let keep = self.dir.join(format!("{id}.toml.pre-1.0.bak"));
            if !keep.exists() {
                write_atomic(&keep, old.as_bytes(), 0o600)?;
            }
        }
        write_atomic(&path, text.as_bytes(), 0o600)?;
        Ok(path)
    }

    pub fn delete(&self, id: &ProfileId) -> Result<(), StoreError> {
        let path = self.path_of(id);
        fs::remove_file(&path).map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                StoreError::NotFound(id.clone())
            } else {
                io_err(&path)(e)
            }
        })
    }
}

fn read_profile(path: &Path) -> Result<Profile, StoreError> {
    let text = fs::read_to_string(path).map_err(io_err(path))?;
    Profile::from_toml(&text).map_err(|e| StoreError::Parse {
        path: path.to_owned(),
        source: Box::new(e),
    })
}

/// Create `dir` (and missing parents); a newly created `dir` gets mode 0700.
pub fn create_private_dir(dir: &Path) -> Result<(), StoreError> {
    if dir.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(dir).map_err(io_err(dir))?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(io_err(dir))
}

/// Write `data` to `path` so that readers see either the old or the new content, never a
/// partial file: write a temp file in the same directory, fsync it, rename it over `path`,
/// then fsync the directory.
pub fn write_atomic(path: &Path, data: &[u8], mode: u32) -> Result<(), StoreError> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    let result = (|| {
        // Made new: never written through a link (or into a file) someone left under
        // this name.
        let _ = fs::remove_file(&tmp);
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        fs::File::open(dir)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(io_err(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Scope;
    use crate::profile::Target;

    fn setup() -> (tempfile::TempDir, ProfileStore) {
        let tmp = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(&Paths::under(tmp.path(), Scope::User));
        (tmp, store)
    }

    fn profile(name: &str) -> Profile {
        Profile::new(
            name,
            Target::Local {
                path: "/mnt/b".into(),
                fs_uuid: None,
            },
            vec!["/home".into()],
        )
    }

    #[test]
    fn crud() {
        let (_t, store) = setup();
        let id = ProfileId::new("home").unwrap();
        assert!(store.list().unwrap().is_empty());
        assert!(matches!(store.load(&id), Err(StoreError::NotFound(_))));

        let path = store.create(&id, &profile("Home")).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(matches!(
            store.create(&id, &profile("x")),
            Err(StoreError::AlreadyExists(_))
        ));
        assert_eq!(store.load(&id).unwrap().name, "Home");

        let mut p = store.load(&id).unwrap();
        p.name = "Renamed".into();
        store.save(&id, &p).unwrap();
        assert_eq!(store.load(&id).unwrap().name, "Renamed");

        store.delete(&id).unwrap();
        assert!(matches!(store.delete(&id), Err(StoreError::NotFound(_))));
    }

    #[test]
    fn a_profile_from_before_1_0_is_kept_when_first_rewritten() {
        let (_t, store) = setup();
        let id = ProfileId::new("home").unwrap();
        let p = profile("Home");
        // As 0.3 wrote it (no schema line).
        let old = format!("# Flux Backup profile.\n{}", p.to_toml().unwrap());
        fs::create_dir_all(&store.dir).unwrap();
        fs::write(store.path_of(&id), &old).unwrap();
        assert_eq!(store.load(&id).unwrap(), p, "read as it is");
        store.save(&id, &p).unwrap();
        let kept = store.dir.join("home.toml.pre-1.0.bak");
        assert_eq!(fs::read_to_string(&kept).unwrap(), old);
        let now = fs::read_to_string(store.path_of(&id)).unwrap();
        assert!(now.contains("# schema 1, written by Flux Backup"), "{now}");
        // Saved again: the copy stays the original; it isn't listed as a profile.
        store.save(&id, &p).unwrap();
        assert_eq!(fs::read_to_string(&kept).unwrap(), old);
        assert_eq!(store.list().unwrap().len(), 1);
        // A profile Flux 1.0 wrote gets no copy.
        let id2 = ProfileId::new("new").unwrap();
        store.create(&id2, &p).unwrap();
        store.save(&id2, &p).unwrap();
        assert!(!store.dir.join("new.toml.pre-1.0.bak").exists());
    }

    #[test]
    fn list_reports_broken_files_without_hiding_others() {
        let (_t, store) = setup();
        store
            .create(&ProfileId::new("b").unwrap(), &profile("B"))
            .unwrap();
        fs::write(store.dir().join("a.toml"), "name = ").unwrap();
        fs::write(store.dir().join("Bad Name.toml"), "").unwrap();
        fs::write(store.dir().join("notes.txt"), "").unwrap();
        let list = store.list().unwrap();
        let ids: Vec<_> = list.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
        assert!(list[0].profile.is_err());
        assert!(list[1].profile.is_ok());
    }
}
