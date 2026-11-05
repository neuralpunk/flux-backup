//! Parsing of `--target` arguments.
//!
//! * `/mnt/backup` → local directory
//! * `[user@]host:/path` → SSH (scp syntax)
//! * `ssh://[user@]host[:port]/path` → SSH with an optional port

use std::path::PathBuf;

use anyhow::{Context, bail};
use flux_core::Target;

pub fn parse(spec: &str) -> anyhow::Result<Target> {
    if let Some(rest) = spec.strip_prefix("ssh://") {
        let (authority, path) = rest
            .find('/')
            .map(|i| rest.split_at(i))
            .context("ssh:// target needs a path, e.g. ssh://nas/srv/backup")?;
        let (user, hostport) = split_user(authority);
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) => (h, Some(p.parse::<u16>().context("invalid port")?)),
            None => (hostport, None),
        };
        return ssh(user, host, port, path);
    }
    if spec.starts_with('/') {
        return Ok(Target::Local {
            path: PathBuf::from(spec),
            fs_uuid: None,
        });
    }
    if let Some((authority, path)) = spec.split_once(':')
        && !authority.contains('/')
    {
        let (user, host) = split_user(authority);
        return ssh(user, host, None, path);
    }
    bail!("target {spec:?} must be an absolute path, [user@]host:/path or ssh://host/path")
}

fn split_user(authority: &str) -> (Option<&str>, &str) {
    match authority.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, authority),
    }
}

fn ssh(user: Option<&str>, host: &str, port: Option<u16>, path: &str) -> anyhow::Result<Target> {
    if host.is_empty() {
        bail!("missing host name");
    }
    if path.is_empty() {
        bail!("missing remote path after ':'");
    }
    Ok(Target::Ssh {
        host: host.to_owned(),
        user: user.map(str::to_owned),
        port,
        path: PathBuf::from(path),
        identity_file: None,
        ssh_options: Vec::new(),
        bandwidth_limit: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssh_parts(t: Target) -> (String, Option<String>, Option<u16>, PathBuf) {
        match t {
            Target::Ssh {
                host,
                user,
                port,
                path,
                ..
            } => (host, user, port, path),
            other => panic!("not ssh: {other:?}"),
        }
    }

    #[test]
    fn local() {
        assert!(matches!(parse("/mnt/usb").unwrap(), Target::Local { .. }));
        assert!(parse("relative/dir").is_err());
    }

    #[test]
    fn scp_style() {
        let (h, u, p, path) = ssh_parts(parse("me@nas:/srv/b").unwrap());
        assert_eq!(
            (h.as_str(), u.as_deref(), p, path),
            ("nas", Some("me"), None, "/srv/b".into())
        );
        let (h, u, _, path) = ssh_parts(parse("nas:backups").unwrap());
        assert_eq!((h.as_str(), u, path), ("nas", None, "backups".into()));
        assert!(parse("nas:").is_err());
        assert!(parse(":/x").is_err());
    }

    #[test]
    fn url_style() {
        let (h, u, p, path) = ssh_parts(parse("ssh://me@nas.local:2222/srv/b").unwrap());
        assert_eq!(
            (h.as_str(), u.as_deref(), p, path),
            ("nas.local", Some("me"), Some(2222), "/srv/b".into())
        );
        assert!(parse("ssh://nas").is_err());
        assert!(parse("ssh://nas:notaport/x").is_err());
    }
}
