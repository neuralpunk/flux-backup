//! Webhooks: telling something off this computer how backups go (a phone through ntfy, a
//! chat, a monitoring service that raises an alarm when pings stop).
//!
//! Each webhook has a URL, the events it wants, extra headers, and a *preset* that shapes
//! the request: plain JSON (documented below), ntfy, Gotify, Slack- and Discord-style
//! messages, or healthchecks.io-style pings.
//!
//! The JSON body:
//! ```json
//! { "event": "failed", "profile": "home", "name": "Home folder", "host": "laptop",
//!   "message": "Backup of “Home folder” failed: …", "time": "2026-11-09T10:00:00Z",
//!   "status": "failed", "snapshot": null, "error": "…" }
//! ```

use serde::{Deserialize, Serialize};

/// A webhook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Webhook {
    pub url: String,
    #[serde(default)]
    pub preset: Preset,
    /// The events sent; empty: failures, warnings and unusual changes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<HookEvent>,
    /// Extra headers (`Authorization: Bearer …`). Kept in this file only: left out of
    /// exports and diagnostics.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<String>,
    /// Off without deleting it.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

/// What shapes the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    /// POST the JSON body.
    #[default]
    Json,
    /// POST the message as text, with ntfy's Title, Priority and Tags headers.
    Ntfy,
    /// POST `{title, message, priority}` (the token is in the URL).
    Gotify,
    /// POST `{"text": …}`.
    Slack,
    /// POST `{"content": …}`.
    Discord,
    /// Pings: `<url>/start` when a backup starts, `<url>` when it succeeds, `<url>/fail`
    /// (with the message) when it fails.
    Healthchecks,
}

/// Something worth telling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    Started,
    Succeeded,
    /// Finished, but with problems (files left out, a source's drive missing, a place
    /// failing).
    Warning,
    Failed,
    /// A scheduled backup didn't run (paused, on battery, a refused network…).
    Skipped,
    RestoreTestFailed,
    UnusualChanges,
    LowSpace,
    /// "Send a test".
    Test,
}

impl HookEvent {
    pub fn label(self) -> &'static str {
        match self {
            HookEvent::Started => "started",
            HookEvent::Succeeded => "succeeded",
            HookEvent::Warning => "warning",
            HookEvent::Failed => "failed",
            HookEvent::Skipped => "skipped",
            HookEvent::RestoreTestFailed => "restore_test_failed",
            HookEvent::UnusualChanges => "unusual_changes",
            HookEvent::LowSpace => "low_space",
            HookEvent::Test => "test",
        }
    }

    fn bad(self) -> bool {
        matches!(
            self,
            HookEvent::Failed | HookEvent::RestoreTestFailed | HookEvent::UnusualChanges
        )
    }
}

impl Webhook {
    /// Does this webhook want `e`? (A test always goes.)
    pub fn wants(&self, e: HookEvent) -> bool {
        if !self.enabled {
            return false;
        }
        if e == HookEvent::Test {
            return true;
        }
        // healthchecks.io-style pings need the start, success and failure to time a run.
        if self.preset == Preset::Healthchecks
            && matches!(
                e,
                HookEvent::Started | HookEvent::Succeeded | HookEvent::Failed
            )
        {
            return true;
        }
        if self.events.is_empty() {
            return matches!(
                e,
                HookEvent::Failed
                    | HookEvent::Warning
                    | HookEvent::UnusualChanges
                    | HookEvent::RestoreTestFailed
            );
        }
        self.events.contains(&e)
    }

    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !(self.url.starts_with("http://") || self.url.starts_with("https://")) {
            out.push(format!(
                "{:?} isn't a web address (http:// or https://)",
                self.url
            ));
        }
        for h in &self.headers {
            if !h.contains(':') {
                out.push(format!("header {h:?}: write it as Name: value"));
            }
        }
        out
    }
}

/// What a webhook says about one event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Message {
    pub event: HookEvent,
    pub profile: String,
    pub name: String,
    pub host: String,
    pub message: String,
    pub time: jiff::Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// An HTTP request to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<String>,
    /// `None`: no body.
    pub body: Option<Vec<u8>>,
}

