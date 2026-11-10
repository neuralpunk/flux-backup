//! Backup templates: where setting up a backup starts. Built in (Home, Documents,
//! Photos, Development, System, Blank), or your own: a backup's settings saved under a
//! name (in `templates/` in the config folder), without where it goes.

use std::path::{Path, PathBuf};

use serde::Serialize;

use flux_core::profile::{Profile, Retention, Target, Trigger};
use flux_core::units::HumanDuration;
use flux_core::{Paths, ProfileId, Scope};

/// A template, for choosing one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Template {
    /// `home`, `documents`…, or `own:<name>` for yours.
    pub id: String,
    pub name: String,
    /// What it does, in a line.
    pub line: String,
    pub icon: String,
    pub own: bool,
}

fn builtin(id: &str, name: &str, line: &str, icon: &str) -> Template {
    Template {
        id: id.into(),
        name: name.into(),
        line: line.into(),
        icon: icon.into(),
        own: false,
    }
}

fn dir(paths: &Paths) -> PathBuf {
    paths.config_dir.join("templates")
}

/// The templates for a scope: the built-in ones, then yours.
pub fn list(paths: &Paths) -> Vec<Template> {
    let mut out = match paths.scope {
        Scope::User => vec![
            builtin(
                "home",
                "Home folder",
                "Everything in your home folder, without caches and the trash. Daily.",
                "home",
            ),
            builtin(
                "documents",
                "Documents",
                "Your documents and desktop, every hour: lose an hour of work at most.",
                "file",
            ),
            builtin(
                "photos",
                "Photos",
                "Your pictures, daily, and kept for years: a snapshot a month for two years, one a year for ten.",
                "layers",
            ),
            builtin(
                "development",
                "Development",
                "Your projects, every hour, without dependencies and build output (node_modules, target, .venv…).",
                "terminal",
            ),
        ],
        Scope::System => vec![builtin(
            "system",
            "System settings",
            "This computer's configuration (/etc, /root, /usr/local, /opt and /srv), daily.",
            "shield",
        )],
    };
    out.push(builtin(
        "blank",
        "Start from nothing",
        "Choose every folder and setting yourself.",
        "plus",
    ));
    let mut own: Vec<Template> = std::fs::read_dir(dir(paths))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let file = e.file_name().to_string_lossy().into_owned();
            let slug = file.strip_suffix(".toml")?.to_owned();
            let text = std::fs::read_to_string(e.path()).ok()?;
            let p = Profile::from_toml(&text).ok()?;
            Some(Template {
                id: format!("own:{slug}"),
                line: describe(&p),
                name: field(&text, "template").unwrap_or(slug),
                icon: "copy".into(),
                own: true,
            })
        })
        .collect();
    own.sort_by(|a, b| a.name.cmp(&b.name));
    out.extend(own);
    out
}

fn field(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|l| {
        l.strip_prefix(&format!("# {name} = "))
            .and_then(|v| serde_json::from_str::<String>(v.trim()).ok())
    })
}

/// A line about one of your templates: its folders and schedule.
fn describe(p: &Profile) -> String {
    let folders: Vec<String> = p
        .sources
        .paths
        .iter()
        .take(3)
        .map(|s| s.display().to_string())
        .collect();
    let more = p.sources.paths.len().saturating_sub(3);
    let when = crate::schedule::summary(p);
    let mut c = when.chars();
    let when = c
        .next()
        .map(|f| f.to_uppercase().chain(c).collect::<String>())
        .unwrap_or_default();
    format!(
        "{}{}. {when}.",
        folders.join(", "),
        if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        },
    )
}

fn first_existing(home: &Path, names: &[&str]) -> PathBuf {
    names
        .iter()
        .map(|n| home.join(n))
        .find(|p| p.is_dir())
        .unwrap_or_else(|| home.join(names[0]))
}

fn retention(hourly: u32, daily: u32, weekly: u32, monthly: u32, yearly: u32) -> Retention {
    Retention {
        keep_hourly: hourly,
        keep_daily: daily,
        keep_weekly: weekly,
        keep_monthly: monthly,
        keep_yearly: yearly,
        ..Retention::default()
    }
}

