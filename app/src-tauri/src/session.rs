//! The app's place on the desktop session bus: one copy running at a time, and whether a
//! system tray is there to show its icon.
//!
//! The first copy owns `io.github.neuralpunk.FluxBackup` on the session bus. Started again
//! (from the app menu, say, while its window is hidden in the tray), Flux asks that copy to
//! show its window and exits. Without a session bus, every copy runs on its own, as before.

use futures_util::StreamExt;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::message::Type;

const NAME: &str = "io.github.neuralpunk.FluxBackup";
const PATH: &str = "/io/github/neuralpunk/FluxBackup";
const INTERFACE: &str = "io.github.neuralpunk.FluxBackup.App";

/// The bus to use instead of the desktop's (tests: a private one).
#[cfg(test)]
static TEST_BUS: std::sync::OnceLock<String> = std::sync::OnceLock::new();

async fn session_bus() -> zbus::Result<zbus::Connection> {
    #[cfg(test)]
    if let Some(address) = TEST_BUS.get() {
        return zbus::conn::Builder::address(address.as_str()).build().await;
    }
    zbus::Connection::session().await
}

/// How this copy of the app starts.
pub enum Start {
    /// The only one: serve [`serve`] on this connection (`None`: no session bus).
    First(Option<zbus::Connection>),
    /// Another copy runs, and was asked to show its window.
    Another,
}

/// Become the running copy, or hand over to the one that is.
pub fn claim(show_other: bool) -> Start {
    zbus::block_on(async {
        let Ok(conn) = session_bus().await else {
            return Start::First(None);
        };
        match conn
            .request_name_with_flags(NAME, RequestNameFlags::DO_NOT_QUEUE)
            .await
        {
            Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {
                Start::First(Some(conn))
            }
            Ok(_) | Err(zbus::Error::NameTaken) => {
                if show_other
                    && let Err(e) = conn
                        .call_method(Some(NAME), PATH, Some(INTERFACE), "Activate", &())
                        .await
                {
                    // It's there but doesn't answer: run anyway rather than nothing at all.
                    tracing::warn!("the running Flux Backup didn't answer: {e}");
                    return Start::First(None);
                }
                Start::Another
            }
            Err(e) => {
                tracing::info!("not single-instance: {e}");
                Start::First(Some(conn))
            }
        }
    })
}

/// Answer `Activate` from later copies with `show` (on a thread of its own).
pub fn serve(conn: zbus::Connection, show: impl Fn() + Send + 'static) {
    std::thread::spawn(move || {
        zbus::block_on(async move {
            let mut incoming = zbus::MessageStream::from(&conn);
            while let Some(Ok(msg)) = incoming.next().await {
                let header = msg.header();
                if header.message_type() != Type::MethodCall
                    || header.path().map(|p| p.as_str()) != Some(PATH)
                {
                    continue;
                }
                if header.member().map(|m| m.as_str()) == Some("Activate") {
                    show();
                    let _ = conn.reply(&header, &()).await;
                } else {
                    let _ = conn
                        .reply_error(
                            &header,
                            "org.freedesktop.DBus.Error.UnknownMethod",
                            &"Flux Backup only knows Activate",
                        )
                        .await;
                }
            }
        });
    });
}

/// Is a system tray there to show the app's icon? A StatusNotifier host (KDE, Xfce,
/// Cinnamon, GNOME with the AppIndicator extension…). Without one the icon isn't shown, so
/// closing the window must quit rather than leave the app where nobody can reach it.
pub fn tray_host() -> bool {
    zbus::block_on(async {
        let Ok(conn) = session_bus().await else {
            return false;
        };
        let reply = conn
            .call_method(
                Some("org.kde.StatusNotifierWatcher"),
                "/StatusNotifierWatcher",
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &(
                    "org.kde.StatusNotifierWatcher",
                    "IsStatusNotifierHostRegistered",
                ),
            )
            .await;
        match reply {
            Ok(m) => m
                .body()
                .deserialize::<zbus::OwnedVariant>()
                .ok()
                .and_then(|v| bool::try_from(v).ok())
                .unwrap_or(false),
            Err(_) => false,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A private session bus for the test (never the desktop's own), stopped when dropped.
    struct Bus(std::process::Child);
    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn one_copy_at_a_time_and_the_tray_host() {
        let Ok(mut child) = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address"])
            .stdout(Stdio::piped())
            .spawn()
        else {
            eprintln!("skipped: no dbus-daemon");
            return;
        };
        let mut address = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let _bus = Bus(child);
        TEST_BUS.set(address.trim().to_owned()).unwrap();

        let Start::First(Some(conn)) = claim(true) else {
            panic!("the first copy owns the name");
        };
        let shown = Arc::new(AtomicUsize::new(0));
        let counter = shown.clone();
        serve(conn, move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        assert!(
            matches!(claim(true), Start::Another),
            "a second copy hands over"
        );
        assert_eq!(
            shown.load(Ordering::SeqCst),
            1,
            "and the first shows its window"
        );
        assert!(
            matches!(claim(false), Start::Another),
            "a login start just exits"
        );
        assert_eq!(shown.load(Ordering::SeqCst), 1, "without showing anything");
        assert!(!tray_host(), "no StatusNotifier host on this bus");

        // A stand-in for the desktop's tray watcher, with a host registered.
        let watcher = zbus::block_on(async {
            let c = session_bus().await.unwrap();
            c.request_name("org.kde.StatusNotifierWatcher")
                .await
                .unwrap();
            c
        });
        std::thread::spawn(move || {
            zbus::block_on(async move {
                let mut incoming = zbus::MessageStream::from(&watcher);
                while let Some(Ok(msg)) = incoming.next().await {
                    let header = msg.header();
                    if header.member().map(|m| m.as_str()) == Some("Get") {
                        let _ = watcher.reply(&header, &zbus::Variant::from(true)).await;
                    }
                }
            });
        });
        assert!(tray_host(), "a registered host shows the icon");
    }
}
