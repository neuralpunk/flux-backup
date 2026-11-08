// Demo backend for running the UI in a browser. Not bundled into the Tauri build's hot
// path: api.ts imports it lazily only when Tauri isn't present.

import type { DirEntry, Found, Issue, JobMessage, Listing, Overview, PreviewRow, Profile, ProfileSummary, RunRecord, SnapshotMeta, SnapshotStats } from "./types";
import { ApiError } from "./types";

const DAY = 86_400_000;
const now = Date.now();
const iso = (t: number) => new Date(t).toISOString();

function stats(files: number, bytes: number, changed: number): SnapshotStats {
  // Real days vary a lot (a few MiB to a few GiB): spread the sample data likewise.
  const spread = [0.002, 0.03, 0.4, 0.008, 1, 0.06, 0.15][changed % 7]!;
  return {
    entries: files + Math.round(files / 9),
    files,
    dirs: Math.round(files / 9),
    symlinks: 12,
    specials: 0,
    total_bytes: bytes,
    new: Math.round(changed * 0.4),
    changed: Math.round(changed * 0.6),
    unchanged: files - changed,
    deleted: Math.round(changed * 0.1),
    copied_bytes: Math.round(bytes * 0.004 * (1 + (changed % 7)) * spread),
    copied_files: changed,
    linked_files: files - changed,
    stored_bytes: Math.round(bytes * 0.002 * (1 + (changed % 5)) * spread),
  };
}

function snapshotSeries(profile: string, count: number, everyMs: number, vault: boolean, endAgo = 3_600_000): SnapshotMeta[] {
  const out: SnapshotMeta[] = [];
  for (let i = count - 1; i >= 0; i--) {
    const t = now - i * everyMs - endAgo;
    const d = new Date(t);
    const id = vault
      ? Math.floor(Math.abs(Math.sin(i + profile.length) * 1e16)).toString(16).padStart(16, "0").slice(0, 16)
      : `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}_${String(d.getHours()).padStart(2, "0")}-00-00`;
    const changed = 40 + ((i * 37) % 300);
    out.push({
      id,
      profile,
      host: "nyx",
      user: "me",
      started: iso(t),
      finished: iso(t + 40_000 + changed * 120),
      status: i === 9 ? "complete_with_errors" : "complete",
      name: i === 20 ? "before-upgrade" : null,
      pinned: i === 20,
      sources: ["/home/me"],
      stats: stats(184_000 + i * 40, 92_000_000_000 - i * 80_000_000, changed),
      errors: i === 9 ? [{ path: "/home/me/.config/secret.db", message: "Permission denied (os error 13)" }] : [],
      errors_total: i === 9 ? 1 : 0,
      flux_version: "0.1.0",
    });
  }
  return out;
}

const snapshots: Record<string, SnapshotMeta[]> = {
  home: snapshotSeries("home", 34, DAY, false),
  docs: snapshotSeries("docs", 48, DAY / 6, true),
  photos: snapshotSeries("photos", 6, 7 * DAY, true, 9 * DAY),
  plain: snapshotSeries("plain", 14, DAY, false, 2 * 3_600_000),
  borg: snapshotSeries("borg", 26, DAY / 2, false, 5 * 3_600_000),
  "system:etc": snapshotSeries("system:etc", 21, DAY, false, 5 * 3_600_000),
};

function run(profile: string, s: SnapshotMeta | undefined, status: RunRecord["status"], error: string | null = null): RunRecord {
  const started = s ? s.started : iso(now - 2 * DAY);
  return {
    run_id: `${profile}-${started}`,
    profile,
    trigger: "scheduled",
    started,
    finished: s ? s.finished : iso(now - 2 * DAY + 4000),
    status,
    snapshot: s?.id ?? null,
    pruned: [],
    error,
    stats: s?.stats ?? null,
    warnings: s?.errors_total ?? 0,
  };
}

const profiles: ProfileSummary[] = [
  { id: "home", sources: ["~"], scope: "user", name: "Home folder", engine: "mirror", target: "/run/media/me/BackupDisk", remote: false, schedule: "daily", errors: [], last_run: null, running: false },
  { id: "docs", sources: ["~/Documents"], scope: "user", name: "Documents → NAS", engine: "vault", target: "ssh://me@nas/srv/flux", remote: true, schedule: "every 4h", errors: [], last_run: null, running: false },
  { id: "photos", sources: ["~/Pictures"], scope: "user", name: "Photo library", engine: "vault", target: "/run/media/me/Archive", remote: false, schedule: "manual + when drive connected", errors: [], last_run: null, running: false },
  { id: "plain", sources: ["~/Projects"], scope: "user", name: "Projects (rsync)", engine: "rsync", target: "/run/media/me/BackupDisk", remote: false, schedule: "daily", errors: [], last_run: null, running: false },
  { id: "borg", sources: ["~/.local/share/mail", "~/Notes"], scope: "user", name: "Mail & Notes (Borg)", engine: "borg", target: "ssh://me@nas/srv/flux", remote: true, schedule: "every 12h", errors: [], last_run: null, running: false },
  { id: "system:etc", sources: ["/etc"], scope: "system", name: "System settings", engine: "mirror", target: "/run/media/me/BackupDisk", remote: false, schedule: "daily", errors: [], last_run: null, running: false },
];
profiles[0]!.last_run = run("home", snapshots.home!.at(-1), "completed");
profiles[1]!.last_run = run("docs", snapshots.docs!.at(-1), "completed");
profiles[2]!.last_run = run("photos", undefined, "failed", "backup target /run/media/me/Archive does not exist (is the drive connected and mounted?)");
profiles[3]!.last_run = run("plain", snapshots.plain!.at(-1), "completed");
profiles[4]!.last_run = run("borg", snapshots.borg!.at(-1), "completed");
profiles[5]!.last_run = run("system:etc", snapshots["system:etc"]!.at(-1), "completed");

