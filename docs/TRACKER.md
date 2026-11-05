# Flux Backup — Tracker

Status legend: ☐ not started · ◐ in progress · ☑ done

| Phase | Title | Status | Commit |
|---|---|---|---|
| 0 | Foundation | ☑ | see `git log` |
| 1 | Mirror engine (local) | ☑ | see `git log` |
| 2 | Lifecycle (retention, diff, hooks) | ☑ | see `git log` |
| 3 | Scheduling & job control | ☑ | see `git log` |
| 4 | Vault engine | ☑ | see `git log` |
| 5 | SSH transport | ☑ | see `git log` |
| 6 | GUI I: shell & core flows | ☑ | see `git log` |
| 7 | GUI II: configuration & polish | ☑ | see `git log` |
| 8 | Root mode | ☐ | |
| 9 | Release | ☐ | |

## Log

### Phase 0 — Foundation
- [x] Repo, licenses (MIT OR Apache-2.0), README, plan, tracker
- [x] Cargo workspace (edition 2024), rustfmt config, GitHub Actions CI (fmt, clippy -D warnings, test)
- [x] `flux-core`: profile model (TOML, `deny_unknown_fields`, schema version), validation with
      field-addressed issues for GUI forms, XDG/FHS paths with `FLUX_HOME` override,
      atomic 0600 profile store, `ByteSize`/`HumanDuration` value types
- [x] `flux-cli`: `profile list|show|create|edit|delete|validate`, `config path`,
      `--json`/`--system`/`-v`, target specs (`/path`, `user@host:/path`, `ssh://user@host:port/path`)
- [x] 18 tests green (unit + CLI integration), clippy clean

Notes:
- Exit codes: 0 ok, 1 error, 2 usage, 3 profile validation failed.
- Retention defaults: keep last 1, everything from the last 24h, 7 daily, 4 weekly, 12 monthly,
  5 yearly; named snapshots are never pruned.

### Phase 1 — Mirror engine (local)
- [x] `flux-core`: gitignore-style filter (absolute paths, `~` expansion, negation, size limit,
      `CACHEDIR.TAG`, `exclude_if_present`), lock-free `Progress` + `Event` reporter,
      host/user/mount info, local target safety checks (missing drive, unmounted mount point,
      `fs_uuid` match)
- [x] `flux-mirror`: repository layout + `flock` lock, snapshot `meta.json`, zstd/postcard
      manifest (sorted, streamed), parallel scanner (rayon; one-file-system, symlink modes
      with loop protection, xattrs/ACLs, optional BLAKE3), merge-join planner, parallel executor
      (hard link / reflink / `copy_file_range` / userspace fallback, source hard links preserved,
      FIFOs and devices, metadata applied in the right order, dirs bottom-up), resume from
      `.partial`, atomic commit with `syncfs`, "nothing changed" skip, free-space precheck
- [x] Restore: to original place or `--to DIR`, identical files skipped, differing files kept
      as `NAME.flux-<snapshot>` (or `--overwrite`), hard links recreated, dry run
- [x] CLI: `backup [--name] [--dry-run]`, `snapshots`, `info`, `ls [-l] [-R]`,
      `restore [--to] [--overwrite] [--dry-run] [-y]`, live progress bar with ETA, Ctrl-C
      keeps a resumable partial, hint for the snapshot that has a missing path, quiet exit on
      broken pipe
