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
| 8 | Root mode | ☑ | see `git log` |
| 9 | Release | ☑ | see `git log` |
| 10 | Compatibility mode I: rsync engine | ☑ | see `git log` |
| 11 | Compatibility mode II: Borg engine | ☑ | see `git log` |
| 12 | Compatibility mode III: the app | ☑ | see `git log` |
| 13 | UX I: foundations | ☑ | see `git log` |
| 14 | UX II: sidebar, profiles, system backups | ☑ | see `git log` |
| 15 | UX III: settings and the rest | ☑ | see `git log` |
| 16 | Export and import profiles | ☑ | see `git log` |
| 17 | Quick wins | ☑ | see `git log` |
| 18 | Finding files | ☑ | see `git log` |
| 19 | Space | ☑ | see `git log` |
| 20 | Control | ☑ | see `git log` |
| 21 | Robustness | ☑ | see `git log` |
| 22 | Release 0.2 | ☑ | see `git log` |
| 23 | Website, guide and README for 0.2 | ☑ | see `git log` |
| 39 | 0.3: Restore, compare and the size limit | ☑ | see `git log` |
| 40 | 0.3: Setup wizard and settings polish | ☑ | see `git log` |
| 41 | 0.3: Import from Back In Time | ☑ | see `git log` |
| 42 | Release 0.3 | ☑ | see `git log` |

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

### Phase 10 — Compatibility mode I: rsync engine (2026-11-07)
- [x] Plan agreed: compatibility mode = engines that store everything in a standard tool's
      native format, plus a recovery kit next to the backups (PLAN.md, phases 10–12)
- [x] `flux-core`: `engine = "rsync"`, `[rsync] extra_args` (options that would break the
      layout are refused), `Engine::is_compat`/`label`
- [x] New crate `flux-compat`: target-neutral `Store` (local folder, or SFTP + remote shell;
      0700 folders, atomic writes, locks, symlinks, free space), tool detection with install
      hints, gitignore → rsync filter translation (reversed for first-match-wins, anchoring
      rules, `**/`, negation), CACHEDIR.TAG / `exclude_if_present` search written as rules,
      snapshot listings (local scan, remote `find -printf`, cached per snapshot), generic
      retention
- [x] rsync engine: `rsync --archive --hard-links --numeric-ids --relative --delete
      --delete-excluded [--acls] [--xattrs] … --link-dest=../../<prev>/data` into
      `snapshots/.partial`, progress from `--info=progress2`, statistics from `--stats`,
      per-file errors for exit 23/24, unchanged runs detected and discarded, rename + `latest`;
      `rsync-command.txt` (exact command, version, exit status) and `rsync-filter.txt` in
      every snapshot; snapshots made without Flux listed and adopted (meta.json with
      statistics) at the next backup; name/pin, delete, prune with pins and space floors,
      check (`--read-data` locally), mount, restore through the generic restore
- [x] Recovery kit: `FLUX-RECOVERY.txt` written per profile and target, layered for people
      new to rsync (in short / new to rsync / copy-paste commands) and for experts (layout,
      exact command, filter semantics); `flux-backup.sh`, a POSIX script doing the same
      backup with rsync only (local or over ssh, marker search with `find`, unique names,
      continues `.partial`, `--dry-run`); rewritten after every backup
- [x] Runner/`AnyRepo`/view/service integration; CLI `--engine rsync`, `flux recovery
      [--script] [--save DIR]`; target test checks rsync locally and on the server
- [x] Tests: unit (translation, rules rendering, marker search, find parsing, listing cache,
      stats parsing, command building, kits, store, ids); CLI end to end locally (hard links
      between and within snapshots, exclusions incl. negation and odd names, caches, ls,
      diff, restore, check --read-data, recovery, **the generated script run with an empty
      environment**, its snapshot listed and browsed by Flux, pin/delete/prune), over the SSH
      stand-in (backup, unchanged, ls, restore, check, kit on the server, **the script run
      remotely**, pin/delete), FUSE mount; clippy clean

Notes / follow-ups:
- Found while testing: the script could move `.partial` *into* an existing snapshot when run
  in the same second as another backup; it now picks a unique name as Flux does.
- Restores from Flux use Flux's own restore over the plain folders (identical files skipped,
  differing ones kept aside, symlink-safe as root); the guide gives the rsync equivalents.
- `rsync --stats` counts files skipped by `--max-size` in its totals, so statistics can be
  slightly high for profiles with a size limit.

### Phase 11 — Compatibility mode II: Borg engine (2026-11-07)
- [x] `flux-core`: `engine = "borg"`, `[borg]` (encryption `repokey-blake2`/`repokey`/`none`,
      compression, password source, `remote_path`, `extra_args` with reserved options
      refused); warnings for what Borg can't do (size limit, following symlinks);
      `Profile::password_source()` for every engine; system-scope keyring warning for Borg
- [x] Passwords: the same sources as Vault (`FLUX_PASSWORD`, keyring, prompt, file,
      command) with Borg wording; `flux key store/forget` work for Borg (verified with
      `borg list` before storing)
