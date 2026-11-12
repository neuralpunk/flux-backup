//! Start at login: an XDG autostart entry that opens Flux in the tray (`--hidden`).
//! Scheduled backups never need the app running; this is only for the tray icon.

use std::path::{Path, PathBuf};

const FILE: &str = "io.github.neuralpunk.FluxBackup.desktop";

fn entry_path() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(config.join("autostart").join(FILE))
}

pub fn enabled() -> bool {
    entry_path().is_some_and(|p| p.exists())
}

/// How to start this app again: the AppImage itself when running from one. A debug
/// build names the installed `flux-backup` instead: it loads its pages from the dev
/// server, which isn't running at login, so starting it then shows "Connection refused".
fn exec() -> std::io::Result<String> {
    if cfg!(debug_assertions) {
        return Ok("flux-backup --hidden".to_owned());
    }
    let exe = match std::env::var_os("APPIMAGE") {
        Some(a) => PathBuf::from(a),
        None => std::env::current_exe()?,
    };
    Ok(exec_line(&exe))
}

/// `"<exe>" --hidden`, quoted the way desktop entries want.
fn exec_line(exe: &Path) -> String {
    let s = exe.to_string_lossy();
    let quoted: String = s
        .chars()
        .flat_map(|c| match c {
            '"' | '`' | '$' | '\\' => vec!['\\', c],
            c => vec![c],
        })
        .collect();
    format!("\"{quoted}\" --hidden")
}

fn entry(exec: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Flux Backup\nComment=Shows your backups in the system tray\nExec={exec}\nIcon=flux-backup\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
    )
}

pub fn set(on: bool) -> std::io::Result<()> {
    let Some(path) = entry_path() else {
        return Err(std::io::Error::other("no home folder"));
    };
    if !on {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    std::fs::create_dir_all(path.parent().expect("has a parent"))?;
    std::fs::write(&path, entry(&exec()?))
}

/// At startup: point an existing entry at this copy of the app, so one written by a
/// copy that has since moved (an AppImage), or by a dev build, still starts at login.
/// Debug builds leave it be, so running `tauri dev` never rewrites a working entry.
pub fn refresh() {
    if cfg!(debug_assertions) {
        return;
    }
    let Some(path) = entry_path() else { return };
    let Ok(old) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(new) = exec().map(|e| entry(&e)) else {
        return;
    };
    if old != new
        && let Err(e) = std::fs::write(&path, new)
    {
        tracing::warn!("couldn't update the start-at-login entry: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_line_quotes_specials() {
        assert_eq!(
            exec_line(Path::new("/opt/my \"flux\"/$x`\\.AppImage")),
            r#""/opt/my \"flux\"/\$x\`\\.AppImage" --hidden"#
        );
    }

    #[test]
    fn debug_builds_name_the_installed_app() {
        // Tests build with debug assertions: the entry must not name this binary.
        assert_eq!(exec().unwrap(), "flux-backup --hidden");
        assert!(entry(&exec().unwrap()).contains("\nExec=flux-backup --hidden\n"));
    }
}
