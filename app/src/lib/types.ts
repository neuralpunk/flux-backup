// Shapes of the JSON exchanged with the Rust side (see app/src-tauri/src/commands.rs).

export type Engine = "mirror" | "vault" | "rsync" | "borg";
/** Compatibility mode: the backups are written by a standard tool, in its own format. */
export const isCompat = (e: Engine | undefined | null) => e === "rsync" || e === "borg";
export const engineLabel: Record<Engine, string> = { mirror: "Mirror", vault: "Vault", rsync: "rsync", borg: "Borg" };
/** How backups are stored, in plain words first (the engine's name is for settings and
 * the command line). */
export const storageWords: Record<Engine, string> = { mirror: "Plain folders", vault: "Encrypted", rsync: "rsync folders", borg: "Borg repository" };
export type Kind = "dir" | "file" | "symlink" | "fifo" | "char_device" | "block_device" | "socket";
export type SnapshotStatus = "complete" | "complete_with_errors";
export type RunStatus =
  | "completed"
  | "completed_with_errors"
  | "unchanged"
  | "dry_run"
  | "skipped"
  | "failed"
  | "cancelled";
export type Phase = "preparing" | "scanning" | "planning" | "transferring" | "finalizing" | "done" | "waiting";

export interface SnapshotStats {
  entries: number;
  files: number;
  dirs: number;
  symlinks: number;
  specials: number;
  total_bytes: number;
  new: number;
  changed: number;
  unchanged: number;
  deleted: number;
  copied_bytes: number;
  copied_files: number;
  linked_files: number;
  stored_bytes: number;
}

export interface FileError {
  path: string;
  message: string;
}

export interface SnapshotMeta {
  id: string;
  profile: string;
  host: string;
  user: string;
  started: string;
  finished: string;
  status: SnapshotStatus;
  name: string | null;
  pinned: boolean;
  sources: string[];
  stats: SnapshotStats;
  errors: FileError[];
  errors_total: number;
  flux_version: string;
  /** Sources whose drive wasn't connected for this snapshot. */
  sources_missing?: { path: string; kept_from?: string | null }[];
  /** The user's note; never affects retention. */
  note?: string | null;
  /** The user's tags; never affect retention. */
  tags?: string[];
}

export interface RunRecord {
  run_id: string;
  profile: string;
  trigger: "manual" | "scheduled";
  started: string;
  finished: string;
  status: RunStatus;
  snapshot: string | null;
  pruned: string[];
  error: string | null;
  stats: SnapshotStats | null;
  warnings: number;
  /** Sources whose drive wasn't connected. */
  sources_missing?: string[];
  /** How the backup went at the profile's other destinations. */
  destinations?: DestinationRun[];
  /** Where the time went (the performance inspector). */
  performance?: Performance | null;
}

export interface DestinationRun {
  name: string;
  status: RunStatus;
  snapshot?: string | null;
  error?: string | null;
  stats?: SnapshotStats | null;
}

/** One more place a profile's backups go (browse it as its own id, `home+nas`). */
export interface DestinationSummary {
  id: string;
  name: string;
  label: string;
  engine: Engine;
  target: string;
  remote: boolean;
  offsite: boolean;
  /** A copy of the main destination's backup. */
  replica?: boolean;
}

export interface ProfileSummary {
  /** System profiles' ids start with `system:`. */
  id: string;
  /** Your own profile, or a system-wide one (runs as root). */
  scope: "user" | "system";
  name: string;
  engine: Engine;
  /** Human-readable target, e.g. "/run/media/me/Backup" or "ssh://nas/srv/backup". */
  target: string;
  remote: boolean;
  /** What it backs up, home shown as `~` (older helpers don't send it). */
  sources?: string[];
  schedule: string;
  /** Files larger than this are left out ("500MB"); absent without a limit. */
  size_limit?: string | null;
  /** Validation errors (empty when the profile is usable). */
  errors: string[];
  last_run: RunRecord | null;
  running: boolean;
  /** The main destination is kept away from here. */
  offsite?: boolean;
  /** More places the backups go. */
  destinations?: DestinationSummary[];
  /** Flux Sentry: how unusual changes are watched for (older helpers don't send it). */
  sentry?: "off" | "normal" | "high";
  /** On hold: Flux Sentry found unusual changes that aren't settled; no backup is made
   * until they are. */
  held?: boolean;
}

