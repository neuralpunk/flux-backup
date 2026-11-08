//! Finding the standard tools and their versions.

use std::process::{Command, Stdio};

use serde::Serialize;

use crate::CompatError;

/// The rsync program. `FLUX_RSYNC` overrides it.
pub fn rsync_program() -> String {
    std::env::var("FLUX_RSYNC").unwrap_or_else(|_| "rsync".into())
}

/// The borg program. `FLUX_BORG` overrides it.
pub fn borg_program() -> String {
    std::env::var("FLUX_BORG").unwrap_or_else(|_| "borg".into())
}

/// Borg 1.2 added `compact` and `recreate --comment`; Borg 2 changed the command line.
const BORG_MIN: (u32, u32) = (1, 2);

pub fn borg_status() -> ToolStatus {
    let program = borg_program();
    // "borg 1.4.5"
    let version = first_line_version(&program, "--version", 1);
    let mm = version.as_deref().and_then(major_minor);
    let supported = mm.is_some_and(|v| v >= BORG_MIN && v.0 == 1);
    let hint = match (&version, mm) {
        (None, _) => Some(
            "Borg is not installed. Install the \u{201c}borgbackup\u{201d} package \
             (for example `sudo dnf install borgbackup` or `sudo apt install borgbackup`)."
                .into(),
        ),
        (Some(v), Some((major, _))) if major >= 2 => Some(format!(
            "Borg {v} is installed; Flux works with Borg 1.2 to 1.4 (Borg 2 changed its \
             commands and repository format)."
        )),
        (Some(v), _) if !supported => Some(format!(
            "Borg {v} is too old; Flux needs Borg {}.{} or newer.",
            BORG_MIN.0, BORG_MIN.1
        )),
        _ => None,
    };
    ToolStatus {
        tool: "borg",
        program,
        version,
        supported,
        hint,
    }
}

/// The installed borg's version, or why it can't be used.
pub fn require_borg() -> Result<String, CompatError> {
    let s = borg_status();
    match (s.supported, s.version) {
        (true, Some(v)) => Ok(v),
        _ => Err(CompatError::Tool(s.hint.unwrap_or_default())),
    }
}

/// Whether a tool is installed, and which version.
#[derive(Debug, Clone, Serialize)]
pub struct ToolStatus {
    pub tool: &'static str,
    pub program: String,
    /// `None` when the program can't be run.
    pub version: Option<String>,
    /// The version is new enough for Flux.
    pub supported: bool,
    /// What to do about it, when not supported.
    pub hint: Option<String>,
}

/// rsync 3.1 added `--info=progress2`.
const RSYNC_MIN: (u32, u32) = (3, 1);

fn first_line_version(program: &str, arg: &str, word: usize) -> Option<String> {
    let out = Command::new(program)
        .arg(arg)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .next()?
        .split_whitespace()
        .nth(word)
        .map(str::to_owned)
}

fn major_minor(v: &str) -> Option<(u32, u32)> {
    let mut it = v.split(|c: char| !c.is_ascii_digit());
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

pub fn rsync_status() -> ToolStatus {
    let program = rsync_program();
    // "rsync  version 3.4.1  protocol version 32"
    let version = first_line_version(&program, "--version", 2);
    let supported = version
        .as_deref()
        .and_then(major_minor)
        .is_some_and(|v| v >= RSYNC_MIN);
    let hint = match (&version, supported) {
        (None, _) => Some(
            "rsync is not installed. Install it with your package manager \
             (for example `sudo dnf install rsync` or `sudo apt install rsync`)."
                .into(),
        ),
        (Some(v), false) => Some(format!(
            "rsync {v} is too old; Flux needs rsync {}.{} or newer.",
            RSYNC_MIN.0, RSYNC_MIN.1
        )),
        _ => None,
    };
    ToolStatus {
        tool: "rsync",
        program,
        version,
        supported,
        hint,
    }
}

/// The installed rsync's version, or why it can't be used.
pub fn require_rsync() -> Result<String, CompatError> {
    let s = rsync_status();
    match (s.supported, s.version) {
        (true, Some(v)) => Ok(v),
        _ => Err(CompatError::Tool(s.hint.unwrap_or_default())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(major_minor("3.4.1"), Some((3, 4)));
        assert_eq!(major_minor("3.5.2-ga8306e06"), Some((3, 5)));
        assert_eq!(major_minor("x"), None);
    }
}