- [x] `flux-compat::borg`: tool detection (1.2–1.4; Borg 2 explained), repository per
      profile next to the kit, local or `ssh://` (BORG_RSH from Flux's ssh options,
      `BORG_REMOTE_PATH`), init with `--make-parent-dirs`; `borg create --json --progress
      --list --filter=AME --log-json` with live progress (estimated from the previous
      archive), per-file warnings, exit 0/1/2; gitignore → Borg patterns (`P sh`,
      reversed, `!` for exclusions); `--exclude-caches`/`--exclude-if-present`/
      `--one-file-system`/`--noacls`/`--noxattrs`/`--files-cache=disabled` for checksum;
      archives listed with `borg list --json` (cached for when a backup holds the lock);
      names and pins in the archive comment (`borg recreate --comment`, braces escaped);
      listings from `--json-lines` (hard links, modes, local times, parent folders
      added); restore by extracting into a staging folder, then the generic restore;
      delete + compact (bytes freed reported), prune with Flux's policy, `borg check
      [--verify-data]`, mount via `borg extract --stdout`; archives made without Flux
      adopted with `borg info`
- [x] Recovery kit for Borg: SET UP lines (`BORG_REPO`, `BORG_PASSCOMMAND` mapped from the
      password source, `BORG_RSH`), list/mount/extract/restore everything, key export,
      delete/compact and the closest `borg prune` line for the profile's retention, check;
      novice and expert sections; `flux-backup.sh` with embedded patterns,
      `{now:…}` archive names, `--dry-run`
- [x] Runner/`AnyRepo`/view/service/CLI integration (`--engine borg`, `ssh test` checks
      borg on the server, target test, `wrong_password`/`password_required` codes)
- [x] Tests: unit (modes, local times, freed bytes, comments, patterns, listings with hard
      links and parents, URLs, kit); CLI end to end with the real borg locally and over the
      SSH stand-in (backup, names, ls incl. negated patterns and caches, diff, restore with
      hard links and no staging left behind, check --verify-data, pin, wrong passphrase),
      the generated script run without Flux and its archive browsed in Flux,
      delete/compact, prune keeping the pin, kit on the server; FUSE mount for Borg;
      every test uses its own `BORG_BASE_DIR`

Notes / follow-ups:
- Borg can't reliably say "nothing changed" (its files cache drops the most recently
  changed files on purpose, and new chunks don't show renames), so every Borg backup makes
  an archive; deduplication makes unchanged ones nearly free.
- Found and fixed while testing: the borg runner logged its environment (with the
  passphrase) at debug level; parent folders were missing from Borg listings, so mounts
  couldn't be walked; mounts loaded the same snapshot twice under parallel lookups; the
  script's `--dry-run` expanded to a stray argument.
- `flux-transport`'s `lock::tests::shared_and_exclusive` failed once under full parallel
  test load and passed 10/10 alone, with and without these changes: a pre-existing
  timing flake to look at separately.

### Phase 12 — Compatibility mode III: the app (2026-11-07)
- [x] Service: `recovery_kit` (guide, script, location, tool status) and `tools` calls (protocol
      version 2, so an older system helper is refused with a clear message); Borg unlock
      (passphrase checked with `borg list`, `new_vault` for a repository that doesn't exist
      yet); `needs_password` and key files for every engine with a password
- [x] Tauri: `recovery_kit`, `tools`, `save_recovery_kit` (writes the two files into a folder
      the user picked, as the user, also for system profiles)
- [x] Editor: **Usable without Flux** row under the encryption switch, opening a chooser
      (Flux's format / rsync / Borg) written for newcomers (what each is good for, what to
      mind), with tool detection and install hints, and an expert disclosure with the real
      commands; the encryption switch follows the engine (rsync explains encrypted drives,
      Borg toggles `repokey-blake2`/`none`, fixed once created); advanced view: four engines
      with "standard tool" tags, Borg encryption/passphrase source/compression/server
      path/extra options, rsync extra options; empty option lists never saved
- [x] Overview: **Usable without Flux** card → **Recovery kit** modal (guide split into its
      sections with jump chips and per-section copy, script tab, copy all, save a copy); kit
      reachable from the locked state; Borg wording for locked/unlock/password dialogs;
      stored vs copied bytes for Borg; header tags (rsync / Borg) with explanations
- [x] Modals never exceed the window (scroll inside); demo data (rsync and Borg profiles,
      `?tools=missing`, `?open=recovery|compat`)
- [x] Website: "Backups that never depend on Flux" section with two screenshots, nav link;
      guide chapter in both tables of contents; engine-card bullet CSS fixed (inline code
      broke into columns); README and user guide
- [x] Checks: svelte-check 0/0, Vite build, clippy (workspace + flux-gui), flux-gui tests,
      service test for the new views; headless screenshots reviewed (overview card, kit in
      dark/light, chooser with tools installed/missing, Borg advanced settings, new profile)
- [x] Found and fixed: `flux diff` picked "the snapshot before" by comparing start times; Borg
      records them to the second, so two backups in one second tied and diff found none
      (intermittent test failure). It now uses the list order.

Notes / follow-ups:
- The real desktop window wasn't driven in this session (screenshots are of the demo build in
  headless Chrome); the Tauri commands are covered by `cargo clippy/test -p flux-gui`.

### Phase 13 — UX I: foundations (2026-11-08)
- [x] Review of the app and research (NN/g on signifiers and context menus, GNOME HIG boxed
      lists, tag/chip/badge conventions, polkit and administrator prompts, Laws of UX):
      docs/UX_PRINCIPLES.md with findings, sources and eight rules; DESIGN.md points to it
- [x] `lib/menu.svelte.ts` + `MenuHost`: one menu for the app, opened by right-click
      (`use:contextMenu`) or a ⋯ button (`MenuButton`); kept inside the window (flips up),
      first item focused, arrows/Home/End/Enter/Escape/Tab, focus returns, closes on outside
      click, resize and blur; the right-clicked item is outlined while its menu is open;
      Menu key and Shift+F10 open a focused item's menu; icons, separators, danger items,
      disabled items with a reason
- [x] The webview's own context menu (Reload, Inspect…) is suppressed except on text that can
      be selected or copied
- [x] Settings building blocks: `.rows`/`Row` (title, subtitle, value, chevron for rows that
      open something, right-hand control slot, lock icon + reason when disabled), `Toggle`
      rebuilt (label left, switch right, whole line clickable, `row` variant), `.switch`,
      `ConfirmDialog`, `.btn-danger-filled`
- [x] Tags restyled as descriptors (outlined, smaller, squarer than buttons; `tag-accent`,
      `tag-warn`)
- [x] Snapshots: list rows get a right-click menu, the ⋯ button shares the same items (browse,
      what changed, compare with now, name, pin, delete; delete disabled with a reason
      when pinned)
- [x] Checks: svelte-check 0/0, Vite build; menus driven in headless Chrome over CDP (real
      right-click, arrow keys, Escape) and screenshots reviewed

### Phase 14 — UX II: sidebar, profiles, system backups (2026-11-08)
- [x] Sidebar "Your backups": each backup a two-line row with its destination icon (drive or
      server) and its status in words ("Backed up 2 hours ago", "Backing up · 42%", "Failed 2
      days ago", "Some files skipped", "Needs fixing in Settings", "Not backed up yet"); the
      icon tile repeats the status colour; "+" in the heading and a "New backup" row
- [x] One menu per backup (`lib/profileMenu.ts`): back up now / stop, browse snapshots,
      activity, settings, recovery guide (compatibility mode), delete (disabled with a reason
      while running); on right-click in the sidebar and a ⋯ button next to *Back up now*
- [x] Backup actions moved into app state (`backupNow`, `stop`, `open`, `deleteProfile`, the
      password prompt and app-level dialogs), so menus work on any backup, not only the
      selected one
- [x] Delete: a confirmation that names the profile, says the backups stay and where, with a
      filled red "Delete profile" that never gets default focus; also a visible row at the
      end of Settings (basic view), apart from everything else
- [x] System backups: a compact sidebar row ("Locked · admin password", chevron) that opens an
      explanation (what system backups are, that the desktop will ask for an administrator
      password, when the helper stops) before the system's password dialog; unlocked: a
      "System backups" heading with + and lock
- [x] Header: descriptor tags with icons (destination, schedule, storage; compatibility mode
      as an accent tag "rsync · works without Flux"; system as a warning tag)
- [x] README, user guide and website updated for the new wording
- [x] Checks: svelte-check 0/0, Vite build; driven in headless Chrome over CDP: right-click
      in the sidebar, ⋯ menu, keyboard to Delete…, delete completed (list and selection
      updated), system explanation, both themes

### Phase 15 — UX III: settings and the rest (2026-11-08)
- [x] No more "Advanced options" mode (so nothing can stay expanded for the next profile; the
      editor always opens on its main page): the main page asks what / where / when / what
      to keep, and every other group is a row with an icon, its current value and ›, opening
      just that group as a sub-page ("‹ All settings"; Save and Cancel stay in place):
      *Leave some things out*, *Connection details* (SSH), *How backups are stored*, *More
      schedule options*, *Keep and remove snapshots*, *Hooks*, *Performance and behaviour*
- [x] Problems show where they are: a row whose group has errors says "1 problem" in red, and
      "To finish… / things to fix" opens that sub-page (or scrolls to the main-page question)
- [x] Settings built from rows: switches on the right with the whole row clickable, selects,
      sliders and inputs at the right of their row, groups of related rows; navigation rows
      carry icons (switches beside them too, so text lines up), control rows on sub-pages
      don't; disabled controls say why ("Fixed when the Borg repository was created",
      "rsync can't encrypt…", "Only for backups at set times")
- [x] *How backups are stored*: four options in two labelled groups, "Flux's own formats"
      (Plain folders, Encrypted) and "Works without Flux" (rsync, Borg), each with what it's
      good for, what to mind and the tool's status; per-format settings below; the separate
      compatibility-mode dialog is gone (its content lives here)
- [x] Fixed: the main-page encryption switch now changes the format (it only flipped
      itself); encryption is choosable again when an existing profile changes format (a new
      repository); missing `exclude_if_present` / `ssh_options` lists normalised on load; the
      profile id is shown under the name ("Saved as home", "Change" for new profiles)
- [x] New settings exposed in the UI: skip folders containing a marker file, extra ssh
      options
- [x] Right-click menus on files (open, restore this, restore the selection, select, copy
      path) and on backup runs (show details, copy the error, back up again); a shared
      clipboard helper
- [x] Screenshots regenerated (README, website); README, user guide, website guide, DESIGN.md
      and UX_PRINCIPLES.md updated
- [x] Checks: svelte-check 0/0, Vite build; driven in headless Chrome: rows open their pages
      and come back, the encryption switch changes the summary to "Encrypted (Vault)", the
      lock lifts after a format change, file right-click, new-profile "To finish"

### Phase 16 — Export and import profiles (2026-11-08)
- [x] `flux_compat::export`: Flux profile (TOML with its id, importable), borgmatic
      (`config.yaml`: sources, repository with label, exclusions as Borg patterns with their
      style spelled out, caches/markers/one-file-system, compression, passphrase command from
      the password source, ssh command, remote path, extra create options for ACLs, xattrs and
      checksums, archive names, retention, checks, hooks), restic (a script: repository, local
      or sftp, password command, exclude file with gitignore → restic translation, caches,
      markers, size limit, `restic forget --prune` with the retention), rsync and Borg scripts
      (the recovery-kit generators); every export lists what it can't carry over (pins and
      names, max age, space floors, the schedule, battery, tool limits) and how to create a
      repository that doesn't exist yet
- [x] Import: `parse_flux` (the exported id is remembered); the service picks the id (given,
      exported, or from the name) and never replaces a profile (`-2`, `-3`…); problems don't
      stop the import, the app opens the profile's settings
- [x] CLI: `flux profile export ID [-f flux|borgmatic|restic|rsync|borg] [-o FILE|DIR]` (notes
      on stderr), `flux profile import FILE [--id ID]`
- [x] App: *Export…* in each backup's menu and *Settings › More › Export this profile*; an
      export dialog (formats with what they're for and what they work with, the notes, a live
      preview of the file, Copy, Save… into a folder you pick); *Import a profile…* behind the
      sidebar's + and on the welcome screen. Files are read and written by the app as you,
      also for system profiles; imported system ids are qualified like the others
- [x] borgmatic keys checked against borgmatic's published schema (all current; the hook keys
      are deprecated in 2.0 but accepted, and the only form 1.8/1.9 understand; the file
      shows the 2.0 `commands:` equivalent; "P sh" avoided since borgmatic doesn't document it)
- [x] Tests: unit (round trip, borgmatic essentials, restic translation and script, formats);
      CLI (all formats, unknown format refused, import refuses a taken id, `--id`, identical
      round trip, the exported rsync script backs up without Flux); by hand: borgmatic YAML
      parsed (PyYAML), scripts `sh -n`, the exported borg script made a real archive with
      the exclusions applied; GUI screenshots of the dialog and the + menu

Notes / follow-ups:
- borgmatic and restic weren't installed here (installing them from PyPI/GitHub wasn't
  allowed in this session), so the borgmatic config wasn't run through `borgmatic config
  validate` and the restic script wasn't run. Worth doing once on a machine that has them.
- Found while testing: the export's note about creating a Borg repository missed
  `--make-parent-dirs`; the import error printed a debug id. Both fixed.

### Phase 17 — Quick wins (2026-11-08)
- [x] Open a file from a snapshot: `Call::OpenFile` restores one file (≤ 4 GiB, not for
      system profiles) into `~/.cache/flux/opened/<snapshot>-<n>`, read-only; the app opens it
      with `xdg-open`. Double-click a file or *Open a copy* in its menu; copies older than a
      day are removed
- [x] Duplicate a profile: "X (copy)" with a free id (`-copy`, `-copy-2`…), its timer set up
      like a saved profile (`edit::apply_timer`, shared with Save); in the backup's menu and
      `flux profile duplicate ID`
- [x] Health warnings on the Overview, in words with what to do: drive not connected for a
      while (with how long), two or more failed runs in a row, overdue for its schedule
      (manual 30 days, hourly 1, daily 3, weekdays 4, weekly 14, intervals 3×), never
      succeeded, under 5 GiB free (red under 1 GiB). An unplugged drive doesn't also show
      "overdue"
- [x] First-backup card after the first snapshot: how to get a file back, "Try it: browse
      files"; dismissed once per profile
- [x] Activity: All / Problems filter; *Copy diagnostics* (`Call::Diagnostics`,
      `flux diagnostics ID`): versions of Flux, the OS, rsync, borg and ssh, the profile with
      the password command and hooks redacted, validation problems, the last 10 runs and the
      last run's messages
- [x] Tests: health rules with a fixed clock, redaction, GUI command qualification; by hand:
      CLI duplicate/diagnostics, GUI screenshots (health cards, first-backup card, Activity
      filter), double-click opening a file in the demo

### Phase 18 — Finding files (2026-11-08)
- [x] Search every snapshot from the Snapshots tab (*Find in all snapshots*): plain text is
      a case-insensitive part of the name (of the path with a `/`), `*?[` make a pattern;
      results still in the newest snapshot first, at most 500 (the total is shown); files
      no longer in the newest snapshot are tagged *Deleted* with the last snapshot that had
      them. Click shows the item in its snapshot (folder opened, item selected),
      double-click opens a copy; Versions, Restore and a context menu on each row.
      `Call::Find` on the existing `flux_fs::history::find` (now also reports the newest
      modification time)
- [x] Versions of a file (`Call::Versions` on `history::history`): each distinct version
      with its modification time, size and snapshots, newest first; *Current* when it's
      the newest snapshot's, a note when the file was deleted; Open and Restore… per
      version. From a file's menu, the Versions button when one file is selected, and
      search results
- [x] Restore preview (`flux_fs::restore::preview`, `Call::RestorePreview`): the same
      selection and identical-file rule as restore, metadata only: new items (and folders),
      items that differ (kept aside or replaced), identical ones, bytes to write, the first
      100 paths. Shown in the restore dialog for both destinations as soon as one is
      chosen; when files would be replaced the button reads "Restore and replace N files"
      in red; with nothing to do, Restore is disabled
- [x] Restore selection factored out of `restore()` (shared by restore and preview)
- [x] Tests: preview (new/differing/identical/new folders, nothing touched), search-box
      matching (case, names vs paths, patterns, non-ASCII), service end to end on a Mirror
      with two snapshots (find, versions, preview in place); GUI: screenshots of search
      results, the versions dialog, the preview with replacing; show-in-snapshot checked
      (folder and selection)

Notes / follow-ups:
- Search and versions read every snapshot's listing (in parallel); Phase 21's path index
  is meant to make that cheap for Vault and Borg.
- Found while testing: restoring a version from the Versions dialog passed no path
  (the dialog's path was read after it was cleared). Fixed.

### Phase 19 — Space (2026-11-08)
- [x] Size estimate (`Call::Estimate`, `space::estimate_impl`): scans the profile being
      edited with its exclusions (no hashing, no xattrs; a local target inside the sources
      left out) and returns files, folders, bytes, unreadable items, missing sources and
      the free space at the destination. Only the newest estimate runs: starting another
      cancels the older one (a generation counter cancels its `Progress`). System profiles
      are counted by the helper
- [x] New-backup page: "About 32 GiB in 51,900 files, after the exclusions", recounted
      0.7 s after the folders, exclusions or destination change (not on other edits);
      warns when it's more than the free space; compression note for Vault and Borg
- [x] Free space recorded after each run (`RunRecord::free_after`, local targets;
      `flux_runner::free_space`/`disk_size`)
- [x] Forecast (`space::forecast`): least squares over the last 90 days of measurements
      plus now; needs 3 points over 3 days; days until full when filling (under ten
      years). In the Overview (`Overview::space`), and a health warning under 30 days
      (not alongside the low-space one)
- [x] Overview *Space on BackupDisk* card: free of size, the forecast in words, a chart of
      used space (2px line, 10% wash, solid hairlines for full and the baseline, end dot
      with a surface ring), the projection dashed to "Full around …", crosshair tooltip
      snapping to measurements, arrow keys (a slider for assistive tech), and a table
      view. Chart colour `--c-chart` per theme, checked with the dataviz validator
      (lightness band, ≥ 3:1 on the card)
- [x] Tests: forecast (filling, steady, too little history, old/missing points), the
      30-day warning, estimate end to end with and without an exclusion, free space
      recorded by a real run; GUI screenshots: chart dark/light and hovered, the estimate

Notes / follow-ups:
- SSH targets get no forecast yet (free space would need the server's statvfs during the
  run).
- Found while testing: the tooltip covered the headline and "Full around…" sat on the
  projection line; both moved.

### Phase 20 — Control (2026-11-08)
- [x] Pause scheduled backups (`flux_runner::prefs`: `paused.json` in the state folder,
      until a time or until resumed; expired pauses are removed). Checked first among the
      quick skips of scheduled runs ("scheduled backups are paused until …"); manual
      backups unaffected. `flux pause [--for 2h | --tomorrow]`, `flux resume`, a line in
      `flux status`; `Call::SetPause` (`hour`, `tomorrow` = 7:00, `resumed`, `off`); in the
      app: a pause dialog from any backup's menu (Resume when paused), Preferences, the
      tray (three items; the window is told with `flux://pause`), a sidebar banner with
      Resume, and "Next backup: Paused" on the Overview
- [x] Skip on metered connections (`schedule.skip_on_metered`, SSH targets only):
      NetworkManager's `Metered` property via `busctl` (yes / guessed yes); unknown counts
      as not metered
- [x] Upkeep after scheduled backups (`[maintenance]`, `flux_runner::maintenance`): a full
      verification about monthly (opt-in), and for Vault deleting unused data about weekly
      (on by default: backups only forget old snapshots). The first time starts the clock;
      times kept in `state/maintenance/<id>.json`; results in the run's log; problems and
      failures notified; never fails the backup
- [x] App preferences (`settings.toml` in the config folder; `Call::Preferences`,
      `SavePreferences`): notify on successful scheduled backups, the exclusions new
      profiles start with (Flux's list until changed, with "Back to Flux's list"). In the
      app also: start in the tray at login (an XDG autostart entry running the app with
      `--hidden`; the window starts hidden and is shown unless `--hidden` and a tray
      exist; uses `$APPIMAGE` when set), and the theme. A Preferences page at the bottom of
      the sidebar
- [x] Fixed a race found by the new tests: a job lock just released could look taken while
      a process forked by another thread still shared the descriptor (until its exec);
      `try_lock` now looks again for up to 0.2 s before calling the job busy
- [x] Tests: metered parsing, settings and pause round trip (expiry), tomorrow 7:00,
      upkeep schedule, paused scheduled runs skipped (manual runs not), weekly Vault upkeep
      after a real scheduled backup (logged, clock restarted), preferences and default
      exclusions through the service; CLI pause/resume by hand; GUI screenshots
      (Preferences while paused, pause dialog, schedule page)

Notes / follow-ups:
- Pause applies to your own backups; system backups (root's timers) have their own pause
  (`sudo flux --system pause`).
- Start at login inside Flatpak would need the background portal; the autostart entry
  covers native packages and AppImage.

### Phase 21 — Robustness (2026-11-08)
- [x] Sparse files stay sparse: after the reflink attempt, a file with fewer blocks than
      its size is copied region by region (`SEEK_DATA`/`SEEK_HOLE`, `copy_file_range` with
      offsets or `pread`/`pwrite`), then its length set so a trailing hole stays a hole.
      Covers Mirror backups and restores (both go through `Copier`); progress still counts
      the whole size. Falls back to the plain copy when the filesystem can't say where the
      data is. Tested on tmpfs and Btrfs (64 MiB image with 8 KiB of data: copy uses
      < 4 MiB, identical contents)
- [x] Snapshot index (`flux_runner::index`): one file per snapshot (Mirror manifest format,
      0600 in a 0700 folder under the cache) written on first use and reused by search and
      Versions for Vault, Borg, rsync and remote Mirror; keyed by snapshot id and start time;
      files of vanished snapshots dropped on each search; removed with the profile. Local
      Mirror reads its manifest directly
- [x] Flaky tests fixed. `flux-transport` `lock::tests::shared_and_exclusive` (and any test
      using the fake ssh): "Text file busy" when another test thread forked while the
      script was open for writing; the script is now created by `install` in a process of
      its own. Same fix for the CLI tests' fake `systemctl`. (Phase 20 fixed the job-lock
      race.) Full suite run three times in a row: all 26 test binaries pass each time
- [x] `flux find`: plain text matches part of a name in any case, like the app's search
      box (`flux find home notes` found nothing before: it was an exact glob)
- [x] Real desktop app: built with the bundled frontend and run on a private GTK Broadway
      display and a private D-Bus session (nothing on the desktop, no tray icon in the
      real session), against a separate `FLUX_HOME` with real backups. It starts, the tray
      code runs, and every view loaded real data through the backend (runs, snapshots,
      free space, the forecast once there was history). Found and fixed: the Space card
      showed (empty) after two backups minutes apart; it now waits for a forecast
- [ ] Visual check of the real WebKitGTK window: not possible here. Under Broadway,
      WebKitGTK 2.54 computes lengths wrongly even for a bare test page (an inline 16px
      font renders tiny, a 244px width computes to 0; also in MiniBrowser, and with
      GDK_SCALE and a 96 DPI settings.ini), so the layout it draws says nothing about a
      normal Wayland/X11 session. Needs a look on a real desktop

Notes / follow-ups:
- Vault restores still write zero runs as data (Vault stores no holes); only Mirror copies
  are sparse.
- The index is built lazily at the first search; building it after each backup would make
  the first search fast too.

### Phase 22 — Release 0.2 (2026-11-08)
- [x] Version 0.2.0: Cargo workspace (all crates, `flux --version`), `app/package.json` and
      its lock file, `tauri.conf.json`, the AppStream metainfo (a 0.2.0 release entry;
      `appstreamcli validate` passes), README status, the website's version tags, the demo
      diagnostics, the release workflow's example tag
- [x] `CHANGELOG.md` (Keep a Changelog): everything since 0.1.0 — compatibility mode,
      export/import, the UX rework, finding files, space, control, health and diagnostics,
      robustness, the fixes, and known limitations
- [ ] Tag `v0.2.0`: left for the maintainer (pushing the tag starts the release workflow)

### Phase 23 — Website, guide and README for 0.2 (2026-11-08)
- [x] New screenshots (demo build, 1200×780, `docs/screenshots/` and
      `site/assets/screenshots/`): the eight existing ones retaken (now with Preferences in
      the sidebar), and eight new: search, versions, restore preview, space forecast,
      health warnings, preferences (paused), export, new backup with its size estimate
- [x] Website: feature cards rewritten for 0.2 (search, versions, restore preview; pause and
      metered; warnings in words and the forecast), six new gallery figures, export to other
      tools in "Without Flux", the CLI sample and checklist; alt text matched to the new
      hero shot
- [x] HTML guide synced with the Markdown guide: "New backup", the size estimate, the
      sidebar menu, metered/pause/upkeep/Preferences, restore preview, open a copy, search,
      versions and the index, health warnings, space, duplicate, diagnostics, and the new
      profile keys; four screenshots in the text (`.guide-shot`)
- [x] README: twelve-picture gallery, app features for 0.2, CLI examples; AppStream gets two
      more screenshots (validates)
- [x] Fixed on the way: search results squeezed file names to nothing at the default window
      size (the size/date and versions columns now show only on wide windows, as a second
      line otherwise); the guide scrolled sideways on phones (a bare code block; now every
      code block in the guide scrolls on its own). Checked: no horizontal overflow at
      500/820/1100/1440 px on both pages, no broken images
- [x] Profile reference (Markdown and HTML) lists `skip_on_metered` and `[maintenance]`

### Phase 24 — GUI overhaul 1: foundation, shell, All backups (2026-11-08)
The app's look is replaced (it read as one grey blob: every item the same rounded box,
blue meaning selected, primary and active at once, setup like a web form, and no answer to
"am I protected?"). New direction, chosen on the Impeccable decision page: **"At home on the
desktop"**, the GNOME/Adwaita screen tradition in graphite and calm blue
(`PRODUCT.md`, `.impeccable/surfaces/app-src-app-svelte.md`). Plan: phase 24 foundation and
shell, 25 a backup's pages, 26 new backup and settings, 27 dialogs, preferences, review and
DESIGN.md.
- [x] Tokens and components (`app/src/app.css`): a blue-graphite shell around a flatter view;
      grouped lists with hairline separators; colour with one job each (blue = act,
      green/amber/red = state, selection = a neutral lift: `bg-selected` replaces the blue
      `bg-primary-container` everywhere); 8px-radius buttons, Adwaita switch, state glyphs;
      AA contrast checked in both themes
- [x] Type: Inter (the face Adwaita Sans is built on) and JetBrains Mono, bundled; IBM Plex
      removed
- [x] Shell: the brand moves into the titlebar; GNOME-style round window buttons; the view
      fills the window edge with one rounded corner
- [x] Sidebar: **All backups** first, then each backup with a state glyph and its state in
      words ("Protected · 59 minutes ago", "Failed 2 days ago"); a failure stays marked with
      a dot until its backup is opened (remembered on this computer); the theme switch moves
      to Preferences only
- [x] **All backups** (new start page): one sentence ("4 of 5 backups are protected."), what
      needs you and why in plain words, Pause/Resume schedules and Back up all now; each
      backup as a row with its destination, state, next backup and a 30-day history strip
      (one mark per day on a fixed size ramp: bigger marks copied more; red = failed)
- [x] `lib/protection.ts`: one answer to "is this backup protecting you?" shared by the
      sidebar and All backups
- [x] Backend: `status` call (next run as a timestamp, last good copy, health in words, 30
      days of history) read from the run log and timer only: cheap for every backup at once
      and never asks for a password; demo data for it; unit test for the day buckets
- [x] Shortcuts: Ctrl+0 opens All backups
- [ ] DESIGN.md still describes Graphite Tonal; it is rewritten from the finished build in
      phase 27

### Phase 25 — GUI overhaul 2: a backup's pages (2026-11-08)
- [x] Header: name, descriptor tags, and Settings · ⋯ · **Back up now** (the one blue
      button; Stop while running). Overview, Snapshots and Activity are a segmented view
      switcher with icons; **Settings is no longer a tab** but its own page with
      "‹ Home folder" to go back (Ctrl+4 still opens it)
- [x] Overview leads with the same answer as All backups, for this backup ("Home folder is
      protected." / "Photo library isn't protecting you.") with last backup, next backup and
      snapshot count in one line, what's wrong in words, then the 30-day strip spread across
      the page (first day and today named). The four stat boxes are gone
- [x] Recent backups, health notes, "Usable without Flux" and Maintenance are grouped lists;
      maintenance is three rows (what each does, and its button)
- [x] Snapshots: month headings, and a mark per snapshot on the history strip's size ramp
      (how much it copied or stored; amber when it finished with problems)
- [x] Activity: All / Problems as a segmented control; list rows match the snapshot list
- [x] A running backup's panel and the "settings have problems" notice restyled

### Phase 26 — GUI overhaul 3: new backup and settings (2026-11-08)
- [x] The editor is a GNOME preferences page: a group title over one grouped list per
      question, every setting a row. No more boxes holding headings holding another list
- [x] What to back up: each folder a row (remove on the right), a last row to type or
      Browse… for another, and "Leave some things out" in the same group; the size estimate
      under it
- [x] Where to keep it: drive or server as a segmented control; drives are radio rows (name,
      free space, mount point, a check on the chosen one), then "Another folder…"; a server
      is entry rows with Test connection in the group; then encryption and storage
- [x] When: a segmented control (every hour / day / week / only when I click), a Time row,
      plug-in and more options as rows
- [x] A new backup is named after what it backs up ("Home folder", "Documents and 1 more")
      until you type a name; the name comes after the three questions when creating and
      first when editing
- [x] Settings pages: How backups are stored is radio rows in two groups; exclusions and
      marker files are grouped lists; suggestion chips and other leftovers restyled
- [x] Missing details on a new backup (no drive, no server yet) read as the next step,
      not in red
- [x] Footer bar: a hairline above, Cancel and the one blue Create backup / Save

### Phase 27 — GUI overhaul 4: dialogs, menus, preferences (2026-11-08)
- [x] Dialogs: 17px semibold titles, a soft shadow and edge, 8/12px shapes; the close button
      is last in the DOM, so a dialog opens with focus on its first real control (Cancel
      on a destructive one, never Close or Delete)
- [x] Icon tiles in dialogs are neutral (blue only acts); code blocks and boxes on the
      control tone; the recovery kit's Guide/Script switch and Compare's filters restyled
- [x] Menus and toasts: tighter, squarer items; an edge in the light theme
- [x] Preferences: the same centred preferences column as Settings; theme as a segmented
      control; default exclusions as a grouped list; the paused state with a state glyph
- [x] Welcome page heading and buttons in the new type scale

### Phase 28 — GUI overhaul 5: finish review, DESIGN.md (2026-11-08)
- [x] Independent finish review (Impeccable finish reviewer) of the whole overhaul against
      the direction contract, at 1440, 1200 (the default window) and 900 (the minimum), both
      themes. Verdict: fix; two fix rounds, every scored fix and regression resolved
- [x] History strip: a continuous log size ramp, 4px (≤1 MiB) to 11px (≥10 GiB) in 13px
      cells, so days differ visibly and never touch; empty days ≥3:1; the snapshot list uses
      the same ramp. The strip stays at the minimum window size (on its own line)
- [x] All backups rows: name / state (and "last good copy …" when narrow) / mono
      "source → destination"; a "Last good copy · Next" column; the backend profile summary
      now sends `sources` (home as `~`)
- [x] One answer everywhere: `nextRun()` words the next backup the same on every page; a
      failure outranks a running backup in the sidebar; one hue per fact; no blue checks
- [x] Drawn help icon in the titlebar; snapshot times, sizes and dates in tabular mono
- [x] Demo data: day volumes spread from MiB to GiB, read from the same stats as the lists
- [x] `docs/DESIGN.md` rewritten for "At home on the desktop" (merged: the icon, tray,
      packaging and window sections kept and corrected), with `.impeccable/design.json`;
      `docs/UX_PRINCIPLES.md` gains rules 9–11 (using vs configuring, one colour one job,
      answer first) and the new building blocks
- [x] Found while documenting: light-theme `primary` (now `#2759a4`) and `fg-muted` (now
      `#555c66`) were just under AA on the shell and the selected lift; the window's
      background colour still had the old shell value; unused `--c-tag`, `pill` and
      `.btn-tonal` removed
- [ ] Not done: the README/website screenshots (`docs/screenshots/`) still show the old look;
      the overhaul has only been checked in the demo build, not yet in the Tauri app on
      real profiles

### Phase 29 — The password is chosen during setup, not at the first backup (2026-11-08)
Asking for a new password at the first backup was confusing ("did I set one?") and broke
scheduled backups: the first run of an encrypted backup needed someone at the screen.
- [x] Backend `set_password` (and `flux key init <profile>`): creates the encrypted Vault or
      Borg repository right away with the chosen password (or checks that it opens the one
      already there), then keeps it where the profile's password comes from: the desktop
      keyring, or a key file that doesn't exist yet (written mode 0600). File and command
      sources need nothing typed. The app also remembers it for the session
- [x] New backup: turning on *Encrypt with a password* shows Password and Repeat it right
      under the switch, with what happens to it in words (keyring, asked every time, key
      file); *Create backup* waits until they're set and match, then creates the encrypted
      repository. If that can't happen yet (drive not connected, server unreachable), the
      backup is saved and says so; the first backup asks as before
- [x] Settings: the same fields, optional, for an encrypted backup that hasn't run yet
      (e.g. one created before this change) or whose encryption was just turned on
- [x] Tests: the service creates the Vault and the key file at setup and refuses a wrong
      password for an existing Vault; end to end, `flux key init` sets up a Borg repository
      and the first backup then runs with no terminal, no FLUX_PASSWORD and no keyring
- [x] README, user guide (Markdown and HTML): set the password at setup; `flux key init`

### Phase 30 — New backup as a step-by-step wizard (2026-11-08)
- [x] A new backup is a guided setup, one question per step, with a stepper along the top
      (blue only on the current step; done steps get a check and can be revisited):
      **What** (folders, exclusions, size estimate) → **Where** (drives, a folder or a
      server) → **Protection** (plain folders, encrypted Vault, rsync or Borg, and the
      password right there when it's encrypted) → **When** (how often, time, plug-in, what
      to keep) → **Review** (name, every answer with a link back to change it, optional
      hooks and performance)
- [x] Each step asks a question in words and explains the choice in one sentence; Continue
      stays off until the step is complete and the footer says what's missing ("To
      continue: choose a drive or folder", "type the server's name", "choose a password");
      Back, Cancel; a short slide in the direction of travel (off under reduced motion);
      the step heading takes focus for screen readers
- [x] Ends on its own screen: "“Home folder” is set up.", when it runs, what and where,
      whether encryption is ready, and **Back up now** / **Later**
- [x] The questions are shared snippets, so a backup's Settings keep the one-page editor
- [x] Guide (Markdown and HTML), website, README and UX principles (rule 12) describe it

### Phase 31 — Settings groups open in a dialog, not a page (2026-11-08)
A row like *Leave some things out* used to swap the whole editor for that group, with
"‹ All settings" to go back. The swapped page looked like the one it replaced, so people
lost track of where they were.
- [x] Each group (exclusions, how backups are stored, connection, more schedule options,
      keep and remove snapshots, hooks, performance) opens in a dialog over the editor: its
      title, one line on what it's for, its settings, and a footer pinned to the bottom
      ("Saved when you click Save." / "Applies to this new backup.", **Done**). Escape or ×
      also close it; the editor and the wizard step stay exactly where they were
- [x] Docs: UX principles rule 7, user guide (Markdown and HTML)

### Phase 32 — "Where it's kept" in place (2026-11-08)
- [x] In the new-backup wizard's Protection step, "Where it's kept" is a dropdown in its own
      row (desktop keyring, ask every time, a file, a command) instead of opening the whole
      "How backups are stored" dialog again; choosing a file or a command adds the key-file
      path or the command as a row right under it, with a line on what Flux does with it
- [x] The group stays when the password comes from a command (only the password fields
      hide), and Continue waits for the key file or the command ("To continue: choose the
      key file" / "type the command that prints the password")

### Phase 33 — Performance 1: the app's own calls (2026-11-08)
From the performance review (issues 1, 2 and 9).
- [x] Run history read from its end: `recent_runs(n)` reads `runs.jsonl` backwards in 64 KiB
      blocks and stops after `n` records (the sidebar asks for 1 per backup on every
      refresh; it used to parse up to 1,200 records, hook output included). The Overview
      reads it once instead of twice. Test: lines across block boundaries
- [x] Opening an encrypted Vault again in the same process skips Argon2id (0.2–0.5 s of CPU
      per key slot tried): unlocked master keys are kept in memory, found by a keyed hash
      of the password (never the password), and only while their key slot still exists.
      Test: right passwords reopen, a wrong one is refused, a removed one stops working
- [x] Borg: the app's service reuses its archive listing of a local repository while
      borg's transaction (`index.N`) is unchanged, for the same passphrase, in memory only
      (a wrong passphrase is never answered from it; the command line always asks borg).
      Checking a passphrase (unlock, `set_password`, `flux key store`) always runs borg
      (`verify`). Tests: cache hits only for the same repository, transaction and
      passphrase; archives written by borg directly show up; a wrong passphrase fails
- [x] The Overview merges the reloads a finished backup triggers (the revision bump and the
      refreshed backup list) into one, and never loads twice at once (`lib/coalesce.ts`)
- [x] Timer status in one `systemctl show` per backup instead of four (unix timestamps,
      formatted by Flux for people). Test: parsing `systemctl show`
- [x] Correction to the review: `runs.jsonl` was already capped (1,000–1,200 records); the
      cost was parsing all of it for every call

### Phase 34 — Performance 2: disk writes (2026-11-08)
From the performance review (issues 4 and 5).
- [x] Vault packs on a local disk are no longer synced one by one (two fsyncs per 16 MiB
      pack, roughly 128,000 for a 1 TB first backup): `Backend::write_pack` writes via a
      temporary file and a rename without syncing, and one `syncfs` (`flush_packs`) runs
      before the index that names the packs is written, in backups and in prune. The index
      stays the commit point: a crash before it leaves packs no index names, which prune
      already removes. Remote (SFTP) packs keep the server-side fsync
- [x] Spinning disks (`flux_fs::disk`): `queue/rotational` from sysfs, for the path's
      device, or (Btrfs and others report a virtual one) the device its mount comes from
      in `/proc/self/mountinfo`. When a source or the target is on one, Mirror copies and
      Vault reads large files (4 MiB and up) two at a time; small files, links and metadata
      stay fully parallel. The run's log says so. Tests: mountinfo (longest mount point,
      escaped spaces), sysfs (a device, or a partition's disk), the gate (at most two large
      files at once; small ones never wait)

### Phase 35 — Performance 3: Vault on many cores (2026-11-08)
From the performance review (issue 3, and the dedup lock from issue 13).
- [x] Files larger than one chunk (8 MiB) are read in 16 MiB batches: while the pool hashes,
      compresses and encrypts one batch in parallel, the reading thread fetches the next
      (`rayon::join`). Chunk order is kept; memory per large file stays about two batches.
      Measured on 16 cores, 1 GiB file to an encrypted Vault: 3.26 s → 2.08 s
      (incompressible), 3.25 s → 1.81 s (compressible); peak memory 95 → 220 MB
- [x] The run's "already stored" set is split into 64 shards by blob id, so threads claiming
      chunks rarely wait on one lock
- [x] Test: a 70 MiB file spanning several batches, plus an identical copy, restores byte for
      byte, is stored once, and backing up the same content again stores almost nothing

### Phase 36 — Performance 4: Vault memory and unchanged runs (2026-11-08)
From the performance review (issues 6 and 10).
- [x] A Vault backup counts its changes with `flux_fs::diff::summarize` over borrowed
      entries: no copy of every entry on both sides, and no list of every change (a first
      backup used to build one for every file just to count them). Same rules as `diff`
      (test: equal summaries)
- [x] The latest snapshot's file list is kept in Flux's cache (`~/.cache/flux-backup/vault`),
      compressed and encrypted with the Vault's own key, and used by the next backup instead
      of rebuilding it from the repository's trees, only when it belongs to exactly that
      snapshot (its root tree id); anything else is ignored and rebuilt. 100,000 files,
      nothing changed: 0.30 s → 0.26 s. Test: the cached list equals `flatten`, unchanged
      and changed runs are right, a damaged cache is ignored
- [x] Found on the way: `deleting_snapshots_frees_their_data` compared two snapshots that
      record `/tmp`'s metadata (a source's parent folders are part of a snapshot), which
      parallel tests change. It now runs under a parent folder of its own

### Phase 37 — Performance 5: Mirror and per-file work (2026-11-08)
From the performance review (issues 7, 11, 12 and the rest of 13).
- [x] Mirror creates directories one depth level at a time (parents first) and sets their
      metadata deepest level first, each level in parallel (both were one sequential loop)
- [x] A copied file's owner, permissions, extended attributes and mtime are set through the
      file Mirror just wrote (`apply_metadata_fd`): no path lookups per call
- [x] Progress counters: the scan adds per directory, Mirror per 256 entries, instead of
      sixteen threads bumping one counter per file
- [x] The job watcher sleeps on inotify (wakes as a job file appears, re-checks every 5 s)
      instead of reading the jobs folder every 0.7 s; without inotify, as before. Test:
      wakes on a new job file. Fixed on the way: its poll timeout dropped sub-second parts
- [x] The new-backup size estimate scans counting only (`count_profile`: no list of every
      file in memory), and picking a destination recounts only when that destination lies
      inside a source (a `$derived` key). Test: counts equal a full scan's
- [x] The copier reads a file's metadata once instead of twice
- [ ] Not changed: the scan still lists extended attributes by path. An fd-relative call
      (`listxattrat`) needs Linux 6.13 and isn't in rustix; opening every file to use its
      descriptor would cost more than the path lookup it saves

### Phase 38 — Performance 6: browsing snapshots (2026-11-08)
From the performance review (issue 8). With this, all 13 issues are done (one part of 11
left as it was, see phase 37).
- [x] The Snapshots browser builds a folder index once, when a snapshot's entries are first
      loaded (each folder → its children); opening a folder is a lookup. It used to count
      every entry's parent and filter every entry again on each click
- [x] The cache of browsed snapshots is now really most-recently-used first (a reused
      snapshot moves to the front)
- [x] Test: for every folder, the index lists exactly what a full pass finds; files and
      entries under a non-folder are no folders
- [x] CHANGELOG: an Unreleased section for the GUI overhaul, the setup wizard, the setup-time
      password and the performance work

### Benchmarks re-run after the performance work (2026-11-08)
- [x] `bench/bench.sh` also compares Vault with Borg (repokey-blake2, zstd,3) when borg 1.x
      is installed, so every number on the site comes from one script; it now touches
      nothing outside its work folder (no systemd timers, keyring entries or notifications)
- [x] Median of three runs on tmpfs, 16 cores (rsync 3.5.2, Borg 1.4.5). Mirror: initial
      0.46 s (was 0.6), no changes with a new snapshot 0.26 s (was 0.37), 1% changed
      0.26 s (was 0.37). Vault: initial 1.73 s (was 1.8), no changes 0.51 s, 1% changed
      0.51 s (was 0.6–1.0); repository 864 MiB. rsync and Borg were re-measured too
      (rsync 7.7 s where 0.2 measured 9.1 s), so the comparisons use today's numbers for both
- [x] Website, README and PRODUCT.md updated to match

### Release 0.2.0 (2026-11-08)
- [x] CHANGELOG: the Unreleased section folded into 0.2.0 (the new look, the setup guide,
      the password at setup, settings in dialogs, the performance work)
- [x] All 16 screenshots retaken in the new look (demo data, 1200 × 780), in
      `docs/screenshots/` and `site/assets/screenshots/`; alt text and captions on the site
      and README updated to what they now show
- [x] AppStream 0.2.0 release notes cover the new look, setup and speed; validated
- [x] Published from `../FluxBackup-public` as tag `v0.2.0`, release built by the Release
      workflow
- [x] Pushed over HTTPS with `gh` as the git credential helper (the router blocks SSH to
      GitHub); `origin` in the public repo still points at the SSH URL
- [x] Published https://github.com/neuralpunk/flux-backup/releases/tag/v0.2.0 with
      hand-written notes; the four checksums match GitHub's own digests
- [x] `SHA256SUMS` listed build paths (`target/release/bundle/deb/…`), so `sha256sum -c`
      failed next to the downloads (0.1.0 too). The 0.2.0 file was replaced with bare
      names; the Release workflow now writes bare names (tested locally). Not yet in the
      public repo: it goes out with the next sync

### Website on GitHub Pages (2026-11-08)
- [x] `.github/workflows/pages.yml` publishes `site/` to https://neuralpunk.github.io/flux-backup/
      on pushes to main that touch the site (or by hand); Pages turned on for the repo
- [x] README links the website and the latest release near the top; the repo's About box
      links the website

### Phase 39 — 0.3: Restore, compare and the size limit (2026-11-08)
Reported: after deleting a folder and a file, "Restore this folder" brought back the
folder with nothing in it, and not the file; comparing showed only a folder change and
"0 bytes". Cause: the profile's size limit was `1` (a bare number is bytes), so the backup
held folders and no files. The engines restore correctly (checked with rsync and Mirror).
- [x] "Skip files larger than" is a number plus a unit (MB, GB, TB; another unit saved
      earlier keeps it, so nothing is rounded); under 1 MB it says "That leaves out almost
      every file"
- [x] Validation warns about limits under 1 MiB (`TINY_SIZE_LIMIT`); test
- [x] `ByteSize` prints the unit, binary or decimal, with the smallest exact number
      ("500MB", not "500000000B"); still round-trips; test
- [x] rsync: `--max-size` skips still counted in rsync's statistics, so a snapshot of empty
      folders said "21 files, 252 MiB". With a size limit the snapshot is now counted from
      what it holds, and the run says "left out N files larger than the size limit" (or
      "no files were backed up…"). CLI test
- [x] Notices: Snapshots says when a size limit is set (with "Change the limit", which opens
      that settings group); Overview warns only about a tiny one; profile summaries carry
      `size_limit` (not for Borg, which ignores it)
- [x] File browser: each row has its own Restore button; the selection stays while moving
      between folders ("N selected in other folders"); items no longer on disk are marked
      *Deleted* (`on_disk` in listings; test), with an "Only deleted" filter and "Restore
      N deleted items"; an empty folder says when the size limit may be why
- [x] Restore dialog: goes back to the original place by default when everything picked
      was deleted; "Show what will be added"; a warning when the snapshot has only empty
      folders there
- [x] Restore requests may name a folder and something inside it (now possible with the
      kept selection): the outermost decides where things go. Used to fail with "not in
      snapshot"; test
- [x] Compare with now: scans without the size limit, keeps big files the snapshot holds
      (they used to show as removed) and counts the rest (`left_out_by_size`), which the
      dialog explains. Borg compares big files too (it ignores the limit). Test
- [x] Compare dialog: folder-date-only changes hidden behind "N folder dates"; fields in
      words (contents, date, permissions…); size shown only when it changed; restore a
      removed file or the old version of a changed one from the row, or "Restore all
      removed"
- [x] USER_GUIDE: restoring single items and deleted ones, compare, the size limit

### Phase 40 — 0.3: Setup wizard and settings polish (2026-11-08)
- [x] "Often left out" suggestions that are added are green (the palette's success colour,
      with a tinted background and edge), not the neutral selection lift
- [x] Step 1's size count: a ↻ button counts again at once (after changing exclusions or
      files on disk; edits still wait 700 ms for more typing); the size and file count are
      in the brand blue so they stand out
