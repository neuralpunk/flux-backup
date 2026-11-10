# Flux Backup threat model

What Flux protects, against whom, and where its protection stops. The Vault's on-disk
format is specified in [VAULT_FORMAT.md](VAULT_FORMAT.md); this document covers the whole
program.

## Assets

1. **Backed-up data**: file contents, names, metadata (owners, permissions, ACLs, xattrs).
2. **Availability of backups**: being able to restore when the original is gone.
3. **The machine itself**: Flux runs as you, and system backups run as root. A flaw must
   not let someone else gain your rights or root's.
4. **Secrets**: Vault passwords and SSH keys.

## Who might attack

| Attacker | Can |
|---|---|
| **Thief or finder of a backup drive** | read and modify everything on it, offline |
| **Untrusted storage** (a NAS, a rented server, a friend's machine) | read and modify everything Flux stores there, watch when and how much it writes |
| **Network attacker** between you and an SSH server | intercept and alter traffic |
| **Another local user** (unprivileged) on the same machine | create files and symlinks in places they own (their home, `/tmp`), run programs as themselves |
| **Malicious content in what's backed up** | file names, symlinks, setuid programs, device nodes, oddly shaped trees |

Out of scope: an attacker who already has your account or root. They can read your files,
your keyring, and anything Flux decrypts. Hardware attacks on the running machine are also
out of scope.

## Backups at rest

### Vault (encrypted)

Against a drive thief or untrusted storage, an encrypted Vault keeps **contents and
metadata confidential and tamper-evident**:

- Every object (data chunks, directory trees, snapshot records, indexes, pack headers) is
  sealed with XChaCha20-Poly1305 under a random 256-bit key. Random 192-bit nonces make
  reuse a non-issue.
- That key is stored only wrapped by your password(s) with Argon2id (64 MiB and 3 passes by
  default; *strong* is 256 MiB and 4 passes, *maximum* 1 GiB and 6) and bound to the
  repository's id, so a key slot can't be moved to another Vault.
- A password can be made to need an **unlock file** too (kept on a USB stick, say): its
  key slot is then wrapped with the password and a hash of the file, so the password alone
  is useless to someone who learned it. Other passwords of the same Vault keep working
  without the file unless they're removed or given it; the app and `flux key unlock-file`
  say which.
- Blob ids are keyed BLAKE3 MACs, and chunk boundaries come from a secretly seeded
  chunker. Ids and boundaries therefore don't reveal whether the Vault contains a known
  file.
- Readers verify every blob's id after decryption, so modified, swapped or truncated data
  is detected (`flux check --read-data` verifies everything; restores and mounts verify
  what they read).

What it does **not** hide: the number and approximate sizes of packs and snapshots, when
backups happened, and roughly how much changed each time. A pack's exact size can tell the
size of a single small file a backup stored; *hide file sizes* pads every pack to a Padmé
length (at most about 12% bigger), so only a coarse size is left. The number of blobs in a
pack (its header's length) stays visible. It can't stop someone with write
access from deleting or corrupting the backups; it only detects that. Keep a second backup
somewhere else if that matters.

Security rests on the password. Argon2id makes each guess expensive, but a short or reused
password can still be guessed. Losing every password makes the Vault unreadable, and
there is deliberately no recovery.

Whether a Vault is encrypted is recorded in its `config`, which is not secret and not
authenticated before the key is known. A profile with `encryption = true` therefore refuses
a Vault whose `config` says it isn't encrypted (someone with write access to the target
could otherwise turn encryption off and have the next backup stored in the clear).

Unencrypted Vaults (`encryption = false`) keep integrity checking and dedup, but not
confidentiality.

**Repair data** (`parity/`, optional since 1.1) is computed from what is stored, after
encryption: it tells someone with the drive nothing the stored files don't, and its
sizes say what theirs do. It is protection against a failing drive, not against a
person: whoever can write to the target can spoil it along with the data. It can't be
used to slip something in, because a repair is only of a file that fails its own check
(a pack's hash, an object's authentication), and the result must pass that check before
it replaces the file. Two things to know: a key slot's repair data holds that key slot,
so it is removed with the password it belongs to (`flux key remove` in Flux 1.1 or
later; a Flux older than that removes the slot and leaves its repair data until a newer
Flux frees space, so remove a password with the newer one); and a snapshot record
changed by an older Flux and damaged afterwards is rebuilt as it was before that change.

### Mirror (plain folders)

Mirror snapshots are ordinary files with their original permissions. The repository folder
is created with mode 0700 and the manifests are private, so other users of the backup
machine can't list it. Anyone who has the drive in hand, or is root wherever it's
mounted, can read everything. Use full-disk encryption (LUKS) on the drive, or a Vault,
for anything sensitive. Integrity: `flux check` confirms every file is there with the
right size, and `--read-data` reads each one back, comparing it with its recorded hash
where one exists (`change_detection = "checksum"`). That catches accidental damage, not
deliberate tampering by someone who can also rewrite the manifest.

### rsync compatibility mode (plain folders)

The same as Mirror: ordinary files with their original permissions, in a repository folder
created with mode 0700, readable by anyone holding the drive. Use an encrypted drive for
anything sensitive. The recovery kit (`FLUX-RECOVERY.txt`, `flux-backup.sh`) lists the
backed-up paths and exclusions, but no secrets. `flux-backup.sh` is a shell script on the
backup drive: anyone who can write to the drive can change it, so read it before running a
copy you didn't make yourself (`flux recovery <profile> --script` prints a fresh one from
your profile). Restores from Flux use the same symlink-safe restore as the other engines;
the rsync commands in the guide are the equivalent for use without Flux and have rsync's
own behaviour.

