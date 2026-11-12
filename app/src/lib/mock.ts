// Demo backend for running the UI in a browser. Not bundled into the Tauri build's hot
// path: api.ts imports it lazily only when Tauri isn't present.

import type { DirEntry, Found, Issue, JobMessage, Listing, Overview, PreviewRow, Profile, ProfileSummary, RunRecord, SnapshotMeta, SnapshotStats } from "./types";
import { type TestReport, type Destination, type Webhook, ApiError } from "./types";
import { fraction, sampleBound } from "./confidence";

/** What the demo pretends is open in the file manager. */
let mockBrowsing: { profile: string; snapshot: string; dir: string; open: string } | null = null;
let mockOpenTaken = false;

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
  "home+usb": snapshotSeries("home+usb", 12, 3 * DAY, true),
  docs: snapshotSeries("docs", 48, DAY / 6, true),
  photos: snapshotSeries("photos", 6, 7 * DAY, true, 9 * DAY),
  plain: snapshotSeries("plain", 14, DAY, false, 2 * 3_600_000),
  borg: snapshotSeries("borg", 26, DAY / 2, false, 5 * 3_600_000),
  "system:etc": snapshotSeries("system:etc", 21, DAY, false, 5 * 3_600_000),
};
// A few notes and tags, as someone would add them.
{
  const h = snapshots.home!;
  Object.assign(h[h.length - 3]!, { note: "Before upgrading to Fedora 46", tags: ["upgrade"] });
  Object.assign(h[h.length - 12]!, { note: "Tax papers scanned and filed", tags: ["taxes", "paperwork"] });
  Object.assign(h[h.length - 20]!, { tags: ["taxes"] });
}

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
  { id: "home", sources: ["~"], scope: "user", name: "Home folder", engine: "mirror", target: "/run/media/me/BackupDisk", remote: false, schedule: "*-*-* 20:00:00", size_limit: "4GB", errors: [], last_run: null, running: false },
  { id: "docs", sources: ["~/Documents"], scope: "user", name: "Documents → NAS", engine: "vault", target: "ssh://me@nas/srv/flux", remote: true, schedule: "every 4h", errors: [], last_run: null, running: false },
  { id: "photos", sources: ["~/Pictures"], scope: "user", name: "Photo library", engine: "vault", target: "/run/media/me/Archive", remote: false, schedule: "manual + when drive connected", errors: [], last_run: null, running: false },
  { id: "plain", sources: ["~/Projects"], scope: "user", name: "Projects (rsync)", engine: "rsync", target: "/run/media/me/BackupDisk", remote: false, schedule: "*-*-* 20:00:00", errors: [], last_run: null, running: false },
  { id: "borg", sources: ["~/.local/share/mail", "~/Notes"], scope: "user", name: "Mail & Notes (Borg)", engine: "borg", target: "ssh://me@nas/srv/flux", remote: true, schedule: "every 12h", errors: [], last_run: null, running: false },
  { id: "system:etc", sources: ["/etc"], scope: "system", name: "System settings", engine: "mirror", target: "/run/media/me/BackupDisk", remote: false, schedule: "*-*-* 20:00:00", errors: [], last_run: null, running: false },
];
/** `?trouble=1`: the demo's other troubles too (a copy that failed, a missing source,
 * low space, unusual changes). Without it, every backup is fine but Photo library, whose
 * drive isn't plugged in, so screenshots don't make Flux look like it's failing. */
const trouble = typeof location !== "undefined" && new URLSearchParams(location.search).get("trouble") === "1";
profiles[0]!.last_run = run("home", snapshots.home!.at(-1), "completed");
profiles[1]!.last_run = run("docs", snapshots.docs!.at(-1), "completed");
profiles[2]!.last_run = trouble
  ? run("photos", undefined, "failed", "backup target /run/media/me/Archive does not exist (is the drive connected and mounted?)")
  : run("photos", snapshots.photos!.at(-1), "completed");
profiles[3]!.last_run = run("plain", snapshots.plain!.at(-1), "completed");
profiles[4]!.last_run = run("borg", snapshots.borg!.at(-1), "completed");
profiles[5]!.last_run = run("system:etc", snapshots["system:etc"]!.at(-1), "completed");
// The home folder also goes to a USB stick (Vault) and, offsite, to the office NAS (rsync).
profiles[0]!.destinations = [
  { id: "home+usb", name: "usb", label: "USB stick", engine: "vault", target: "/run/media/me/STICK", remote: false, offsite: false },
  { id: "home+office", name: "office", label: "Office NAS", engine: "rsync", target: "ssh://me@office-nas/srv/flux", remote: true, offsite: true },
];
profiles[0]!.last_run!.destinations = [
  { name: "usb", status: "completed", snapshot: snapshots["home+usb"]!.at(-1)!.id },
  trouble
    ? { name: "office", status: "failed", error: "ssh: connect to host office-nas port 22: No route to host" }
    : { name: "office", status: "completed", snapshot: snapshots.home!.at(-1)!.id },
];

/** Demo: the Projects backup found what looks like ransomware. */
function demoUnusual() {
  const s = snapshots.plain!;
  const files = Array.from({ length: 48 }, (_, i) => `/home/me/Projects/thesis/chapter${i + 1}.odt`);
  return {
    signals: ["99 files changed or deleted, 58% of them: 19 times what this backup usually changes (3.1%)", "48 files renamed to end in .locked", "3 files that look like ransom notes (HOW_TO_DECRYPT.txt)"],
    affected: files.flatMap((f) => [f, `${f}.locked`]).concat(["/home/me/Projects/HOW_TO_DECRYPT.txt"]),
    affected_total: 99,
    by_folder: [["/home/me/Projects/thesis", 96], ["/home/me/Projects", 3]] as [string, number][],
    by_extension: [["locked", 48], ["odt", 48], ["txt", 3]] as [string, number][],
    // (An rsync backup: looked at after it was written. Flux's own formats are looked
    // at before, and such a backup isn't made at all: `stopped`.)
    snapshot: s.at(-1)!.id,
    before: s.at(-2)!.id,
  };
}
/** `?unusual=1`: only Projects' unusual changes (for Flux Sentry's screenshot). */
let unusualDismissed = !trouble && !(typeof location !== "undefined" && new URLSearchParams(location.search).get("unusual") === "1");

/** Demo restore tests, newest first. */
const restoreTests: Record<string, unknown[]> = {
  home: [{ started: new Date(now - 3 * DAY).toISOString(), finished: new Date(now - 3 * DAY + 4000).toISOString(), snapshot: "", mode: "confidence", result: "passed", picked: 459, tested: 459, bytes: 216_500_000, stopped_early: false, problems: [], problems_total: 0, guarantee: { files: 184_000, damaged: 1_840, damage: "1%", certainty: "99%" } }],
};

/** Every profile, and each destination as a profile of its own (`home+usb`). */
function allProfiles(): ProfileSummary[] {
  const out = [...profiles];
  for (const p of profiles)
    for (const d of p.destinations ?? [])
      out.push({ ...p, id: d.id, name: `${p.name} → ${d.label}`, engine: d.engine, target: d.target, remote: d.remote, offsite: d.offsite, destinations: [], last_run: null, running: false });
  return out;
}

