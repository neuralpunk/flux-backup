//! A small synchronous SFTP v3 client (draft-ietf-secsh-filexfer-02), with the OpenSSH
//! extensions Flux needs: `posix-rename@openssh.com` (atomic replace),
//! `fsync@openssh.com` and `statvfs@openssh.com`.
//!
//! It talks to any process speaking SFTP on stdin/stdout, normally `ssh -s host sftp`.
//! Reads and writes are pipelined (many requests in flight), which matters a lot on
//! high-latency links.

use std::collections::{HashMap, HashSet};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout};

const INIT: u8 = 1;
const VERSION: u8 = 2;
const OPEN: u8 = 3;
const CLOSE: u8 = 4;
const READ: u8 = 5;
const WRITE: u8 = 6;
const LSTAT: u8 = 7;
const OPENDIR: u8 = 11;
const READDIR: u8 = 12;
const REMOVE: u8 = 13;
const MKDIR: u8 = 14;
const RMDIR: u8 = 15;
const REALPATH: u8 = 16;
const RENAME: u8 = 18;
const STATUS: u8 = 101;
const HANDLE: u8 = 102;
const DATA: u8 = 103;
const NAME: u8 = 104;
const ATTRS: u8 = 105;
const EXTENDED: u8 = 200;
const EXTENDED_REPLY: u8 = 201;

const FX_OK: u32 = 0;
const FX_EOF: u32 = 1;
const FX_NO_SUCH_FILE: u32 = 2;
const FX_PERMISSION_DENIED: u32 = 3;

pub const OPEN_READ: u32 = 0x01;
pub const OPEN_WRITE: u32 = 0x02;
pub const OPEN_CREAT: u32 = 0x08;
pub const OPEN_TRUNC: u32 = 0x10;
pub const OPEN_EXCL: u32 = 0x20;

const ATTR_SIZE: u32 = 0x1;
const ATTR_UIDGID: u32 = 0x2;
const ATTR_PERMISSIONS: u32 = 0x4;
const ATTR_ACMODTIME: u32 = 0x8;
const ATTR_EXTENDED: u32 = 0x8000_0000;

/// Bytes per READ/WRITE request; every server accepts 32 KiB, OpenSSH much more.
const CHUNK: usize = 64 * 1024;
/// Requests kept in flight for pipelined transfers.
const WINDOW: usize = 64;
const MAX_PACKET: u32 = 1 << 20;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Attrs {
    pub size: Option<u64>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub permissions: Option<u32>,
    pub mtime: Option<u32>,
}

impl Attrs {
    pub fn is_dir(&self) -> bool {
        self.permissions.is_some_and(|p| p & 0o170000 == 0o040000)
    }
    pub fn is_file(&self) -> bool {
        self.permissions.is_some_and(|p| p & 0o170000 == 0o100000)
    }
}

#[derive(Debug, Clone)]
pub struct StatVfs {
    pub frsize: u64,
    pub bavail: u64,
    pub files: u64,
    pub favail: u64,
}

impl StatVfs {
    pub fn free_bytes(&self) -> u64 {
        self.bavail.saturating_mul(self.frsize)
    }
}

/// Map an SFTP status to an `io::Error` with a fitting kind.
fn status_error(code: u32, msg: &str, what: &str) -> io::Error {
    let kind = match code {
        FX_NO_SUCH_FILE => io::ErrorKind::NotFound,
        FX_PERMISSION_DENIED => io::ErrorKind::PermissionDenied,
        FX_EOF => io::ErrorKind::UnexpectedEof,
        _ => io::ErrorKind::Other,
    };
    let msg = if msg.is_empty() {
        format!("SFTP status {code}")
    } else {
        msg.to_owned()
    };
    io::Error::new(kind, format!("{what}: {msg}"))
}

// ---- wire encoding ----

struct Buf(Vec<u8>);