/** Demo: system profiles need "authenticating" first. */
let systemUnlocked = false;
const FLUX_EXCLUDE = ["~/.cache/", "~/.local/share/Trash/", "node_modules/"];
let prefs = { notify_success: false, default_exclude: null as string[] | null };
let pausedUntil: string | null | undefined =
  typeof location !== "undefined" && new URLSearchParams(location.search).get("paused") === "1" ? iso(Date.now() + 9 * 3_600_000) : undefined;
let autostartOn = false;
const prefsView = () => ({
  notify_success: prefs.notify_success,
  default_exclude: prefs.default_exclude ?? FLUX_EXCLUDE,
  default_exclude_custom: prefs.default_exclude != null,
  pause: pausedUntil === undefined ? null : { until: pausedUntil },
});

/** Demo: `?first=1` gives "Home folder" a single snapshot (the first-backup card). */
if (typeof location !== "undefined" && new URLSearchParams(location.search).get("first") === "1") snapshots.home = snapshots.home!.slice(-1);

let unlocked = new Set<string>(["home", "photos", "plain"]);

function locked(id: string) {
  const p = profiles.find((x) => x.id === id);
  return (p?.engine === "vault" || p?.engine === "borg") && !unlocked.has(id);
}

/** Demo: `?tools=missing` shows the app without rsync and borg installed. */
const toolsMissing = typeof location !== "undefined" && new URLSearchParams(location.search).get("tools") === "missing";

function tools() {
  return [
    toolsMissing
      ? { tool: "rsync", program: "rsync", version: null, supported: false, hint: "rsync is not installed. Install it with your package manager (for example `sudo dnf install rsync` or `sudo apt install rsync`)." }
      : { tool: "rsync", program: "rsync", version: "3.4.1", supported: true, hint: null },
    toolsMissing
      ? { tool: "borg", program: "borg", version: null, supported: false, hint: "Borg is not installed. Install the “borgbackup” package (for example `sudo dnf install borgbackup` or `sudo apt install borgbackup`)." }
      : { tool: "borg", program: "borg", version: "1.4.5", supported: true, hint: null },
  ];
}

function recoveryKit(id: string) {
  const p = profiles.find((x) => x.id === id)!;
  const newest = snapshots[id]?.at(-1)?.id ?? "2026-11-07_20-00-00";
  if (p.engine === "borg") {
    const repo = "ssh://me@nas/srv/flux/flux-borg/nyx/me/borg/repo";
    return {
      engine: "borg",
      location: "me@nas:/srv/flux/flux-borg/nyx/me/borg",
      tool: tools()[1],
      recovery: `FLUX BACKUP — RECOVERY GUIDE (Borg compatibility mode)
=============================================================

These are backups of “${p.name}”, made by Flux Backup in Borg compatibility mode.
They're stored in a standard Borg repository, written by borg itself. You don't
need Flux to get your files back or to keep making backups: borg is enough.

  Profile:     ${p.name} (borg)
  Repository:  ${repo}
  Encryption:  repokey-blake2        Compression: zstd,3

IN SHORT
--------
  Your files are inside the repository folder above, in Borg's own format
  (compressed, deduplicated and encrypted), so you can't open them there directly.
  To get them back you need the program borg and your passphrase, then:
    1. open a terminal and paste the lines under SET UP,
    2. type   borg list                    to see your backups (archives),
    3. mount them like a folder (BROWSE) or extract what you need (RESTORE).

NEW TO BORG
-----------
  borg is a free backup program in every Linux distribution. Install the
  “borgbackup” package if  borg --version  says it's missing.

  * Each backup is an “archive”, named after the date and time it was made.
    In commands, ::NAME means “archive NAME in the repository”.
  * borg extract writes files into the folder you're in, so cd there first.

SET UP (once per terminal window)
-------------------------------
  export BORG_REPO=${repo}
  export BORG_PASSCOMMAND='secret-tool lookup service flux-backup profile borg'

LIST THE BACKUPS
----------------
  borg list                              every archive
  borg list ::${newest}     the files in one archive

BROWSE
------
  mkdir -p ~/flux-backups && borg mount :: ~/flux-backups
  When you're done:  borg umount ~/flux-backups

RESTORE ONE FILE
----------------
  mkdir -p ~/restored && cd ~/restored && borg extract ::${newest} home/me/path/to/file

RESTORE EVERYTHING
------------------
  cd / && sudo --preserve-env=BORG_REPO,BORG_RSH borg extract --list ::${newest}

KEEP A COPY OF THE KEY
----------------------
  borg key export :: ~/flux-borg-key-borg.txt

FOR BORG USERS
--------------
  borg create --stats --compression=zstd,3 --exclude-caches --patterns-from=flux-patterns.txt '::{now:%Y-%m-%d_%H-%M-%S}' /home/me
`,
      script: `#!/bin/sh
# Flux Backup recovery kit: back up "${p.name}" (borg) with plain borg, without Flux.
set -eu

export BORG_REPO=${repo}
[ -n "\${BORG_PASSPHRASE:-}\${BORG_PASSCOMMAND:-}" ] || export BORG_PASSCOMMAND='secret-tool lookup service flux-backup profile borg'
...
`,
    };
  }
  const repo = "/run/media/me/BackupDisk/flux-rsync/nyx/me/plain";
  return {
    engine: "rsync",
    location: repo,
    tool: tools()[0],
    recovery: `FLUX BACKUP — RECOVERY GUIDE (rsync compatibility mode)
==============================================================

These are backups of “${p.name}”, made by Flux Backup in rsync compatibility mode.
Every snapshot is a plain folder written by rsync, a standard Linux copying tool.
You don't need Flux to get your files back or to keep making backups.

IN SHORT
--------
  Your files are ordinary files and folders. To get one back, open
    ${repo}/latest/data
  in your file manager, go to the file's usual place (for example
  /home/me/path/to), and copy it out. That's all: no program, no password.

NEW TO RSYNC
------------
  rsync copies files and folders. Each command below is one line: copy the whole
  line into a terminal and press Enter.
    * A slash at the end of a folder (folder/) means “what's inside this folder”.
    * --dry-run only shows what would happen. Add it first when you're unsure.

WHERE YOUR FILES ARE
--------------------
  snapshots/<date and time>/data/<the file's usual path>
  for example
    snapshots/${newest}/data/home/me/path/to/file

RESTORE ONE FILE
----------------
  rsync -aHAX ${repo}/latest/data/home/me/path/to/file /home/me/path/to/

RESTORE A FOLDER (to where it was)
----------------------------------
  rsync -aHAX --numeric-ids ${repo}/latest/data/home/me/ /home/me/

MAKE A NEW BACKUP WITHOUT FLUX
------------------------------
  sh ${repo}/flux-backup.sh            (or add --dry-run to only show what would change)

FOR RSYNC USERS
---------------
  rsync --archive --hard-links --numeric-ids --relative --delete --delete-excluded --acls --xattrs --exclude-from=${repo}/snapshots/.partial/rsync-filter.txt '--link-dest=../../<previous snapshot>/data' /home/me/ ${repo}/snapshots/.partial/data/
`,
    script: `#!/bin/sh
# Flux Backup recovery kit: back up "${p.name}" (plain) with plain rsync, without Flux.
set -eu
REPO=${repo}
...
`,
  };
}