### Borg compatibility mode

The repository is a standard Borg 1.x repository with Borg's own protections: with the
default `repokey-blake2`, contents and metadata are encrypted and authenticated by Borg, and
the key is stored in the repository protected by the passphrase (`encryption = "none"`
gives neither). Flux passes the passphrase to borg in its environment (`BORG_PASSPHRASE`)
for the duration of one borg process; it never logs it or writes it to disk (except where
the profile's password source keeps it: keyring, key file). The recovery guide lists the
repository location, encryption mode and password source, but no secrets. Archive
comments (Flux's names and pins) are inside the encrypted repository. Restores extract into
a temporary folder, inside the `--to` folder or Flux's cache, removed afterwards; restores
as root then go through the symlink-safe restore. Borg's caches (`~/.cache/borg`) and
security folder are Borg's own; their threat model is Borg's.

## In transit (SSH)

Flux uses the system `ssh`, so host-key verification and authentication are OpenSSH's.
Scheduled runs use `BatchMode=yes`: they fail rather than accept an unknown host key or
prompt for a password. Accept a new server's key once, interactively (`flux ssh test`).
Connection-sharing sockets (`ControlPath`) live in your private runtime directory.

A Vault on an SSH server is encrypted before it leaves the machine; the server stores
opaque packs. Mirror over SSH stores plain files on the server (see above), so the server
must be trusted with the contents.

Lock files (in the Vault's `locks/`, written on local disks and over SFTP alike)
coordinate concurrent clients; a malicious server can ignore or forge them, and the
result is detected corruption (or a refused backup), not silent corruption, for Vaults.

## On the machine

### Your own profiles

Profiles, run logs and state are written atomically with mode 0600 inside private
(0700) folders under `~/.config`, `~/.local/state` and `$XDG_RUNTIME_DIR`. A running
backup's status socket lives in the private runtime directory: only you (and root) can
watch or cancel it.

Hooks are shell commands *you* configure. They run as you, with your environment.

### Passwords

- The app keeps passwords you type in memory only (zeroized when dropped) until it quits,
  or, with *lock it again in the app*, until the Vault has gone unused that long.
  It passes them to the background `flux backup` process in its environment
  (`FLUX_PASSWORD`). Only you and root can read that, via `/proc/<pid>/environ`.
- The `keyring` source stores the password in the desktop keyring (Secret Service), which
  is unlocked with your login session.
- `file` sources should be mode 0600. Key files written by the app for system Vaults are
  created 0600 inside a 0700 folder under `/etc/flux-backup/keys/`, readable by root only.
- Passwords are never written to logs, run records or the helper's log.

### Mounted snapshots

`flux mount` is read-only, `nosuid` and `nodev`, and leaves permission checks to the
kernel using the stored owners and modes. It isn't mounted with `allow_other`, so other
users (root included) can't enter it at all. Setuid programs and device nodes inside
snapshots are inert there.

### The desktop app

The window is a web view with a strict Content Security Policy: only the app's own
scripts, no remote content, and IPC only to its own backend. File names and other text
from backups are rendered as text, never as HTML.

## System backups (root)

This is where a mistake would hurt most, so the design keeps root's surface small.

**Privilege separation.** The app never runs as root. For system profiles it starts
`/usr/libexec/flux-backup/flux-helper` through `pkexec`; polkit asks for an administrator's
password (`auth_admin_keep`, action `io.github.neuralpunk.FluxBackup.helper`). The helper:

- speaks one protocol: JSON lines with a fixed set of typed calls (the same calls the app
  makes for your own profiles). Unknown methods, unknown fields and lines over 4 MiB are
  refused. There is no "run this command" or "read this file" call;
- validates every profile id (no `/`, no `..`), and works only on profiles in
  `/etc/flux-backup/`;
- logs which calls it receives, never their parameters;
- exits when the app closes the pipe, or after 15 minutes without requests;
- writes timers that run only a root-owned `flux` binary that nobody else can modify.

**Trust boundary.** Whoever controls the app's process while it's unlocked can make any of
those calls for up to 15 idle minutes. That's the logged-in administrator's session, the
same boundary as `sudo`'s timestamp. An administrator can deliberately configure hooks,
which then run as root; that is intended and documented.

Environment variables can't redirect the helper: pkexec clears the environment, so the
`FLUX_*` overrides meant for testing never reach it. The `flux` binary its timers run is
found next to the helper or in `/usr/bin`, never through a variable.

**Restores as root.** Restoring writes into places other users may control (their home
folders, a chosen destination). A path-based restore could be redirected by a symlink
swapped in at the right moment, so Flux's restore never resolves destination paths the
ordinary way:

- Destination folders are opened through file descriptors, one component at a time
  (`openat2` with `RESOLVE_NO_SYMLINKS`, or an explicit walk with `O_NOFOLLOW`).
  Symlinks are followed only when owned by root or by the administrator who asked
  (`PKEXEC_UID`/`SUDO_UID`). `..` in a link target is resolved against descriptors already
  held, so it can't climb out through a swapped folder.
- Files are created with `O_CREAT | O_EXCL | O_NOFOLLOW` in the held folder and filled
  through that descriptor; an existing file or link at the name is renamed aside or removed
  first, never written through.
- Ownership, permissions, times and xattrs are set on the open descriptor
  (`fchown`, `fchmod`, `futimens`, `fsetxattr`). Symlinks and special files are handled
  with `*at(…, AT_SYMLINK_NOFOLLOW)`. For special files, a descriptor is checked to still
  be the node Flux created before its mode is set.
- When root restores into a folder owned by someone else, setuid/setgid bits are dropped
  and device nodes are skipped. A user shouldn't end up holding a setuid-root copy of an
  old, perhaps vulnerable, program, or a raw disk device.

**Exports as root.** A snapshot of a system backup saved as an archive is written by the
helper into a folder you choose, often your own. The archive and its checksum file are
made new there (never written through a file or link already under that name) and are
readable by root only: they hold what the system backup holds, `/etc/shadow` included.
The files are staged in a private folder made for the export and read back from that
folder as it was opened, following no link, so renaming it away or putting another in its
place gets nothing out of root.

**Backups as root** read the source tree. The scanner doesn't follow symlinks (unless the
profile asks for it), and nothing read from sources is executed. Files are read some time
after they were scanned, so each is opened below its source folder with every symlink on
the way refused by the kernel (`openat2` with `RESOLVE_NO_SYMLINKS` and `RESOLVE_BENEATH`;
on kernels without it, the file itself is opened with `O_NOFOLLOW`), without blocking, and
read only if it is a regular file. A user can't swap a file of theirs for a link to
someone else's between the scan and the read, and a named pipe put in its place isn't
waited on. Links to and above a source folder are followed, as given.

## Malicious backup content

- **Names**: a sound snapshot only holds plain relative paths. A damaged or forged one
  (someone with write access to an unencrypted target can rewrite a Mirror manifest or an
  unencrypted Vault's trees) might contain absolute paths or `..` components. Restores
  skip such entries before reading or writing anything, and report them; mounts don't show
  them.
- **Symlinks**: restored as symlinks, never followed while restoring (see above), and
  only folders have anything below them: an entry a forged snapshot lists below a link
  is skipped and reported, not written to where the link points. A snapshot's stored
  files (a Mirror's or rsync backup's data folder, what Borg extracted) are opened
  below that folder with every link refused, so a link planted among them can't make a
  restore copy a file from elsewhere. `flux-vault-recover` does the same: every folder
  on the way to a file must be a real folder, and a file is always created new.
- **Saved settings**: the copy of a profile kept with its backups (`flux-profile.toml`)
  is read as a description, not as instructions. Hooks, webhooks, password commands, ssh
  options (`ProxyCommand`) and extra rsync/borg arguments in it are dropped when a backup
  is recovered: a drive anyone could write to must not be able to run a program on the
  computer recovering from it. Its name and exclusions are text an attacker could write
  too: in the scripts, guides and settings for other tools that Flux makes from a profile,
  control characters in them are replaced, and a here-document's end marker is never a
  line of its contents, so none of it becomes a command.
- **Hard links**: recreated with `linkat` between two destinations of the same restore,
  both reached through held descriptors.
- **Special files**: FIFOs are recreated; device nodes only as root, and never into
  another user's folder.

## Residual risks and advice

- Keep at least two backups in different places; no format survives a deleted target.
- Use a Vault (or full-disk encryption) for drives that leave the house.
- Use long, unique Vault passwords, and keep a spare one (`flux key add`).
- Treat an unlocked system session in the app like an open `sudo` session.
- Mirror over SSH trusts the server with your data; Vault over SSH doesn't.