export interface Overview {
  profile: ProfileSummary;
  snapshot_count: number;
  oldest: string | null;
  latest: SnapshotMeta | null;
  recent_runs: RunRecord[];
  /** RFC 3339. */
  next_run: string | null;
  timer_active: boolean;
  free_bytes: number | null;
  /** Why snapshots couldn't be read (e.g. the drive isn't connected). */
  repo_error: string | null;
  /** Things worth knowing, in words. */
  health: Health[];
  /** Free space, its history and forecast (local targets). */
  space: Space | null;
  /** Restore tests: the setting and the latest. */
  restore_test?: { every: TestEvery; last: TestReport | null; last_passed: string | null };
  /** Unusual changes found and not yet dismissed. */
  unusual?: UnusualReport | null;
  /** What Flux Sentry has learned of this backup: the share of its files that usually
   * changes, the share at which it would warn, and how many backups it learned from. */
  sentry?: { usual: number; warns_at: number | null; runs: number } | null;
}

/** What unusual-change detection found in a backup. */
export interface UnusualReport {
  signals: string[];
  affected: string[];
  affected_total: number;
  by_folder: [string, number][];
  by_extension: [string, number][];
  /** The snapshot with the changes, and the one before it (kept). */
  snapshot: string;
  before?: string | null;
  /** Seen in the files before the backup was written: it wasn't made, and `snapshot`
   * names the alarm, not a snapshot. */
  stopped?: boolean;
  /** The snapshot from before is pinned (false: there was none, or it couldn't be). */
  before_pinned?: boolean;
  /** Flux pinned it for this alarm. */
  pinned_by_flux?: boolean;
  /** Flux Sentry's reading of the amount of change (when it had history to judge by). */
  sentry?: { usual: number; share: number; times: number; score: number; runs: number; seen: number; unusual: boolean } | null;
}

export type TestEvery = "off" | "backup" | "week" | "month";
export type TestMode = "random_files" | "percent" | "full" | "rolling" | "confidence";

/** `[restore_test]` in a profile. */
export interface RestoreTestSettings {
  every?: TestEvery;
  mode?: TestMode;
  files?: number;
  percent?: number;
  /** `confidence`: the share of damaged files a test must not miss ("1%")… */
  damage?: string;
  /** …and how sure it must be of catching it ("99%"). */
  certainty?: string;
  window?: string | null;
  max_runtime?: string | null;
  max_read?: string | null;
  compare_with_source?: boolean;
}

export interface TestProblem {
  path: string;
  problem: "could_not_restore" | "size_differs" | "content_differs" | "metadata_differs" | "differs_from_source";
  detail?: string | null;
}

/** What a restore test that passed proves: had `damaged` or more of the snapshot's
 * `files` (a share `damage`) been damaged, it would have found it, `certainty` sure. */
export interface TestGuarantee {
  files: number;
  damaged: number;
  damage: string;
  certainty: string;
}

/** A restore test's report. */
export interface TestReport {
  started: string;
  finished: string;
  snapshot: string;
  mode: TestMode;
  result: "passed" | "warning" | "failed";
  picked: number;
  tested: number;
  bytes: number;
  stopped_early: boolean;
  problems: TestProblem[];
  problems_total: number;
  error?: string | null;
  guarantee?: TestGuarantee | null;
}

/** A health warning, in words. */
export interface Health {
  level: "warning" | "danger";
  /** What it's about: overdue, failing, low_space, unplugged, source_missing,
   * destination_failing, restore_test_failed, unverified, unusual. */
  kind?: string;
  text: string;
  /** What to do about it. */
  hint: string | null;
}

/** One local calendar day of a backup's history. */
export interface Day {
  /** YYYY-MM-DD. */
  date: string;
  state: "ok" | "failed" | "none";
  /** Copied (Mirror, rsync) or stored (Vault, Borg) that day. */
  bytes: number;
}