- [x] `InfoDetails`: an information block (tinted, info icon, chevron), closed until
      clicked. Step 3's "What exactly Flux does" uses it
- [x] Step 4: "What happens each time it backs up, in plain words", built from the answers
      (when, what it copies and where, catching up and battery, what it keeps, how to get
      files back); closed by default
- [x] Performance and behaviour: "Recommended defaults" puts the page's settings back to
      a new profile's (fetched from the backend for the profile's scope); says "Using the
      recommended settings" when they already are
- [x] USER_GUIDE updated

### Phase 41 — 0.3: Import from Back In Time (2026-11-08)
- [x] `flux_compat::backintime`: reads `~/.config/backintime/config` (format 6, any number
      of profiles from `profiles=1:2:…`) into Flux profiles: folders and files,
      exclusions, size limit, one file system, local or SSH destination (port, user, key,
      bandwidth), schedule (every mode: daily/weekly/monthly at a time, custom hours,
      hourly and 5 min–12 h intervals, anacron-style "repeatedly", at boot, on connect),
      smart remove / remove older than / free space and inodes, and the options (ACLs,
      xattrs, checksums, links, notifications, keep files on restore, ionice)
- [x] rsync exclusion patterns become gitignore ones with the same meaning (`.cache/*` →
      `**/.cache/*`, `#*#` → `\#*#`, `***` → `**`); every change is listed in the notes
