//! The typed protocol between the desktop app and `flux-helper`, and the dispatcher that
//! runs a [`Call`] against a [`Service`].
//!
//! JSON lines over the helper's stdin/stdout:
//!
//! * app → helper: [`Request`] `{"id": 7, "call": {"method": "overview", "params": {"id": "etc"}}}`
//! * helper → app: [`Output`]: one `ready` line first, then `reply` lines (matched by id,
//!   in any order) and `notice` lines (job progress, restore progress, changes)
//!
//! Only the calls listed in [`Call`] exist; unknown methods or fields are rejected, and
//! lines are limited to [`MAX_LINE`] bytes.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::watch::Notice;
use crate::{ApiError, Result, Service, actions, edit, views};

/// Bumped on incompatible changes; the app refuses a helper that speaks another version.
pub const PROTOCOL_VERSION: u32 = 1;
/// Longest accepted line (a profile, a list of restore paths…).
pub const MAX_LINE: usize = 4 << 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: u64,
    pub call: Call,
}

/// Everything the app can ask for. Each maps to one function of this crate.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "method",
    content = "params",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Call {
    Profiles,
    Overview {
        id: String,
    },
    Snapshots {
        id: String,
    },
    ListDir {
        id: String,
        snapshot: String,
        path: String,
    },
    Unlock {
        id: String,
        password: String,
        #[serde(default)]
        create: bool,
    },
    SaveKeyFile {
        id: String,
    },
    BackupNow {
        id: String,
    },
    Cancel {
        id: String,
    },
    Restore {
        id: String,
        snapshot: String,
        paths: Vec<String>,
        to: Option<String>,
        overwrite: bool,
    },
    GetProfile {
        id: String,
    },
    NewProfile,
    ValidateProfile {
        profile: Value,
    },
    SaveProfile {
        id: String,
        profile: Value,
        create: bool,
    },
    DeleteProfile {
        id: String,
    },
    SuggestId {
        name: String,
    },
    RetentionPreview {
        id: String,
        retention: Value,
    },
    ExcludeSuggestions {
        sources: Vec<String>,
    },
    Drives,
    TestTarget {
        profile: Value,
    },
    UpdateSnapshot {
        id: String,
        snapshot: String,
        name: Option<String>,
        set_name: bool,
        pinned: Option<bool>,
    },
    DeleteSnapshot {
        id: String,
        snapshot: String,
    },
    Compare {
        id: String,
        old: String,
        new: Option<String>,
    },
    Runs {
        id: String,
        limit: usize,
    },
    RunEvents {
        id: String,
        run_id: String,
    },
    Prune {
        id: String,
        dry_run: bool,
    },
    Check {
        id: String,
        read_data: bool,
    },
}

impl Call {
    /// The profile this call is about, if any.
    pub fn profile_id_mut(&mut self) -> Option<&mut String> {
        use Call::*;
        match self {
            Overview { id }
            | Snapshots { id }
            | ListDir { id, .. }
            | Unlock { id, .. }
            | SaveKeyFile { id }
            | BackupNow { id }
            | Cancel { id }
            | Restore { id, .. }
            | GetProfile { id }
            | SaveProfile { id, .. }
            | DeleteProfile { id }
            | RetentionPreview { id, .. }
            | UpdateSnapshot { id, .. }
            | DeleteSnapshot { id, .. }
            | Compare { id, .. }
            | Runs { id, .. }
            | RunEvents { id, .. }
            | Prune { id, .. }
            | Check { id, .. } => Some(id),
            Profiles
            | NewProfile
            | ValidateProfile { .. }
            | SuggestId { .. }
            | ExcludeSuggestions { .. }
            | Drives
            | TestTarget { .. } => None,
        }
    }

    pub fn method(&self) -> &'static str {
        use Call::*;
        match self {
            Profiles => "profiles",
            Overview { .. } => "overview",
            Snapshots { .. } => "snapshots",
            ListDir { .. } => "list_dir",
            Unlock { .. } => "unlock",
            SaveKeyFile { .. } => "save_key_file",
            BackupNow { .. } => "backup_now",
            Cancel { .. } => "cancel",
            Restore { .. } => "restore",
            GetProfile { .. } => "get_profile",
            NewProfile => "new_profile",
            ValidateProfile { .. } => "validate_profile",
            SaveProfile { .. } => "save_profile",
            DeleteProfile { .. } => "delete_profile",
            SuggestId { .. } => "suggest_id",
            RetentionPreview { .. } => "retention_preview",
            ExcludeSuggestions { .. } => "exclude_suggestions",
            Drives => "drives",
            TestTarget { .. } => "test_target",
            UpdateSnapshot { .. } => "update_snapshot",
            DeleteSnapshot { .. } => "delete_snapshot",
            Compare { .. } => "compare",
            Runs { .. } => "runs",
            RunEvents { .. } => "run_events",
            Prune { .. } => "prune",
            Check { .. } => "check",
        }
    }
}

/// What the helper writes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Output {
    /// First line: the helper is running (and authorized).
    Ready {
        version: String,
        protocol: u32,
        uid: u32,
    },
    Reply {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ok: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        err: Option<ApiError>,
    },
    Notice {
        notice: Notice,
    },
}

fn json<T: Serialize>(v: T) -> Result<Value> {
    Ok(serde_json::to_value(v)?)
}