/** How a backup is doing, without opening its repository (no password needed). */
export interface BackupStatus {
  /** RFC 3339. */
  next_run: string | null;
  timer_active: boolean;
  /** The last backup that kept a copy. RFC 3339. */
  last_good: string | null;
  health: Health[];
  /** The last 30 days, oldest first. */
  days: Day[];
  /** Restore tests are on and none passed within their interval. */
  unverified?: boolean;
}

/** What a profile being set up would back up. */
export interface Estimate {
  files: number;
  folders: number;
  bytes: number;
  unreadable: number;
  missing: string[];
  free: number | null;
}

export interface Space {
  free: number;
  size: number | null;
  /** Oldest first, ending with now. */
  points: { t: string; free: number }[];
  /** Change in free bytes per day (negative: filling up); null without enough history. */
  per_day: number | null;
  days_left: number | null;
}

export interface DirEntry {
  name: string;
  path: string;
  kind: Kind;
  size: number;
  mtime: number;
  mode: number;
  link: string | null;
  /** Number of direct children, for directories. */
  children: number | null;
  /** Whether it's at its original place now: `false` when deleted or moved since the
   * backup (null/absent: couldn't check). */
  on_disk?: boolean | null;
}

export interface Listing {
  path: string;
  /** The items asked for, folders first then by name: the first 2,000 when there are more. */
  entries: DirEntry[];
  /** Items in the folder. */
  total: number;
  /** Items the filter lets through (`entries` is the first of them). */
  matching: number;
  /** Items in the folder that aren't on the computer any more. */
  missing: number;
}

export interface ProgressSnapshot {
  phase: Phase;
  scanned: number;
  entries_total: number;
  entries_done: number;
  bytes_total: number;
  bytes_done: number;
  warnings: number;
  elapsed_secs: number;
  bytes_per_sec: number;
  eta_secs: number | null;
}

export type JobEvent =
  | { event: "phase"; phase: Phase }
  | { event: "warning"; path: string; message: string }
  | { event: "info"; message: string };

export type JobMessage =
  | { type: "hello"; info: { profile: string; run_id: string; started: string; trigger: string } }
  | { type: "progress"; progress: ProgressSnapshot; shutdown_after?: boolean }
  | { type: "event"; event: JobEvent }
  | { type: "finished"; status: RunStatus; error: string | null; shutdown?: boolean };

export interface RestoreOutcome {
  restored: number;
  bytes: number;
  skipped_identical: number;
  moved_aside: string[];
  errors: FileError[];
  would_write: string[];
  /** Exact restores: items not in the snapshot that were removed (or moved aside). */
  removed?: number;
  /** Exact restores: why some folders were left as they are. */
  exact_notes?: string[];
}

/** A path found in any snapshot. */
export interface Found {
  path: string;
  kind: "file" | "dir" | "symlink" | "fifo" | "char_device" | "block_device" | "socket";
  /** Distinct versions across the snapshots. */
  versions: number;
  snapshots: number;
  /** Newest snapshot containing it. */
  latest: string;
  latest_size: number;
  latest_mtime: number;
  /** Whether it's at its place on the computer now (`null`: can't be checked). */
  on_disk: boolean | null;
}

export interface FindResult {
  hits: Found[];
  total: number;
  snapshots: number;
}

/** One distinct version of a path, held by snapshots `first`…`last`. */
export interface Version {
  first: string;
  last: string;
  snapshots: number;
  kind: Found["kind"];
  size: number;
  mtime: number;
}

/** What a restore would do. */
export interface RestorePreview {
  new: number;
  differ: number;
  identical: number;
  new_folders: number;
  bytes: number;
  keep_existing: boolean;
  differ_paths: string[];
  new_paths: string[];
  unsafe_paths: number;
  /** Exact restores: items not in the snapshot that would be removed, and the first few. */
  remove?: number;
  remove_paths?: string[];
  /** Exact restores: why some folders are left as they are. */
  exact_notes?: string[];
}

/** Scheduled backups paused until a moment, or until resumed (`until: null`). */
export interface Pause {
  until: string | null;
}

