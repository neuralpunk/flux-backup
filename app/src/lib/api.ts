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
  type Version,
  type Estimate,
  type Pause,
  type Preferences,
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
  backupNow: (id: string) => call<void>("backup_now", { id }),
  cancel: (id: string) => call<void>("cancel", { id }),
  restore: (id: string, snapshot: string, paths: string[], to: string | null, overwrite: boolean) =>
    call<RestoreOutcome>("restore", { id, snapshot, paths, to, overwrite }),

  getProfile: (id: string) => call<Editable>("get_profile", { id }),
  newProfile: (system = false) => call<Editable>("new_profile", { system }),
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

  updateSnapshot: (id: string, snapshot: string, change: { name?: string | null; pinned?: boolean }) =>
    call<SnapshotMeta>("update_snapshot", {
      id,
      snapshot,
      name: change.name ?? null,
      setName: "name" in change,
      pinned: change.pinned ?? null,
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
  /** Copy a profile; returns the new id. */
  duplicateProfile: (id: string) => call<string>("duplicate_profile", { id }),
  /** Text for a bug report (secrets hidden). */
  diagnostics: (id: string) => call<string>("diagnostics", { id }),
  preferences: () => call<Preferences>("preferences"),
  savePreferences: (notifySuccess: boolean, defaultExclude: string[] | null) =>
    call<Preferences>("save_preferences", { notifySuccess, defaultExclude }),
  setPause: (pause: "hour" | "tomorrow" | "resumed" | "off") => call<Pause | null>("set_pause", { pause }),
  autostart: () => call<boolean>("autostart"),
  setAutostart: (on: boolean) => call<void>("set_autostart", { on }),
  find: (id: string, query: string) => call<FindResult>("find", { id, query }),
  versions: (id: string, path: string) => call<Version[]>("versions", { id, path }),
  estimate: (profile: Profile, system = false) => call<Estimate>("estimate", { profile, system }),
  restorePreview: (id: string, snapshot: string, paths: string[], to: string | null, overwrite: boolean) =>
    call<RestorePreview>("restore_preview", { id, snapshot, paths, to, overwrite }),
  exportFormats: () => call<ExportFormat[]>("export_formats"),
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