/// The request for `m` to webhook `w`.
pub fn request(w: &Webhook, m: &Message) -> Request {
    let mut headers = w.headers.clone();
    let json = |v: serde_json::Value| Some(serde_json::to_vec(&v).unwrap_or_default());
    let title = format!("Flux Backup: {}", m.name);
    let (method, url, body) = match w.preset {
        Preset::Json => {
            headers.push("Content-Type: application/json".into());
            (
                "POST",
                w.url.clone(),
                Some(serde_json::to_vec(m).unwrap_or_default()),
            )
        }
        Preset::Ntfy => {
            headers.push(format!("Title: {title}"));
            headers.push(format!(
                "Priority: {}",
                if m.event.bad() { "high" } else { "default" }
            ));
            headers.push(format!(
                "Tags: {}",
                if m.event.bad() {
                    "warning"
                } else {
                    "floppy_disk"
                }
            ));
            ("POST", w.url.clone(), Some(m.message.clone().into_bytes()))
        }
        Preset::Gotify => {
            headers.push("Content-Type: application/json".into());
            (
                "POST",
                w.url.clone(),
                json(serde_json::json!({
                    "title": title,
                    "message": m.message,
                    "priority": if m.event.bad() { 8 } else { 4 },
                })),
            )
        }
        Preset::Slack => {
            headers.push("Content-Type: application/json".into());
            (
                "POST",
                w.url.clone(),
                json(serde_json::json!({ "text": m.message })),
            )
        }
        Preset::Discord => {
            headers.push("Content-Type: application/json".into());
            (
                "POST",
                w.url.clone(),
                json(serde_json::json!({ "content": m.message })),
            )
        }
        Preset::Healthchecks => {
            let base = w.url.trim_end_matches('/');
            let url = match m.event {
                HookEvent::Started => format!("{base}/start"),
                e if e.bad() || e == HookEvent::Warning => format!("{base}/fail"),
                _ => base.to_owned(),
            };
            ("POST", url, Some(m.message.clone().into_bytes()))
        }
    };
    Request {
        method,
        url,
        headers,
        body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(event: HookEvent) -> Message {
        Message {
            event,
            profile: "home".into(),
            name: "Home folder".into(),
            host: "laptop".into(),
            message: "Backup of “Home folder” failed: drive not connected".into(),
            time: jiff::Timestamp::from_second(1_800_000_000).unwrap(),
            status: Some("failed".into()),
            snapshot: None,
            error: Some("drive not connected".into()),
        }
    }

    fn hook(preset: Preset) -> Webhook {
        Webhook {
            url: "https://example.org/hook".into(),
            preset,
            events: Vec::new(),
            headers: vec!["Authorization: Bearer x".into()],
            enabled: true,
        }
    }

    #[test]
    fn presets_shape_the_request() {
        let r = request(&hook(Preset::Json), &msg(HookEvent::Failed));
        let v: serde_json::Value = serde_json::from_slice(r.body.as_ref().unwrap()).unwrap();
        assert_eq!(
            (v["event"].as_str(), v["profile"].as_str()),
            (Some("failed"), Some("home"))
        );
        assert!(r.headers.contains(&"Authorization: Bearer x".into()));
        let r = request(&hook(Preset::Ntfy), &msg(HookEvent::Failed));
        assert!(r.headers.contains(&"Priority: high".into()));
        assert_eq!(r.body.unwrap(), msg(HookEvent::Failed).message.into_bytes());
        let r = request(&hook(Preset::Discord), &msg(HookEvent::Failed));
        assert!(
            String::from_utf8(r.body.unwrap())
                .unwrap()
                .starts_with("{\"content\":")
        );
        let hc = hook(Preset::Healthchecks);
        assert_eq!(
            request(&hc, &msg(HookEvent::Started)).url,
            "https://example.org/hook/start"
        );
        assert_eq!(
            request(&hc, &msg(HookEvent::Succeeded)).url,
            "https://example.org/hook"
        );
        assert_eq!(
            request(&hc, &msg(HookEvent::Failed)).url,
            "https://example.org/hook/fail"
        );
    }

    #[test]
    fn which_events_go() {
        let h = hook(Preset::Json);
        assert!(h.wants(HookEvent::Failed) && h.wants(HookEvent::UnusualChanges));
        assert!(!h.wants(HookEvent::Succeeded) && !h.wants(HookEvent::Started));
        let all = Webhook {
            events: vec![HookEvent::Succeeded],
            ..h.clone()
        };
        assert!(all.wants(HookEvent::Succeeded) && !all.wants(HookEvent::Failed));
        let hc = Webhook {
            events: vec![HookEvent::LowSpace],
            ..hook(Preset::Healthchecks)
        };
        assert!(hc.wants(HookEvent::Started), "pings always time the run");
        assert!(
            hook(Preset::Healthchecks).wants(HookEvent::Succeeded),
            "even with no events chosen"
        );
        let off = Webhook {
            enabled: false,
            ..h
        };
        assert!(!off.wants(HookEvent::Failed) && !off.wants(HookEvent::Test));
        assert_eq!(hook(Preset::Json).problems(), Vec::<String>::new());
        let bad = Webhook {
            url: "example.org".into(),
            headers: vec!["no colon".into()],
            ..hook(Preset::Json)
        };
        assert_eq!(bad.problems().len(), 2);
    }
}