export interface Preferences {
  /** Webhooks told about every backup. */
  webhooks?: Webhook[];
  notify_success: boolean;
  default_exclude: string[];
  default_exclude_custom: boolean;
  pause: Pause | null;
  /** A backup that starts while another runs waits for it. */
  one_at_a_time?: boolean;
  /** What runs scheduled backups (absent from older backends: systemd). */
  scheduler?: Scheduler;
  /** Why cron can't be used here, in words (null: it can). */
  cron_problem?: string | null;
  /** Whether a cron daemon is running (null: can't tell). */
  cron_running?: boolean | null;
  /** The comparison program chosen: a command, "" for none, null for automatic. */
  diff_tool?: string | null;
  /** The one used now, [command, name], and the known ones installed. */
  diff_tool_used?: [string, string] | null;
  diff_tools?: [string, string][];
  /** Closing the window keeps the app in the system tray (absent from older backends). */
  close_to_tray?: boolean;
}

export type Scheduler = "systemd" | "cron";

/** One line of a comparison: unchanged, removed or added, or a run of unchanged lines
 * left out (`skip`, `count` of them). */
export interface DiffLine {
  kind: "same" | "removed" | "added" | "skip";
  old: number | null;
  new: number | null;
  text: string;
  count: number;
}

/** Two versions of a file compared. */
export interface FileDiff {
  old_file: string;
  new_file: string;
  old_size: number;
  new_size: number;
  identical: boolean;
  /** Not text: only sizes and sameness are known. */
  binary: boolean;
  too_large: boolean;
  added: number;
  removed: number;
  lines: DiffLine[];
  truncated: number;
}

export type ApiErrorCode =
  | "password_required"
  | "wrong_password"
  | "new_vault"
  | "empty"
  | "busy"
  | "locked"
  | "auth_cancelled"
  | "auth_failed"
  | "helper_missing"
  | "helper_failed"
  | "bad_request"
  | "cancelled"
  /** The backup drive isn't plugged in. */
  | "not_connected"
  /** The backup stopped right after it started (the message says why). */
  | "backup_failed"
  /** Stop: no backup of this profile is running. */
  | "not_running"
  | "error";

export interface SystemStatus {
  /** The root helper is installed. */
  available: boolean;
  /** An administrator authenticated in this session. */
  unlocked: boolean;
}

export const SYSTEM_PREFIX = "system:";
export const isSystem = (id: string | null | undefined) => !!id && id.startsWith(SYSTEM_PREFIX);

export class ApiError extends Error {
  constructor(
    public code: ApiErrorCode,
    message: string,
  ) {
    super(message);
  }
}

// ---- Profiles (mirror of flux_core::profile, as serialized) ----

export type Target =
  | { kind: "local"; path: string; fs_uuid?: string | null }
  | {
      kind: "ssh";
      host: string;
      user?: string | null;
      port?: number | null;
      path: string;
      identity_file?: string | null;
      ssh_options?: string[];
      bandwidth_limit?: string | null;
    };

export type PasswordSource =
  | { from: "keyring" }
  | { from: "prompt" }
  | { from: "file"; path: string }
  | { from: "command"; command: string };