// A small, realistic file tree for browsing.
const tree: Record<string, [string, "dir" | "file" | "symlink", number][]> = {
  "/": [["home", "dir", 0]],
  "/home": [["me", "dir", 0]],
  "/home/me": [
    [".config", "dir", 0],
    ["Documents", "dir", 0],
    ["Music", "dir", 0],
    ["Pictures", "dir", 0],
    ["Projects", "dir", 0],
    [".bashrc", "file", 3_771],
    [".gitconfig", "file", 412],
    ["notes.md", "file", 18_204],
    ["latest-photo.jpg", "symlink", 0],
  ],
  "/home/me/Documents": [
    ["Taxes", "dir", 0],
    ["Thesis", "dir", 0],
    ["contract-signed.pdf", "file", 1_482_113],
    ["budget-2026.ods", "file", 92_004],
    ["passport-scan.png", "file", 3_804_552],
  ],
  "/home/me/Documents/Thesis": [
    ["figures", "dir", 0],
    ["thesis.tex", "file", 214_881],
    ["references.bib", "file", 44_120],
    ["thesis.pdf", "file", 8_120_331],
  ],
  "/home/me/Projects": [
    ["flux-backup", "dir", 0],
    ["website", "dir", 0],
  ],
  "/home/me/Pictures": [
    ["2026", "dir", 0],
    ["wallpaper.png", "file", 6_204_118],
  ],
  "/home/me/Music": [["playlist.m3u", "file", 2_310]],
  "/home/me/.config": [["flux-backup", "dir", 0]],
};

/** Free space after each backup: the home drive fills up, the NAS stays steady. */
function spaceFor(id: string): Overview["space"] {
  if (id !== "home" && id !== "plain") return null;
  const size = 2_000_000_000_000;
  const free = 742_000_000_000;
  const filling = id === "home";
  const points = Array.from({ length: 31 }, (_, i) => {
    const daysAgo = 60 - i * 2;
    const wobble = Math.sin(i * 1.7) * 4_000_000_000;
    const f = filling ? free + daysAgo * 9_500_000_000 + wobble : free + wobble + (i % 7 === 0 ? 12_000_000_000 : 0);
    return { t: iso(now - daysAgo * DAY - (daysAgo ? 3_600_000 : 0)), free: Math.round(f) };
  });
  points[points.length - 1] = { t: iso(now), free };
  return { free, size, points, per_day: filling ? -9_500_000_000 : 120_000_000, days_left: filling ? 78 : null };
}

function listing(path: string): Listing {
  const items = tree[path] ?? [];
  const entries: DirEntry[] = items.map(([name, kind, size], i) => {
    const full = path === "/" ? `/${name}` : `${path}/${name}`;
    return {
      name,
      path: full,
      kind,
      size,
      mtime: Math.floor((now - (i + 1) * 3.3 * DAY) / 1000),
      mode: kind === "dir" ? 0o755 : 0o644,
      link: kind === "symlink" ? "Pictures/2026/IMG_2041.jpg" : null,
      children: kind === "dir" ? (tree[full]?.length ?? 0) : null,
    };
  });
  return { path, entries };
}