- [x] Engine: rsync (the same plain hard-linked folders); EncFS profiles become an
      encrypted Vault. Snapshot folder, count, newest and whether the drive is connected
      are reported; Back In Time's snapshots are never touched
- [x] Durations read as written ("2years", not 730 days shown as "1year 11months…")
- [x] Tests: a trimmed real config with a local and an SSH+EncFS profile, schedules, a
      missing file; the converted profile validates. Checked against the real config on
      the development machine (two profiles) with a scratch FLUX_HOME
- [x] Service `backintime_profiles` (user scope only); app: the wizard's first step
      offers each profile ("Use these settings", with folders → destination and its
      snapshots); after using one it says what came over, what happens to the old
      backups, and the notes, with "Start from scratch". A profile's own name is kept
- [x] The offer has a background of its own (a blue-tinted gradient with a blue edge),
      green-tinted once a profile is used; checked in dark and light
- [x] CLI: `flux profile backintime [NUMBER] [--id] [--engine]`
- [x] USER_GUIDE: "Coming from Back In Time?"

### Phase 42 — Release 0.3 (2026-11-08)
- [x] Version 0.3.0: Cargo workspace (`flux --version` says 0.3.0), `app/package.json` and
      its lock file, `tauri.conf.json`; Cargo.lock updated