export interface Profile {
  version: number;
  name: string;
  engine: Engine;
  target: Target;
  sources: {
    paths: string[];
    exclude: string[];
    exclude_larger_than?: string | null;
    exclude_caches: boolean;
    exclude_if_present?: string[];
    one_file_system: boolean;
    /** Sources on a drive of their own (recorded when saved with the drive connected). */
    volumes?: SourceVolume[];
  };
  retention: {
    enabled: boolean;
    keep_last: number;
    keep_within?: string | null;
    keep_hourly: number;
    keep_daily: number;
    keep_weekly: number;
    keep_monthly: number;
    keep_yearly: number;
    max_age?: string | null;
    keep_named: boolean;
    min_free_space?: string | null;
    min_free_inodes_percent?: number | null;
    /** Never fewer complete snapshots than this (missing: 1). */
    safety_floor?: number;
    /** Missing: "hold". */
    after_problems?: "hold" | "prune";
    keep_history_days?: number | null;
    keep_history_runs?: number | null;
  };
  schedule: {
    trigger: "manual" | "calendar" | "interval";
    on_calendar?: string | null;
    every?: string | null;
    catch_up: boolean;
    skip_on_battery: boolean;
    skip_on_metered?: boolean;
    on_connect: boolean;
  };
  options: {
    preserve_acl: boolean;
    preserve_xattr: boolean;
    symlinks: "preserve" | "follow_unsafe" | "follow";
    change_detection: "metadata" | "checksum";
    continue_on_errors: boolean;
    snapshot_if_unchanged: boolean;
    nice: number;
    io_priority: "normal" | "best_effort_low" | "idle";
    notify: boolean;
    backup_before_restore: boolean;
    threads: number;
  };
  hooks?: { pre_backup?: string[]; post_backup?: string[]; on_error?: string[]; timeout?: string | null };
  vault?: VaultOptions | null;
  rsync?: { extra_args?: string[] } | null;
  borg?: BorgOptions | null;
  /** Upkeep after scheduled backups (absent: the defaults). */
  maintenance?: { verify_monthly: boolean; free_space_weekly: boolean };
  offsite?: boolean;
  destinations?: Destination[];
  network?: NetworkRules;
  restore_test?: RestoreTestSettings;
  /** Missing: "normal". */
  unusual_changes?: "off" | "normal" | "high";
  /** Flux Sentry plants tripwire files in the folders this backs up. */
  tripwires?: boolean;
  webhooks?: Webhook[];
}

export type BorgEncryption = "repokey-blake2" | "repokey" | "none";

export interface BorgOptions {
  encryption: BorgEncryption;
  compression: string;
  password_source: PasswordSource;
  remote_path?: string | null;
  extra_args?: string[];
}

/** Whether a standard tool (rsync, borg) is installed. */
export interface ToolStatus {
  tool: "rsync" | "borg";
  program: string;
  version: string | null;
  supported: boolean;
  hint: string | null;
}

export interface ExportFormat {
  id: "flux" | "borgmatic" | "restic" | "rsync" | "borg";
  label: string;
  description: string;
  works_with: string;
}

export interface ProfileExport {
  format: ExportFormat["id"];
  filename: string;
  contents: string;
  executable: boolean;
  /** What couldn't be carried over. */
  notes: string[];
}

/** How to use a compatibility-mode profile's backups without Flux. */
export interface RecoveryKit {
  engine: Engine;
  /** Where the backups and the kit are (a path, or user@host:path). */
  location: string;
  /** FLUX-RECOVERY.txt */
  recovery: string;
  /** flux-backup.sh */
  script: string;
  tool: ToolStatus;
}

export interface Issue {
  severity: "error" | "warning";
  field: string;
  message: string;
}

export interface Editable {
  id: string;
  profile: Profile;
  issues: Issue[];
}

export interface SaveResult {
  id: string;
  issues: Issue[];
  saved: boolean;
  schedule: string | null;
}

export interface PreviewRow {
  id: string;
  time: string;
  keep: boolean;
  reasons: string[];
  expired: boolean;
  name: string | null;
  pinned: boolean;
}

export interface Suggestion {
  pattern: string;
  label: string;
  why: string;
  size: number | null;
}

export interface Drive {
  mount_point: string;
  label: string;
  fs_uuid: string | null;
  fs_type: string;
  free: number | null;
  total: number | null;
}

export interface TargetTest {
  ok: boolean;
  messages: string[];
}

export interface Change {
  path: string;
  change: "added" | "removed" | "modified" | "type_changed";
  kind: Kind;
  fields: string[];
  old_size: number | null;
  new_size: number | null;
}

export interface Comparison {
  old: string;
  new: string;
  summary: { added: number; removed: number; modified: number; type_changed: number; bytes_delta: number };
  changes: Change[];
  truncated: boolean;
  /** Comparing with the files on disk: files there larger than the size limit, which
   * the backups leave out (so they aren't compared). */
  left_out_by_size?: number;
  size_limit?: string | null;
}

export interface LoggedEvent {
  t: string;
  event: "phase" | "warning" | "info";
  phase?: Phase;
  path?: string;
  message?: string;
}

