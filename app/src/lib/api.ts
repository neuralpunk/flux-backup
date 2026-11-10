// The only module that talks to the backend. In the Tauri app it uses IPC; opened in a
// plain browser (`npm run dev`) it falls back to demo data, so the UI can be developed and
// reviewed without the Rust side.

import {
  ApiError,
  type ApiErrorCode,
  type CheckSummary,
  type Comparison,
  type Drive,
  type Editable,
  type ExportFormat,
  type FindResult,
  type Issue,
  type JobMessage,
  type Listing,
  type LoggedEvent,
  type Overview,
  type BackupStatus,
  type PreviewRow,
  type Profile,
  type ProfileSummary,
  type ProfileExport,
  type PruneSummary,
  type RecoveryKit,
  type RestoreOutcome,
  type RunRecord,
  type SaveResult,
  type SnapshotMeta,
  type Suggestion,
  type SystemStatus,
  type TargetTest,
  type ToolStatus,
  type RestorePreview,
  type BitProfile,
  type Version,
  type Estimate,
  type Pause,
  type Preferences,
  type Scheduler,
  type FileDiff,
  type About,
  type LinkName,
  type PasswordHashing,
  type VaultSecurity,
  type UnlockFileResult,
  type Destination,
  type TestReport,
  type Webhook,
  type BackupSearch,
  type FoundBackup,
  type RecoverThen,
  type Target,
  type ArchiveFormat,
  type ChecksumKind,
  type ExportOutcome,
  type Explained,
  type Template,
  type Storage,
  type Coverage,
  type ReadinessCheck,
} from "./types";

export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

type Mock = typeof import("./mock");
let mockModule: Promise<Mock> | null = null;
const mock = () => (mockModule ??= import("./mock"));

async function call<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  if (!inTauri) {
    const m = await mock();
    return m.handle(cmd, args) as Promise<T>;
  }
  const { invoke } = await import("@tauri-apps/api/core");
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    const err = e as { code?: ApiErrorCode; message?: string };
    throw new ApiError(err.code ?? "error", err.message ?? String(e));
  }
}