- [x] CHANGELOG 0.3.0 (Added: Back In Time import, restoring, setup and settings;
      Changed; Fixed: rsync stats with a size limit, compare and the size limit, nested
      restore paths); compare link
- [x] AppStream 0.3.0 release notes; `appstreamcli validate` passes
- [x] README status says 0.3
- [ ] Not done here: website text ("Version 0.2" tags on the site), new screenshots, tag
      `v0.3.0` and the public sync; the release itself is the maintainer's call

### Phase 43 — Unplugged drive and stuck "Backing up" (2026-11-08)
Reported: an imported Back In Time profile pointing at an unmounted `/mnt/external_backup`
showed "Backing up…" forever; Stop said "no backup of back-in-time is running"; only a
restart showed the failure. Cause: the `flux backup` child failed in its first
milliseconds (target check), so its status socket was gone before the job watcher
connected; the app had marked the profile running and nothing ever unmarked it.
- [x] `backup_now` checks a local target first (`not_connected`, in words: "external_backup
      isn't connected: nothing is mounted at /mnt/external_backup. Plug in the drive, then
      back up again."; another drive mounted there; a file, not a folder)
- [x] `backup_now` waits (up to 5 s) until the job is registered or has ended; a run that
      ended at once returns its error from the run log (or stderr) as `backup_failed`, or
      `{running: false}`; stderr is read concurrently
- [x] `cancel` with nothing running: `not_running`; the app then unmarks it, refreshes, and
      says it had already stopped
- [x] App safety net: a started backup with no `hello` within 4 s is re-checked with the
      backend instead of spinning
- [x] Validation warns (`target.connected`) when the drive isn't plugged in; shown on the
      destination step, and the wizard's last step offers Done instead of Back up now
- [x] Test: failing start, quick success, Stop with nothing running, unplugged drive
      refused, validation warning. Demo: Photo library (Archive unplugged) shows the toast
- [x] CHANGELOG 0.3.0 Fixed, AppStream 0.3.0 note, USER_GUIDE "An external drive"

### Phase 44 — cron as an alternative scheduler: backend and CLI (2026-11-09)
Asked: an advanced setting to run scheduled backups with systemd or crontab, systemd the
default.
- [x] `calendar.rs`: systemd calendar expressions read by Flux (shorthands, weekdays,
      `[Y-]M-D`, `H:M[:S]`, lists, `a..b` ranges, `a/n` steps; refuses what systemd
      refuses: backwards weekday ranges, `*/n`; no time zones or `~`); next run in a
      zone (DST-safe); the cron fields for it. Cross-checked with
      `systemd-analyze calendar --iterations=5 --base-time` for 18 expressions × 3 bases
- [x] `cron.rs`: a block in the user's crontab (`crontab -l` / `crontab -`) or
      `/etc/cron.d/flux-backup` for system profiles; lines outside it untouched. Lines run
      `flux backup <id> --scheduled --if-due` with `XDG_RUNTIME_DIR` and the session bus
      (keyring, notifications), `%` escaped. Calendar: its times plus `*/15` catch-up
      when `catch_up`; interval: the timer's check period. Status: next run computed by
      Flux, last check
- [x] `--if-due` on calendar schedules: due when one of its times came since the last
      check (`<state>/schedule/<id>.last`, set when the schedule is installed); with
      catch-up off, a run over 10 minutes late is skipped; at most once per time
- [x] `Scheduler` (systemd or cron, from `settings.toml` `scheduler = "cron"`) used by
      the CLI and the service; `schedule::switch` sets the new one up first, undoes it on
      failure, then removes the old entries and saves the choice. Saving other
      preferences keeps it
- [x] `on_connect` needs systemd: said by `flux schedule install/use` and on save
- [x] Paths: an `XDG_RUNTIME_DIR` that doesn't exist (cron while logged out) is ignored
- [x] CLI: `flux schedule use systemd|cron`; `flux schedule status` names the scheduler
      (CRON column), hints when no cron daemon runs
- [x] Tests: calendar (forms, steps, leap day, bad input, cron fields, zones, systemd
      cross-check), crontab block round trip keeping other lines, lines per schedule kind,
      quoting; CLI: switch to cron and back with Back In Time's line kept, not due, missed
      run caught up once, catch-up off. CLI tests point `FLUX_CRONTAB`/`FLUX_CRON_DIR`
      into the temp dir so the real crontab is never touched. A generated table passes
      cronie's `crontab -T`
- [x] USER_GUIDE "systemd timers or cron"
- [x] Phase 45 follows: the app, service calls, CHANGELOG

### Phase 45 — cron as an alternative scheduler: the app (2026-11-09)
- [x] Service: `preferences` reports `scheduler`, `cron_problem` (why cron can't be used,
      in words) and `cron_running`; `set_scheduler` moves every valid profile's schedule
      (`schedule::switch`); validation warns on `schedule.on_connect` with cron
- [x] Tauri command `set_scheduler`; `api.setScheduler`; demo (`?cron=0`: not installed)
- [x] Preferences › Advanced › "What runs scheduled backups": systemd timers
      (recommended) or cron, each with what it does and doesn't; cron disabled with the
      reason when it isn't installed, a warning when no daemon runs; a spinner while
      switching and a toast after; a note that system backups switch with
      `sudo flux --system schedule use cron`. Checked in the demo (both states)
- [x] Settings: "Also when this drive is plugged in" is disabled with cron (unless already
      on, so it can be turned off), with the reason; its warning shows in warning colour
- [x] Test: the preference, its warning, and saving other preferences keeping it
- [x] CHANGELOG 0.3.0 "Scheduling with cron", AppStream note, USER_GUIDE points at the
      preference

### Phase 46 — Back In Time's rsync options; one backup at a time (2026-11-09)
Asked: the features Back In Time has that Flux lacked, in phases (46: these two; 47 exact
restore; 48 shut down afterwards; 49 compare versions in a diff tool).
- [x] Import: `snapshots.rsync_options.value` is split like a shell would and becomes
      `[rsync] extra_args`; arguments Flux reserves (`RSYNC_RESERVED_ARGS`) or that
      aren't options are listed in the notes; EncFS profiles (Vault) say they're unused.
      Test, including the converted profile validating
- [x] `one_at_a_time` in `settings.toml`: after the job is registered (so it shows and can
      be stopped), a backup takes the scope's turn (`jobs/turn.lock`, flock); while
      another holds it, the new phase `Waiting` and an event naming the backup it waits
      for; stopped while waiting ends as cancelled. Dry runs don't wait. Test: waits,
      then runs; stopped while waiting; off means no waiting
- [x] Service: `preferences.one_at_a_time`; `save_preferences` takes it (left out:
      unchanged), keeping the scheduler. Test
- [x] App: Preferences › Advanced › "Run one backup at a time"; sidebar and All backups
      say "Waiting for its turn…"; the job panel says what it waits for. Checked in the
      demo
- [x] USER_GUIDE, CHANGELOG 0.3.0

### Phase 47 — Exact restore (2026-11-09)
- [x] `RestoreRequest.exact: Option<&Filter>`: after files are restored and before folder
      dates are set, each restored folder is listed through a read-only descriptor; what
      the snapshot doesn't have is removed with the existing set-aside rule (renamed
      `name.flux-<snapshot>` with keep_existing, otherwise deleted). Left alone: what the
      profile's filter excludes (patterns, hard excludes), files over the size limit,
      contents of marker/cache folders, anything on another filesystem, what this or an
      earlier restore set aside (`.flux-` in the name). `RestoreOutcome.removed`