impl Buf {
    fn new(kind: u8, id: Option<u32>) -> Self {
        let mut b = Buf(Vec::with_capacity(64));
        b.0.extend_from_slice(&[0, 0, 0, 0, kind]);
        if let Some(id) = id {
            b.u32(id);
        }
        b
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.u32(v.len() as u32);
        self.0.extend_from_slice(v);
        self
    }
    fn finish(mut self) -> Vec<u8> {
        let len = (self.0.len() - 4) as u32;
        self.0[..4].copy_from_slice(&len.to_be_bytes());
        self.0
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn bad() -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, "malformed SFTP packet")
    }
    fn u32(&mut self) -> io::Result<u32> {
        let (a, b) = self.0.split_at_checked(4).ok_or_else(Self::bad)?;
        self.0 = b;
        Ok(u32::from_be_bytes(a.try_into().expect("4 bytes")))
    }
    fn u64(&mut self) -> io::Result<u64> {
        let (a, b) = self.0.split_at_checked(8).ok_or_else(Self::bad)?;
        self.0 = b;
        Ok(u64::from_be_bytes(a.try_into().expect("8 bytes")))
    }
    fn bytes(&mut self) -> io::Result<&'a [u8]> {
        let n = self.u32()? as usize;
        let (a, b) = self.0.split_at_checked(n).ok_or_else(Self::bad)?;
        self.0 = b;
        Ok(a)
    }
    fn attrs(&mut self) -> io::Result<Attrs> {
        let flags = self.u32()?;
        let mut a = Attrs::default();
        if flags & ATTR_SIZE != 0 {
            a.size = Some(self.u64()?);
        }
        if flags & ATTR_UIDGID != 0 {
            a.uid = Some(self.u32()?);
            a.gid = Some(self.u32()?);
        }
        if flags & ATTR_PERMISSIONS != 0 {
            a.permissions = Some(self.u32()?);
        }
        if flags & ATTR_ACMODTIME != 0 {
            let _atime = self.u32()?;
            a.mtime = Some(self.u32()?);
        }
        if flags & ATTR_EXTENDED != 0 {
            for _ in 0..self.u32()? {
                self.bytes()?;
                self.bytes()?;
            }
        }
        Ok(a)
    }
}

enum Reply {
    Status(u32, String),
    Handle(Vec<u8>),
    Data(Vec<u8>),
    Name(Vec<(Vec<u8>, Attrs)>),
    Attrs(Attrs),
    Extended(Vec<u8>),
}

/// One SFTP session over a child process's stdin/stdout.
pub struct Sftp {
    child: Child,
    w: BufWriter<ChildStdin>,
    r: BufReader<ChildStdout>,
    next_id: u32,
    extensions: HashSet<String>,
    /// Replies that arrived while waiting for another id.
    stash: HashMap<u32, Reply>,
}

