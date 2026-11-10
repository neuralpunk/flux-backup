//! Sending webhook messages (see `flux_core::webhook`), with `curl` (`FLUX_CURL` replaces
//! it in tests). Best effort: a webhook that fails never fails a backup; the run's log
//! says so.

use std::io::Write;
use std::process::{Command, Stdio};

use flux_core::webhook::{HookEvent, Message, Webhook, request};
use flux_core::{Paths, Profile, ProfileId};

use crate::prefs::Settings;

/// How long one webhook may take.
const TIMEOUT_SECS: u32 = 15;

/// Send `m` to `w`. The HTTP status, or why it couldn't be sent.
pub fn deliver(w: &Webhook, m: &Message) -> Result<u16, String> {
    let r = request(w, m);
    let curl = std::env::var_os("FLUX_CURL").unwrap_or_else(|| "curl".into());
    let mut cmd = Command::new(curl);
    cmd.args(["-sS", "-o", "/dev/null", "-w", "%{http_code}", "--max-time"])
        .arg(TIMEOUT_SECS.to_string())
        .args(["-X", r.method]);
    for h in &r.headers {
        cmd.arg("-H").arg(h);
    }
    if r.body.is_some() {
        cmd.args(["--data-binary", "@-"]);
    }
    cmd.arg(&r.url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("can't run curl: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(r.body.as_deref().unwrap_or_default());
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let code: u16 = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0);
    if !out.status.success() || code == 0 {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        return Err(if err.is_empty() {
            "no answer".into()
        } else {
            err
        });
    }
    if !(200..300).contains(&code) {
        return Err(format!("the server answered {code}"));
    }
    Ok(code)
}

/// Every webhook that hears about this profile: Preferences' and its own.
pub fn hooks_for(paths: &Paths, profile: &Profile) -> Vec<Webhook> {
    let mut v = Settings::load(paths).webhooks;
    v.extend(profile.webhooks.iter().cloned());
    v
}

/// Tell every interested webhook about `event`. Returns, for each one that failed, its
/// address and why (for the run's log).
pub fn send(
    paths: &Paths,
    id: &ProfileId,
    profile: &Profile,
    event: HookEvent,
    message: String,
    extra: (Option<String>, Option<String>, Option<String>),
) -> Vec<(String, String)> {
    let hooks: Vec<Webhook> = hooks_for(paths, profile)
        .into_iter()
        .filter(|w| w.wants(event))
        .collect();
    if hooks.is_empty() {
        return Vec::new();
    }
    let (status, snapshot, error) = extra;
    let m = Message {
        event,
        profile: id.to_string(),
        name: profile.name.clone(),
        host: flux_core::sys::hostname(),
        message,
        time: jiff::Timestamp::now(),
        status,
        snapshot,
        error,
    };
    hooks
        .iter()
        .filter_map(|w| deliver(w, &m).err().map(|e| (w.url.clone(), e)))
        .collect()
}