export const api = {
  profiles: () => call<ProfileSummary[]>("profiles"),
  overview: (id: string) => call<Overview>("overview", { id }),
  /** How a backup is doing, from its run log and timer (cheap; never asks for a password). */
  status: (id: string) => call<BackupStatus>("status", { id }),
  snapshots: (id: string) => call<SnapshotMeta[]>("snapshots", { id }),
  listDir: (id: string, snapshot: string, path: string) => call<Listing>("list_dir", { id, snapshot, path }),
  unlock: (id: string, password: string, create = false) => call<void>("unlock", { id, password, create }),
  /** Keep the unlocked Vault password in a root-only key file, for scheduled backups. */
  saveKeyFile: (id: string) => call<string>("save_key_file", { id }),
  /** While setting a backup up: create its encrypted repository now with this password
   * (or check it opens the existing one) and keep it where the profile's password comes
   * from, so the first backup, even a scheduled one, needs nobody to type it. */
  setPassword: (id: string, password: string) => call<void>("set_password", { id, password }),
  /** `running: false` when it was already over by the time the call returned. */
  backupNow: (id: string, shutdown = false) => call<{ running: boolean } | null>("backup_now", { id, shutdown }),
  /** Shut the computer down when this running backup ends (or no longer). */
  shutdownAfter: (id: string, on: boolean) => call<void>("shutdown_after", { id, on }),
  /** Cancel a shutdown scheduled after a backup. */
  cancelShutdown: () => call<void>("cancel_shutdown"),
  cancel: (id: string) => call<void>("cancel", { id }),
  /** `exact`: also remove what the restored folders hold that the snapshot doesn't. */
  restore: (id: string, snapshot: string, paths: string[], to: string | null, overwrite: boolean, exact = false) =>
    call<RestoreOutcome>("restore", { id, snapshot, paths, to, overwrite, exact }),

  getProfile: (id: string) => call<Editable>("get_profile", { id }),
  newProfile: (system = false) => call<Editable>("new_profile", { system }),
  /** A Vault's format and the newest one (`null`: not a Vault, or not there). */
  vaultFormat: (id: string) => call<{ format: number; newest: number } | null>("vault_format", { id }),
  /** Move a Vault to the newest format. */
  upgradeVault: (id: string) => call<{ format: number; newest: number } | null>("upgrade_vault", { id }),
  /** What the snapshots hold and take, and what dedup and compression save. */
  storage: (id: string) => call<Storage>("storage", { id }),
  /** The templates a new backup can start from (built in, then yours). */
  templates: (system = false) => call<Template[]>("templates", { system }),
  newFromTemplate: (template: string, system = false) => call<Editable>("new_from_template", { template, system }),
  /** Save a backup's settings as a template, without where it goes. */
  saveTemplate: (id: string, name: string) => call<Template[]>("save_template", { id, name }),
  deleteTemplate: (template: string, system = false) => call<Template[]>("delete_template", { template, system }),
  /** Back In Time's profiles on this computer (yours only), to start a new one from. */
  backintimeProfiles: () => call<BitProfile[]>("backintime_profiles"),
  validateProfile: (profile: Profile, system = false) => call<Issue[]>("validate_profile", { profile, system }),
  saveProfile: (id: string, profile: Profile, create: boolean) => call<SaveResult>("save_profile", { id, profile, create }),
  deleteProfile: (id: string) => call<void>("delete_profile", { id }),
  suggestId: (name: string, system = false) => call<string>("suggest_id", { name, system }),
  retentionPreview: (id: string, retention: Profile["retention"]) => call<PreviewRow[]>("retention_preview", { id, retention }),
  excludeSuggestions: (sources: string[]) => call<Suggestion[]>("exclude_suggestions", { sources }),
  drives: () => call<Drive[]>("drives"),
  testTarget: (profile: Profile, system = false) => call<TargetTest>("test_target", { profile, system }),

  /** System (root) profiles: is the helper there, and has an administrator unlocked it? */
  systemStatus: () => call<SystemStatus>("system_status"),
  /** Start the helper; the desktop's polkit dialog asks for an administrator password. */
  systemUnlock: () => call<void>("system_unlock"),
  systemLock: () => call<void>("system_lock"),

  /** Change a snapshot's name, pin, note or tags (only the keys given change). */
  updateSnapshot: (id: string, snapshot: string, change: { name?: string | null; pinned?: boolean; note?: string | null; tags?: string[] }) =>
    call<SnapshotMeta>("update_snapshot", {
      id,
      snapshot,
      name: change.name ?? null,
      setName: "name" in change,
      pinned: change.pinned ?? null,
      note: change.note ?? null,
      setNote: "note" in change,
      tags: change.tags ?? null,
    }),
  /** Bytes freed (Vault, Borg), or `null` (Mirror, rsync: folders simply go). */
  deleteSnapshot: (id: string, snapshot: string) => call<number | null>("delete_snapshot", { id, snapshot }),
  compare: (id: string, old: string, newer: string | null) => call<Comparison>("compare", { id, old, new: newer }),
  runs: (id: string, limit = 100) => call<RunRecord[]>("runs", { id, limit }),
  runEvents: (id: string, runId: string) => call<LoggedEvent[]>("run_events", { id, runId }),
  prune: (id: string, dryRun: boolean) => call<PruneSummary>("prune", { id, dryRun }),
  check: (id: string, readData: boolean) => call<CheckSummary>("check", { id, readData }),

  /** Compatibility mode: the recovery guide and script for a profile. */
  recoveryKit: (id: string) => call<RecoveryKit>("recovery_kit", { id }),
  /** Save the guide and script into a folder; returns the files written. */
  saveRecoveryKit: (id: string, dir: string) => call<string[]>("save_recovery_kit", { id, dir }),
  /** Copy a file out of a snapshot and open it in its usual program; returns the copy. */
  openFile: (id: string, snapshot: string, path: string) => call<string>("open_snapshot_file", { id, snapshot, path }),
  /** Compare `path` in snapshot `oldSnap` with `newSnap`, or with the file now (null). */
  diffFile: (id: string, path: string, oldSnap: string, newSnap: string | null) => call<FileDiff>("diff_file", { id, path, old: oldSnap, new: newSnap }),
  /** Open two files in the comparison program chosen in the preferences. */
  openDiffTool: (left: string, right: string) => call<void>("open_diff_tool", { left, right }),
  /** A command, "" for none, or null for the first one installed. */
  setDiffTool: (tool: string | null) => call<Preferences>("set_diff_tool", { tool }),
  /** Copy a profile; returns the new id. */
  duplicateProfile: (id: string) => call<string>("duplicate_profile", { id }),
  /** Text for a bug report (secrets hidden). */
  diagnostics: (id: string) => call<string>("diagnostics", { id }),
  preferences: () => call<Preferences>("preferences"),
  /** `oneAtATime` left out: unchanged. */
  savePreferences: (notifySuccess: boolean, defaultExclude: string[] | null, oneAtATime?: boolean) =>
    call<Preferences>("save_preferences", { notifySuccess, defaultExclude, oneAtATime }),
  setPause: (pause: "hour" | "tomorrow" | "resumed" | "off") => call<Pause | null>("set_pause", { pause }),
  /** Run scheduled backups with systemd timers or cron; every schedule moves over. */
  setScheduler: (scheduler: Scheduler) => call<Preferences>("set_scheduler", { scheduler }),
  autostart: () => call<boolean>("autostart"),
  setAutostart: (on: boolean) => call<void>("set_autostart", { on }),
  find: (id: string, query: string) => call<FindResult>("find", { id, query }),
  versions: (id: string, path: string) => call<Version[]>("versions", { id, path }),
  estimate: (profile: Profile, system = false) => call<Estimate>("estimate", { profile, system }),
  restorePreview: (id: string, snapshot: string, paths: string[], to: string | null, overwrite: boolean, exact = false) =>
    call<RestorePreview>("restore_preview", { id, snapshot, paths, to, overwrite, exact }),
  exportFormats: () => call<ExportFormat[]>("export_formats"),
  /** Versions of the app, the `flux` command and the helper, for the About dialog. */
  about: () => call<About>("about"),
  /** A Vault's passwords, their hashing and unlock file (lists keys even while locked). */
  vaultSecurity: (id: string) => call<VaultSecurity>("vault_security", { id }),
  /** Hash the password in use again with this strength (needs the Vault unlocked). */
  strengthenKey: (id: string, hashing: PasswordHashing) => call<void>("strengthen_key", { id, hashing }),
  /** Require an unlock file (`create`: a new one at `path`), or stop (`path: null`). */
  setUnlockFile: (id: string, path: string | null, create = false) => call<UnlockFileResult>("set_unlock_file", { id, path, create }),
  /** Milliseconds hashing one password takes on this computer. */
  timeHashing: (hashing: PasswordHashing) => call<number>("time_hashing", { hashing }),
  /** Add a destination to a saved backup (saved at once; `password` sets up an encrypted
   * one). Returns the saved profile. */
  addDestination: (id: string, destination: Destination, password: string | null) => call<Profile>("add_destination", { id, destination, password }),
  /** Remove a destination (its backups stay). Returns the saved profile. */
  removeDestination: (id: string, name: string) => call<Profile>("remove_destination", { id, name }),
  /** Test a restore now (the backup's settings; by default 10 random files). */
  restoreTest: (id: string) => call<TestReport>("restore_test", { id }),
  /** The latest restore tests, newest first. */
  restoreTests: (id: string) => call<TestReport[]>("restore_tests", { id }),
  /** The webhooks told about every backup (replaces the list). */
  setWebhooks: (webhooks: Webhook[]) => call<Preferences>("set_webhooks", { webhooks }),
  /** Send a test message; the HTTP status it answered. */
  testWebhook: (webhook: Webhook) => call<number>("test_webhook", { webhook }),
  /** Is `path` backed up by `profile` (the editor's draft), and why? */
  explain: (profile: Profile, path: string) => call<Explained>("explain", { profile, path }),
  /** A folder's items, each backed up or left out by `profile`, with totals. */
  coverage: (profile: Profile, dir: string) => call<Coverage>("coverage", { profile, dir }),
  /** Save files of a snapshot (all of it with `paths` empty) as an archive in folder `dir`. */
  exportSnapshot: (id: string, snapshot: string, paths: string[], format: ArchiveFormat, dir: string, checksums: ChecksumKind | null) =>
    call<ExportOutcome>("export_snapshot", { id, snapshot, paths, format, dir, checksums }),
  /** The Flux backups on a drive or a server; an encrypted Vault's with its password. */
  findBackups: (target: Target, password: string | null) => call<BackupSearch>("find_backups", { target, password }),
  /** Set up a backup found there on this computer. */
  recover: (target: Target, found: FoundBackup, then: RecoverThen, id: string | null, password: string | null) =>
    call<{ id: string; note: string | null }>("recover", { target, engine: found.engine, kept: found.kept, then, id, password }),
  /** Could this backup be recovered on another computer? */
  readiness: (id: string) => call<ReadinessCheck[]>("readiness", { id }),
  /** Tick (or untick) a readiness question only you can answer. */
  setReadiness: (id: string, key: string, yes: boolean) => call<ReadinessCheck[]>("set_readiness", { id, key, yes }),
  /** "This was me": the unusual changes in `snapshot` were your own. */
  dismissUnusual: (id: string, snapshot: string) => call<void>("dismiss_unusual", { id, snapshot }),
  /** Forget the Vault's password in the app (lock it again now). */
  lockVault: (id: string) => call<void>("lock_vault", { id }),
  /** Open one of the project's pages in the web browser. */
  openLink: (which: LinkName) => call<void>("open_link", { which }),
  exportProfile: (id: string, format: string) => call<ProfileExport>("export_profile", { id, format }),
  /** Save an export into a folder; returns the file written. */
  saveExport: (id: string, format: string, dir: string) => call<string>("save_export", { id, format, dir }),
  /** Import the Flux profile in a file; returns its new id and any problems. */
  importProfile: (path: string, system = false) => call<{ id: string; issues: Issue[] }>("import_profile", { path, system }),
  /** Are rsync and borg installed? */
  tools: () => call<ToolStatus[]>("tools"),
};

/** Subscribe to backend events. Returns an unsubscribe function. */
export async function listen<T>(event: string, handler: (payload: T) => void): Promise<() => void> {
  if (!inTauri) {
    const m = await mock();
    return m.listen(event, handler as (p: unknown) => void);
  }
  const { listen } = await import("@tauri-apps/api/event");
  return listen<T>(event, (e) => handler(e.payload));
}

export interface JobEventPayload {
  profile: string;
  message: JobMessage;
}

/** Pick a file. `null` if cancelled. */
export async function pickFile(title: string): Promise<string | null> {
  if (!inTauri) return window.prompt(title, "/home/me/.ssh/flux_backup_ed25519");
  const { open } = await import("@tauri-apps/plugin-dialog");
  const r = await open({ directory: false, multiple: false, title });
  return typeof r === "string" ? r : null;
}

/** Pick a folder (restore destination). `null` if cancelled. */
export async function pickFolder(title: string): Promise<string | null> {
  if (!inTauri) {
    return window.prompt(title, "/tmp/flux-restore");
  }
  const { open } = await import("@tauri-apps/plugin-dialog");
  const r = await open({ directory: true, multiple: false, title });
  return typeof r === "string" ? r : null;
}
