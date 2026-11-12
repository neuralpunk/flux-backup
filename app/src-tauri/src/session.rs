//! The app's place on the desktop session bus: one copy running at a time, and whether a
//! system tray is there to show its icon.
//!
//! The first copy owns `io.github.neuralpunk.FluxBackup` on the session bus. Started again
//! (from the app menu, say, while its window is hidden in the tray), Flux asks that copy to
//! show its window and exits. Started with a request (`--previous-versions PATH`, from a
//! file manager), it hands that over instead (`Open`). Without a session bus, every copy
//! runs on its own, as before.
//!
//! Also the desktop's accent colour, from the XDG settings portal (GNOME 47+, KDE), for the
//! app's own blue to follow.

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

/// What a file manager asked for: (`"versions"` or `"deleted"`, an absolute path); or a
/// notification: (`"backup"`, a backup's id).
pub type Request = (String, String);

/// Become the running copy, or hand over to the one that is (with `request`, if any).
pub fn claim(show_other: bool, request: Option<&Request>) -> Start {
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
                let asked = match request {
                    Some(r) => Some(
                        conn.call_method(Some(NAME), PATH, Some(INTERFACE), "Open", r)
                            .await,
                    ),
                    None if show_other => Some(
                        conn.call_method(Some(NAME), PATH, Some(INTERFACE), "Activate", &())
                            .await,
                    ),
                    None => None,
                };
                if let Some(Err(e)) = asked {
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

/// Answer `Activate` from later copies with `show`, and `Open` with `open` then `show`
/// (on a thread of its own).
pub fn serve(
    conn: zbus::Connection,
    show: impl Fn() + Send + 'static,
    open: impl Fn(Request) + Send + 'static,
) {
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
                let member = header.member().map(|m| m.as_str().to_owned());
                if member.as_deref() == Some("Activate") {
                    show();
                    let _ = conn.reply(&header, &()).await;
                } else if member.as_deref() == Some("Open")
                    && let Ok(r) = msg.body().deserialize::<Request>()
                {
                    open(r);
                    show();
                    let _ = conn.reply(&header, &()).await;
                } else {
                    let _ = conn
                        .reply_error(
                            &header,
                            "org.freedesktop.DBus.Error.UnknownMethod",
                            &"Flux Backup only knows Activate and Open",
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

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SETTINGS: &str = "org.freedesktop.portal.Settings";
const APPEARANCE: &str = "org.freedesktop.appearance";

/// An accent colour as the portal gives it: red, green and blue, each 0–1.
pub type Accent = [f64; 3];

/// The portal's `(ddd)`, possibly wrapped in a variant (its older `Read`). Out of range
/// means none is set.
fn accent_of(v: &zbus::Variant<'_>) -> Option<Accent> {
    use zbus::Variant as Value;
    match v {
        Value::Variant(inner) => accent_of(inner),
        Value::Structure(s) => {
            let f: Vec<f64> = s
                .fields()
                .iter()
                .filter_map(|x| match x {
                    Value::F64(n) => Some(*n),
                    _ => None,
                })
                .collect();
            let [r, g, b] = f[..] else { return None };
            [r, g, b]
                .iter()
                .all(|c| (0.0..=1.0).contains(c))
                .then_some([r, g, b])
        }
        _ => None,
    }
}

/// The desktop's accent colour, if it has one (no portal, or none chosen: `None`).
pub fn accent() -> Option<Accent> {
    zbus::block_on(async {
        let conn = session_bus().await.ok()?;
        let args = (APPEARANCE, "accent-color");
        let reply = match conn
            .call_method(Some(PORTAL), PORTAL_PATH, Some(SETTINGS), "ReadOne", &args)
            .await
        {
            Ok(m) => m,
            // Portals before version 2 have only `Read`.
            Err(_) => conn
                .call_method(Some(PORTAL), PORTAL_PATH, Some(SETTINGS), "Read", &args)
                .await
                .ok()?,
        };
        let v = reply.body().deserialize::<zbus::OwnedVariant>().ok()?;
        accent_of(&v)
    })
}

/// Calls `changed` whenever the desktop's accent colour changes, until the session ends.
pub fn watch_accent(changed: impl Fn(Option<Accent>) + Send + 'static) {
    std::thread::spawn(move || {
        zbus::block_on(async move {
            let Ok(conn) = session_bus().await else {
                return;
            };
            let Ok(rule) = zbus::MatchRule::builder()
                .msg_type(Type::Signal)
                .interface(SETTINGS)
                .member("SettingChanged")
                .build()
            else {
                return;
            };
            let Ok(mut signals) = zbus::MessageStream::for_match_rule(rule, &conn, None).await
            else {
                return;
            };
            while let Some(Ok(msg)) = signals.next().await {
                if let Ok((ns, key, v)) = msg
                    .body()
                    .deserialize::<(String, String, zbus::OwnedVariant)>()
                    && ns == APPEARANCE
                    && key == "accent-color"
                {
                    changed(accent_of(&v));
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn the_portals_accent_colour() {
        use zbus::{Structure, Variant as Value};
        let rgb = |r: f64, g: f64, b: f64| {
            Value::Structure(
                Structure::builder()
                    .add_field(r)
                    .add_field(g)
                    .add_field(b)
                    .build()
                    .unwrap(),
            )
        };
        assert_eq!(accent_of(&rgb(0.2, 0.5, 0.9)), Some([0.2, 0.5, 0.9]));
        // The older `Read` wraps it once more.
        let wrapped = Value::Variant(Box::new(rgb(0.2, 0.5, 0.9)));
        assert_eq!(accent_of(&wrapped), Some([0.2, 0.5, 0.9]));
        // Out of range: no accent chosen.
        assert_eq!(accent_of(&rgb(-1.0, -1.0, -1.0)), None);
        assert_eq!(accent_of(&Value::from(1u32)), None);
    }

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

        let Start::First(Some(conn)) = claim(true, None) else {
            panic!("the first copy owns the name");
        };
        let shown = Arc::new(AtomicUsize::new(0));
        let counter = shown.clone();
        let opened = Arc::new(std::sync::Mutex::new(Vec::<Request>::new()));
        let got = opened.clone();
        serve(
            conn,
            move || {
                counter.fetch_add(1, Ordering::SeqCst);
            },
            move |r| got.lock().unwrap().push(r),
        );
        assert!(
            matches!(claim(true, None), Start::Another),
            "a second copy hands over"
        );
        assert_eq!(
            shown.load(Ordering::SeqCst),
            1,
            "and the first shows its window"
        );
        assert!(
            matches!(claim(false, None), Start::Another),
            "a login start just exits"
        );
        assert_eq!(shown.load(Ordering::SeqCst), 1, "without showing anything");
        // A file manager's request reaches the running copy, which shows its window.
        let r: Request = ("versions".into(), "/home/me/notes.md".into());
        assert!(matches!(claim(true, Some(&r)), Start::Another));
        assert_eq!(*opened.lock().unwrap(), vec![r]);
        assert_eq!(shown.load(Ordering::SeqCst), 2);
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