- [x] `restore::preview` with `exact`: `remove` and the first paths (`remove_paths`)
- [x] `flux_fs::scan::exact_filter(profile)`: the profile's filter plus a local backup
      drive. Service `restore`/`restore_preview` take `exact` (default false); Tauri too
- [x] CLI `flux restore --exact`: lists and asks before removing (unless `-y`), dry run
      lists `remove …`; says how many were removed
- [x] App: "Make the folders exactly as they were" under "Back to where it was"; the
      preview says how many go and how (moved aside or deleted) and lists them; the button
      says "Restore and remove N items" (danger style when deleting). Checked in the demo
- [x] Tests: service (Mirror) preview counts, set aside by default with excluded and
      too-large files untouched, kept copies untouched on a second run; CLI dry run and
      `--exact --overwrite -y`
- [x] USER_GUIDE "Exactly as it was", CHANGELOG 0.3.0

### Phase 48 — Shut down when done (2026-11-09)
- [x] `BackupJob.shutdown_after`; the job socket takes `{"cmd":"shutdown_after","on":…}`
      and reports it in every `progress` message; `finished` says `shutdown: true` when one
      was scheduled. At the end, unless stopped or a dry run: `shutdown --poweroff +1`
      (`desktop::schedule_poweroff`, `FLUX_SHUTDOWN` overrides the program), an event and
      a notification; if refused, a warning and no shutdown
