//! Running jobs: one at a time per profile, observable and cancellable.
//!
//! While a backup runs, `<runtime>/jobs/` holds:
//!
//! * `<id>.lock`: `flock`ed by the job, so a second run of the same profile is refused
//! * `<id>.json`: [`JobInfo`] (pid, run id, socket path)
//! * a Unix socket (path in the JSON) speaking JSON lines:
//!   - server → client: [`Message`]s (`hello`, then `progress` about 5×/s, `event`s, `finished`)
//!   - client → server: `{"cmd":"cancel"}`
//!
//! The runtime directory is private to the user (`$XDG_RUNTIME_DIR` is mode 0700), and the
//! socket and files are 0600.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::Duration;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use flux_core::progress::{Event, Progress, ProgressSnapshot};
use flux_core::{Paths, ProfileId};

use crate::runlog::{RunStatus, Trigger};

/// Events kept for clients that connect late.
const BACKLOG: usize = 200;
/// Events kept at most (later warnings are counted, not stored).
const MAX_EVENTS: usize = 10_000;

pub fn jobs_dir(paths: &Paths) -> PathBuf {
    paths.runtime_dir.join("jobs")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobInfo {
    pub profile: String,
    pub run_id: String,
    pub pid: u32,
    pub started: Timestamp,
    pub trigger: Trigger,
    pub socket: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    Hello {
        info: JobInfo,
    },
    Progress {
        progress: ProgressSnapshot,
    },
    Event {
        event: Event,
    },
    Finished {
        status: RunStatus,
        error: Option<String>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    Cancel,
}

/// Held for the whole run; removes the info file on drop.
#[derive(Debug)]
pub struct JobLock {
    _file: File,
    info_path: PathBuf,
}

impl Drop for JobLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.info_path);
    }
}

fn private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

/// Take the profile's job lock. `Ok(None)` if another job of this profile is running.
pub fn try_lock(paths: &Paths, id: &ProfileId) -> io::Result<Option<JobLock>> {
    let dir = jobs_dir(paths);
    private_dir(&dir)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(dir.join(format!("{id}.lock")))?;
    // A lock just released can look taken for a moment: a process forked by another thread
    // of this program shares the descriptor until it execs (close-on-exec). So look again
    // a few times before calling the job busy.
    for attempt in 0..5 {
        match rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => {
                return Ok(Some(JobLock {
                    _file: file,
                    info_path: dir.join(format!("{id}.json")),
                }));
            }
            Err(rustix::io::Errno::WOULDBLOCK) if attempt < 4 => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(rustix::io::Errno::WOULDBLOCK) => return Ok(None),
            Err(e) => return Err(e.into()),
        }
    }
    unreachable!("the last attempt returns")
}

/// Socket path for a job. Unix socket paths are limited to ~107 bytes; if the runtime
/// directory is too deep, use a short private directory under /tmp instead.
fn socket_path(paths: &Paths, id: &ProfileId) -> io::Result<PathBuf> {
    let p = jobs_dir(paths).join(format!("{id}.sock"));
    if p.as_os_str().len() < 100 {
        return Ok(p);
    }
    let hash = fnv_hex(jobs_dir(paths).as_os_str().as_encoded_bytes());
    let dir = std::env::temp_dir().join(format!("flux-{}-{hash}", flux_core::sys::euid()));
    private_dir(&dir)?;
    Ok(dir.join(format!("{id}.sock")))
}

fn fnv_hex(data: &[u8]) -> String {
    // FNV-1a: only needs to tell runtime dirs apart, not resist attackers.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in data {
        h = (h ^ u64::from(*b)).wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// State shared between the job and its status clients.
pub struct Shared<'a> {
    pub info: JobInfo,
    pub progress: &'a Progress,
    events: Mutex<Vec<Event>>,
    finished: Mutex<Option<(RunStatus, Option<String>)>>,
    stop: AtomicBool,
}

impl<'a> Shared<'a> {
    pub fn new(info: JobInfo, progress: &'a Progress) -> Self {
        Self {
            info,
            progress,
            events: Mutex::new(Vec::new()),
            finished: Mutex::new(None),
            stop: AtomicBool::new(false),
        }
    }

    pub fn push_event(&self, ev: &Event) {
        if let Ok(mut v) = self.events.lock()
            && v.len() < MAX_EVENTS
        {
            v.push(ev.clone());
        }
    }

    pub fn finish(&self, status: RunStatus, error: Option<String>) {
        if let Ok(mut f) = self.finished.lock() {
            *f = Some((status, error));
        }
    }

    /// Stop accepting clients; existing clients get `finished` and are closed.
    pub fn shutdown(&self) {
        self.stop.store(true, Relaxed);
    }
}

/// A bound status socket plus the info file advertising it.
pub struct Server {
    listener: UnixListener,
    socket: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket);
    }
}

/// Bind the job's socket and publish its info file.
pub fn bind(paths: &Paths, id: &ProfileId, mut info: JobInfo) -> io::Result<(Server, JobInfo)> {
    let socket = socket_path(paths, id)?;
    let _ = fs::remove_file(&socket); // stale from a crashed run; we hold the lock
    let listener = UnixListener::bind(&socket)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    info.socket = socket.clone();
    let json = serde_json::to_vec_pretty(&info).map_err(io::Error::other)?;
    flux_core::store::write_atomic(&jobs_dir(paths).join(format!("{id}.json")), &json, 0o600)
        .map_err(io::Error::other)?;
    Ok((Server { listener, socket }, info))
}

impl Server {
    /// Accept and serve clients until [`Shared::shutdown`]. Run this in a scoped thread.
    pub fn serve<'s>(&'s self, scope: &'s std::thread::Scope<'s, '_>, shared: &'s Shared<'_>) {
        while !shared.stop.load(Relaxed) {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    scope.spawn(move || serve_client(stream, shared));
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50))
                }
                Err(e) => {
                    tracing::debug!("status socket accept failed: {e}");
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    }
}