- [x] `bench/bench.sh` vs rsync `--link-dest`: 7× faster initial, ~25× faster incremental on tmpfs
      (results in README; snapshots verified identical to source and to rsync's)
- [x] 48 tests green, clippy `-D warnings` clean; manual Ctrl-C/resume check on 6 GiB

Notes / follow-ups:
- Owners are recorded in the manifest always, but applied to the data tree only when running as
  root; restore as root applies them. `security.*` xattrs are deliberately not copied.
- Sparse files are copied densely for now (follow-up: `SEEK_HOLE`/`SEEK_DATA`).
- Restoring as root into user-writable directories should guard against symlink swaps
  (`openat`/`O_NOFOLLOW` walk). Must be done before root mode (Phase 8).
- The Vault engine and SSH targets are rejected with a clear message until Phases 4–5.

### Phase 2 — Lifecycle
- [x] `flux-core::retention`: engine-agnostic policy (pinned, named, newest, keep_last,
      keep_within, hourly/daily/ISO-weekly/monthly/yearly buckets in local time preferring
      healthy snapshots, max_age), space-pruning candidate order
- [x] Mirror: `pinned` flag, `update_meta`, crash-safe delete (`.deleting-*` rename, cleaned up
      on next lock, `latest` re-pointed), `prune_locked` with free-space/inode floors, auto-prune
      after backup, `make_room` before a backup that wouldn't fit
- [x] `diff` (snapshot↔snapshot, snapshot↔live scan; added/removed/modified/type-changed with
      changed fields and byte delta), `find` (glob over names or full paths, versions per path,
      parallel over manifests), `history` (versions of one path; manifest lookups stop early)
- [x] `flux-core::hooks`: `sh -c`, `FLUX_*` env, timeout kills the hook's whole process group,
      output captured; `flux-core::power`: battery detection (ignores peripheral batteries)
- [x] New crate `flux-runner`: battery skip + nice/ionice for scheduled runs, logind sleep
      inhibitor, pre/post/on_error hooks, failure notifications (notify-rust), run history
      (`runs.jsonl`) + per-run event logs with rotation
- [x] CLI: `prune [--dry-run]`, `name`, `pin`, `unpin`, `delete [--force] [-y]`, `diff [--live]
      [--path] [--stat]`, `find [-i] [-s]`, `history`, `log [--run ID|last] [-n]`;
      `backup --no-prune`, hidden `backup --scheduled` (for Phase 3 timers)
- [x] MSRV corrected to 1.91 (code uses let-chains and `floor_char_boundary`)
- [x] 72 tests green, clippy clean; manual checks: priority applied only to scheduled runs,
      logind inhibitor visible in `systemd-inhibit --list`, failure recorded and notified

Notes / follow-ups:
- Success notifications are deliberately off (failures and partial runs only); the GUI will show
  status. Could become a setting.
- `find`/`history` decode every manifest; fine for hundreds of snapshots. A per-repo path index
  would make it instant (candidate for the GUI phase).

### Phase 3 — Scheduling & job control
- [x] `flux-runner::schedule`: unit generation (service, calendar timer with `Persistent=`,
      anacron-style interval timer running `--if-due`, `<id>.connect.service` wanted by the
      drive's mount unit), `systemd-escape --path` re-implementation (checked against systemd),
      calendar validation via `systemd-analyze calendar`, marker-protected writes (foreign units
      are never overwritten), apply/remove/sync/status, linger detection; test overrides
      `FLUX_SYSTEMD_UNIT_DIR` and `FLUX_SYSTEMCTL`
- [x] `flux-runner::jobs`: per-profile `flock` job lock, info file, Unix status socket (JSON lines:
      hello/progress/event/finished; `cancel` command), backlog replay for late clients, socket
      path fallback when the runtime dir is too deep for `sun_path`, pid-based discovery
- [x] Runner: job lock + status server around every run; not-due / on-battery / busy skips are not
      recorded (a 15-minute check would flood the history)
- [x] CLI: `schedule install|remove|sync|status`, `status`, `watch [--json]` (JSON lines), `cancel`;
      `backup --if-due`; scheduled runs exit 0 when busy; `profile edit/delete` keep timers in sync;
      quiet exit on broken pipe for returned I/O errors too
- [x] 84 tests green, clippy clean; generated calendar/interval/connect units pass
      `systemd-analyze --user verify`; live status/busy/watch/cancel verified on a 6 GiB backup

Notes / follow-ups:
- Not yet verified against a live systemd user manager (installing a real timer was blocked by
  the session's safety policy). To try it:
  `flux schedule install <profile>`, then `systemctl --user list-timers 'flux-backup-*'` and
  `journalctl --user -u 'flux-backup-*'`.
- Units call the `flux` binary that installed them (`current_exe`); packaging (Phase 9) should
  re-run `flux schedule sync` after the binary moves.

### Phase 4 — Vault engine
- [x] Refactor: new crate `flux-fs` (entry model, scanner, fs ops, snapshot metadata, outcome
      types, engine-generic diff/find/history and restore via a `RestoreSource` trait); Mirror
      moved onto it with no behaviour change (its 21 tests unchanged)
- [x] `docs/VAULT_FORMAT.md`: layout, keys, objects, blobs, packs, index (with `supersedes`),
      trees, snapshot records, concurrency/crash-safety rules, threat model
- [x] `flux-vault`: XChaCha20-Poly1305 objects, Argon2id key slots bound to the repo id,
      keyed-BLAKE3 blob ids, FastCDC 2020 with secret seed (in-memory chunking for files up to
      8 MiB), zstd (kept only when smaller), 16 MiB data/tree packs, parallel index load,
      shared/exclusive `flock`, `Backend` trait + local backend (atomic writes)
- [x] Backup: shared repo per target, per-owner (profile/host/user) parent, metadata-based
      reuse without reading, in-run dedup, deduplicated trees, early "unchanged" exit; restore
      through the generic restore; `check` (index↔packs, headers, every snapshot's
      trees/blobs, `--read-data` verifies pack hashes and every blob); prune = forget + GC
      (delete/repack at >20% garbage without re-encrypting, orphan packs, crash-safe order)
- [x] Keys: list/add/remove/passwd, never the last key; passwords from `FLUX_PASSWORD`, keyring
      (`secret-tool`), prompt, file, command; asking goes through a callback (terminal now, GUI
      later; scheduled runs can't ask)
- [x] `flux-runner::repo::AnyRepo`: snapshots/find/entries/restore/update/delete/prune/check for
      both engines; every CLI command works with Vault; new `flux check [--read-data]` (Mirror
      too) and `flux key list|add|remove|passwd|store|forget`
- [x] Dependencies optimized in dev builds (crypto-heavy tests 2× faster)
- [x] 97 tests green, clippy clean; benchmark vs Borg (README): 8× faster initial, ~10× faster
      incremental, same repository size, restore byte-identical

Notes / follow-ups:
- Found and fixed via benchmark: fastcdc's StreamCDC allocates an 8 MiB buffer per file (36 s →
  1.8 s for 100k files).
- Interactive keyring flow (prompt + store on first backup) needs a terminal; covered by code
  review, not by an automated test (tests use `FLUX_PASSWORD`).
- Vault backups forget snapshots but don't reclaim space; run `flux prune`. A weekly maintenance
  timer could do that (candidate for Phase 7 settings).
- Tree blobs are read one range at a time; a tree-pack cache would speed up `ls`/`diff` on huge
  snapshots.

### Phase 5 — SSH transport
- [x] New crate `flux-transport`: synchronous SFTP v3 client (pipelined reads/writes, 64 KiB × 64 in
      flight, short-read handling, `posix-rename`/`fsync`/`statvfs` OpenSSH extensions), system
      `ssh` wrapper (ControlMaster with a short-path fallback, BatchMode for scheduled runs,
      ServerAlive, port/identity/extra options, rsync `-e` string, shell quoting), SFTP session
      pool, remote lock files (shared/exclusive, write-then-recheck, refreshed every 5 min, stale
      after 30 min)
- [x] Vault: `SftpBackend` (atomic tmp+rename writes, `mkdir -p` on demand, nested `data/`
      listing, lock files, upload bandwidth limiter)
- [x] Mirror: `remote` module (local scan and plan; `rsync -aH --numeric-ids --files-from --from0
      --link-dest=../../<prev>/data [-A] [-X] [--bwlimit]` with progress2 parsing and per-file
      error extraction for exit 23/24; manifest/meta uploaded over SFTP; atomic rename into place;
      locally cached manifests; delete/prune/latest via remote shell; free-space check and
      make-room; restore over SFTP; check via one remote `find -printf` per snapshot)
- [x] `AnyRepo` gains `MirrorRemote`; Vault picks the local or SFTP backend; runner dispatches
      both engines to SSH targets; `flux ssh test|keygen|copy-id`
- [x] Tests: SFTP client against the real OpenSSH `sftp-server`; remote locks; CLI end-to-end for
      both engines over a local ssh stand-in that runs the real sftp-server and rsync (backup,
      unchanged, ls, restore, check, delete, prune, hard links on the "server"); 102 tests green,
      clippy clean
- [x] Throughput sanity (no network latency): 20k files + 200 MiB — Vault/SFTP 0.9 s initial,
      0.2 s incremental; Mirror/rsync 4.8 s initial, 0.06 s incremental; restores byte-identical

Notes / follow-ups:
- Not yet run against a real remote sshd (none is running on the dev machine). Try
  `flux ssh test <profile>` against a real server first.
- A resumed remote Mirror snapshot can keep extraneous files from the interrupted run in its data
  tree (rsync with `--files-from` doesn't delete). They aren't in the manifest, so restore and
  check ignore them.
- `check --read-data` isn't available for remote Mirror (sizes only); Vault verifies fully.
- A Vault used both locally and over SFTP at the same time wouldn't share locks (`flock` vs lock
  files). Uncommon; documented.

### Phase 6 — GUI I: shell & core flows
- [x] `app/`: Vite + Svelte 5 (runes) + Tailwind 4; semantic design tokens (dark default, light,
      follow-system) exposed through `@theme inline`; built-in icon set; native `<dialog>` modals;
      focus rings, reduced-motion support, ARIA roles on tabs/timeline/progress
- [x] Views: sidebar (profiles with health dots, running pulse, engine badge, theme switch),
      profile header (target, schedule, engine, "Back up now"), live job panel (phase, %,
      bytes, rate, ETA, items, warnings, Stop), Overview (failure banner with a hint, stat cards,
      latest snapshot, recent runs, locked-Vault and unreadable-target states), Snapshots
      (timeline strip with keyboard navigation, day-grouped list, file browser with breadcrumbs,
      filter, multi-select), restore dialog (another folder or in place, keep or overwrite,
      result summary), Vault unlock dialog, toasts
- [x] `app/src-tauri` (crate `flux-gui`, binary `flux-backup`): commands profiles/overview/
      snapshots/list_dir/unlock/backup_now/cancel/restore on `flux-runner`; blocking work off the
      async runtime; in-memory Vault passwords (zeroized); entry cache for browsing; job watcher
      relaying every status socket as `flux://job` + `flux://changed`; backups as detached
      `flux backup` processes; dialog plugin for folder picking; CSP; generated icons
- [x] Demo backend for browser development (`npm run dev`; `?profile=&tab=&theme=&simulate=backup`)
- [x] Checks: svelte-check (0 warnings), Vite build, clippy, backend tests against real Mirror and
      Vault backups (2), full `tauri build --no-bundle`; headless-Chrome screenshots reviewed (dark,
      light, snapshots, locked, running) and fixed (time wrapping, pluralization, icon, demo data)
- [x] GUI crate is a workspace member but not a default member; CI gains frontend and GUI jobs

Notes / follow-ups:
- The desktop window itself wasn't launched in this session (it would open on the user's screen);
  run `npm run tauri dev` to try it.
- Restore runs without a progress bar in the GUI yet (spinner + summary).

### Phase 7 — GUI II: configuration & polish
- [x] Backend (`flux-gui`): `edit.rs` (get/new/validate/save/delete profile, id suggestion, retention
      preview on real snapshots, exclusion suggestions with measured sizes under the sources,
      mounted-drive detection with filesystem UUIDs, non-interactive target test incl. SSH);
      `actions.rs` (name/pin/delete snapshot, compare two snapshots or snapshot↔live capped at
      5,000 changes, runs and run events, prune, check, restore with `flux://restore` progress);
      saving updates the profile's systemd timer; tray icon (open, back up all, quit)
- [x] Profile editor: seven sections with error badges, field-addressed inline issues, live
      validation; engine cards; drive picker / folder picker / SSH fields + test; Vault
      encryption, password source, compression; sources with folder picker; exclusions with
      one-click suggestions; schedule presets (hourly, daily, weekdays, weekly, custom systemd
      expression) and intervals; retention form with live keep/remove preview; hooks; advanced
      options; delete profile with confirmation; unsaved-changes indicator
- [x] New-profile flow (sidebar +, first-run welcome), Activity tab (runs + log), snapshot
      actions (name, pin, compare with previous / with now, delete) and compare dialog with
      filters, Overview maintenance card (check, verify, free up space with confirmation),
      restore progress bar, keyboard shortcuts with help dialog
- [x] Accessibility: WCAG AA contrast for subtle text in both themes (checked: ≥ 4.7:1), switch
      roles on toggles, labelled icon buttons, radiogroups/tabs, native dialogs, focus rings
- [x] Tests: backend editing/actions test (save/validate/typo rejection/create/delete/preview/
      name+pin/compare/runs); 105 tests total; svelte-check 0 warnings; clippy clean; full
      `tauri build --no-bundle`; headless screenshots of every new view reviewed and fixed
      (card alignment, disabled inputs, sidebar truncation, demo retention preview)

Notes / follow-ups:
- The desktop window, tray menu and native file dialogs were not exercised interactively in this
  session (they open on the user's desktop); try `npm run tauri dev`.
- Root (system) profiles aren't shown in the GUI yet (Phase 8).

### Phase 8 — Root mode
- [x] `flux-service` crate: the app's backend (views, editing, snapshot actions, restore with
      progress callbacks, job watcher) moved out of the Tauri crate, tauri-free, shared by the
      app and the helper; `Service` carries the backup command and the CLI path for timers
      (fixes timers written by the app pointing at the app binary instead of `flux`)
- [x] Typed protocol (`protocol::Call`, 27 calls, `deny_unknown_fields`, 4 MiB line limit,
      version handshake) and `dispatch`; `HelperClient` (pkexec launch, ready handshake,
      pkexec 126/127 → cancelled/not authorized, replies by id, notices, exit callback)
- [x] `flux-helper`: session over stdin/stdout for system paths; background backups by
      re-running itself with `--run-backup <id>`; relays system jobs; exits on EOF (after
      in-flight requests) or 15 idle minutes; only accepts a root-owned, non-writable `flux`
      for timers; logs call names (never parameters)
- [x] polkit policy `packaging/polkit/io.github.neuralpunk.FluxBackup.helper.policy`
      (`auth_admin_keep`, exec path `/usr/libexec/flux-backup/flux-helper`)
- [x] Symlink-safe restore (`flux_fs::safe`): `openat2(RESOLVE_NO_SYMLINKS)` fast path, else a
      component-wise walk over `O_PATH` descriptors that follows only symlinks owned by root
      or the invoking user (`PKEXEC_UID`/`SUDO_UID`), `..` resolved by our own descriptor
      chain; files created `O_EXCL|O_NOFOLLOW` and filled through the descriptor
      (`RestoreSource::write_file` takes a `File`); metadata via `fchown/fchmod/futimens/
      fsetxattr`, `*at(…, NOFOLLOW)` for links, `/proc/self/fd` only for specials; set-aside
      by `renameat`; setuid/setgid dropped and devices skipped when root restores into a
      folder owned by someone else
- [x] System scope: `Profile::validate_in(scope)` warns when a system Vault would need a keyring
      or a prompt; new system profiles default to `/etc`, `/root`, `/usr/local` without
      notifications; CLI suggests `sudo` on permission errors in `--system`
- [x] New Vaults from the app: `unlock` answers `new_vault` when no Vault exists yet, then
      accepts the password with `create`; `save_key_file` writes the unlocked password to
      `<config>/keys/<id>` (0600) and points the profile at it
- [x] GUI: routes `system:` ids to the helper and rewrites ids in replies; System section
      with Authenticate / lock / new system profile; System badge; editor in system mode;
      password dialog with create mode and "keep for scheduled backups"; theme label "Auto";
      demo mode (`?system=unlocked`, `?new=system`)
- [x] Tests: safe-walk unit tests, planted-symlink restore test (link as restored dir, link
      as restored file, untrusted link above the destination), helper end-to-end session
      (create profile → system timer units → background backup → browse → restore → close),
      out-of-protocol requests refused, CLI `--system` test, new-Vault/key-file test,
      id-rewriting test; 116 tests; clippy clean; svelte-check 0/0; `tauri build --no-bundle`;
      headless screenshots (locked, unlocked, new system profile) reviewed

Notes / follow-ups:
- Not run as real root or through a real polkit dialog in this session (that needs an
  administrator password on this machine). The helper's root-only paths (chown, device
  nodes, setid stripping) are exercised by logic tests only.
- Restoring 20,000 files still takes 0.06 s on this machine (Btrfs reflinks).

### Phase 9 — Release
- [x] `flux mount <profile> <dir> [--snapshot]`: read-only FUSE filesystem (`flux-mount`, fuser
      0.18 without libfuse, mounts through `fusermount3`) for Mirror, Mirror over SSH and Vault;
      one folder per snapshot (local start time, made unique with the id) plus `latest`;
      lazily loaded listings (6 kept), stable inode numbers, xattrs/ACLs served, `ro,nosuid,
      nodev,default_permissions`, no `allow_other`; Ctrl-C or `fusermount3 -u` unmounts
- [x] `flux_runner::view`: random-access readers (`pread`, ranged SFTP, Vault chunks located by
      plaintext length with the last chunk cached); `RemoteMirror::read_file_range`
- [x] `flux completions <shell>` (bash, zsh, fish, elvish, powershell) and hidden
      `flux manpages <dir>` (46 pages: `flux.1` and one per subcommand)
- [x] `packaging/linux/flux-backup.desktop` (also Tauri's desktop template) and
      `io.github.neuralpunk.FluxBackup.metainfo.xml`; both pass `desktop-file-validate` and
      `appstreamcli validate`
- [x] Packages: Tauri bundles (deb, rpm, AppImage) with `beforeBundleCommand` →
      `packaging/prepare-dist.sh`; deb/rpm add `/usr/bin/flux`,
      `/usr/libexec/flux-backup/flux-helper`, the polkit policy, man pages, completions,
      metainfo and a scalable icon, with dependencies (fuse3, polkit, ssh) and recommends
      (secret-tool, rsync); product name `flux-backup` so files have no spaces; standard icon
      sizes; `packaging/install.sh` (PREFIX/DESTDIR) for source installs; release workflow
      on `v*` tags (deb, rpm, AppImage, CLI tarball, SHA256SUMS, draft release); CI installs
      fuse3
- [x] Docs: `docs/USER_GUIDE.md` (install, concepts, engines, targets, sources, schedules,
      restore and mount, retention, Vault passwords, checks, system backups, hooks,
      monitoring, profile reference, environment, troubleshooting); `docs/THREAT_MODEL.md`
      (assets, attackers, at rest, in transit, on the machine, root helper and restores,
      malicious content, residual risks); README install/mount/build sections
- [x] Security fix found while writing the threat model: snapshot entries with `..`,
      absolute or empty components (a forged Mirror manifest or unencrypted Vault tree) are
      now skipped and reported by restore before anything is read or written, and hidden by
      mounts (`entry::is_clean_path`)
- [x] Tests: mount end to end for Mirror and Vault (skipped without FUSE), forged-path
      restore, clean-path rules, snapshot folder names, completions and man pages; deb and
      rpm built and inspected (contents, dependencies, desktop file)

Notes / follow-ups:
- The AppImage wasn't built locally: Tauri downloads linuxdeploy for it, so it's left to the
  release workflow. The AppImage carries the app and `flux` but not the system helper
  (root can't run programs from inside a user's FUSE mount).
- Packages weren't installed on this machine (that needs root); their contents were
  inspected instead.

### Fix — deleting Vault snapshots didn't free space (reported 2026-11-05)
- [x] Cause: deleting a Vault snapshot only removed its record; its data stayed until the next
      prune, so "Verify every byte" still read it ("All 0 snapshots are intact; 217 MiB read
      back"). Also, "Free up space" stopped at "nothing to remove" when retention kept every
      snapshot, so that data could never be reclaimed from the app.
- [x] Deleting (app and `flux delete`) now takes the exclusive lock, removes the records and
      collects garbage at once, keeping anything other snapshots in the shared Vault use; the
      app and CLI report the space freed
- [x] `check` warns about data no snapshot uses; "Free up space" collects it even when no
      snapshot is due for removal
- [x] Also fixed: the GC message counted orphaned packs twice
- [x] Tests: delete the only snapshot → no packs left, `check --read-data` reads 0 B; delete
      one owner's snapshot in a shared Vault → nothing freed, the other restores intact;
      leftover data from the old behaviour → check warns, prune frees it (123 tests)

### Redesign — "Graphite Tonal" theme, simpler setup, less clutter (2026-11-05)
- [x] Theme from the design handoff (docs/DESIGN.md): tonal tokens without borders, IBM Plex
      Sans/Mono bundled (@fontsource), floating content panel with a 12px gutter, pill nav,
      tabs and buttons (one filled per view), custom titlebar on an undecorated window
      (drag region, window buttons, window permissions in capabilities), Dark/Light/Auto
      following the desktop live; light-theme status colours darkened to meet AA (≥ 4.5:1)
- [x] "Stack" icon: Tauri bundle icons, Windows/macOS formats regenerated from the 1024 px
      master, hicolor 16–512, clean scalable and symbolic SVGs (written from the spec,
      without the handoff files' embedded metadata), tray icon that lights up while any
      backup runs (`image-png`); packages and install.sh ship the full set
- [x] Less clutter: Snapshots without the duplicate timeline, one-line list rows, "What
      changed" plus a ⋯ menu for rare actions, file browser footer only when useful;
      Overview stat cards plus one *Recent backups* card; Activity rows show status and
      when (details in tooltips); shortcuts moved to the titlebar
- [x] Profile setup: three questions (what, where, when) and an encryption switch, with a
      one-line retention summary; **Advanced options** reveals exclusions and suggestions,
      SSH details, engine and Vault settings, schedules, retention with preview, hooks,
      options, profile id and delete. Remembered while the app runs; opens on its own when
      saving fails on an advanced field. New profiles show missing fields as "To finish: …"
      instead of errors
- [x] Checks: svelte-check 0/0, Vite build, clippy, `tauri build --no-bundle`; screenshots of
      every view reviewed (dark, light, new profile, settings simple/advanced, snapshots,
      activity, system); README screenshots regenerated

Notes / follow-ups:
- The undecorated window, its drag region and edge resizing need a look in the real
  desktop session (WebKitGTK); not visible in headless screenshots.
- "Pause" from the reference design isn't implemented (Stop continues next time).