export interface CheckSummary {
  snapshots: number;
  checked: number;
  bytes_read: number;
  errors: string[];
  warnings: string[];
  /** Damage that was rebuilt from repair data. */
  repaired?: string[];
  /** Of the errors, how many the repair data could rebuild. */
  repairable?: number;
  /** Stored files given repair data they lacked. */
  parity_added?: number;
}

export interface PruneSummary {
  decisions: { id: string; keep: boolean }[];
  removed: string[];
  removed_for_space: string[];
  space_floor_unmet: boolean;
  bytes_freed: number | null;
  dry_run: boolean;
}

/** A profile set up in Back In Time, as a new Flux profile to start from. */
export interface BitProfile {
  /** Back In Time's number for it. */
  number: number;
  name: string;
  /** local, local_encfs, ssh or ssh_encfs. */
  mode: string;
  profile: Profile;
  /** What didn't carry over exactly. */
  notes: string[];
  snapshots_dir: string | null;
  snapshot_count: number;
  latest: string | null;
  /** Its destination is there now (a drive: connected). */
  connected: boolean;
}

/** One installed program of Flux Backup (the `flux` command, the helper). */
export interface Component {
  path: string;
  /** `null` when it couldn't be run. */
  version: string | null;
}

/** What the About dialog shows. */
export interface About {
  version: string;
  cli: Component;
  /** `null` when no helper is installed (system backups unavailable). */
  helper: Component | null;
  /** The `flux` command or the helper reports another version than the app. */
  mismatch: boolean;
  os: string;
  kernel: string;
  /** The WebKitGTK version drawing the window (app only). */
  webkit?: string | null;
}

/** The project's pages the app can open. */
export type LinkName = "website" | "guide" | "issues" | "source" | "changes";

/** How hard a Vault password is hashed (Argon2id). */
export type PasswordHashing =
  | { level: "standard" }
  | { level: "strong" }
  | { level: "maximum" }
  | { level: "custom"; memory_mib: number; passes: number; threads: number };

export interface VaultOptions {
  compression_level: number;
  encryption: boolean;
  password_source: PasswordSource;
  /** Missing: standard. */
  hashing?: PasswordHashing;
  hide_sizes?: boolean;
  /** Repair data kept with what's stored, in percent of it (missing or 0: none). */
  parity?: number;
  lock_after_minutes?: number | null;
  unlock_file?: string | null;
  /** Keep flux-vault-recover next to the Vault on a drive (missing: yes). */
  keep_recovery_tool?: boolean;
}

/** One password (key slot) of a Vault. */
export interface KeyInfo {
  id: string;
  label: string;
  created: string;
  /** The one that opened the Vault in the app. */
  current: boolean;
  hashing: PasswordHashing;
  unlock_file: boolean;
}

export interface VaultSecurity {
  keys: KeyInfo[];
  unlocked: boolean;
  hashing: PasswordHashing;
  unlock_file: string | null;
  unlock_file_present: boolean;
  unlock_ms: number;
}

export interface UnlockFileResult {
  unlock_file: string | null;
  /** Other passwords that still open the Vault without the file. */
  without_file: KeyInfo[];
}

export type WhenMissing = "skip" | "wait" | "fail";

/** A source on a drive of its own, and what a backup does when that drive isn't there. */
export interface SourceVolume {
  path: string;
  fs_uuid: string;
  when_missing: WhenMissing;
  /** With "wait": how long at most ("1h"); missing: 1 hour. */
  wait_for?: string | null;
}

/** One more destination, as stored in the profile. */
export interface Destination {
  name: string;
  label?: string | null;
  engine: Engine;
  target: Target;
  offsite?: boolean;
  /** "replicate": copied from the main destination after each run. */
  mode?: "independent" | "replicate";
  vault?: VaultOptions | null;
  rsync?: unknown;
  borg?: unknown;
}

/** A profile's or destination's id → the profile's id and the destination's name. */
export function splitDestination(id: string): [string, string | null] {
  const i = id.indexOf("+");
  return i < 0 ? [id, null] : [id.slice(0, i), id.slice(i + 1)];
}

