# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

A Linux desktop app: Tauri 2 shell with a Svelte 5 + Tailwind 4 UI rendered by WebKitGTK
4.1 (not Chromium), plus a static marketing site and user guide in `site/`. It follows
Linux desktop conventions (GNOME HIG boxed lists, polkit prompts, tray, systemd), not a
native toolkit.

## Users

**Both, GUI-first.** The primary user is a Linux desktop user who wants their files
backed up on a schedule and to get them back when something goes wrong. They may never
open a terminal, and many come from Back in Time. The desktop app has to work for them
with no reading first.

Technical users (homelabbers, sysadmins, people who script) are served by the `flux` CLI
at full parity with the app. When the two audiences pull in different directions, the
app's clarity for the non-technical user wins; the CLI is where depth and scriptability
live.

The jobs:
- set up a backup: what, where, when, and whether to encrypt;
- trust that it keeps running (schedules, warnings in words when it doesn't);
- find and restore a file or a whole folder, including an older version or a deleted file;
- back up the whole computer (system backups, via an administrator password).

## Product Purpose

Flux Backup makes snapshot backups for Linux. It keeps Back in Time's proven model
(profiles, scheduled snapshots, include/exclude lists, smart retention, easy restore),
rebuilt in Rust to be faster, more capable and nicer to use. Success means people set up
a backup in about a minute, stop thinking about it, and get their files back when they
need them.

## Positioning

All four claims are core and future work may lead with any of them:

1. **Backups never depend on Flux.** Mirror snapshots are plain folders browsable with any
   file manager; compatibility mode has the system `rsync` or `borg` write the backups in
   their own formats; a recovery kit (`FLUX-RECOVERY.txt`, `flux-backup.sh`) sits next to
   the data with the exact commands to restore without Flux.
2. **Speed.** Native Rust engines that use every core: measured seconds where rsync and
   Borg take minutes (see Evidence on Hand).
3. **Understandable without reading.** Warnings in words, previews before restores and
   retention changes, administrator prompts explained before they appear, settings that
   show their current value (docs/UX_PRINCIPLES.md).
4. **Trustworthy safety.** Crash-safe snapshots (built aside, renamed into place),
   "verify every byte", authenticated encryption, refusal to write to an unmounted drive's
   empty mount point, and a published threat model.

## Operating Context

- Runs on the user's own Linux desktop. Backups go to local or external (USB) drives or
  over SSH/SFTP.
- Schedules are systemd timers (user and system), so backups run with the app closed.
  "Back up now" starts `flux backup` as a separate process; the app follows any running
  backup live, whether scheduled, started from a terminal, or started in the app.
- A tray icon shows activity; the app can start hidden in the tray at login.
- System backups (`/etc`, `/root`, other users' files) go through a polkit-authorised
  helper. They lock again after 15 idle minutes.
- Passwords come from the desktop keyring, a prompt, a file or a command. Vault passwords
  typed into the app stay in memory only.

## Capabilities and Constraints

- **Engines, chosen per profile:** Mirror (hard-link snapshots, plain folders), Vault
  (content-defined dedup, zstd, XChaCha20-Poly1305 + Argon2id), and compatibility mode:
  rsync (`--link-dest` folders) or Borg 1.x (`repokey-blake2`).
- **App surfaces:** Overview (progress, last/next backup, free space, health warnings,
  space forecast, maintenance), Snapshots (instant file browser, what changed, find in all
  snapshots, versions of a file, restore with preview), Activity (run logs, Problems
  filter, Copy diagnostics), profile editor (three questions plus grouped settings
  sub-pages), Preferences (pause schedules, notifications, tray, theme, default
  exclusions), System backups, recovery guide, export to other tools.
- **The CLI and the app have parity:** anything the app does, `flux` does.
- **Rendering engine:** WebKitGTK/JavaScriptCore is stricter than Chromium (date parsing,
  for example). Verify behaviour in the real app, not only in a browser via `npm run dev`
  (which uses demo data).
- **Terminology:** users see "backups" (not "profiles") in the sidebar; "snapshots";
  "Vault"; "Mirror"; "Usable without Flux" for compatibility mode; "System backups".
  Profiles are TOML files, one per backup.
- **License:** MIT OR Apache-2.0. Clean-room: Back in Time (GPL-2.0+) was studied for
  behaviour only.
- **Distribution:** `.deb`, `.rpm` and AppImage, containing the app, `flux`, the helper,
  man pages and completions.
- **Status:** 0.2. The docs tell people to keep another copy of anything precious until
  they trust it.

## Brand Commitments

- Name: **Flux Backup**; the CLI is `flux`. Logo: `site/assets/logo.svg`.
- Voice: plain, concrete words; say what happens and what stays ("The backups themselves
  stay on BackupDisk"); warnings say what to do next; no jargon in the app where a plain
  word works.
- The app's visual system is "Graphite Tonal" (docs/DESIGN.md); how the interface behaves
  is in docs/UX_PRINCIPLES.md.

## Evidence on Hand

- **Benchmarks** (`bench/bench.sh`; 1 GiB = 100,000 small files + 20 × 20 MiB, tmpfs,
  16 cores; output checked byte for byte), from README.md:
  - Mirror vs `rsync -aHAX --link-dest`: initial 0.46 s vs 4.28 s; no changes (new
    snapshot) 0.26 s vs 7.67 s; 1% changed 0.26 s vs 7.73 s.
  - Vault vs Borg 1.4.5: initial 1.73 s vs 15.81 s; no changes (new snapshot) 0.51 s vs
    8.17 s; 1% changed 0.51 s vs 8.18 s; repository 864 MiB vs 869 MiB.
  - Median of three runs of `bench/bench.sh` (2026-11-08, after the performance work).
  - Stated caveat: much of the gap comes from using every core, so expect a smaller gap
    on spinning disks.
- **Screenshots** of every major app surface, dark and light: `docs/screenshots/`.
- **Docs:** README.md, docs/USER_GUIDE.md, docs/THREAT_MODEL.md, docs/VAULT_FORMAT.md,
  CHANGELOG.md, site/index.html, site/guide.html.
- **Absent, never to be fabricated:** user counts, testimonials, press, named customers,
  third-party audits of the encryption, or benchmarks beyond the ones above.

## Product Principles

1. **The user's data must outlive the tool.** Prefer formats and recovery paths that work
   without Flux; never trap backups in something only Flux can read without saying so.
2. **Clarity for the non-technical user wins conflicts.** Depth goes to the CLI and to
   progressive disclosure, never in front of the common path.
3. **Never lose or misrepresent data.** Safety checks, previews before destructive or
   overwriting actions, and honest status ("Not backed up yet", a failure in words with
   what to do next).
4. **Fast is a feature people feel.** Instant browsing and quick backups are part of the
   product, not a benchmark footnote.
5. **Say it in words.** Status, warnings and disabled controls explain themselves in place.

## Accessibility & Inclusion

Target: **WCAG 2.2 AA, in full**, in both dark and light themes:
- text and UI contrast of at least 4.5:1 (3:1 for large text and UI component edges);
- full keyboard operation, with visible focus; context menus are also reachable from the
  keyboard (Menu key, Shift+F10) and from on-screen buttons;
- screen-reader names, roles and states for every control and for live progress;
- reduced-motion support;
- nothing conveyed by colour alone (status is also in words).