fn every_hour(p: &mut Profile) {
    p.schedule.trigger = Trigger::Interval;
    p.schedule.every = Some(HumanDuration::from_secs(3600));
    p.schedule.on_calendar = None;
}

/// The new backup template `id` starts from. `base` is a new backup's defaults (this
/// computer's home folder, your usual exclusions, daily).
pub fn apply(paths: &Paths, id: &str, base: Profile, home: &Path) -> Result<Profile, String> {
    let mut p = base;
    match id {
        "home" | "system" => {}
        "documents" => {
            p.name = "Documents".into();
            p.sources.paths = vec![home.join("Documents")];
            if home.join("Desktop").is_dir() {
                p.sources.paths.push(home.join("Desktop"));
            }
            // Office's lock and owner files.
            p.sources.exclude = vec!["*.tmp".into(), "~$*".into(), ".~lock.*#".into()];
            every_hour(&mut p);
            p.retention = retention(24, 14, 8, 12, 0);
        }
        "photos" => {
            p.name = "Photos".into();
            p.sources.paths = vec![home.join("Pictures")];
            p.sources.exclude = vec![".thumbnails/".into(), "*.tmp".into()];
            p.retention = retention(0, 7, 4, 24, 10);
        }
        "development" => {
            p.name = "Projects".into();
            p.sources.paths = vec![first_existing(
                home,
                &["Projects", "src", "code", "dev", "git"],
            )];
            p.sources.exclude = [
                "node_modules/",
                "target/",
                "build/",
                "dist/",
                ".venv/",
                "venv/",
                "__pycache__/",
                ".tox/",
                ".mypy_cache/",
                ".pytest_cache/",
                ".gradle/",
                ".next/",
                "*.o",
                "*.pyc",
                "*.tmp",
            ]
            .map(String::from)
            .to_vec();
            every_hour(&mut p);
            p.retention = retention(24, 14, 8, 6, 0);
        }
        "blank" => {
            p.name = String::new();
            p.sources.paths = Vec::new();
            p.sources.exclude = Vec::new();
        }
        own => {
            let slug = own
                .strip_prefix("own:")
                .ok_or_else(|| format!("there's no template {own:?}"))?;
            let text = std::fs::read_to_string(dir(paths).join(format!("{slug}.toml")))
                .map_err(|_| format!("there's no template {own:?}"))?;
            let mut t = Profile::from_toml(&text).map_err(|e| format!("template {slug}: {e}"))?;
            t.target = p.target;
            p = t;
        }
    }
    if id == "system" {
        for extra in ["/opt", "/srv"] {
            let e = PathBuf::from(extra);
            if e.is_dir() && !p.sources.paths.contains(&e) {
                p.sources.paths.push(e);
            }
        }
    }
    Ok(p)
}

/// Save a backup's settings as a template called `name`: everything but where it goes
/// (the destination, other places, the computer it was recovered from) and webhooks.
/// Returns its id.
pub fn save(paths: &Paths, name: &str, p: &Profile) -> Result<String, String> {
    let name = name.trim();
    let slug = ProfileId::slugify(name).ok_or("give the template a name")?;
    let mut t = p.clone();
    t.target = Target::Local {
        path: PathBuf::new(),
        fs_uuid: None,
    };
    t.destinations.clear();
    t.origin = None;
    t.webhooks.clear();
    t.offsite = false;
    if let Some(v) = t.vault.as_mut() {
        v.unlock_file = None;
    }
    let body = t.to_toml().map_err(|e| e.to_string())?;
    let text = format!(
        "# A Flux Backup template: new backups can start from it.\n# template = {}\n\n{body}",
        serde_json::to_string(name).unwrap_or_default()
    );
    std::fs::create_dir_all(dir(paths)).map_err(|e| e.to_string())?;
    flux_core::store::write_atomic(
        &dir(paths).join(format!("{slug}.toml")),
        text.as_bytes(),
        0o600,
    )
    .map_err(|e| e.to_string())?;
    Ok(format!("own:{slug}"))
}