- [x] CLI `flux backup --shutdown`; service `backup_now { shutdown }` (passes the flag),
      `shutdown_after { id, on }`, `cancel_shutdown` (`shutdown -c`); Tauri commands
- [x] App: "Back up now, then shut down" in the profile menu; "Shut down the computer when
      done" under a running backup; a countdown banner with Cancel shutdown when it's
      scheduled. Checked in the demo (menu, running, countdown)
- [x] Test (CLI, fake `shutdown`): `--shutdown` schedules `--poweroff +1`, nothing without
      it, a refused shutdown is a warning. Every CLI test points `FLUX_SHUTDOWN` at
      `false`, so tests never shut a machine down. The real `shutdown` wasn't run here
- [x] USER_GUIDE, CHANGELOG 0.3.0

### Phase 49 — Compare file versions (2026-11-09)
- [x] `filediff.rs`: compare two local files; text (no NUL near the start, UTF-8, up to
      8 MB each) line by line with `similar` (3 lines of context, unchanged runs as `skip`
      with a count, at most 5000 lines then counted); binary and too-large ones say only
      whether they're identical. `diff_file_impl` compares a snapshot's copy (shared
      `extras::copy_out`, read-only, in the cache) with another snapshot's or the file on
      disk; refused for system backups
- [x] Comparison programs: `diff_tool` in `settings.toml` (a command, `""` for none),
      else the first installed of meld, kompare, kdiff3, diffuse. Service `diff_file`,
      `diff_tool`, `set_diff_tool`; preferences list the installed ones and the one used.
      Tauri `open_diff_tool(left, right)` takes the program from the settings, never the
      page
