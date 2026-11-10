# Fixtures: backups and profiles written by each release

Each `v*/` folder was written by that release of Flux itself (built from its release
commit), not by today's code:

| Folder | Written by | Engines |
|---|---|---|
| `v0.1` | Flux 0.1.0 (87a7b95) | mirror, vault (format 1) |
| `v0.2` | Flux 0.2.0 (dbaa8d8) | mirror, vault (format 1), rsync, borg |
| `v0.3` | Flux 0.3.0 (25ec953) | mirror, vault (format 1), rsync, borg |
| `v1.0` | Flux 1.0.0 | mirror, vault (format 2), rsync, borg |

In each: `profiles/` (the profile files that release wrote), `tgt/` (the backups: two
snapshots each, the second after a change), and `expected.json` (the files' SHA-256 in
each snapshot, the password `fixture`). The test `backups_from_every_release_are_read`
(in `crates/flux-cli/tests/cli.rs`) reads them all with this build, restores them, and
carries on backing up into them.

They were made in a user namespace so they hold no real computer or user name: the
computer is `fixture-host`, the user `root`, the files are under `/srv`.

To make one (here for a `flux` binary built from a release commit):

```sh
mkdir work
unshare -rUmu --fork sh -c "hostname fixture-host && mount --bind $PWD/work /srv && \
  python3 generate.py /path/to/flux mirror,vault,rsync,borg"
# then copy work/home/config/profiles, work/tgt and work/expected.json into v<version>/
```

Never edit them by hand: they stand for what users' disks hold.