/// Run `call` against `service`. `notify` receives restore progress.
pub fn dispatch(service: &Service, call: Call, notify: &(dyn Fn(Notice) + Sync)) -> Result<Value> {
    let s = service;
    match call {
        Call::Profiles => json(views::profiles_impl(s)?),
        Call::Overview { id } => json(views::overview_impl(s, id)?),
        Call::Snapshots { id } => json(views::snapshots_impl(s, id)?),
        Call::ListDir { id, snapshot, path } => json(views::list_dir_impl(s, id, snapshot, path)?),
        Call::Unlock {
            id,
            password,
            create,
        } => json(views::unlock_impl(s, id, password, create)?),
        Call::SaveKeyFile { id } => json(edit::save_key_file_impl(s, id)?),
        Call::BackupNow { id } => json(views::backup_now_impl(s, id)?),
        Call::Cancel { id } => json(views::cancel_impl(s, id)?),
        Call::Restore {
            id,
            snapshot,
            paths,
            to,
            overwrite,
        } => {
            let profile = id.clone();
            let progress = move |progress| {
                notify(Notice::Restore {
                    profile: profile.clone(),
                    progress,
                })
            };
            json(actions::restore_impl(
                s, id, snapshot, paths, to, overwrite, &progress,
            )?)
        }
        Call::GetProfile { id } => json(edit::get_profile_impl(s, id)?),
        Call::NewProfile => json(edit::new_profile_impl(s)?),
        Call::ValidateProfile { profile } => json(edit::validate_profile_impl(s, profile)?),
        Call::SaveProfile {
            id,
            profile,
            create,
        } => json(edit::save_profile_impl(s, id, profile, create)?),
        Call::DeleteProfile { id } => json(edit::delete_profile_impl(s, id)?),
        Call::SuggestId { name } => json(edit::suggest_id_impl(s, name)?),
        Call::RetentionPreview { id, retention } => {
            json(edit::retention_preview_impl(s, id, retention)?)
        }
        Call::ExcludeSuggestions { sources } => json(edit::exclude_suggestions_impl(s, sources)?),
        Call::Drives => json(edit::drives_impl(s)?),
        Call::TestTarget { profile } => json(edit::test_target_impl(s, profile)?),
        Call::UpdateSnapshot {
            id,
            snapshot,
            name,
            set_name,
            pinned,
        } => json(actions::update_snapshot_impl(
            s, id, snapshot, name, set_name, pinned,
        )?),
        Call::DeleteSnapshot { id, snapshot } => {
            json(actions::delete_snapshot_impl(s, id, snapshot)?)
        }
        Call::Compare { id, old, new } => json(actions::compare_impl(s, id, old, new)?),
        Call::Runs { id, limit } => json(actions::runs_impl(s, id, limit)?),
        Call::RunEvents { id, run_id } => json(actions::run_events_impl(s, id, run_id)?),
        Call::Prune { id, dry_run } => json(actions::prune_impl(s, id, dry_run)?),
        Call::Check { id, read_data } => json(actions::check_impl(s, id, read_data)?),
    }
}

/// Parse one request line, refusing anything not in the protocol.
pub fn parse_request(line: &str) -> std::result::Result<Request, (Option<u64>, ApiError)> {
    if line.len() > MAX_LINE {
        return Err((None, ApiError::new("bad_request", "request too long")));
    }
    serde_json::from_str::<Request>(line).map_err(|e| {
        // Answer the id if it can be read, so the caller isn't left waiting.
        let id = serde_json::from_str::<Value>(line)
            .ok()
            .and_then(|v| v.get("id")?.as_u64());
        (id, ApiError::new("bad_request", e.to_string()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip_and_unknowns_are_refused() {
        let r = parse_request(
            r#"{"id":3,"call":{"method":"list_dir","params":{"id":"etc","snapshot":"s","path":"/"}}}"#,
        )
        .unwrap();
        assert_eq!(r.id, 3);
        assert_eq!(r.call.method(), "list_dir");
        let r = parse_request(r#"{"id":4,"call":{"method":"profiles"}}"#).unwrap();
        assert!(matches!(r.call, Call::Profiles));

        for bad in [
            r#"{"id":5,"call":{"method":"run_shell","params":{"cmd":"id"}}}"#,
            r#"{"id":5,"call":{"method":"overview","params":{"id":"x","extra":1}}}"#,
            r#"{"id":5,"call":{"method":"overview","params":{}}}"#,
            r#"{"id":5,"call":{"method":"profiles"},"sudo":true}"#,
        ] {
            let (id, e) = parse_request(bad).unwrap_err();
            assert_eq!(id, Some(5), "{bad}");
            assert_eq!(e.code, "bad_request");
        }
        let long = format!(r#"{{"id":1,"pad":"{}"}}"#, "x".repeat(MAX_LINE));
        assert_eq!(parse_request(&long).unwrap_err().1.code, "bad_request");
    }

    #[test]
    fn profile_ids_are_found() {
        let mut c = Call::Restore {
            id: "etc".into(),
            snapshot: "s".into(),
            paths: vec![],
            to: None,
            overwrite: false,
        };
        *c.profile_id_mut().unwrap() = "x".into();
        assert!(matches!(c, Call::Restore { ref id, .. } if id == "x"));
        assert!(Call::Drives.profile_id_mut().is_none());
    }
}
