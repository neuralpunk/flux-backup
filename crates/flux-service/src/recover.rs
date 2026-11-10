//! Recovering backups on a new computer, and how ready a backup is for that.

use serde::Serialize;

use flux_core::profile::{Engine, KeptAs, Target};

use flux_runner::recover::{self as rc, Check, Search, Then};

use crate::{ApiError, Result, Service};

fn target(value: serde_json::Value) -> Result<Target> {
    serde_json::from_value(value).map_err(|e| ApiError::new("invalid", format!("target: {e}")))
}

/// The Flux backups on a drive or a server; an encrypted Vault's with its password.
pub fn find_backups_impl(
    state: &Service,
    target_v: serde_json::Value,
    password: Option<String>,
) -> Result<Search> {
    let t = target(target_v)?;
    let pw = password.filter(|p| !p.is_empty());
    rc::find(&state.paths, &t, pw.as_deref(), true).map_err(|e| ApiError::new("error", e))
}

#[derive(Serialize)]
pub struct Recovered {
    pub id: String,
    /// A message for people when something was left to do (the password not kept…).
    pub note: Option<String>,
}

/// Set up the backup `kept` (of `engine`) found at `target` on this computer.
pub fn recover_impl(
    state: &Service,
    target_v: serde_json::Value,
    engine: Engine,
    kept: KeptAs,
    then: Then,
    id: Option<String>,
    password: Option<String>,
) -> Result<Recovered> {
    let t = target(target_v)?;
    let pw = password.filter(|p| !p.is_empty());
    let search =
        rc::find(&state.paths, &t, pw.as_deref(), true).map_err(|e| ApiError::new("error", e))?;
    let found = search
        .found
        .iter()
        .find(|f| f.engine == engine && f.kept == kept)
        .ok_or_else(|| ApiError::new("error", "That backup isn't there any more."))?;
    let (pid, p) = rc::recover(
        &state.paths,
        found,
        &t,
        then,
        id.as_deref().filter(|s| !s.is_empty()),
    )
    .map_err(|e| ApiError::new("error", e))?;
    let mut note = None;
    if let Some(pw) = pw
        && p.password_source().is_some()
        && let Err(e) = crate::views::set_password_impl(state, pid.to_string(), pw)
    {
        note = Some(format!(
            "The backup was set up, but its password couldn't be kept: {e}. It's asked for when needed."
        ));
    }
    if then == Then::CarryOn
        && let Some(msg) = crate::edit::apply_timer(state, &pid, &p)
        && msg.starts_with("Saved, but")
    {
        note = Some(msg);
    }
    Ok(Recovered {
        id: pid.to_string(),
        note,
    })
}

/// Could this backup be recovered on another computer?
pub fn readiness_impl(state: &Service, id: String) -> Result<Vec<Check>> {
    let (pid, p) = state.load(&id)?;
    Ok(rc::readiness(&state.paths, &pid, &p))
}

/// Tick (or untick) something only you can tell: `password_elsewhere`, `key_exported`.
pub fn set_readiness_impl(
    state: &Service,
    id: String,
    key: String,
    yes: bool,
) -> Result<Vec<Check>> {
    let (pid, p) = state.load(&id)?;
    rc::set_answer(&state.paths, &pid, &key, yes).map_err(|e| ApiError::new("invalid", e))?;
    Ok(rc::readiness(&state.paths, &pid, &p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_core::profile::Profile;

    #[test]
    fn find_recover_and_readiness() {
        let t = tempfile::tempdir().unwrap();
        let (src, tgt) = (t.path().join("src"), t.path().join("tgt"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&tgt).unwrap();
        std::fs::write(src.join("a"), "a").unwrap();
        let old = flux_core::Paths::under(t.path().join("old"), flux_core::Scope::User);
        let id = flux_core::ProfileId::new("home").unwrap();
        let mut p = Profile::new(
            "Home",
            Target::Local {
                path: tgt.clone(),
                fs_uuid: None,
            },
            vec![src],
        );
        p.options.notify = false;
        p.schedule.trigger = flux_core::profile::Trigger::Manual;
        flux_core::store::ProfileStore::new(&old)
            .create(&id, &p)
            .unwrap();
        let r = flux_runner::run_backup(
            &flux_runner::BackupJob {
                paths: &old,
                id: &id,
                profile: &p,
                name: None,
                dry_run: false,
                trigger: flux_runner::Trigger::Manual,
                prune: true,
                only_if_due: false,
                ask: None,
                shutdown_after: false,
            },
            &flux_core::progress::Progress::new(),
            &flux_core::progress::silent,
        );
        assert_eq!(r.record.status, flux_runner::RunStatus::Completed);
        let old_state = Service::new(old.clone());
        let checks = readiness_impl(&old_state, "home".into()).unwrap();
        assert!(checks.iter().any(|c| c.key == "settings" && c.ok));
        assert!(
            set_readiness_impl(&old_state, "home".into(), "password_elsewhere".into(), true)
                .is_ok()
        );

        let new = Service::new(flux_core::Paths::under(
            t.path().join("new"),
            flux_core::Scope::User,
        ));
        let tv = serde_json::json!({ "kind": "local", "path": tgt });
        let s = find_backups_impl(&new, tv.clone(), None).unwrap();
        assert_eq!(s.found.len(), 1);
        let kept = s.found[0].kept.clone();
        let r = recover_impl(
            &new,
            tv.clone(),
            Engine::Mirror,
            kept.clone(),
            Then::Restore,
            None,
            None,
        )
        .unwrap();
        assert_eq!(r.id, "home");
        assert!(r.note.is_none());
        let (_, np) = new.load("home").unwrap();
        assert_eq!(np.name, "Home");
        // Gone: a clear message.
        let gone = KeptAs {
            profile: "other".into(),
            ..kept
        };
        assert!(recover_impl(&new, tv, Engine::Mirror, gone, Then::Restore, None, None).is_err());
    }
}
