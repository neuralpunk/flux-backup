//! Getting the password of a Vault (or the passphrase of a Borg repository) for a profile.
//!
//! Order: `FLUX_PASSWORD` (for scripts), then the profile's `vault.password_source` (or
//! `borg.password_source`):
//!
//! * `keyring`: the desktop keyring (Secret Service, through `secret-tool`), with
//!   attributes `service=flux-backup profile=<id>`. If it's missing and someone can be
//!   asked, Flux asks and stores the answer.
//! * `prompt`: ask every time
//! * `file`: first line of a file
//! * `command`: first line printed by a shell command
//!
//! "Asking" goes through a callback: the CLI prompts on the terminal, the GUI shows a
//! dialog, and scheduled runs can't ask at all.

use std::io::Write;
use std::process::{Command, Stdio};

use zeroize::Zeroizing;

use flux_core::ProfileId;
use flux_core::profile::{PasswordSource, Profile};

pub type Secret = Zeroizing<String>;

/// Ask the user: `(prompt, confirm)`. `confirm` means "new password: ask twice".
/// Returns `None` if the user cancelled or nobody can be asked.
pub type Ask<'a> = &'a (dyn Fn(&str, bool) -> Option<Secret> + Sync);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Open,
    Create,
}

#[derive(Debug, thiserror::Error)]
pub enum PasswordError {
    #[error("no password available for {0}: {1}")]
    Unavailable(String, String),
    #[error("password file {0}: {1}")]
    File(String, String),
    #[error("password command failed: {0}")]
    Command(String),
    #[error("keyring: {0}")]
    Keyring(String),
}

fn secret_tool() -> String {
    std::env::var("FLUX_SECRET_TOOL").unwrap_or_else(|_| "secret-tool".into())
}

/// The profile's password from the keyring, if stored.
pub fn keyring_lookup(id: &ProfileId) -> Result<Option<Secret>, PasswordError> {
    let out = Command::new(secret_tool())
        .args(["lookup", "service", "flux-backup", "profile", id.as_str()])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| PasswordError::Keyring(format!("cannot run secret-tool ({e}); install libsecret's secret-tool or use another password_source")))?;
    let s = Zeroizing::new(String::from_utf8_lossy(&out.stdout).into_owned());
    Ok((out.status.success() && !s.is_empty()).then_some(s))
}

/// Store the profile's password in the keyring (replacing any stored one).
pub fn keyring_store(
    id: &ProfileId,
    profile_name: &str,
    password: &str,
) -> Result<(), PasswordError> {
    let mut child = Command::new(secret_tool())
        .args([
            "store",
            &format!("--label=Flux Backup: {profile_name}"),
            "service",
            "flux-backup",
            "profile",
            id.as_str(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| PasswordError::Keyring(e.to_string()))?;
    child
        .stdin
        .take()
        .expect("piped")
        .write_all(password.as_bytes())
        .map_err(|e| PasswordError::Keyring(e.to_string()))?;
    let out = child
        .wait_with_output()
        .map_err(|e| PasswordError::Keyring(e.to_string()))?;
    if !out.status.success() {
        return Err(PasswordError::Keyring(
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        ));
    }
    Ok(())
}

pub fn keyring_clear(id: &ProfileId) -> Result<(), PasswordError> {
    let _ = Command::new(secret_tool())
        .args(["clear", "service", "flux-backup", "profile", id.as_str()])
        .status();
    Ok(())
}

fn first_line(s: &str) -> Secret {
    Zeroizing::new(s.lines().next().unwrap_or("").to_owned())
}

/// The password for a Vault profile, or the passphrase for a Borg profile.
pub fn obtain(
    id: &ProfileId,
    profile: &Profile,
    purpose: Purpose,
    ask: Option<Ask<'_>>,
) -> Result<Secret, PasswordError> {
    if let Ok(pw) = std::env::var("FLUX_PASSWORD")
        && !pw.is_empty()
    {
        return Ok(Zeroizing::new(pw));
    }
    let source = profile.password_source().unwrap_or_default();
    let what = if profile.engine == flux_core::Engine::Borg {
        "Borg passphrase"
    } else {
        "Vault password"
    };
    let prompt = match purpose {
        Purpose::Open => format!("{what} for “{}”: ", profile.name),
        Purpose::Create => format!("New {what} for “{}”: ", profile.name),
    };
    let unavailable = |why: &str| PasswordError::Unavailable(id.to_string(), why.to_owned());
    match source {
        PasswordSource::Keyring => {
            if purpose == Purpose::Open
                && let Some(pw) = keyring_lookup(id)?
            {
                return Ok(pw);
            }
            let ask = ask.ok_or_else(|| {
                unavailable(
                    "it isn't in the keyring and nobody can be asked (run `flux key store` once)",
                )
            })?;
            let pw =
                ask(&prompt, purpose == Purpose::Create).ok_or_else(|| unavailable("cancelled"))?;
            keyring_store(id, &profile.name, &pw)?;
            Ok(pw)
        }
        PasswordSource::Prompt => {
            let ask = ask.ok_or_else(|| {
                unavailable("password_source is \"prompt\" and nobody can be asked")
            })?;
            ask(&prompt, purpose == Purpose::Create).ok_or_else(|| unavailable("cancelled"))
        }
        PasswordSource::File { path } => {
            let text = Zeroizing::new(
                std::fs::read_to_string(&path)
                    .map_err(|e| PasswordError::File(path.display().to_string(), e.to_string()))?,
            );
            use std::os::unix::fs::PermissionsExt;
            if let Ok(m) = std::fs::metadata(&path)
                && m.permissions().mode() & 0o077 != 0
            {
                tracing::warn!(
                    "password file {} is readable by others; chmod 600 it",
                    path.display()
                );
            }
            Ok(first_line(&text))
        }
        PasswordSource::Command { command } => {
            let out = Command::new("sh")
                .arg("-c")
                .arg(&command)
                .stdin(Stdio::null())
                .stderr(Stdio::inherit())
                .output()
                .map_err(|e| PasswordError::Command(e.to_string()))?;
            if !out.status.success() {
                return Err(PasswordError::Command(format!(
                    "`{command}` exited with {}",
                    out.status
                )));
            }
            Ok(first_line(&Zeroizing::new(
                String::from_utf8_lossy(&out.stdout).into_owned(),
            )))
        }
    }
}

/// Ask on the terminal (no echo). `None` when there is no terminal.
pub fn terminal_ask(prompt: &str, confirm: bool) -> Option<Secret> {
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return None;
    }
    loop {
        let pw = Zeroizing::new(rpassword::prompt_password(prompt).ok()?);
        if pw.is_empty() {
            eprintln!("The password must not be empty.");
            continue;
        }
        if !confirm {
            return Some(pw);
        }
        let again = Zeroizing::new(rpassword::prompt_password("Repeat it: ").ok()?);
        if *again == *pw {
            return Some(pw);
        }
        eprintln!("The passwords don't match; try again.");
    }
}