function fullProfile(p: ProfileSummary): Profile {
  return {
    version: 1,
    name: p.name,
    engine: p.engine,
    target: p.remote
      ? { kind: "ssh", host: "nas", user: "me", port: null, path: "/srv/flux", identity_file: "/home/me/.ssh/flux_backup_ed25519", bandwidth_limit: null }
      : { kind: "local", path: p.target, fs_uuid: "7A1C-22F0" },
    sources: { paths: ["/home/me"], exclude: ["~/.cache/", "node_modules/", "*.tmp"], exclude_larger_than: null, exclude_caches: true, exclude_if_present: [".nobackup"], one_file_system: false },
    retention: { enabled: true, keep_last: 1, keep_within: "1day", keep_hourly: 0, keep_daily: 7, keep_weekly: 4, keep_monthly: 12, keep_yearly: 5, max_age: null, keep_named: true, min_free_space: null },
    schedule: p.schedule.startsWith("every")
      ? { trigger: "interval", every: "4h", catch_up: true, skip_on_battery: true, on_connect: false }
      : p.schedule.startsWith("manual")
        ? { trigger: "manual", catch_up: true, skip_on_battery: false, on_connect: true }
        : { trigger: "calendar", on_calendar: "*-*-* 20:00:00", catch_up: true, skip_on_battery: false, on_connect: false },
    options: { preserve_acl: true, preserve_xattr: true, symlinks: "preserve", change_detection: "metadata", continue_on_errors: true, snapshot_if_unchanged: false, nice: 10, io_priority: "best_effort_low", notify: true, backup_before_restore: true, threads: 0 },
    hooks: { pre_backup: [], post_backup: [], on_error: [] },
    vault: p.engine === "vault" ? { compression_level: 3, encryption: true, password_source: { from: "keyring" } } : null,
    borg: p.engine === "borg" ? { encryption: "repokey-blake2", compression: "zstd,3", password_source: { from: "keyring" }, remote_path: null, extra_args: [] } : null,
  };
}

const stored: Record<string, Profile> = Object.fromEntries(profiles.map((p) => [p.id, fullProfile(p)]));
stored["system:etc"]!.sources = { ...stored["system:etc"]!.sources, paths: ["/etc", "/root", "/usr/local"], exclude: ["/root/.cache/"] };
stored["system:etc"]!.options.notify = false;

function validate(p: Profile): Issue[] {
  const v: Issue[] = [];
  const err = (field: string, message: string) => v.push({ severity: "error", field, message });
  if (!p.name.trim()) err("name", "name must not be empty");
  if (!p.sources.paths.length) err("sources.paths", "add at least one file or folder to back up");
  p.sources.paths.forEach((x, i) => !x.startsWith("/") && err(`sources.paths[${i}]`, `${x} is not absolute`));
  if (p.target.kind === "local" && !p.target.path.startsWith("/")) err("target.path", "choose a drive or folder");
  if (p.target.kind === "ssh" && !p.target.host.trim()) err("target.host", "host must be a non-empty name without spaces");
  if (p.schedule.trigger === "calendar" && !p.schedule.on_calendar?.trim()) err("schedule.on_calendar", "a calendar schedule needs on_calendar");
  if (p.schedule.on_connect && !(p.target.kind === "local" && p.target.fs_uuid)) err("schedule.on_connect", "backup on connect needs a drive (fs_uuid)");
  return v;
}

function preview(id: string, r: Profile["retention"]): PreviewRow[] {
  const s = [...(snapshots[id] ?? [])].reverse();
  const days = new Set<string>();
  const weeks = new Set<string>();
  const months = new Set<string>();
  const years = new Set<string>();
  const within = r.keep_within ? (/(\d+)\s*d/.exec(r.keep_within)?.[1] ? Number(/(\d+)\s*d/.exec(r.keep_within)![1]) * DAY : 0) : 0;
  return s.map((x, i) => {
    const reasons: string[] = [];
    if (x.pinned) reasons.push("pinned");
    if (x.name && r.keep_named) reasons.push("named");
    if (i === 0) reasons.push("newest");
    if (r.enabled) {
      if (i < r.keep_last) reasons.push("last-n");
      const d = new Date(x.started);
      const day = d.toDateString();
      if (!days.has(day) && days.size < r.keep_daily) {
        days.add(day);
        reasons.push("daily");
      }
      const wk = `${d.getFullYear()}-${Math.floor((d.getTime() / DAY + 3) / 7)}`;
      if (!weeks.has(wk) && weeks.size < r.keep_weekly) {
        weeks.add(wk);
        reasons.push("weekly");
      }
      const mo = `${d.getFullYear()}-${d.getMonth()}`;
      if (!months.has(mo) && months.size < r.keep_monthly) {
        months.add(mo);
        reasons.push("monthly");
      }
      const yr = String(d.getFullYear());
      if (!years.has(yr) && years.size < r.keep_yearly) {
        years.add(yr);
        reasons.push("yearly");
      }
      if (within && now - d.getTime() < within) reasons.push("recent");
    } else reasons.push("retention off");
    return { id: x.id, time: x.started, keep: reasons.length > 0, reasons, expired: false, name: x.name, pinned: x.pinned };
  });
}

const listeners = new Map<string, Set<(p: unknown) => void>>();

export function listen(event: string, handler: (p: unknown) => void): () => void {
  if (!listeners.has(event)) listeners.set(event, new Set());
  listeners.get(event)!.add(handler);
  return () => listeners.get(event)?.delete(handler);
}

function emit(event: string, payload: unknown) {
  listeners.get(event)?.forEach((h) => h(payload));
}