fn send(w: &mut impl Write, m: &Message) -> io::Result<()> {
    let mut line = serde_json::to_vec(m).map_err(io::Error::other)?;
    line.push(b'\n');
    w.write_all(&line)
}

fn serve_client(stream: UnixStream, shared: &Shared<'_>) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let Ok(reader) = stream.try_clone() else {
        return;
    };
    std::thread::scope(|s| {
        // Commands from the client.
        s.spawn(|| {
            for line in BufReader::new(reader).lines() {
                let Ok(line) = line else { break };
                if let Ok(Command::Cancel) = serde_json::from_str(&line) {
                    tracing::info!("cancel requested through the status socket");
                    shared.progress.cancel();
                }
            }
        });
        let mut w = io::BufWriter::new(&stream);
        let mut sent = shared
            .events
            .lock()
            .map(|v| v.len().saturating_sub(BACKLOG))
            .unwrap_or(0);
        let result = (|| -> io::Result<()> {
            send(
                &mut w,
                &Message::Hello {
                    info: shared.info.clone(),
                },
            )?;
            loop {
                let fresh: Vec<Event> = shared
                    .events
                    .lock()
                    .map(|v| v[sent.min(v.len())..].to_vec())
                    .unwrap_or_default();
                sent += fresh.len();
                for event in fresh {
                    send(&mut w, &Message::Event { event })?;
                }
                send(
                    &mut w,
                    &Message::Progress {
                        progress: shared.progress.snapshot(),
                    },
                )?;
                let finished = shared.finished.lock().ok().and_then(|f| f.clone());
                if let Some((status, error)) = finished {
                    send(&mut w, &Message::Finished { status, error })?;
                    w.flush()?;
                    return Ok(());
                }
                w.flush()?;
                if shared.stop.load(Relaxed) {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        })();
        if let Err(e) = result {
            tracing::debug!("status client gone: {e}");
        }
        // Unblock the reader thread.
        let _ = stream.shutdown(std::net::Shutdown::Both);
    });
}

/// Jobs currently running. Liveness is judged by the recorded pid, not by probing the lock:
/// probing would briefly take the lock and could make a job that's just starting see
/// "busy".
pub fn running(paths: &Paths) -> Vec<JobInfo> {
    let Ok(rd) = fs::read_dir(jobs_dir(paths)) else {
        return Vec::new();
    };
    let mut out: Vec<JobInfo> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_slice::<JobInfo>(&fs::read(e.path()).ok()?).ok())
        .filter(|info| Path::new(&format!("/proc/{}", info.pid)).exists())
        .collect();
    out.sort_by(|a, b| a.profile.cmp(&b.profile));
    out
}

/// A connection to a running job.
pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Client {
    pub fn connect(paths: &Paths, id: &ProfileId) -> io::Result<Self> {
        let info = running(paths)
            .into_iter()
            .find(|j| j.profile == id.as_str())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no backup of {id} is running"),
                )
            })?;
        let stream = UnixStream::connect(&info.socket)?;
        Ok(Self {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
        })
    }

    pub fn cancel(&mut self) -> io::Result<()> {
        self.writer.write_all(b"{\"cmd\":\"cancel\"}\n")
    }

    /// Next message; `None` when the job closed the connection.
    pub fn next_message(&mut self) -> io::Result<Option<Message>> {
        let mut line = String::new();
        loop {
            line.clear();
            if self.reader.read_line(&mut line)? == 0 {
                return Ok(None);
            }
            if let Ok(m) = serde_json::from_str(&line) {
                return Ok(Some(m));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_is_exclusive_and_cleans_up() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::under(t.path(), flux_core::Scope::User);
        let id = ProfileId::new("p").unwrap();
        let l = try_lock(&paths, &id).unwrap().unwrap();
        assert!(try_lock(&paths, &id).unwrap().is_none());
        drop(l);
        assert!(try_lock(&paths, &id).unwrap().is_some());
    }

    #[test]
    fn serve_watch_and_cancel() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::under(t.path(), flux_core::Scope::User);
        let id = ProfileId::new("p").unwrap();
        let _lock = try_lock(&paths, &id).unwrap().unwrap();
        let progress = Progress::new();
        let info = JobInfo {
            profile: "p".into(),
            run_id: "r1".into(),
            pid: std::process::id(),
            started: Timestamp::now(),
            trigger: Trigger::Manual,
            socket: PathBuf::new(),
        };
        let (server, info) = bind(&paths, &id, info).unwrap();
        let shared = Shared::new(info, &progress);
        shared.push_event(&Event::Info {
            message: "early".into(),
        });
        std::thread::scope(|s| {
            s.spawn(|| server.serve(s, &shared));
            // Discoverable through the info file.
            let jobs = running(&paths);
            assert_eq!(jobs.len(), 1);
            let mut c = Client::connect(&paths, &id).unwrap();
            assert!(matches!(
                c.next_message().unwrap(),
                Some(Message::Hello { .. })
            ));
            assert!(
                matches!(c.next_message().unwrap(), Some(Message::Event { .. })),
                "backlog is replayed"
            );
            c.cancel().unwrap();
            for _ in 0..50 {
                if progress.is_cancelled() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(progress.is_cancelled());
            shared.finish(RunStatus::Cancelled, None);
            let mut last = None;
            while let Some(m) = c.next_message().unwrap() {
                last = Some(m);
            }
            assert!(matches!(
                last,
                Some(Message::Finished {
                    status: RunStatus::Cancelled,
                    ..
                })
            ));
            shared.shutdown();
        });
    }
}