/** Demo: system profiles need "authenticating" first. */
let systemUnlocked = false;
const FLUX_EXCLUDE = ["~/.cache/", "~/.local/share/Trash/", "node_modules/"];
let prefs = {
  notify_success: false,
  default_exclude: null as string[] | null,
  scheduler: "systemd" as "systemd" | "cron",
  one_at_a_time: false,
  diff_tool: null as string | null,
  close_to_tray: true,
  weekly_note: true,
  quiet_hours: null as { from: string; to: string } | null,
  webhooks: [{ url: "https://ntfy.sh/flux-laptop-backups", preset: "ntfy" }] as Webhook[],
};
const DIFF_TOOLS: [string, string][] = [["meld", "Meld"]];
let pausedUntil: string | null | undefined =
  typeof location !== "undefined" && new URLSearchParams(location.search).get("paused") === "1" ? iso(Date.now() + 9 * 3_600_000) : undefined;
let autostartOn = false;
const readinessAnswers: Record<string, boolean> = {};
let vaultUpgraded = false;
const demoTemplates: { id: string; name: string; line: string; icon: string; own: boolean }[] = [
  { id: "own:work-laptop", name: "Work laptop", line: "/home/me/work, /home/me/Documents. Daily.", icon: "copy", own: true },
];

/** A small home folder for the coverage view: [name, folder?, size]. */
const DEMO_TREE: Record<string, [string, boolean, number][]> = {
  "/home/me": [
    [".cache", true, 3_800_000_000],
    [".local", true, 1_200_000_000],
    ["Documents", true, 2_400_000_000],
    ["Downloads", true, 14_600_000_000],
    ["Music", true, 38_000_000_000],
    ["Pictures", true, 61_000_000_000],
    ["Projects", true, 9_100_000_000],
    ["Videos", true, 120_000_000_000],
    [".bashrc", false, 3_700],
  ],
  "/home/me/Projects": [
    ["flux", true, 2_100_000_000],
    ["site", true, 640_000_000],
    ["notes.md", false, 12_000],
  ],
  "/home/me/Projects/site": [
    ["node_modules", true, 412_000_000],
    ["src", true, 4_100_000],
    ["dist", true, 9_800_000],
    ["package.json", false, 1_200],
  ],
};

/** The demo's idea of a backup's rules: enough of gitignore to show the tester. */
function demoVerdict(prof: Profile, path: string): { in: boolean; text: string } {
  const home = "/home/me";
  const source = prof.sources.paths.map((p) => p.replace(/^~/, home)).find((s) => path === s || path.startsWith(`${s.replace(/\/$/, "")}/`));
  if (!source) return { in: false, text: "Not backed up: it isn't in any of the folders this backup takes." };
  const parts = path.slice(source.length).split("/").filter(Boolean);
  let at = source;
  for (const [i, part] of parts.entries()) {
    at = `${at}/${part}`;
    const last = i === parts.length - 1;
    for (const raw of prof.sources.exclude) {
      const pat = raw.trim();
      if (!pat || pat.startsWith("!")) continue;
      const dirOnly = pat.endsWith("/");
      const body = pat.replace(/\/$/, "").replace(/^~/, home);
      const glob = new RegExp(`^${body.replace(/[.+^${}()|[\]\\]/g, "\\$&").replace(/\*/g, "[^/]*")}$`);
      const hit = body.startsWith("/") ? glob.test(at) : glob.test(part);
      if (hit && (!dirOnly || !last || DEMO_TREE[at] !== undefined || !part.includes("."))) {
        return { in: false, text: at === path ? `Left out by the rule ${raw}.` : `Left out: it's in ${at}, which the rule ${raw} leaves out.` };
      }
    }
    if (prof.sources.exclude_caches && part === ".cache") return { in: false, text: `Left out: ${at} is marked as a cache folder (it holds a CACHEDIR.TAG).` };
  }
  return { in: true, text: `Backed up: it's in ${source}.` };
}
const prefsView = () => ({
  notify_success: prefs.notify_success,
  close_to_tray: prefs.close_to_tray,
  default_exclude: prefs.default_exclude ?? FLUX_EXCLUDE,
  default_exclude_custom: prefs.default_exclude != null,
  pause: pausedUntil === undefined ? null : { until: pausedUntil },
  scheduler: prefs.scheduler,
  one_at_a_time: prefs.one_at_a_time,
  // `?cron=0`: cron isn't installed here.
  cron_problem: typeof location !== "undefined" && new URLSearchParams(location.search).get("cron") === "0" ? "cron isn't installed. Install it (on Fedora, the cronie package) and start it, or keep systemd timers." : null,
  cron_running: true,
  diff_tool: prefs.diff_tool,
  webhooks: prefs.webhooks,
  diff_tool_used: prefs.diff_tool === "" ? null : prefs.diff_tool ? ([prefs.diff_tool, DIFF_TOOLS.find(([c]) => c === prefs.diff_tool)?.[1] ?? prefs.diff_tool] as [string, string]) : DIFF_TOOLS[0]!,
  diff_tools: DIFF_TOOLS,
  weekly_note: prefs.weekly_note,
  quiet_hours: prefs.quiet_hours,
});

/** Demo: `?first=1` gives "Home folder" a single snapshot (the first-backup card). */
if (typeof location !== "undefined" && new URLSearchParams(location.search).get("first") === "1") snapshots.home = snapshots.home!.slice(-1);

let unlocked = new Set<string>(["home", "photos", "plain"]);
/** The demo Vaults' recovery code (it unlocks them, then asks for a new password). */
let mockRecoveryCode = "7KQ2M-WX9RT-3HJ5B-NP8VD-C4FZ6-Y1GAE";

function locked(id: string) {
  const p = allProfiles().find((x) => x.id === id);
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
  const p = allProfiles().find((x) => x.id === id)!;
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

/** Deleted since the snapshots were made, so the browser marks them. */
const deletedNow = new Set(["/home/me/Documents/Taxes", "/home/me/Documents/passport-scan.png", "/home/me/notes-old.md"]);

function listing(path: string, filter: string | null = null, onlyMissing = false): Listing {
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
      on_disk: !deletedNow.has(full),
    };
  });
  const f = (filter ?? "").trim().toLowerCase();
  const wanted = entries
    .filter((e) => (!f || e.name.toLowerCase().includes(f)) && (!onlyMissing || e.on_disk === false))
    .sort((a, b) => Number(b.kind === "dir") - Number(a.kind === "dir") || a.name.toLowerCase().localeCompare(b.name.toLowerCase()));
  return { path, entries: wanted.slice(0, 2000), total: entries.length, matching: wanted.length, missing: entries.filter((e) => e.on_disk === false).length };
}

function fullProfile(p: ProfileSummary): Profile {
  return {
    version: 1,
    name: p.name,
    engine: p.engine,
    target: p.remote
      ? { kind: "ssh", host: "nas", user: "me", port: null, path: "/srv/flux", identity_file: "/home/me/.ssh/flux_backup_ed25519", bandwidth_limit: null }
      : { kind: "local", path: p.target, fs_uuid: "7A1C-22F0" },
    sources: { paths: ["/home/me"], exclude: ["~/.cache/", "node_modules/", "*.tmp"], exclude_larger_than: p.size_limit ?? null, exclude_caches: true, exclude_if_present: [".nobackup"], one_file_system: false },
    retention: { enabled: true, keep_last: 1, keep_within: "1day", keep_hourly: 0, keep_daily: 7, keep_weekly: 4, keep_monthly: 12, keep_yearly: 5, max_age: null, keep_named: true, min_free_space: null },
    schedule: p.schedule.startsWith("every")
      ? { trigger: "interval", every: "4h", catch_up: true, skip_on_battery: true, on_connect: false }
      : p.schedule.startsWith("manual")
        ? { trigger: "manual", catch_up: true, skip_on_battery: false, on_connect: true }
        : { trigger: "calendar", on_calendar: "*-*-* 20:00:00", catch_up: true, skip_on_battery: false, on_connect: false },
    options: { preserve_acl: true, preserve_xattr: true, symlinks: "preserve", change_detection: "metadata", continue_on_errors: true, snapshot_if_unchanged: false, nice: 10, io_priority: "best_effort_low", notify: true, backup_before_restore: true, threads: 0 },
    hooks: { pre_backup: [], post_backup: [], on_error: [] },
    ...(p.id === "home" ? { restore_test: { every: "month" as const, mode: "confidence" as const } } : {}),
    vault: p.engine === "vault" ? { compression_level: 3, encryption: true, password_source: { from: "keyring" } } : null,
    borg: p.engine === "borg" ? { encryption: "repokey-blake2", compression: "zstd,3", password_source: { from: "keyring" }, remote_path: null, extra_args: [] } : null,
  };
}