function simulateBackup(id: string) {
  const p = profiles.find((x) => x.id === id)!;
  p.running = true;
  const total = 1_840_000_000;
  const started = Date.now();
  const send = (message: JobMessage) => emit("flux://job", { profile: id, message });
  send({ type: "hello", info: { profile: id, run_id: "demo", started: iso(started), trigger: "manual" } });
  let tick = 0;
  const timer = setInterval(() => {
    tick++;
    const secs = (Date.now() - started) / 1000;
    const phase = tick < 6 ? "scanning" : tick < 9 ? "planning" : tick < 48 ? "transferring" : "finalizing";
    const done = phase === "transferring" ? Math.min(total, ((tick - 9) / 39) * total) : phase === "finalizing" ? total : 0;
    send({
      type: "progress",
      progress: {
        phase,
        scanned: Math.min(184_000, tick * 31_000),
        entries_total: 184_000,
        entries_done: Math.round((done / total) * 184_000),
        bytes_total: phase === "transferring" || phase === "finalizing" ? total : 0,
        bytes_done: done,
        warnings: tick > 20 ? 1 : 0,
        elapsed_secs: secs,
        bytes_per_sec: done / Math.max(1, secs - 2),
        eta_secs: phase === "transferring" ? ((total - done) / total) * 10 : null,
      },
    });
    if (tick === 21) send({ type: "event", event: { event: "warning", path: "/home/me/.cache/locked.db", message: "Permission denied (os error 13)" } });
    if (tick >= 52) {
      clearInterval(timer);
      const s: SnapshotMeta = { ...snapshots[id]!.at(-1)!, id: id === "home" ? "2026-11-05_10-00-00" : "a1b2c3d4e5f60718", started: iso(started), finished: iso(Date.now()), name: null, pinned: false };
      snapshots[id]!.push(s);
      p.running = false;
      p.last_run = run(id, s, "completed");
      send({ type: "finished", status: "completed", error: null });
      emit("flux://changed", { profile: id });
    }
  }, 200);
}

const delay = (ms: number) => new Promise((r) => setTimeout(r, ms));