/** `[network]`: Wi-Fi rules and upload limits by time of day (backups to a server). */
export interface NetworkRules {
  wifi_only?: string[];
  wifi_never?: string[];
  bandwidth?: BandwidthWindow[];
}

export interface BandwidthWindow {
  /** `*`, `Mon..Fri`, `Sat,Sun`… */
  days: string;
  /** `HH:MM` */
  from: string;
  to: string;
  /** `"2MB"`; missing: unlimited. */
  limit?: string | null;
}

export type WebhookPreset = "json" | "ntfy" | "gotify" | "slack" | "discord" | "healthchecks";
export type WebhookEvent = "started" | "succeeded" | "warning" | "failed" | "skipped" | "restore_test_failed" | "unusual_changes" | "low_space";

/** A webhook: where to tell how backups go, off this computer. */
export interface Webhook {
  url: string;
  preset?: WebhookPreset;
  /** Empty: failures, warnings, unusual changes and failed restore tests. */
  events?: WebhookEvent[];
  headers?: string[];
  enabled?: boolean;
}

/** The names a backup is kept under: the computer, user and profile that made it. */
export interface KeptAs {
  host: string;
  user: string;
  profile: string;
}

/** A backup found on a drive or a server (Recover an existing backup). */
export interface FoundBackup {
  engine: Engine;
  kept: KeptAs;
  location: string;
  name: string;
  sources: string[];
  /** Its settings were kept with it. */
  saved: boolean;
  snapshots: number | null;
  newest: string | null;
  /** Made on this computer by this user. */
  here: boolean;
}

export interface BackupSearch {
  found: FoundBackup[];
  /** There's an encrypted Vault: its backups are listed with its password. */
  vault_locked: boolean;
  wrong_password: boolean;
}

/** After recovering: only get files back, or carry on backing up from here. */
export type RecoverThen = "restore" | "carry_on";

/** One line of the recovery readiness checklist. */
export interface ReadinessCheck {
  key: string;
  ok: boolean;
  /** Only you can tell: tick it. */
  asked: boolean;
  title: string;
  detail: string;
}

/** Snapshot export: .tar.zst keeps everything, ZIP is for sharing. */
export type ArchiveFormat = "tar_zst" | "zip";
export type ChecksumKind = "sha256" | "blake3";

export interface ExportOutcome {
  archive: string;
  size: number;
  files: number;
  bytes: number;
  manifest: string | null;
  /** What the format couldn't hold, in words. */
  left_out: string[];
  errors: string[];
}

/** The rule tester's answer: is a path backed up, and why. */
export interface Explained {
  path: string;
  backed_up: boolean;
  text: string;
}

/** One item of a folder in the coverage view. */
export interface CoverageItem {
  name: string;
  path: string;
  dir: boolean;
  size: number;
  backed_up: boolean;
  why: string;
  files_in: number;
  bytes_in: number;
  files_out: number;
  bytes_out: number;
}

export interface Coverage {
  path: string;
  backed_up: boolean;
  why: string;
  items: CoverageItem[];
  /** The totals stopped at the time limit. */
  partial: boolean;
}

/** Where setting up a backup starts. */
export interface Template {
  /** `home`, `documents`…, or `own:<name>`. */
  id: string;
  name: string;
  line: string;
  icon: string;
  own: boolean;
}

/** Where a run's time went. */
export interface Performance {
  /** Wall-clock seconds of each phase, in order. */
  phases: { name: string; secs: number }[];
  /** Seconds per stage of the work, summed over threads. */
  stages?: { name: string; secs: number }[];
  writes: number;
  bytes_written: number;
  write_ms?: number | null;
  /** Bytes of files processed per second while copying. */
  throughput?: number | null;
}

/** What a backup's snapshots hold, what they take, and what dedup and compression save. */
export interface Storage {
  snapshots: number;
  latest_bytes: number;
  latest_files: number;
  logical_bytes: number;
  stored_bytes: number | null;
  dedup_saved: number | null;
  compression_saved: number | null;
  shared_with: number;
  basis: string;
  /** Vaults with repair data: what it takes (part of `stored_bytes`), and how many of
   * the stored files have it. */
  repair_bytes?: number | null;
  packs?: number;
  packs_with_repair?: number;
}