const stored: Record<string, Profile> = Object.fromEntries(profiles.map((p) => [p.id, fullProfile(p)]));
stored["system:etc"]!.sources = { ...stored["system:etc"]!.sources, paths: ["/etc", "/root", "/usr/local"], exclude: ["/root/.cache/"] };
stored["system:etc"]!.options.notify = false;
stored.home!.destinations = [
  { name: "usb", label: "USB stick", engine: "vault", target: { kind: "local", path: "/run/media/me/STICK", fs_uuid: "3C11-77AB" }, offsite: false, vault: { compression_level: 3, encryption: true, password_source: { from: "keyring" } } },
  { name: "office", label: "Office NAS", engine: "rsync", target: { kind: "ssh", host: "office-nas", user: "me", port: null, path: "/srv/flux", identity_file: null, bandwidth_limit: null }, offsite: true },
];
// The documents backup also takes the scans on an external disk, recognised by its UUID.
stored["docs"]!.sources = { ...stored["docs"]!.sources, paths: ["/home/me/Documents", "/mnt/scans"], volumes: [{ path: "/mnt/scans", fs_uuid: "5B3E-0C17", when_missing: "skip" }] };

function validate(p: Profile): Issue[] {
  const v: Issue[] = [];
  const err = (field: string, message: string) => v.push({ severity: "error", field, message });
  if (!p.name.trim()) err("name", "name must not be empty");
  if (!p.sources.paths.length) err("sources.paths", "add at least one file or folder to back up");
  p.sources.paths.forEach((x, i) => !x.startsWith("/") && err(`sources.paths[${i}]`, `${x} is not absolute`));
  if (p.target.kind === "local" && !p.target.path.startsWith("/")) err("target.path", "choose a drive or folder");
  if (p.target.kind === "ssh" && !p.target.host.trim()) err("target.host", "host must be a non-empty name without spaces");
  if (p.schedule.trigger === "calendar" && !p.schedule.on_calendar?.trim()) err("schedule.on_calendar", "choose the times it runs (for example: daily, or Mon..Fri 18:00)");
  if (p.schedule.on_connect && !(p.target.kind === "local" && p.target.fs_uuid)) err("schedule.on_connect", "backup on connect needs a drive (fs_uuid)");
  // The demo's USB stick is FAT32.
  if (p.target.kind === "local" && p.target.path.startsWith("/run/media/me/USB STICK") && (p.engine === "mirror" || p.engine === "rsync")) {
    v.push({ severity: "warning", field: "target.path", message: "This drive is formatted FAT32, which can't keep hard links, links or Linux permissions: every snapshot is a full copy, links and permissions are lost, and no file of 4 GB or more fits. A Vault backup suits it (it keeps all of that in its own files); or format the drive as ext4, which erases it." });
  }
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

/** Demo: backups that shut the "computer" down when done. */
const shutdownAfter: Record<string, boolean> = {};

function simulateBackup(id: string, shutdown = false) {
  shutdownAfter[id] = shutdown;
  const p = allProfiles().find((x) => x.id === id)!;
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
        // As a real backup of this size would say (the demo's is over in seconds).
        eta_secs: phase === "transferring" ? ((total - done) / total) * 480 : null,
      },
      shutdown_after: shutdownAfter[id] ?? false,
    });
    if (tick === 21) send({ type: "event", event: { event: "warning", path: "/home/me/.cache/locked.db", message: "Permission denied (os error 13)" } });
    if (tick >= 52) {
      clearInterval(timer);
      const s: SnapshotMeta = { ...snapshots[id]!.at(-1)!, id: id === "home" ? "2026-11-05_10-00-00" : "a1b2c3d4e5f60718", started: iso(started), finished: iso(Date.now()), name: null, pinned: false };
      snapshots[id]!.push(s);
      p.running = false;
      p.last_run = run(id, s, "completed");
      send({ type: "finished", status: "completed", error: null, shutdown: shutdownAfter[id] ?? false });
      emit("flux://changed", { profile: id });
    }
  }, 200);
}

const delay = (ms: number) => new Promise((r) => setTimeout(r, ms));

