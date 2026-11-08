// Shapes of the JSON exchanged with the Rust side (see app/src-tauri/src/commands.rs).

export type Engine = "mirror" | "vault" | "rsync" | "borg";
/** Compatibility mode: the backups are written by a standard tool, in its own format. */
export const isCompat = (e: Engine | undefined | null) => e === "rsync" || e === "borg";
export const engineLabel: Record<Engine, string> = { mirror: "Mirror", vault: "Vault", rsync: "rsync", borg: "Borg" };
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
export type Phase = "preparing" | "scanning" | "planning" | "transferring" | "finalizing" | "done";

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
  /** Validation errors (empty when the profile is usable). */
  errors: string[];
  last_run: RunRecord | null;
  running: boolean;
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
}

/** A health warning, in words. */
export interface Health {
  level: "warning" | "danger";
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
}

export interface Listing {
  path: string;
  entries: DirEntry[];
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
  | { type: "progress"; progress: ProgressSnapshot }
  | { type: "event"; event: JobEvent }
  | { type: "finished"; status: RunStatus; error: string | null };

export interface RestoreOutcome {
  restored: number;
  bytes: number;
  skipped_identical: number;
  moved_aside: string[];
  errors: FileError[];
  would_write: string[];
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
}

/** Scheduled backups paused until a moment, or until resumed (`until: null`). */
export interface Pause {
  until: string | null;
}

export interface Preferences {
  notify_success: boolean;
  default_exclude: string[];
  default_exclude_custom: boolean;
  pause: Pause | null;
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
  vault?: { compression_level: number; encryption: boolean; password_source: PasswordSource } | null;
  rsync?: { extra_args?: string[] } | null;
  borg?: BorgOptions | null;
  /** Upkeep after scheduled backups (absent: the defaults). */
  maintenance?: { verify_monthly: boolean; free_space_weekly: boolean };
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
}

export interface PruneSummary {
  decisions: { id: string; keep: boolean }[];
  removed: string[];
  removed_for_space: string[];
  space_floor_unmet: boolean;
  bytes_freed: number | null;
  dry_run: boolean;
}