/// Remove one of your templates.
pub fn delete(paths: &Paths, id: &str) -> Result<(), String> {
    let slug = id
        .strip_prefix("own:")
        .ok_or("only your own templates can be removed")?;
    std::fs::remove_file(dir(paths).join(format!("{slug}.toml")))
        .map_err(|_| format!("there's no template {id:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(home: &Path) -> Profile {
        let mut p = Profile::new(
            "Home folder",
            Target::Local {
                path: PathBuf::new(),
                fs_uuid: None,
            },
            vec![home.to_path_buf()],
        );
        p.sources.exclude = vec!["~/.cache/".into()];
        p.schedule.trigger = Trigger::Calendar;
        p.schedule.on_calendar = Some("daily".into());
        p
    }

    #[test]
    fn built_in_templates() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("me");
        std::fs::create_dir_all(home.join("code")).unwrap();
        std::fs::create_dir_all(home.join("Desktop")).unwrap();
        let paths = Paths::under(t.path().join("flux"), Scope::User);
        let ids: Vec<_> = list(&paths).into_iter().map(|t| t.id).collect();
        assert_eq!(ids, ["home", "documents", "photos", "development", "blank"]);
        let p = apply(&paths, "home", base(&home), &home).unwrap();
        assert_eq!(p, base(&home), "the defaults as they are");
        let d = apply(&paths, "documents", base(&home), &home).unwrap();
        assert_eq!(
            d.sources.paths,
            [home.join("Documents"), home.join("Desktop")]
        );
        assert_eq!(d.schedule.trigger, Trigger::Interval);
        assert_eq!(d.retention.keep_hourly, 24);
        let dev = apply(&paths, "development", base(&home), &home).unwrap();
        assert_eq!(
            dev.sources.paths,
            [home.join("code")],
            "the folder that's there"
        );
        assert!(dev.sources.exclude.contains(&"node_modules/".into()));
        let ph = apply(&paths, "photos", base(&home), &home).unwrap();
        assert_eq!(
            (ph.retention.keep_monthly, ph.retention.keep_yearly),
            (24, 10)
        );
        for p in [&d, &dev, &ph] {
            assert!(
                p.validate()
                    .iter()
                    .all(|i| i.severity != flux_core::Severity::Error
                        || i.field.starts_with("target")),
                "{:?}",
                p.validate()
            );
        }
        let b = apply(&paths, "blank", base(&home), &home).unwrap();
        assert!(b.sources.paths.is_empty() && b.name.is_empty());
        assert!(apply(&paths, "nonsense", base(&home), &home).is_err());
        let sys = Paths::under(t.path().join("sys"), Scope::System);
        let ids: Vec<_> = list(&sys).into_iter().map(|t| t.id).collect();
        assert_eq!(ids, ["system", "blank"]);
    }

    #[test]
    fn your_own_templates() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("me");
        let paths = Paths::under(t.path().join("flux"), Scope::User);
        let mut p = base(&home);
        p.name = "Work".into();
        p.target = Target::Local {
            path: "/run/media/me/USB".into(),
            fs_uuid: Some("1234".into()),
        };
        p.sources.exclude.push("*.iso".into());
        p.webhooks.push(flux_core::webhook::Webhook {
            url: "https://ntfy.sh/secret".into(),
            preset: flux_core::webhook::Preset::Ntfy,
            events: Vec::new(),
            headers: Vec::new(),
            enabled: true,
        });
        let id = save(&paths, "Work laptop", &p).unwrap();
        assert_eq!(id, "own:work-laptop");
        let mine = list(&paths).into_iter().find(|t| t.own).unwrap();
        assert_eq!(
            (mine.name.as_str(), mine.icon.as_str()),
            ("Work laptop", "copy")
        );
        assert!(mine.line.contains("Daily"), "{}", mine.line);
        let text =
            std::fs::read_to_string(paths.config_dir.join("templates/work-laptop.toml")).unwrap();
        assert!(
            !text.contains("ntfy") && !text.contains("/run/media"),
            "{text}"
        );
        // A new backup from it: its settings, with the new backup's (empty) destination.
        let n = apply(&paths, &id, base(&home), &home).unwrap();
        assert_eq!(n.name, "Work");
        assert!(n.sources.exclude.contains(&"*.iso".into()));
        assert_eq!(n.target, base(&home).target);
        delete(&paths, &id).unwrap();
        assert!(list(&paths).iter().all(|t| !t.own));
        assert!(delete(&paths, "home").is_err());
    }
}
