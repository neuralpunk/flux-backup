//! Start at login: an XDG autostart entry that opens Flux in the tray (`--hidden`).
//! Scheduled backups never need the app running; this is only for the tray icon.

use std::path::PathBuf;

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

/// How to start this app again: the AppImage itself when running from one.
fn exec() -> std::io::Result<String> {
    let exe = match std::env::var_os("APPIMAGE") {
        Some(a) => PathBuf::from(a),
        None => std::env::current_exe()?,
    };
    // Desktop entry quoting: wrap in double quotes, escape what's special inside.
    let s = exe.to_string_lossy();
    let quoted: String = s
        .chars()
        .flat_map(|c| match c {
            '"' | '`' | '$' | '\\' => vec!['\\', c],
            c => vec![c],
        })
        .collect();
    Ok(format!("\"{quoted}\" --hidden"))
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
    let text = format!(
        "[Desktop Entry]\nType=Application\nName=Flux Backup\nComment=Shows your backups in the system tray\nExec={}\nIcon=flux-backup\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        exec()?
    );
    std::fs::write(&path, text)
}