- [x] App: Compare on each version; the dialog widens to a comparison with From/To
      pickers (versions, or "Now, on your computer"), +N −M, line numbers, colored
      removed/added lines, folded runs, Open in <program>; Preferences › Advanced ›
      "Compare file versions with". Checked in the demo
- [x] Tests: context and folding, identical, binary, trailing folded run; service:
      snapshot vs. disk identical, changed, file gone
- [x] USER_GUIDE, CHANGELOG 0.3.0

### Phase 50 — Storage meters (2026-11-09)
Asked mid-way through phase 49: a visual indicator of how much space a drive has.
- [x] `StorageMeter.svelte`: used against size, `role="meter"` with a spoken value; amber
      from 85% full, red from 95%, with the words saying so too (never color alone);
      full and compact (lists) forms
- [x] Overview › "Space on …": the meter on top; shown whenever the drive is connected
      (it used to wait for a forecast); the chart only once there are 3+ points
- [x] Setup and Settings › Where: each drive with its mount point and a compact meter
- [x] Demo: a nearly full USB stick. Checked in the demo (Overview, Where)
- [x] USER_GUIDE, CHANGELOG 0.3.0

### Phase 51 — Restored files no longer marked Deleted (2026-11-09)
Files just restored stayed marked Deleted in the snapshot browser: the service checks the
disk on every listing, but the app didn't list the folder again.
- [x] `app.diskRevision`, bumped when a restore ends (even with errors: some files may be
      written). The file browser lists the folder again quietly (rows stay on screen),
      search runs again, and a comparison with the files on disk compares again
- [x] File browser: a refresh button next to the filter (F5 or Ctrl+R; listed in the
      shortcuts); also re-checked when the window gets focus. Refreshes asked for say when
      they fail; automatic ones are silent and never move to another folder
- [x] Search: each hit carries `on_disk` (checked on every search); *Deleted* means not
      on the computer now, falling back to "not in the newest snapshot" when it can't be
      checked
- [x] Test: delete, listing and search say so, restore, both show it back. Demo: restores
      to the original place clear the mock's missing set; checked in the demo
- [x] USER_GUIDE, CHANGELOG 0.3.0

### Phase 52 — Style polish: bars for history, Adwaita colours (2026-11-09)
The row of history dots read as noise, and the look could sit closer to GNOME. Directions
were compared side by side in the real app (demo data, both themes); the owner chose bars
and the Adwaita palette.
- [x] History strip: one bar per day on the fixed log ramp (`lib/ramp.ts`, shared with the
      snapshot list); failed days a full red bar, empty days a 2px baseline stub; each
      day's whole column carries its title. A bar chart on a backup's page
- [x] Snapshot list marks are bars on the same ramp, standing on the text's baseline
- [x] Tokens: Adwaita neutral greys, shell lighter than the view in dark; `accent` /
      `on-accent` for the primary button and on switch (white text, 4.9:1 dark, 5.1:1
      light); `--group-shadow` gives light groups Adwaita's card shadow; dark `danger`
      brightened to #f39488 (4.8:1 on controls). Every text pair re-measured for AA
- [x] Recent backups rows: size moved next to the time instead of floating mid-row
- [x] Window background colour before first paint matches the new shell
- [x] All 16 screenshots retaken (demo data, 1200 × 780) in `docs/screenshots/` and
      `site/assets/screenshots/`; captions still match
- [x] DESIGN.md, CHANGELOG 0.3.0; svelte-check clean, build passes, design detector clean

### Release 0.3.0 (2026-11-09)
- [x] CHANGELOG and AppStream 0.3.0 dated 2026-11-09 (the fixes through Phase 51 and the
      new look landed today); AppStream notes mention the new look; `appstreamcli
      validate` passes
- [x] Website says 0.3 (hero badge, caution card, the guide's tag); new hero heading;
      screenshots retaken (Phase 52 and after)
- [x] `cargo test --locked` passes; svelte-check and the app build pass
- [x] Synced to `../FluxBackup-public` as one commit, "Flux Backup 0.3.0" (a mirror of this
      repo's tracked files, as for 0.1.0 and 0.2.0), tagged `v0.3.0`, pushed over HTTPS
      with `gh` as the credential helper; the Release workflow builds the draft release