impl Drop for Sftp {
    fn drop(&mut self) {
        let _ = self.w.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Sftp {
    /// Start a session on a spawned child (stdin/stdout piped).
    pub fn start(mut child: Child) -> io::Result<Self> {
        let w = BufWriter::with_capacity(
            256 << 10,
            child
                .stdin
                .take()
                .ok_or_else(|| io::Error::other("no stdin"))?,
        );
        let r = BufReader::with_capacity(
            256 << 10,
            child
                .stdout
                .take()
                .ok_or_else(|| io::Error::other("no stdout"))?,
        );
        let mut s = Sftp {
            child,
            w,
            r,
            next_id: 1,
            extensions: HashSet::new(),
            stash: HashMap::new(),
        };
        let mut init = Buf::new(INIT, None);
        init.u32(3);
        s.w.write_all(&init.finish())?;
        s.w.flush()?;
        let (kind, body) = s.read_packet().map_err(|e| {
            io::Error::new(
                e.kind(),
                format!(
                    "SFTP handshake failed (is the remote reachable, and does it allow SFTP?): {e}"
                ),
            )
        })?;
        if kind != VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected reply to SFTP init",
            ));
        }
        let mut rd = Reader(&body);
        let _version = rd.u32()?;
        while !rd.0.is_empty() {
            let name = String::from_utf8_lossy(rd.bytes()?).into_owned();
            rd.bytes()?;
            s.extensions.insert(name);
        }
        Ok(s)
    }

    pub fn has_extension(&self, name: &str) -> bool {
        self.extensions.contains(name)
    }

    fn read_packet(&mut self) -> io::Result<(u8, Vec<u8>)> {
        let mut len = [0u8; 4];
        self.r.read_exact(&mut len)?;
        let len = u32::from_be_bytes(len);
        if len == 0 || len > MAX_PACKET + 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("bad SFTP packet length {len}"),
            ));
        }
        let mut body = vec![0u8; len as usize];
        self.r.read_exact(&mut body)?;
        let kind = body[0];
        body.remove(0);
        Ok((kind, body))
    }

    fn send(&mut self, kind: u8, fill: impl FnOnce(&mut Buf)) -> io::Result<u32> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let mut b = Buf::new(kind, Some(id));
        fill(&mut b);
        self.w.write_all(&b.finish())?;
        Ok(id)
    }

    fn recv(&mut self, want: u32) -> io::Result<Reply> {
        self.w.flush()?;
        if let Some(r) = self.stash.remove(&want) {
            return Ok(r);
        }
        loop {
            let (kind, body) = self.read_packet()?;
            let mut rd = Reader(&body);
            let id = rd.u32()?;
            let reply = match kind {
                STATUS => {
                    let code = rd.u32()?;
                    let msg = rd
                        .bytes()
                        .map(|m| String::from_utf8_lossy(m).into_owned())
                        .unwrap_or_default();
                    Reply::Status(code, msg)
                }
                HANDLE => Reply::Handle(rd.bytes()?.to_vec()),
                DATA => Reply::Data(rd.bytes()?.to_vec()),
                NAME => {
                    let n = rd.u32()?;
                    let mut v = Vec::with_capacity(n as usize);
                    for _ in 0..n {
                        let name = rd.bytes()?.to_vec();
                        let _long = rd.bytes()?;
                        v.push((name, rd.attrs()?));
                    }
                    Reply::Name(v)
                }
                ATTRS => Reply::Attrs(rd.attrs()?),
                EXTENDED_REPLY => Reply::Extended(rd.0.to_vec()),
                other => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("unexpected SFTP reply {other}"),
                    ));
                }
            };
            if id == want {
                return Ok(reply);
            }
            self.stash.insert(id, reply);
        }
    }

    fn expect_ok(&mut self, id: u32, what: &str) -> io::Result<()> {
        match self.recv(id)? {
            Reply::Status(FX_OK, _) => Ok(()),
            Reply::Status(c, m) => Err(status_error(c, &m, what)),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{what}: unexpected reply"),
            )),
        }
    }

    fn expect_handle(&mut self, id: u32, what: &str) -> io::Result<Vec<u8>> {
        match self.recv(id)? {
            Reply::Handle(h) => Ok(h),
            Reply::Status(c, m) => Err(status_error(c, &m, what)),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{what}: unexpected reply"),
            )),
        }
    }

    pub fn open(&mut self, path: &str, flags: u32, mode: u32) -> io::Result<Vec<u8>> {
        self.open_bytes(path.as_bytes(), flags, mode)
    }

    /// [`Sftp::open`] for a path as the server has it: file names on Linux are bytes, not
    /// necessarily UTF-8, and SFTP sends them as they are.
    pub fn open_bytes(&mut self, path: &[u8], flags: u32, mode: u32) -> io::Result<Vec<u8>> {
        let id = self.send(OPEN, |b| {
            b.bytes(path).u32(flags).u32(ATTR_PERMISSIONS).u32(mode);
        })?;
        self.expect_handle(id, &String::from_utf8_lossy(path))
    }

    pub fn close(&mut self, h: &[u8]) -> io::Result<()> {
        let id = self.send(CLOSE, |b| {
            b.bytes(h);
        })?;
        self.expect_ok(id, "close")
    }

    pub fn lstat(&mut self, path: &str) -> io::Result<Attrs> {
        let id = self.send(LSTAT, |b| {
            b.bytes(path.as_bytes());
        })?;
        match self.recv(id)? {
            Reply::Attrs(a) => Ok(a),
            Reply::Status(c, m) => Err(status_error(c, &m, path)),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "lstat: unexpected reply",
            )),
        }
    }

    pub fn remove(&mut self, path: &str) -> io::Result<()> {
        let id = self.send(REMOVE, |b| {
            b.bytes(path.as_bytes());
        })?;
        self.expect_ok(id, path)
    }

    pub fn rmdir(&mut self, path: &str) -> io::Result<()> {
        let id = self.send(RMDIR, |b| {
            b.bytes(path.as_bytes());
        })?;
        self.expect_ok(id, path)
    }

    pub fn mkdir(&mut self, path: &str, mode: u32) -> io::Result<()> {
        let id = self.send(MKDIR, |b| {
            b.bytes(path.as_bytes()).u32(ATTR_PERMISSIONS).u32(mode);
        })?;
        self.expect_ok(id, path)
    }

    /// Create `path` and its parents (like `mkdir -p`).
    pub fn mkdir_p(&mut self, path: &str, mode: u32) -> io::Result<()> {
        let mut cur = String::new();
        for (i, part) in path.split('/').enumerate() {
            if i == 0 && part.is_empty() {
                cur.push('/');
                continue;
            }
            if part.is_empty() {
                continue;
            }
            if !cur.is_empty() && !cur.ends_with('/') {
                cur.push('/');
            }
            cur.push_str(part);
            match self.lstat(&cur) {
                Ok(a) if a.is_dir() => continue,
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!("{cur} is not a directory"),
                    ));
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => self.mkdir(&cur, mode)?,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Rename, replacing `to` atomically if it exists (needs the OpenSSH extension; plain
    /// SFTP rename fails when the target exists).
    pub fn rename(&mut self, from: &str, to: &str) -> io::Result<()> {
        let id = if self.has_extension("posix-rename@openssh.com") {
            self.send(EXTENDED, |b| {
                b.bytes(b"posix-rename@openssh.com")
                    .bytes(from.as_bytes())
                    .bytes(to.as_bytes());
            })?
        } else {
            self.send(RENAME, |b| {
                b.bytes(from.as_bytes()).bytes(to.as_bytes());
            })?
        };
        self.expect_ok(id, from)
    }

    pub fn realpath(&mut self, path: &str) -> io::Result<String> {
        let id = self.send(REALPATH, |b| {
            b.bytes(path.as_bytes());
        })?;
        match self.recv(id)? {
            Reply::Name(v) if !v.is_empty() => Ok(String::from_utf8_lossy(&v[0].0).into_owned()),
            Reply::Status(c, m) => Err(status_error(c, &m, path)),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "realpath: unexpected reply",
            )),
        }
    }

    pub fn statvfs(&mut self, path: &str) -> io::Result<StatVfs> {
        if !self.has_extension("statvfs@openssh.com") {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "server lacks statvfs@openssh.com",
            ));
        }
        let id = self.send(EXTENDED, |b| {
            b.bytes(b"statvfs@openssh.com").bytes(path.as_bytes());
        })?;
        match self.recv(id)? {
            Reply::Extended(body) => {
                let mut r = Reader(&body);
                let _bsize = r.u64()?;
                let frsize = r.u64()?;
                let _blocks = r.u64()?;
                let _bfree = r.u64()?;
                let bavail = r.u64()?;
                let files = r.u64()?;
                let _ffree = r.u64()?;
                let favail = r.u64()?;
                Ok(StatVfs {
                    frsize,
                    bavail,
                    files,
                    favail,
                })
            }
            Reply::Status(c, m) => Err(status_error(c, &m, path)),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "statvfs: unexpected reply",
            )),
        }
    }

    /// Directory listing without `.` and `..`.
    pub fn readdir(&mut self, path: &str) -> io::Result<Vec<(String, Attrs)>> {
        let id = self.send(OPENDIR, |b| {
            b.bytes(path.as_bytes());
        })?;
        let h = self.expect_handle(id, path)?;
        let mut out = Vec::new();
        let result = loop {
            let id = self.send(READDIR, |b| {
                b.bytes(&h);
            })?;
            match self.recv(id)? {
                Reply::Name(v) => {
                    for (n, a) in v {
                        if n != b"." && n != b".." {
                            out.push((String::from_utf8_lossy(&n).into_owned(), a));
                        }
                    }
                }
                Reply::Status(FX_EOF, _) => break Ok(()),
                Reply::Status(c, m) => break Err(status_error(c, &m, path)),
                _ => {
                    break Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "readdir: unexpected reply",
                    ));
                }
            }
        };
        let _ = self.close(&h);
        result.map(|()| out)
    }

    /// Read `len` bytes from `offset` (fewer at end of file), pipelined.
    pub fn read_range(
        &mut self,
        path: &str,
        offset: u64,
        len: u64,
        sink: impl FnMut(&[u8]) -> io::Result<()>,
    ) -> io::Result<u64> {
        self.read_range_bytes(path.as_bytes(), offset, len, sink)
    }

    /// [`Sftp::read_range`] for a path given as bytes (see [`Sftp::open_bytes`]).
    pub fn read_range_bytes(
        &mut self,
        path: &[u8],
        offset: u64,
        len: u64,
        mut sink: impl FnMut(&[u8]) -> io::Result<()>,
    ) -> io::Result<u64> {
        let h = self.open_bytes(path, OPEN_READ, 0)?;
        let result = self.read_handle(&h, offset, len, &mut sink);
        let _ = self.close(&h);
        result
    }

    fn read_handle(
        &mut self,
        h: &[u8],
        offset: u64,
        len: u64,
        sink: &mut impl FnMut(&[u8]) -> io::Result<()>,
    ) -> io::Result<u64> {
        let end = offset.saturating_add(len);
        let mut next = offset;
        let mut pending: std::collections::VecDeque<(u32, u64, u32)> = Default::default();
        let mut done = 0u64;
        let mut eof = false;
        loop {
            while !eof && pending.len() < WINDOW && next < end {
                let want = (end - next).min(CHUNK as u64) as u32;
                let id = self.send(READ, |b| {
                    b.bytes(h).u64(next).u32(want);
                })?;
                pending.push_back((id, next, want));
                next += u64::from(want);
            }
            let Some((id, off, want)) = pending.pop_front() else {
                break;
            };
            match self.recv(id)? {
                Reply::Data(d) => {
                    if off != offset + done {
                        // Should not happen: replies are consumed in request order.
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "out-of-order SFTP data",
                        ));
                    }
                    sink(&d)?;
                    done += d.len() as u64;
                    if (d.len() as u32) < want {
                        // Short read: re-request the remainder of this range synchronously.
                        let mut pos = off + d.len() as u64;
                        let stop = off + u64::from(want);
                        while pos < stop {
                            let id = self.send(READ, |b| {
                                b.bytes(h).u64(pos).u32((stop - pos) as u32);
                            })?;
                            match self.recv(id)? {
                                Reply::Data(d2) if !d2.is_empty() => {
                                    sink(&d2)?;
                                    done += d2.len() as u64;
                                    pos += d2.len() as u64;
                                }
                                Reply::Status(FX_EOF, _) | Reply::Data(_) => {
                                    eof = true;
                                    break;
                                }
                                Reply::Status(c, m) => return Err(status_error(c, &m, "read")),
                                _ => {
                                    return Err(io::Error::new(
                                        io::ErrorKind::InvalidData,
                                        "read: unexpected reply",
                                    ));
                                }
                            }
                        }
                    }
                }
                Reply::Status(FX_EOF, _) => eof = true,
                Reply::Status(c, m) => return Err(status_error(c, &m, "read")),
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "read: unexpected reply",
                    ));
                }
            }
            if eof {
                // Drain the replies still in flight.
                while let Some((id, ..)) = pending.pop_front() {
                    let _ = self.recv(id)?;
                }
                break;
            }
        }
        Ok(done)
    }

    pub fn read_file(&mut self, path: &str) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        self.read_range(path, 0, u64::MAX, |d| {
            out.extend_from_slice(d);
            Ok(())
        })?;
        Ok(out)
    }

    /// Write `data` to a new file `path` (pipelined); `fsync` if the server supports it.
    pub fn write_new(&mut self, path: &str, data: &[u8], mode: u32) -> io::Result<()> {
        let h = self.open(path, OPEN_WRITE | OPEN_CREAT | OPEN_TRUNC, mode)?;
        let result = (|| {
            let mut pending = std::collections::VecDeque::new();
            for (i, chunk) in data.chunks(CHUNK).enumerate() {
                let off = (i * CHUNK) as u64;
                let id = self.send(WRITE, |b| {
                    b.bytes(&h).u64(off).bytes(chunk);
                })?;
                pending.push_back(id);
                if pending.len() >= WINDOW {
                    let id = pending.pop_front().expect("non-empty");
                    self.expect_ok(id, path)?;
                }
            }
            while let Some(id) = pending.pop_front() {
                self.expect_ok(id, path)?;
            }
            if self.has_extension("fsync@openssh.com") {
                let id = self.send(EXTENDED, |b| {
                    b.bytes(b"fsync@openssh.com").bytes(&h);
                })?;
                self.expect_ok(id, path)?;
            }
            Ok(())
        })();
        let closed = self.close(&h);
        result.and(closed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    pub fn local_server() -> Option<Sftp> {
        let bin = [
            "/usr/libexec/openssh/sftp-server",
            "/usr/lib/openssh/sftp-server",
            "/usr/lib/ssh/sftp-server",
        ]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())?;
        let child = Command::new(bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        Sftp::start(child).ok()
    }

    #[test]
    fn talks_to_openssh_sftp_server() {
        let Some(mut s) = local_server() else {
            eprintln!("skipping: no sftp-server installed");
            return;
        };
        assert!(s.has_extension("posix-rename@openssh.com"));
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("a/b/c");
        let d = d.to_str().unwrap();
        s.mkdir_p(d, 0o700).unwrap();
        assert!(s.lstat(d).unwrap().is_dir());

        // Big enough to exercise pipelining and short reads.
        let data: Vec<u8> = (0..5_000_000u32).map(|i| (i % 253) as u8).collect();
        let f = format!("{d}/file");
        s.write_new(&f, &data, 0o600).unwrap();
        assert_eq!(s.lstat(&f).unwrap().size, Some(data.len() as u64));
        assert!(s.read_file(&f).unwrap() == data);
        let mut part = Vec::new();
        s.read_range(&f, 1_234_567, 100_000, |c| {
            part.extend_from_slice(c);
            Ok(())
        })
        .unwrap();
        assert!(part == data[1_234_567..1_334_567]);

        // Atomic replace.
        let g = format!("{d}/other");
        s.write_new(&g, b"new", 0o600).unwrap();
        s.rename(&g, &f).unwrap();
        assert_eq!(s.read_file(&f).unwrap(), b"new");

        let names: Vec<String> = s.readdir(d).unwrap().into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["file"]);
        assert_eq!(
            s.lstat(&format!("{d}/nope")).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert!(s.statvfs(d).unwrap().free_bytes() > 0);
        s.remove(&f).unwrap();
        s.rmdir(d).unwrap();
        assert!(s.realpath(".").unwrap().starts_with('/'));
    }
}