export async function handle(cmd: string, args: Record<string, unknown>): Promise<unknown> {
  await delay(120);
  const id = args.id as string;
  const needsUnlock = () => {
    const p = profiles.find((x) => x.id === id);
    if ((p?.engine === "vault" || p?.engine === "borg") && !unlocked.has(id)) throw new ApiError("password_required", `The backups of “${p.name}” are locked.`);
  };
  if (id?.startsWith("system:") && !systemUnlocked) throw new ApiError("locked", "Authenticate to manage system backups.");
  switch (cmd) {
    case "profiles":
      return structuredClone(profiles.filter((p) => p.scope === "user" || systemUnlocked));
    case "system_status":
      return { available: true, unlocked: systemUnlocked };
    case "system_unlock":
      await delay(700);
      systemUnlocked = true;
      emit("flux://system", { unlocked: true });
      return null;
    case "system_lock":
      systemUnlocked = false;
      emit("flux://system", { unlocked: false });
      return null;
    case "set_password":
      unlocked.add(id);
      return null;
    case "save_key_file":
      return `/etc/flux-backup/keys/${id.replace("system:", "")}`;
    case "overview": {
      if (locked(id)) throw new ApiError("password_required", "The Vault is locked.");
      const p = profiles.find((x) => x.id === id)!;
      const s = snapshots[id] ?? [];
      const o: Overview = {
        profile: structuredClone(p),
        snapshot_count: s.length,
        oldest: s[0]?.started ?? null,
        latest: s.at(-1) ?? null,
        recent_runs: s.slice(-6).reverse().map((x, i) =>
          i === 3 ? { ...run(id, x, "unchanged"), stats: null } : run(id, x, x.status === "complete" ? "completed" : "completed_with_errors"),
        ),
        next_run: p.schedule.startsWith("manual") ? null : new Date(now + 5 * 3_600_000).toISOString(),
        timer_active: !p.schedule.startsWith("manual"),
        free_bytes: id === "photos" ? null : 742_000_000_000,
        repo_error: null,
        space: spaceFor(id),
        health:
          id === "photos"
            ? [
                { level: "warning", text: "Archive isn't connected. The last backup to it was 9 days ago.", hint: "Plug in Archive to back up." },
              ]
            : [],
      };
      if (id === "photos") o.recent_runs.unshift(p.last_run!);
      return o;
    }
    case "status": {
      const p = profiles.find((x) => x.id === id)!;
      const s = snapshots[id] ?? [];
      const stored = p.engine === "vault" || p.engine === "borg";
      const day = (t: number) => {
        const d = new Date(t);
        return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
      };
      const days = Array.from({ length: 30 }, (_, i) => ({ date: day(now - (29 - i) * DAY), state: "none" as "ok" | "failed" | "none", bytes: 0 }));
      for (const x of s) {
        const d = days.find((d) => d.date === day(Date.parse(x.finished)));
        if (d) (d.state = "ok"), (d.bytes += (stored ? x.stats?.stored_bytes : x.stats?.copied_bytes) ?? 0);
      }
      if (id === "photos") days[27]!.state = "failed";
      return {
        next_run: p.schedule.startsWith("manual") ? null : iso(now + (id === "docs" || id === "borg" ? 1 : 5) * 3_600_000),
        timer_active: !p.schedule.startsWith("manual"),
        last_good: s.at(-1)?.finished ?? null,
        health: id === "photos" ? [{ level: "warning", text: "Archive isn't connected. The last backup to it was 9 days ago.", hint: "Plug in Archive to back up." }] : [],
        days,
      };
    }
    case "snapshots":
      needsUnlock();
      return structuredClone(snapshots[id] ?? []);
    case "list_dir":
      needsUnlock();
      return listing((args.path as string) || "/");
    case "unlock":
      if (args.password !== "demo") throw new ApiError("wrong_password", "Wrong password (try “demo”).");
      unlocked = new Set([...unlocked, id]);
      return null;
    case "backup_now":
      if (profiles.find((x) => x.id === id)?.running) throw new ApiError("busy", "A backup is already running.");
      simulateBackup(id);
      return null;
    case "cancel":
      return null;
    case "restore":
      for (let i = 1; i <= 8; i++) {
        await delay(150);
        emit("flux://restore", { profile: id, progress: { phase: "transferring", scanned: 0, entries_total: 120, entries_done: i * 15, bytes_total: 18_402_113, bytes_done: (18_402_113 * i) / 8, warnings: 0, elapsed_secs: i * 0.15, bytes_per_sec: 9_000_000, eta_secs: (8 - i) * 0.15 } });
      }
      return { restored: (args.paths as string[]).length * 12, bytes: 18_402_113, skipped_identical: 3, moved_aside: [], errors: [], would_write: [] };
    case "get_profile":
      return { id, profile: structuredClone(stored[id]!), issues: validate(stored[id]!) };
    case "new_profile": {
      const system = !!args.system;
      const p = fullProfile({ id: "", scope: system ? "system" : "user", name: system ? "System settings" : "New profile", engine: "mirror", target: "", remote: false, schedule: "daily", errors: [], last_run: null, running: false });
      p.target = { kind: "local", path: "", fs_uuid: null };
      if (system) {
        p.sources.paths = ["/etc", "/root", "/usr/local"];
        p.sources.exclude = ["/root/.cache/", "*.tmp"];
        p.options.notify = false;
      }
      return { id: "", profile: p, issues: [] };
    }
    case "validate_profile":
      return validate(args.profile as Profile);
    case "save_profile": {
      const p = args.profile as Profile;
      const issues = validate(p);
      if (issues.some((i) => i.severity === "error")) return { id, issues, saved: false, schedule: null };
      stored[id] = structuredClone(p);
      const existing = profiles.find((x) => x.id === id);
      const target = p.target.kind === "local" ? p.target.path : `ssh://${p.target.host}${p.target.path}`;
      const schedule = p.schedule.trigger === "manual" ? "manual" : p.schedule.trigger === "interval" ? `every ${p.schedule.every}` : (p.schedule.on_calendar ?? "");
      if (existing) Object.assign(existing, { name: p.name, engine: p.engine, target, schedule });
      else {
        profiles.push({ id, scope: id.startsWith("system:") ? "system" : "user", name: p.name, engine: p.engine, target, remote: p.target.kind === "ssh", schedule, errors: [], last_run: null, running: false });
        snapshots[id] = [];
      }
      return { id, issues, saved: true, schedule: p.schedule.trigger === "manual" ? null : `Timer updated: ${schedule}` };
    }
    case "delete_profile": {
      const i = profiles.findIndex((x) => x.id === id);
      if (i >= 0) profiles.splice(i, 1);
      return null;
    }
    case "suggest_id":
      return ((args.name as string).toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "profile");
    case "retention_preview":
      return preview(id, args.retention as Profile["retention"]);
    case "exclude_suggestions":
      return [
        { pattern: "~/.cache/", label: "Application caches", why: "rebuilt automatically", size: 6_204_000_000 },
        { pattern: "~/.local/share/Trash/", label: "Trash", why: "files you already deleted", size: 1_120_000_000 },
        { pattern: "node_modules/", label: "node_modules", why: "reinstalled with npm/pnpm", size: null },
        { pattern: "**/target/debug/", label: "Rust debug builds", why: "rebuilt by cargo", size: null },
        { pattern: "~/.local/share/Steam/steamapps/", label: "Steam games", why: "downloadable again", size: 148_000_000_000 },
        { pattern: "~/Downloads/", label: "Downloads", why: "often re-downloadable", size: 23_400_000_000 },
        { pattern: "*.tmp", label: "Temporary files", why: "short-lived by definition", size: null },
      ];
    case "drives":
      return [
        { mount_point: "/run/media/me/BackupDisk", label: "BackupDisk", fs_uuid: "7A1C-22F0", fs_type: "ext4", free: 742_000_000_000, total: 1_000_000_000_000 },
        { mount_point: "/run/media/me/Archive", label: "Archive", fs_uuid: "b3e1c0de-4f11-4a5e-9d2a-0c1d2e3f4a5b", fs_type: "btrfs", free: 3_100_000_000_000, total: 4_000_000_000_000 },
      ];
    case "test_target":
      await delay(600);
      return { ok: true, messages: ["logged in to me@nas", "rsync is available", "/srv/flux is writable", "1.8 TiB free"] };
    case "update_snapshot": {
      const snap = snapshots[id]?.find((x) => x.id === args.snapshot);
      if (!snap) throw new ApiError("error", "snapshot not found");
      if (args.setName) snap.name = (args.name as string | null) || null;
      if (args.pinned != null) snap.pinned = args.pinned as boolean;
      return structuredClone(snap);
    }
    case "delete_snapshot": {
      const gone = snapshots[id]?.find((x) => x.id === args.snapshot);
      snapshots[id] = (snapshots[id] ?? []).filter((x) => x.id !== args.snapshot);
      const e = profiles.find((p) => p.id === id)?.engine;
      return e === "vault" || e === "borg" ? (gone?.stats.stored_bytes ?? 0) : null;
    }
    case "compare":
      await delay(400);
      return {
        old: args.old,
        new: args.new ?? "now",
        summary: { added: 3, removed: 1, modified: 4, type_changed: 0, bytes_delta: 2_340_112 },
        truncated: false,
        changes: [
          { path: "/home/me/Documents/Thesis/thesis.tex", change: "modified", kind: "file", fields: ["content", "mtime"], old_size: 210_002, new_size: 214_881 },
          { path: "/home/me/Documents/Thesis/thesis.pdf", change: "modified", kind: "file", fields: ["content", "mtime"], old_size: 8_002_119, new_size: 8_120_331 },
          { path: "/home/me/Documents/Thesis/figures/results-v3.png", change: "added", kind: "file", fields: [], old_size: null, new_size: 1_904_224 },
          { path: "/home/me/Documents/budget-2026.ods", change: "modified", kind: "file", fields: ["content", "mtime"], old_size: 91_002, new_size: 92_004 },
          { path: "/home/me/Pictures/2026/IMG_2041.jpg", change: "added", kind: "file", fields: [], old_size: null, new_size: 4_211_877 },
          { path: "/home/me/Pictures/2026/IMG_2042.jpg", change: "added", kind: "file", fields: [], old_size: null, new_size: 3_987_120 },
          { path: "/home/me/notes-old.md", change: "removed", kind: "file", fields: [], old_size: 7_441_870, new_size: null },
          { path: "/home/me/.bashrc", change: "modified", kind: "file", fields: ["mtime", "mode"], old_size: 3_771, new_size: 3_771 },
        ],
      };
    case "runs": {
      const list = [...(snapshots[id] ?? [])].reverse().map((x, i) => (i === 2 ? { ...run(id, x, "unchanged"), stats: null } : run(id, x, x.status === "complete" ? "completed" : "completed_with_errors")));
      const p = profiles.find((x) => x.id === id);
      if (p?.last_run?.status === "failed") list.unshift(p.last_run);
      return list;
    }
    case "run_events":
      return [
        { t: iso(now - 60_000), event: "info", message: "pre_backup hook `mount /mnt/nas` ok:" },
        { t: iso(now - 59_000), event: "phase", phase: "scanning" },
        { t: iso(now - 50_000), event: "phase", phase: "planning" },
        { t: iso(now - 49_000), event: "phase", phase: "transferring" },
        { t: iso(now - 30_000), event: "warning", path: "/home/me/.config/secret.db", message: "Permission denied (os error 13)" },
        { t: iso(now - 12_000), event: "phase", phase: "finalizing" },
        { t: iso(now - 11_000), event: "info", message: "pruned snapshot 2026-09-02_20-00-00" },
        { t: iso(now - 10_000), event: "phase", phase: "done" },
      ];
    case "prune":
      await delay(500);
      return { decisions: [], removed: args.dryRun ? ["2026-09-02_20-00-00"] : [], removed_for_space: [], space_floor_unmet: false, bytes_freed: args.dryRun ? null : 1_840_000_000, dry_run: args.dryRun };
    case "check":
      await delay(args.readData ? 1500 : 600);
      return { snapshots: snapshots[id]?.length ?? 0, checked: 112_435, bytes_read: args.readData ? 92_000_000_000 : 0, errors: [], warnings: [] };
    case "tools":
      return tools();
    case "open_snapshot_file":
      await delay(400);
      return `/home/me/.cache/flux-backup/opened/${args.snapshot}/${(args.path as string).split("/").pop()}`;
    case "duplicate_profile": {
      const src = profiles.find((x) => x.id === id)!;
      const copy = { ...structuredClone(src), id: `${id}-copy`, name: `${src.name} (copy)`, last_run: null };
      profiles.push(copy);
      stored[copy.id] = { ...structuredClone(stored[id]!), name: copy.name };
      snapshots[copy.id] = [];
      return copy.id;
    }
    case "find": {
      await delay(500);
      const q = (args.query as string).trim().toLowerCase();
      const snaps = snapshots[id] ?? [];
      const hits: Found[] = [];
      for (const [dir, items] of Object.entries(tree)) {
        for (const [name, kind, size] of items) {
          if (!name.toLowerCase().includes(q)) continue;
          const path = dir === "/" ? `/${name}` : `${dir}/${name}`;
          const versions = kind === "dir" ? 1 : 1 + (name.length % 4);
          hits.push({ path, kind, versions, snapshots: snaps.length, latest: snaps.at(-1)!.id, latest_size: size, latest_mtime: Math.floor((now - 3 * DAY) / 1000) });
        }
      }
      // A file that was deleted a while ago: only in older snapshots.
      if ("notes-old.md".includes(q) && snaps.length > 6) {
        hits.push({ path: "/home/me/notes-old.md", kind: "file", versions: 2, snapshots: snaps.length - 5, latest: snaps.at(-6)!.id, latest_size: 7_441_870, latest_mtime: Math.floor((now - 9 * DAY) / 1000) });
      }
      return { hits, total: hits.length, snapshots: snaps.length };
    }
    case "versions": {
      await delay(350);
      const snaps = snapshots[id] ?? [];
      const n = Math.min(4, snaps.length);
      const step = Math.max(1, Math.floor(snaps.length / n));
      return Array.from({ length: n }, (_, i) => {
        const first = snaps[i * step]!;
        const last = snaps[Math.min(snaps.length - 1, (i + 1) * step - 1)]!;
        const count = i === n - 1 ? snaps.length - i * step : step;
        return { first: first.id, last: i === n - 1 ? snaps.at(-1)!.id : last.id, snapshots: count, kind: "file", size: 18_204 - (n - 1 - i) * 1_310, mtime: Math.floor(new Date(first.started).getTime() / 1000) - 3600 };
      });
    }
    case "preferences":
      return prefsView();
    case "save_preferences":
      prefs = { notify_success: args.notifySuccess as boolean, default_exclude: (args.defaultExclude as string[] | null) ?? null };
      return prefsView();
    case "set_pause": {
      const k = args.pause as string;
      const tomorrow = new Date();
      tomorrow.setDate(tomorrow.getDate() + 1);
      tomorrow.setHours(7, 0, 0, 0);
      pausedUntil = k === "off" ? undefined : k === "hour" ? iso(Date.now() + 3_600_000) : k === "tomorrow" ? tomorrow.toISOString() : null;
      return prefsView().pause;
    }
    case "autostart":
      return autostartOn;
    case "set_autostart":
      autostartOn = args.on as boolean;
      return null;
    case "estimate": {
      await delay(900);
      const paths = (args.profile as Profile).sources.paths;
      const excluded = (args.profile as Profile).sources.exclude.length;
      return { files: 61_200 * paths.length - excluded * 3_100, folders: 7_400 * paths.length, bytes: 41_800_000_000 * paths.length - excluded * 2_600_000_000, unreadable: 3, missing: [], free: 742_000_000_000 };
    }
    case "restore_preview": {
      await delay(300);
      const inPlace = args.to == null;
      const differ = inPlace ? ["/home/me/notes.md", "/home/me/Documents/budget-2026.ods"] : [];
      return {
        new: inPlace ? 3 : 14,
        differ: differ.length,
        identical: inPlace ? 9 : 0,
        new_folders: inPlace ? 0 : 2,
        bytes: 4_211_877,
        keep_existing: !args.overwrite,
        differ_paths: differ,
        new_paths: ["/home/me/Pictures/2026/IMG_2041.jpg"],
        unsafe_paths: 0,
      };
    }
    case "diagnostics":
      return `Flux Backup diagnostics\n\nFlux Backup 0.2.0 (demo)\n--- profile ${id} ---\n…`;
    case "export_formats":
      return [
        { id: "flux", label: "Flux Backup profile", description: "This profile as a file, to import on another computer or keep as a copy.", works_with: "Flux Backup (flux profile import)" },
        { id: "borgmatic", label: "borgmatic configuration", description: "The same backup as a borgmatic config: Borg with schedules, retention and checks.", works_with: "borgmatic 1.8 or newer, and Borg" },
        { id: "restic", label: "restic script", description: "A script that runs the same backup with restic, including retention.", works_with: "restic 0.16 or newer" },
        { id: "rsync", label: "rsync script", description: "A script that runs the same backup with rsync: plain-folder snapshots.", works_with: "rsync 3.1 or newer" },
        { id: "borg", label: "Borg script", description: "A script that runs the same backup with borg create.", works_with: "Borg 1.2 to 1.4" },
      ];
    case "export_profile": {
      const p = profiles.find((x) => x.id === id)!;
      const f = args.format as string;
      const notes = f === "flux" ? [] : ["Pinned and named snapshots are a Flux feature; other tools' retention may remove them.", "The schedule (at *-*-* 20:00:00) isn't part of the export: run it from cron or a systemd timer."];
      const contents =
        f === "flux"
          ? `# Export of the Flux Backup profile “${p.name}” (${id}).\n# flux-profile-id = "${id}"\n\nversion = 1\nname = "${p.name}"\nengine = "${p.engine}"\n\n[target]\nkind = "local"\npath = "${p.target}"\n\n[sources]\npaths = ["/home/me"]\nexclude = ["~/.cache/", "node_modules/", "*.tmp"]\n`
          : f === "borgmatic"
            ? `# borgmatic configuration of the Flux Backup profile “${p.name}” (${id}).\n${notes.map((n) => "# Note: " + n).join("\n")}\n\nsource_directories:\n    - "/home/me"\n\nrepositories:\n    - path: "${p.target}/flux-borg/nyx/me/${id}/repo"\n      label: "flux-${id}"\n\npatterns:\n    - "! sh:**/node_modules"\n    - "! sh:**/*.tmp"\nexclude_caches: true\ncompression: "zstd,3"\nkeep_daily: 7\nkeep_weekly: 4\n`
            : `#!/bin/sh\n# ${f} equivalent of the Flux Backup profile “${p.name}” (${id}).\nset -eu\n...\n`;
      return { format: f, filename: f === "flux" ? `${id}.flux-profile.toml` : f === "borgmatic" ? `${id}-borgmatic.yaml` : `${id}-${f}-backup.sh`, contents, executable: f !== "flux" && f !== "borgmatic", notes };
    }
    case "save_export":
      return `${args.dir}/${id}-export`;
    case "import_profile":
      return { id: "home", issues: [] };
    case "recovery_kit":
      return recoveryKit(id);
    case "save_recovery_kit": {
      const short = id.replace("system:", "");
      return [`${args.dir}/${short}-FLUX-RECOVERY.txt`, `${args.dir}/${short}-flux-backup.sh`];
    }
    default:
      throw new ApiError("error", `unknown command ${cmd}`);
  }
}