export async function handle(cmd: string, args: Record<string, unknown>): Promise<unknown> {
  await delay(120);
  const id = args.id as string;
  const needsUnlock = () => {
    const p = allProfiles().find((x) => x.id === id);
    if ((p?.engine === "vault" || p?.engine === "borg") && !unlocked.has(id)) throw new ApiError("password_required", `The backups of “${p.name}” are locked.`);
  };
  if (id?.startsWith("system:") && !systemUnlocked) throw new ApiError("locked", "Authenticate to manage system backups.");
  switch (cmd) {
    case "profiles":
      // `?empty=1`: a new computer, no backups yet (the welcome screen).
      if (typeof location !== "undefined" && new URLSearchParams(location.search).get("empty") === "1") return [];
      return structuredClone(profiles.filter((p) => p.scope === "user" || systemUnlocked)).map((p) => ({ ...p, sentry: stored[p.id]?.unusual_changes ?? "normal", held: p.id === "plain" && !unusualDismissed && (stored[p.id]?.unusual_changes ?? "normal") !== "off" }));
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
      const p = allProfiles().find((x) => x.id === id)!;
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
        // Demo: BackupDisk reports its health; Archive (Photo library's) doesn't.
        drive_health: p.remote ? null : { reports: id !== "photos", checked: new Date(now - 2 * 3_600_000).toISOString(), said: null },
        health:
          id === "photos"
            ? [
                { level: "warning", kind: "unplugged", text: "Archive isn't connected. The last backup to it was 9 days ago.", hint: "Plug in Archive to back up." },
              ]
            : [],
      };
      if (id === "photos" && p.last_run?.status === "failed") o.recent_runs.unshift(p.last_run);
      if (id === "home" && o.recent_runs[0]) o.recent_runs[0].destinations = p.last_run?.destinations;
      if (id === "plain" && !unusualDismissed) {
        o.unusual = demoUnusual();
        o.health = [{ level: "danger", kind: "unusual", text: `Backups are on hold: Flux Sentry found unusual changes in snapshot ${o.unusual.snapshot}: ${o.unusual.signals.join("; ")}.`, hint: "If you didn't make them, don't delete anything: the snapshot before them is kept. Otherwise choose “This was me”." }, ...o.health];
      }
      // What Flux Sentry has learned of each backup (the demo's Projects is the busy one).
      if ((stored[id]?.unusual_changes ?? "normal") !== "off") o.sentry = id === "plain" ? { usual: 0.031, warns_at: 0.41, runs: 34 } : { usual: 0.0042, warns_at: 0.05, runs: 34 };
      const tests = (restoreTests[id] ?? []) as TestReport[];
      o.restore_test = { every: stored[id]?.restore_test?.every ?? (id === "home" ? "month" : "off"), last: tests[0] ?? null, last_passed: tests.find((t) => t?.result !== "failed")?.started ?? null };
      return o;
    }
    case "status": {
      const p = allProfiles().find((x) => x.id === id)!;
      const s = snapshots[id] ?? [];
      const stored = p.engine === "vault" || p.engine === "borg";
      const day = (t: number) => {
        const d = new Date(t);
        return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
      };
      // Calendar days, not 24-hour steps: the day the clocks go back is 25 hours long,
      // and stepping by hours named it twice (late in the evening, for a week after).
      const daysAgo = (k: number) => {
        const d = new Date(now);
        d.setDate(d.getDate() - k);
        return d.getTime();
      };
      const days = Array.from({ length: 30 }, (_, i) => ({ date: day(daysAgo(29 - i)), state: "none" as "ok" | "failed" | "none", bytes: 0 }));
      for (const x of s) {
        const d = days.find((d) => d.date === day(Date.parse(x.finished)));
        if (d) (d.state = "ok"), (d.bytes += (stored ? x.stats?.stored_bytes : x.stats?.copied_bytes) ?? 0);
      }
      if (id === "photos" && trouble) days[27]!.state = "failed";
      return {
        next_run: p.schedule.startsWith("manual") ? null : iso(now + (id === "docs" || id === "borg" ? 1 : 5) * 3_600_000),
        timer_active: !p.schedule.startsWith("manual"),
        last_good: s.at(-1)?.finished ?? null,
        health:
          id === "photos"
            ? [{ level: "warning", kind: "unplugged", text: "Archive isn't connected. The last backup to it was 9 days ago.", hint: "Plug in Archive to back up." }]
            : id === "plain" && !unusualDismissed
              ? [{ level: "danger", kind: "unusual", text: `Unusual changes in snapshot ${demoUnusual().snapshot}: ${demoUnusual().signals.join("; ")}.`, hint: "If you didn't make them, don't delete anything: the snapshot before them is kept. Otherwise choose “This was me”." }]
            : !trouble
              ? []
              : id === "home"
              ? [{ level: "warning", kind: "destination_failing", text: "The copy on Office NAS failed in the last backup.", hint: "Check that Office NAS is reachable; the other places are fine." }]
              : id === "docs"
              ? [{ level: "warning", kind: "source_missing", text: "The drive with scans wasn't connected for the last 2 backups. Its files were kept as they were before; changes since then aren't backed up.", hint: "Connect the drive with /mnt/scans before the next backup." }]
                : id === "borg"
                  ? [{ level: "warning", kind: "low_space", text: "Only 3.1 GiB free where the backups go.", hint: "Free up space (Maintenance), keep fewer snapshots, or use a bigger drive." }]
                  : [],
        unverified: id === "docs" || id === "plain",
        days,
      };
    }
    case "snapshots":
      needsUnlock();
      return structuredClone(snapshots[id] ?? []);
    case "list_dir":
      needsUnlock();
      return listing((args.path as string) || "/", (args.filter as string | null) ?? null, !!args.missing);
    case "unlock": {
      // The demo's recovery code (the one "recovery_code" shows), typed any which way.
      const code = String(args.password).toUpperCase().replace(/[^0-9A-Z]/g, "").replace(/O/g, "0").replace(/[IL]/g, "1");
      const byCode = code === mockRecoveryCode.replace(/-/g, "");
      if (args.password !== "demo" && !byCode) throw new ApiError("wrong_password", "Wrong password (try “demo”).");
      unlocked = new Set([...unlocked, id]);
      return { recovery_code: byCode };
    }
    case "replace_password":
      await delay(700);
      mockRecoveryCode = "3MZ8Q-HT5KW-R2NVD-9CJ4X-PB7FA-6YGE0";
      return { recovery_code: mockRecoveryCode, kept: "keyring", unlock_file: false, unlock_file_dropped: null, removed: (args.remove as string[]).length };
    case "backup_now":
      if (allProfiles().find((x) => x.id === id)?.running) throw new ApiError("busy", "A backup is already running.");
      if (id === "photos")
        throw new ApiError("not_connected", "Archive isn't connected: nothing is mounted at /run/media/me/Archive. Plug in the drive, then back up again.");
      simulateBackup(id, !!args.shutdown);
      return { running: true };
    case "shutdown_after":
      if (!allProfiles().find((x) => x.id === id)?.running) throw new ApiError("not_running", "This backup isn't running any more.");
      shutdownAfter[id] = !!args.on;
      return null;
    case "cancel_shutdown":
      return null;
    case "cancel":
      if (!allProfiles().find((x) => x.id === id)?.running) throw new ApiError("not_running", "This backup isn't running any more.");
      return null;
    case "restore":
      for (let i = 1; i <= 8; i++) {
        await delay(150);
        emit("flux://restore", { profile: id, progress: { phase: "transferring", scanned: 0, entries_total: 120, entries_done: i * 15, bytes_total: 18_402_113, bytes_done: (18_402_113 * i) / 8, warnings: 0, elapsed_secs: i * 0.15, bytes_per_sec: 9_000_000, eta_secs: (8 - i) * 0.15 } });
      }
      // Back in place: no longer missing.
      if (args.to == null) for (const p of args.paths as string[]) for (const d of [...deletedNow]) if (d === p || d.startsWith(p + "/")) deletedNow.delete(d);
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
    case "vault_format": {
      const p = allProfiles().find((x) => x.id === id);
      return p?.engine === "vault" ? { format: id === "photos" && !vaultUpgraded ? 1 : 2, newest: 2 } : null;
    }
    case "upgrade_vault":
      vaultUpgraded = true;
      return { format: 2, newest: 2 };
    case "storage": {
      needsUnlock();
      const p = allProfiles().find((x) => x.id === id);
      const n = (snapshots[id] ?? []).length;
      const latest = 86_400_000_000;
      if (p?.engine === "vault" || p?.engine === "borg")
        return {
          snapshots: n,
          latest_bytes: latest,
          latest_files: 184_000,
          logical_bytes: latest * n,
          stored_bytes: 121_000_000_000,
          dedup_saved: latest * n - 139_000_000_000,
          compression_saved: 18_000_000_000,
          shared_with: id === "docs" ? 1 : 0,
          basis: p.engine === "vault" ? "Measured from the Vault's index: each piece of data is stored once, compressed and encrypted." : "Borg's own statistics (borg info).",
        };
      return {
        snapshots: n,
        latest_bytes: latest,
        latest_files: 184_000,
        logical_bytes: latest * n,
        stored_bytes: 131_000_000_000,
        dedup_saved: latest * n - 131_000_000_000,
        compression_saved: null,
        shared_with: 0,
        basis: "Unchanged files are shared between snapshots (hard links); the space is what each snapshot copied, so it's an estimate.",
      };
    }
    case "templates": {
      const own = demoTemplates.map((t) => ({ ...t }));
      return args.system
        ? [
            { id: "system", name: "System settings", line: "This computer's configuration (/etc, /root, /usr/local, /opt and /srv), daily.", icon: "shield", own: false },
            { id: "blank", name: "Start from nothing", line: "Choose every folder and setting yourself.", icon: "plus", own: false },
            ...own,
          ]
        : [
            { id: "home", name: "Home folder", line: "Everything in your home folder, without caches and the trash. Daily.", icon: "home", own: false },
            { id: "documents", name: "Documents", line: "Your documents and desktop, every hour: lose an hour of work at most.", icon: "file", own: false },
            { id: "photos", name: "Photos", line: "Your pictures, daily, and kept for years: a snapshot a month for two years, one a year for ten.", icon: "layers", own: false },
            { id: "development", name: "Development", line: "Your projects, every hour, without dependencies and build output (node_modules, target, .venv…).", icon: "terminal", own: false },
            { id: "blank", name: "Start from nothing", line: "Choose every folder and setting yourself.", icon: "plus", own: false },
            ...own,
          ];
    }
    case "new_from_template": {
      const e = (await handle("new_profile", { system: args.system })) as { id: string; profile: Profile; issues: Issue[] };
      const p = e.profile;
      switch (args.template) {
        case "documents":
          Object.assign(p, { name: "Documents" });
          p.sources.paths = ["/home/me/Documents", "/home/me/Desktop"];
          p.sources.exclude = ["*.tmp", "~$*", ".~lock.*#"];
          p.schedule = { ...p.schedule, trigger: "interval", every: "1h", on_calendar: null };
          break;
        case "photos":
          Object.assign(p, { name: "Photos" });
          p.sources.paths = ["/home/me/Pictures"];
          p.sources.exclude = [".thumbnails/", "*.tmp"];
          break;
        case "development":
          Object.assign(p, { name: "Projects" });
          p.sources.paths = ["/home/me/Projects"];
          p.sources.exclude = ["node_modules/", "target/", "build/", "dist/", ".venv/", "__pycache__/", "*.o", "*.pyc", "*.tmp"];
          p.schedule = { ...p.schedule, trigger: "interval", every: "1h", on_calendar: null };
          break;
        case "blank":
          Object.assign(p, { name: "" });
          p.sources.paths = [];
          p.sources.exclude = [];
          break;
        case "home":
        case "system":
          break;
        default:
          Object.assign(p, { name: "Work" });
          p.sources.paths = ["/home/me/work", "/home/me/Documents"];
      }
      return e;
    }
    case "save_template": {
      const name = args.name as string;
      demoTemplates.push({ id: `own:${name.toLowerCase().replace(/[^a-z0-9]+/g, "-")}`, name, line: "/home/me/work, /home/me/Documents. Daily.", icon: "copy", own: true });
      return handle("templates", {});
    }
    case "delete_template": {
      const i = demoTemplates.findIndex((t) => t.id === args.template);
      if (i >= 0) demoTemplates.splice(i, 1);
      return handle("templates", { system: args.system });
    }
    case "backintime_profiles": {
      // Demo: Back In Time is set up, with two profiles (?bit=0 hides them).
      if (typeof location !== "undefined" && new URLSearchParams(location.search).get("bit") === "0") return [];
      const base = fullProfile({ id: "", scope: "user", name: "Main profile", engine: "rsync", target: "/run/media/me/BackupDisk", remote: false, schedule: "daily", errors: [], last_run: null, running: false });
      base.target = { kind: "local", path: "/run/media/me/BackupDisk", fs_uuid: null };
      base.sources.paths = ["/home/me/Documents", "/home/me/Pictures", "/home/me/Music", "/home/me/.config"];
      base.sources.exclude = ["**/.cache/*", "*.swp", "**/.local/share/[Tt]rash*", "\\#*#", "node_modules/"];
      base.schedule = { ...base.schedule, trigger: "calendar", on_calendar: "*-*-* 07:00:00" };
      base.retention = { ...base.retention, keep_within: "2days", keep_daily: 14, keep_weekly: 8, keep_monthly: 12, keep_yearly: 0, max_age: "2years" };
      const second = structuredClone(base);
      second.name = "Work laptop";
      second.engine = "vault";
      second.vault = fullProfile({ id: "", scope: "user", name: "", engine: "vault", target: "", remote: true, schedule: "daily", errors: [], last_run: null, running: false }).vault;
      second.sources.paths = ["/home/me/Projects"];
      second.target = { kind: "ssh", host: "nas", user: "me", port: null, path: "/srv/bit", identity_file: null, ssh_options: [], bandwidth_limit: null };
      return [
        { number: 1, name: "Main profile", mode: "local", profile: base, notes: ["Some exclusions are written differently in Flux, with the same meaning: .cache/* → **/.cache/*, #*# → \\#*#."], snapshots_dir: "/run/media/me/BackupDisk/backintime/laptop/me/1", snapshot_count: 41, latest: "20261108-070002-511", connected: true },
        { number: 2, name: "Work laptop", mode: "ssh_encfs", profile: second, notes: ["Back In Time encrypted these backups with EncFS, which Flux doesn't use. They'll be an encrypted Vault instead: choose its password on the Protection step."], snapshots_dir: null, snapshot_count: 0, latest: null, connected: true },
      ];
    }
    case "validate_profile":
      return validate(args.profile as Profile);
    case "save_profile": {
      const p = args.profile as Profile;
      const issues = validate(p);
      if (issues.some((i) => i.severity === "error")) return { id, issues, saved: false, schedule: null };
      stored[id] = structuredClone(p);
      const existing = allProfiles().find((x) => x.id === id);
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
    case "folder_sizes": {
      await delay(300);
      const known: Record<string, number> = { "/home/me": 412_000_000_000, "/home/me/Documents": 18_400_000_000, "/home/me/Pictures": 96_300_000_000, "~/Videos/": 210_000_000_000, "~/.cache/": 6_204_000_000, "~/Downloads/": 23_400_000_000 };
      return (args.paths as string[]).map((path) => {
        const k = Object.keys(known).find((x) => x.replace(/\/$/, "") === path.replace(/\/$/, ""));
        return { path, size: k ? known[k]! : /[*?[]/.test(path) || !/^[~/]/.test(path) ? null : 1_300_000_000, partial: path.replace(/\/$/, "") === "/home/me" };
      });
    }
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
        { mount_point: "/run/media/me/USB STICK", label: "USB STICK", fs_uuid: "5E2D-91AA", fs_type: "vfat", free: 1_300_000_000, total: 32_000_000_000 },
      ];
    case "test_target":
      await delay(600);
      return { ok: true, messages: ["logged in to me@nas", "rsync is available", "/srv/flux is writable", "1.8 TiB free"] };
    case "update_snapshot": {
      const snap = snapshots[id]?.find((x) => x.id === args.snapshot);
      if (!snap) throw new ApiError("error", "snapshot not found");
      if (args.setName) snap.name = (args.name as string | null) || null;
      if (args.pinned != null) snap.pinned = args.pinned as boolean;
      if (args.setNote) snap.note = ((args.note as string | null) ?? "").trim() || null;
      if (args.tags) snap.tags = [...new Set((args.tags as string[]).map((t) => t.trim()).filter(Boolean))];
      return structuredClone(snap);
    }
    case "browse_in_files": {
      await delay(500);
      mockBrowsing = { profile: id, snapshot: args.snapshot as string, dir: `/run/user/1000/flux-backup/browse/${id}@${args.snapshot}`, open: `/run/user/1000/flux-backup/browse/${id}@${args.snapshot}/home/me` };
      return mockBrowsing;
    }
    case "take_open_request": {
      // Demo only: `?open=versions:/home/me/notes.md` acts as a file manager's request.
      const q = typeof location !== "undefined" ? new URLSearchParams(location.search).get("open") : null;
      if (!q || mockOpenTaken) return null;
      mockOpenTaken = true;
      const [kind, ...rest] = q.split(":");
      return { kind, path: rest.join(":") };
    }
    case "backup_for_path":
      return (args.path as string).startsWith("/home/me") ? "home" : null;
    case "folder_plan": {
      // Demo: every backup of your own takes /home/me (the home folder).
      await delay(300);
      const path = (args.path as string).replace(/\/+$/, "");
      const shown = path.startsWith("/home/me") ? "~" + path.slice(8) : path;
      const inHome = path.startsWith("/home/me/");
      const dir = !/\.[a-z0-9]{1,5}$/i.test(path);
      const mine = profiles.filter((p) => p.scope === "user");
      const want = args.want as "exclude" | "include";
      return {
        path,
        shown,
        is_dir: dir,
        size: 2_400_000_000,
        partial: false,
        backups: mine
          .filter(() => want === "include" || inHome)
          .map((p) => ({
            id: p.id,
            name: p.name,
            backed_up: inHome,
            why: inHome ? "Backed up: it's in /home/me." : "Not in any folder this backup takes.",
            change: want === "exclude" ? { kind: "add_rule", rule: `${shown}${dir ? "/" : ""}` } : inHome ? null : { kind: "add_folder", folder: path, replaces: [] },
            blocked: null,
          })),
      };
    }
    case "apply_folder_change":
      await delay(400);
      return { changed: (args.ids as string[]).map((id) => [id, profiles.find((p) => p.id === id)?.name ?? id]), failed: [] };
    case "recovery_code":
      await delay(600);
      return args.remove ? null : mockRecoveryCode;
    case "browsing":
      return mockBrowsing;
    case "stop_browsing":
      mockBrowsing = null;
      return null;
    case "delete_snapshots": {
      const asked = new Set(args.snapshots as string[]);
      const list = snapshots[id] ?? [];
      const pinned = list.filter((x) => asked.has(x.id) && x.pinned).map((x) => x.id);
      const gone = list.filter((x) => asked.has(x.id) && !x.pinned);
      snapshots[id] = list.filter((x) => !gone.includes(x));
      const e = allProfiles().find((p) => p.id === id)?.engine;
      const freed = e === "vault" || e === "borg" ? gone.reduce((t, x) => t + x.stats.stored_bytes, 0) : null;
      return { deleted: gone.map((x) => x.id), pinned, freed };
    }
    case "delete_snapshot": {
      const gone = snapshots[id]?.find((x) => x.id === args.snapshot);
      snapshots[id] = (snapshots[id] ?? []).filter((x) => x.id !== args.snapshot);
      const e = allProfiles().find((p) => p.id === id)?.engine;
      return e === "vault" || e === "borg" ? (gone?.stats.stored_bytes ?? 0) : null;
    }
    case "compare":
      await delay(400);
      return {
        old: args.old,
        new: args.new ?? "now",
        summary: { added: 3, removed: 1, modified: 6, type_changed: 0, bytes_delta: 2_340_112 },
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
          { path: "/home/me/Documents", change: "modified", kind: "dir", fields: ["mtime"], old_size: 0, new_size: 0 },
          { path: "/home/me/Pictures/2026", change: "modified", kind: "dir", fields: ["mtime"], old_size: 0, new_size: 0 },
        ],
        left_out_by_size: args.new ? 0 : 2,
        size_limit: args.new ? null : "4GB",
      };
    case "runs": {
      const vault = allProfiles().find((x) => x.id === id)?.engine === "vault";
      const perf = (i: number) => ({
        phases: [
          { name: "Preparing", secs: 0.4 },
          { name: "Scanning", secs: 6.2 + (i % 3) },
          { name: "Comparing", secs: 1.1 },
          { name: "Copying", secs: 38.5 + i * 2 },
          { name: "Finalizing", secs: 2.3 },
        ],
        stages: vault
          ? [
              { name: "Reading files", secs: 21.4 },
              { name: "Cutting into chunks", secs: 9.8 },
              { name: "Finding what's already stored", secs: 6.1 },
              { name: "Compressing and encrypting", secs: 44.7 },
              { name: "Writing and uploading", secs: 12.9 },
            ]
          : [],
        writes: vault ? 46 : 0,
        bytes_written: vault ? 731_000_000 : 0,
        write_ms: vault ? 280 : null,
        throughput: 58_000_000,
      });
      const list = [...(snapshots[id] ?? [])]
        .reverse()
        .map((x, i) => (i === 2 ? { ...run(id, x, "unchanged"), stats: null } : run(id, x, x.status === "complete" ? "completed" : "completed_with_errors")))
        .map((r, i): RunRecord => ({ ...r, performance: perf(i) }));
      const p = allProfiles().find((x) => x.id === id);
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
    case "preview_file": {
      await delay(250);
      const p = String(args.path);
      if (/\.(png|jpe?g|gif|webp|svg)$/i.test(p))
        return {
          kind: "image",
          url: `data:image/svg+xml,${encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" width="640" height="420"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#3a6ea5"/><stop offset="1" stop-color="#f2b880"/></linearGradient></defs><rect width="640" height="420" fill="url(#g)"/><circle cx="470" cy="120" r="54" fill="#fff6d8"/><path d="M0 330 L160 210 L290 300 L420 190 L640 340 V420 H0Z" fill="#2c3e50"/></svg>')}`,
        };
      if (/\.(md|txt|toml|conf|rc|sh|py|rs|ts|json|gitconfig|bashrc)$|\/\.[a-z]+$/i.test(p))
        return {
          kind: "text",
          text: `# ${p.split("/").pop()}\n\nAs it was in this snapshot.\n\n[user]\n    name = Me\n    email = me@example.org\n\n[core]\n    editor = nano\n`,
          more: false,
        };
      return { kind: "other", too_large: false };
    }
    case "open_snapshot_file":
      await delay(400);
      return `/home/me/.cache/flux-backup/opened/${args.snapshot}/${(args.path as string).split("/").pop()}`;
    case "duplicate_profile": {
      const src = allProfiles().find((x) => x.id === id)!;
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
          hits.push({ path, kind, versions, snapshots: snaps.length, latest: snaps.at(-1)!.id, latest_size: size, latest_mtime: Math.floor((now - 3 * DAY) / 1000), on_disk: !deletedNow.has(path) });
        }
      }
      // A file that was deleted a while ago: only in older snapshots.
      if ("notes-old.md".includes(q) && snaps.length > 6) {
        hits.push({ path: "/home/me/notes-old.md", kind: "file", versions: 2, snapshots: snaps.length - 5, latest: snaps.at(-6)!.id, latest_size: 7_441_870, latest_mtime: Math.floor((now - 9 * DAY) / 1000), on_disk: !deletedNow.has("/home/me/notes-old.md") });
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
    case "set_diff_tool":
      prefs = { ...prefs, diff_tool: (args.tool as string | null) ?? null };
      return prefsView();
    case "open_diff_tool":
      return null;
    case "diff_file": {
      await delay(300);
      const lines = [
        { kind: "skip", old: null, new: null, text: "", count: 12 },
        { kind: "same", old: 13, new: 13, text: "## Groceries", count: 1 },
        { kind: "same", old: 14, new: 14, text: "- oat milk", count: 1 },
        { kind: "removed", old: 15, new: null, text: "- bread (the seeded one)", count: 1 },
        { kind: "added", old: null, new: 15, text: "- bread", count: 1 },
        { kind: "added", old: null, new: 16, text: "- coffee beans, 2 bags", count: 1 },
        { kind: "same", old: 16, new: 17, text: "- apples", count: 1 },
        { kind: "same", old: 17, new: 18, text: "", count: 1 },
        { kind: "skip", old: null, new: null, text: "", count: 30 },
        { kind: "same", old: 48, new: 49, text: "## Call back", count: 1 },
        { kind: "removed", old: 49, new: null, text: "- the dentist (Tuesday)", count: 1 },
        { kind: "same", old: 50, new: 50, text: "- Sam, about the trip", count: 1 },
      ];
      return { old_file: "/tmp/a", new_file: "/tmp/b", old_size: 1820, new_size: 1838, identical: args.old === args.new, binary: false, too_large: false, added: 2, removed: 2, lines: args.old === args.new ? [] : lines, truncated: 0 };
    }
    case "save_preferences":
      prefs = {
        ...prefs,
        notify_success: args.notifySuccess as boolean,
        default_exclude: (args.defaultExclude as string[] | null) ?? null,
        one_at_a_time: (args.oneAtATime as boolean | undefined) ?? prefs.one_at_a_time,
      };
      return prefsView();
    case "set_scheduler":
      await delay(400);
      prefs = { ...prefs, scheduler: args.scheduler as "systemd" | "cron" };
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
    case "set_close_to_tray":
      prefs = { ...prefs, close_to_tray: args.on as boolean };
      return prefsView();
    case "set_quiet_hours":
      if (args.from && args.from === args.to) throw new ApiError("bad_request", "Quiet hours need a start and an end, at different times of day.");
      prefs = { ...prefs, quiet_hours: args.from && args.to ? { from: args.from as string, to: args.to as string } : null };
      return prefsView();
    case "set_weekly_note":
      prefs = { ...prefs, weekly_note: args.on as boolean };
      return prefsView();
    case "tray_available":
      return true;
    case "set_text_size":
      // The demo has no window to zoom: the page is zoomed instead.
      if (typeof document !== "undefined") document.documentElement.style.setProperty("zoom", String(args.scale));
      return null;
    case "desktop_accent": {
      // The demo: `?accent=2190a4` stands in for the desktop's choice.
      const a = typeof location !== "undefined" ? new URLSearchParams(location.search).get("accent") : null;
      const n = a && /^#?[0-9a-f]{6}$/i.test(a) ? parseInt(a.replace("#", ""), 16) : null;
      return n == null ? null : [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
    }
    case "calendar_preview": {
      // The demo reads "[days] HH:MM" only; the app reads what systemd does.
      const m = /^(?:([A-Za-z,.]+)\s+)?(?:\*-\*-\*\s+)?(\d{1,2}):(\d{2})(?::00)?$/.exec(String(args.expr).trim());
      if (!m) throw new Error("not a calendar expression Flux can read");
      const names = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];
      const ok = (d: Date) => {
        if (!m[1]) return true;
        const day = names[d.getDay()]!;
        return m[1].toLowerCase().split(",").some((part) => {
          const [a, b] = part.split("..");
          if (!b) return part.slice(0, 3) === day;
          const i = names.indexOf(a!.slice(0, 3)), j = names.indexOf(b.slice(0, 3)), k = d.getDay();
          return i <= j ? k >= i && k <= j : k >= i || k <= j;
        });
      };
      const out: string[] = [];
      const d = new Date();
      d.setHours(Number(m[2]), Number(m[3]), 0, 0);
      for (let n = 0; out.length < 3 && n < 30; n++, d.setDate(d.getDate() + 1)) if (d.getTime() > Date.now() && ok(d)) out.push(d.toISOString());
      return out;
    }
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
        remove: args.exact ? 2 : 0,
        remove_paths: args.exact ? ["/home/me/Documents/draft-copy.odt", "/home/me/Documents/scratch"] : [],
      };
    }
    case "diagnostics":
      return `Flux Backup diagnostics\n\nFlux Backup ${__APP_VERSION__} (demo)\n--- profile ${id} ---\n…`;
    case "about":
      return {
        version: __APP_VERSION__,
        cli: { path: "/usr/bin/flux", version: __APP_VERSION__ },
        helper: { path: "/usr/libexec/flux-backup/flux-helper", version: __APP_VERSION__ },
        mismatch: false,
        os: "Fedora Linux 45 (Workstation Edition)",
        kernel: "7.2.15-300.fc45.x86_64",
        webkit: "2.54.0",
      };
    case "open_link":
      return null;
    case "time_hashing": {
      const h = args.hashing as { level: string; memory_mib?: number; passes?: number };
      const cost = h.level === "strong" ? 4 * 4 : h.level === "maximum" ? 16 * 6 : h.level === "custom" ? ((h.memory_mib ?? 64) / 64) * (h.passes ?? 3) : 3;
      return Math.round(cost * 95);
    }
    case "vault_security": {
      const p = allProfiles().find((x) => x.id === id);
      const v = p?.engine === "vault" ? (stored[id]?.vault ?? null) : null;
      if (!v) throw new ApiError("error", "Only encrypted Vault backups have these settings.");
      const open = unlocked.has(id);
      return {
        keys: [
          { id: "8f3a51c2d07e4b19", label: "initial", created: "2026-10-02T09:14:00Z", current: open, hashing: { level: "standard" }, unlock_file: !!v.unlock_file },
          { id: "c41d9e07a2b65f38", label: "spare, in the safe", created: "2026-10-02T09:20:00Z", current: false, hashing: { level: "standard" }, unlock_file: false },
        ],
        unlocked: open,
        hashing: v.hashing ?? { level: "standard" },
        unlock_file: v.unlock_file ?? null,
        unlock_file_present: !!v.unlock_file,
        unlock_ms: 290,
        cipher: "XAES-256-GCM",
      };
    }
    case "strengthen_key":
      if (!unlocked.has(id)) throw new ApiError("password_required", "Unlock the backups first: this needs the Vault's password.");
      return null;
    case "set_unlock_file":
      if (!unlocked.has(id)) throw new ApiError("password_required", "Unlock the backups first: this needs the Vault's password.");
      return { unlock_file: args.path ?? null, without_file: args.path ? [{ id: "c41d9e07a2b65f38", label: "spare, in the safe", created: "2026-10-02T09:20:00Z", current: false, hashing: { level: "standard" }, unlock_file: false }] : [] };
    case "restore_test": {
      needsUnlock();
      const snap = snapshots[id]?.at(-1);
      // As the profile says: sized by what it must prove (the large-backup count), or so many files.
      const t = stored[id]?.restore_test ?? {};
      const files = 184_000;
      const sure = t.mode === "confidence";
      const n = sure ? sampleBound(t.damage, t.certainty) : t.mode === "random_files" || !t.mode ? (t.files ?? 10) : 2_300;
      const worth = sure ? { files, damaged: Math.ceil(files * fraction(t.damage, 0.01)), damage: t.damage ?? "1%", certainty: t.certainty ?? "99%" } : t.mode === "random_files" || !t.mode ? { files, damaged: Math.ceil(files * (1 - 0.01 ** (1 / n))), damage: `${Math.ceil(100 * (1 - 0.01 ** (1 / n)))}%`, certainty: "99%" } : null;
      const report = { started: new Date().toISOString(), finished: new Date().toISOString(), snapshot: snap?.id ?? "", mode: t.mode ?? "random_files", result: "passed", picked: n, tested: n, bytes: n * 471_859, stopped_early: false, problems: [], problems_total: 0, guarantee: worth };
      restoreTests[id] = [report, ...(restoreTests[id] ?? [])];
      readinessAnswers[`${id}:tested`] = true;
      return report;
    }
    case "set_webhooks":
      prefs = { ...prefs, webhooks: args.webhooks as Webhook[] };
      return prefsView();
    case "test_webhook":
      return 200;
    case "explain": {
      const prof = args.profile as Profile;
      const path = (args.path as string).replace(/^~(?=\/|$)/, "/home/me");
      if (!path.startsWith("/")) throw new ApiError("error", "Type a full path, starting with / or ~/.");
      const v = demoVerdict(prof, path);
      return { path, backed_up: v.in, text: v.text };
    }
    case "coverage": {
      const prof = args.profile as Profile;
      const dir = (args.dir as string).replace(/\/$/, "") || "/";
      const here = demoVerdict(prof, dir);
      const kids = DEMO_TREE[dir] ?? [];
      await new Promise((r) => setTimeout(r, 200));
      const items = kids.map(([name, isDir, size]) => {
        const path = `${dir}/${name}`;
        const v = here.in ? demoVerdict(prof, path) : { in: false, text: here.text };
        const out = !v.in;
        const total = isDir ? size : 0;
        const innerOut = isDir && v.in && name === "Projects" ? 412_000_000 : 0;
        return {
          name,
          path,
          dir: isDir,
          size: isDir ? 0 : size,
          backed_up: v.in,
          why: v.text,
          files_in: out ? 0 : isDir ? Math.round(total / 400_000) + 1 : 1,
          bytes_in: out ? 0 : total - innerOut || (isDir ? 0 : size),
          files_out: out ? (isDir ? Math.round(total / 400_000) + 1 : 1) : innerOut ? 18_400 : 0,
          bytes_out: out ? total || size : innerOut,
        };
      });
      return { path: dir, backed_up: here.in, why: here.text, items, partial: false };
    }
    case "export_snapshot": {
      const zip = args.format === "zip";
      const paths = args.paths as string[];
      const what = paths.length === 1 ? `-${paths[0]!.split("/").pop()}` : "";
      const archive = `${args.dir}/${id}-2026-11-09-0830${what}.${zip ? "zip" : "tar.zst"}`;
      await new Promise((r) => setTimeout(r, 600));
      return {
        archive,
        size: zip ? 48_200_000 : 41_700_000,
        files: 1_284,
        bytes: 63_400_000,
        manifest: args.checksums ? `${archive}.${args.checksums === "sha256" ? "sha256" : "b3"}` : null,
        left_out: zip ? ["3 symbolic links left out"] : [],
        errors: [],
      };
    }
    case "find_backups": {
      // A drive with an encrypted Vault (password "demo") and a Mirror backup.
      const vaultOpen = args.password === "demo";
      const t = args.target as { kind: string; path?: string };
      const where = t.kind === "local" ? t.path : "server";
      const found = [
        {
          engine: "mirror",
          kept: { host: "old-laptop", user: "me", profile: "photos" },
          location: `${where}/flux-backup/old-laptop/me/photos`,
          name: "Photo library",
          sources: ["/home/me/Pictures"],
          saved: true,
          snapshots: 41,
          newest: iso(Date.now() - 3 * 86_400_000),
          here: false,
        },
        ...(vaultOpen
          ? [
              {
                engine: "vault",
                kept: { host: "old-laptop", user: "me", profile: "home" },
                location: `${where}/flux-vault`,
                name: "Home folder",
                sources: ["/home/me"],
                saved: true,
                snapshots: 128,
                newest: iso(Date.now() - 2 * 86_400_000),
                here: false,
              },
            ]
          : []),
      ];
      return { found, vault_locked: !vaultOpen, wrong_password: !!args.password && !vaultOpen };
    }
    case "recover":
      return { id: (args.kept as { profile: string }).profile === "photos" ? "photos" : "home", note: null };
    case "readiness":
    case "set_readiness":
    case "export_borg_key": {
      if (cmd === "set_readiness") readinessAnswers[`${id}:${args.key}`] = args.yes as boolean;
      if (cmd === "export_borg_key") {
        await delay(500);
        readinessAnswers[`${id}:key_exported`] = true;
        return `${args.dir as string}/borg-key-${id}.txt`;
      }
      const p = profiles.find((x) => x.id === id);
      const encrypted = p?.engine === "vault" || p?.engine === "borg";
      const asked = (k: string) => readinessAnswers[`${id}:${k}`] ?? false;
      const tested = id === "home" || readinessAnswers[`${id}:tested`];
      return [
        { key: "settings", ok: true, asked: false, title: "Its settings are kept with the backups", detail: `A copy of the folders, exclusions and rules, ${p?.engine === "vault" ? "in the Vault, encrypted" : "next to them"}: on a new computer, Recover an existing backup finds it.` },
        ...(encrypted
          ? [{ key: "password_elsewhere", ok: asked("password_elsewhere"), asked: true, title: "The password is kept somewhere other than this computer", detail: "Without it, nobody can open these backups, you included. It's in this computer's keyring, which goes with the computer: keep it in a password manager or on paper too." }]
          : []),
        ...(p?.engine === "borg"
          ? [{ key: "key_exported", ok: asked("key_exported"), asked: true, title: "The Borg key is exported", detail: "If the repository's key is damaged, the passphrase alone can't open it. Export the key (borg key export ssh://me@nas/srv/flux/borg borg-key.txt) and keep the file with the passphrase." }]
          : []),
        { key: "restore_test", ok: !!tested, asked: false, title: "A restore test has passed", detail: tested ? "Files were last read back and checked on 2 November 2026." : "Flux reads a few files back from the backups and checks every byte against what was stored." },
        // As the backend says it: this place, or one of its other places, is kept elsewhere.
        (stored[id]?.offsite || stored[id]?.destinations?.some((d) => d.offsite))
          ? { key: "offsite", ok: true, asked: false, title: "A copy is kept somewhere else", detail: "A fire or a theft wouldn't take every copy." }
          : { key: "offsite", ok: false, asked: false, title: "A copy is kept somewhere else", detail: "A fire or a theft takes a computer and the drive next to it together. Add a place that's kept elsewhere: another drive you keep away from home, or a server." },
      ];
    }
    case "reveal_password":
      await delay(200);
      return "demo";
    case "copy_unlock_file":
      return `${args.dir as string}/stick.key`;
    case "dismiss_unusual":
      unusualDismissed = true;
      return null;
    case "restore_tests":
      return restoreTests[id] ?? [];
    case "add_destination": {
      const d = args.destination as Destination;
      const base = stored[id]!;
      base.destinations = [...(base.destinations ?? []), d];
      const summary = profiles.find((x) => x.id === id)!;
      summary.destinations = [...(summary.destinations ?? []), { id: `${id}+${d.name}`, name: d.name, label: d.label ?? d.name, engine: d.engine, target: d.target.kind === "local" ? d.target.path : `ssh://${d.target.host}${d.target.path}`, remote: d.target.kind === "ssh", offsite: !!d.offsite }];
      return structuredClone(base);
    }
    case "remove_destination": {
      const base = stored[id]!;
      base.destinations = (base.destinations ?? []).filter((x) => x.name !== args.name);
      const summary = profiles.find((x) => x.id === id)!;
      summary.destinations = (summary.destinations ?? []).filter((x) => x.name !== args.name);
      return structuredClone(base);
    }
    case "lock_vault":
      unlocked.delete(id);
      return null;
    case "export_formats":
      return [
        { id: "flux", label: "Flux Backup profile", description: "This profile as a file, to import on another computer or keep as a copy.", works_with: "Flux Backup (flux profile import)" },
        { id: "borgmatic", label: "borgmatic configuration", description: "The same backup as a borgmatic config: Borg with schedules, retention and checks.", works_with: "borgmatic 1.8 or newer, and Borg" },
        { id: "restic", label: "restic script", description: "A script that runs the same backup with restic, including retention.", works_with: "restic 0.16 or newer" },
        { id: "rsync", label: "rsync script", description: "A script that runs the same backup with rsync: plain-folder snapshots.", works_with: "rsync 3.1 or newer" },
        { id: "borg", label: "Borg script", description: "A script that runs the same backup with borg create.", works_with: "Borg 1.2 to 1.4" },
      ];
    case "export_profile": {
      const p = allProfiles().find((x) => x.id === id)!;
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
