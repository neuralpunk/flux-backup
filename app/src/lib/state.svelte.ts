// Application state shared across components (Svelte 5 runes).

import { api, listen, pickFile, type JobEventPayload } from "./api";
import { ApiError, type BackupStatus, type JobEvent, type Pause, type ProfileSummary, type ProgressSnapshot, type RunStatus, type Scheduler, type SystemStatus } from "./types";

export interface Job {
  profile: string;
  progress: ProgressSnapshot | null;
  warnings: { path: string; message: string }[];
  finished: { status: RunStatus; error: string | null } | null;
  startedAt: number;
  /** The computer shuts down when this backup ends. */
  shutdownAfter?: boolean;
}

export type Theme = "dark" | "light" | "auto";
export type Tab = "overview" | "snapshots" | "activity" | "settings";

function loadTheme(): Theme {
  try {
    const t = localStorage.getItem("flux.theme");
    if (t === "dark" || t === "light" || t === "auto") return t;
    if (t === "system") return "auto"; // the name before 0.1.1
  } catch {
    /* storage unavailable */
  }
  return "dark";
}

/** Failed runs the user has already looked at (profile id → run id), so a failure stays
 * marked in the sidebar until its backup is opened. Remembered on this computer only. */
function loadSeen(): Record<string, string> {
  try {
    return JSON.parse(localStorage.getItem("flux.seenFailures") ?? "{}") as Record<string, string>;
  } catch {
    return {};
  }
}

/** The desktop's preference (WebKitGTK follows the GTK theme). */
const prefersDark = typeof window !== "undefined" ? window.matchMedia("(prefers-color-scheme: dark)") : null;

class AppState {
  profiles = $state<ProfileSummary[]>([]);
  /** Which destination's snapshots the Snapshots tab shows (a `home+usb` id; `null`: the
   * main one). Forgotten when another backup is opened. */
  snapshotsOf = $state<string | null>(null);
  /** A snapshot the Snapshots tab should open on (then forgotten). */
  showSnapshot = $state<string | null>(null);
  loading = $state(true);
  loadError = $state<string | null>(null);
  selectedId = $state<string | null>(null);
  /** "All backups" is showing (the start page: are you protected?). */
  home = $state(true);
  /** How each backup is doing (run log and timer), for All backups and the sidebar. */
  statuses = $state<Record<string, BackupStatus>>({});
  private seenFailures = $state<Record<string, string>>(loadSeen());
  tab = $state<Tab>("overview");
  /** A settings group the editor opens on next (e.g. "exclusions"), then forgets. */
  settingsPage = $state<string | null>(null);
  /** The new-profile editor is open. */
  creating = $state(false);
  /** …for a system profile. */
  creatingSystem = $state(false);
  /** System (root) profiles: helper installed / unlocked by an administrator. */
  system = $state<SystemStatus>({ available: false, unlocked: false });
  unlockingSystem = $state(false);
  showShortcuts = $state(false);
  /** A password dialog for profile `id`; `retry` runs once it's unlocked. */
  unlock = $state<{ id: string; retry: () => void } | null>(null);
  /** A dialog opened from a menu (any backup, not only the selected one). */
  /** Setting up a backup: the template chosen (`null`: choosing one). */
  creatingTemplate = $state<string | null>(null);
  dialog = $state<{ kind: "delete-profile" | "recovery" | "recover" | "save-template" | "system-unlock" | "export" | "pause" | "about"; id?: string } | null>(null);
  /** The app's preferences page is showing (instead of a backup). */
  prefsOpen = $state(false);
  /** Scheduled backups are paused (until a moment, or until resumed). */
  pause = $state<Pause | null>(null);
  /** When a shutdown scheduled after a backup happens (ms since the epoch). */
  shutdownAt = $state<number | null>(null);
  /** What runs scheduled backups (Preferences › Advanced). */
  scheduler = $state<Scheduler>("systemd");
  /** Backups being started or stopped from the app (buttons show it). */
  starting = $state<Record<string, boolean>>({});
  stopping = $state<Record<string, boolean>>({});
  jobs = $state<Record<string, Job>>({});
  theme = $state<Theme>(loadTheme());
  private systemDark = $state(prefersDark?.matches ?? true);
  /** What's actually shown. */
  resolvedTheme = $derived<"dark" | "light">(this.theme === "auto" ? (this.systemDark ? "dark" : "light") : this.theme);
  /** Bumped when a profile's snapshots change, so views reload. */
  revision = $state(0);
  /** Bumped after a restore writes files, so views that mark what's on the computer
   * ("Deleted") check again. */
  diskRevision = $state(0);
  toasts = $state<{ id: number; kind: "ok" | "bad" | "info"; text: string }[]>([]);

  selected = $derived(this.profiles.find((p) => p.id === this.selectedId) ?? null);

  async refresh() {
    try {
      const list = await api.profiles();
      this.profiles = list;
      this.loadError = null;
      if (!this.selectedId || !list.some((p) => p.id === this.selectedId)) {
        this.selectedId = list[0]?.id ?? null;
      }
      void this.loadStatuses();
    } catch (e) {
      this.loadError = (e as Error).message;
    } finally {
      this.loading = false;
    }
  }

  /** Reload every backup's status (cheap: no repository is opened). */
  async loadStatuses() {
    await Promise.all(
      this.profiles.map(async (p) => {
        try {
          this.statuses[p.id] = await api.status(p.id);
        } catch {
          /* an older backend or helper: All backups shows what the summary knows */
        }
      }),
    );
  }

  /** The last run failed and nobody has opened this backup since. */
  unseenFailure(p: ProfileSummary): boolean {
    return p.last_run?.status === "failed" && this.seenFailures[p.id] !== p.last_run.run_id;
  }

  private markSeen(id: string) {
    const r = this.profiles.find((p) => p.id === id)?.last_run;
    if (r?.status !== "failed" || this.seenFailures[id] === r.run_id) return;
    this.seenFailures[id] = r.run_id;
    try {
      localStorage.setItem("flux.seenFailures", JSON.stringify(this.seenFailures));
    } catch {
      /* storage unavailable: it stays marked this session */
    }
  }

  newProfile(system = false) {
    this.creatingSystem = system;
    this.creatingTemplate = null;
    this.creating = true;
    this.prefsOpen = false;
    this.home = false;
  }

  /** A profile, or one of its destinations (`home+usb`) as a profile of its own: its id,
   * name ("Home folder → USB stick") and engine. */
  profileOrDestination(id: string): { id: string; name: string; engine: ProfileSummary["engine"] } | null {
    const p = this.profiles.find((x) => x.id === id);
    if (p) return p;
    const plus = id.indexOf("+");
    if (plus < 0) return null;
    const base = this.profiles.find((x) => x.id === id.slice(0, plus));
    const d = base?.destinations?.find((x) => x.id === id);
    return base && d ? { id, name: `${base.name} → ${d.label}`, engine: d.engine } : null;
  }

  openPreferences() {
    this.creating = false;
    this.home = false;
    this.prefsOpen = true;
  }

  /** Show All backups. */
  openHome() {
    this.creating = false;
    this.prefsOpen = false;
    this.home = true;
  }

  async loadPause() {
    try {
      const p = await api.preferences();
      this.pause = p.pause;
      this.scheduler = p.scheduler ?? "systemd";
    } catch {
      /* older backend */
    }
  }

  /** Pause scheduled backups for an hour, until tomorrow morning, until resumed; or resume. */
  async setPause(kind: "hour" | "tomorrow" | "resumed" | "off") {
    try {
      this.pause = await api.setPause(kind);
      this.toast("info", this.pause ? `Scheduled backups paused ${pauseUntil(this.pause)}. Backing up by hand still works.` : "Scheduled backups will run again.");
    } catch (e) {
      this.toast("bad", (e as Error).message);
    }
  }

  /** Show backup `id` on `tab`. */
  open(id: string, tab: Tab = "overview") {
    this.creating = false;
    this.prefsOpen = false;
    this.home = false;
    this.selectedId = id;
    this.tab = tab;
    this.markSeen(id);
  }

  /** Open `id`'s settings at one group of them (a page of the editor). */
  openSettings(id: string, page: string) {
    this.settingsPage = page;
    this.open(id, "settings");
  }

  /** Start a backup of `id` (asks for the password first if it needs one). */
  async backupNow(id: string, shutdown = false) {
    this.starting[id] = true;
    const name = this.profiles.find((x) => x.id === id)?.name ?? id;
    try {
      const r = await api.backupNow(id, shutdown);
      if (r && !r.running) {
        // Over before its progress could be followed (nothing had changed, say).
        this.toast("ok", `Backup of “${name}” finished.`);
        await this.afterRun();
        return;
      }
      const p = this.profiles.find((x) => x.id === id);
      if (p) p.running = true;
      this.watchStart(id);
    } catch (e) {
      if (e instanceof ApiError && e.code === "password_required") {
        this.open(id);
        this.unlock = { id, retry: () => void this.backupNow(id, shutdown) };
      } else if (e instanceof ApiError && e.code === "backup_failed") {
        this.toast("bad", `Backup of “${name}” failed: ${e.message}`);
        await this.afterRun();
      } else this.toast("bad", (e as Error).message);
    } finally {
      this.starting[id] = false;
    }
  }

  /** Reload what a finished run changes: the list, statuses and the open views. */
  private async afterRun() {
    await this.refresh();
    this.revision++;
  }

  /** Safety net: if a started backup never reports in (it ended before its progress could
   * be followed), ask the backend whether it's still running instead of spinning forever. */
  private watchStart(id: string) {
    setTimeout(() => {
      const p = this.profiles.find((x) => x.id === id);
      if (p?.running && !this.jobs[id]) void this.afterRun();
    }, 4000);
  }

  /** Shut the computer down when `id`'s running backup ends (or no longer). */
  async setShutdownAfter(id: string, on: boolean) {
    const job = this.jobs[id];
    try {
      await api.shutdownAfter(id, on);
      if (job) job.shutdownAfter = on;
      if (on) this.toast("info", "The computer shuts down a minute after this backup ends.");
    } catch (e) {
      this.toast("bad", (e as Error).message);
    }
  }

  /** Cancel the shutdown scheduled after a backup. */
  async cancelShutdown() {
    try {
      await api.cancelShutdown();
      this.shutdownAt = null;
      this.toast("ok", "Shutdown cancelled.");
    } catch (e) {
      this.toast("bad", `Couldn't cancel the shutdown: ${(e as Error).message}. Run shutdown -c in a terminal.`);
    }
  }

  /** Stop a running backup; the next one continues where it stopped. */
  async stop(id: string) {
    this.stopping[id] = true;
    try {
      await api.cancel(id);
    } catch (e) {
      this.stopping[id] = false;
      if (e instanceof ApiError && e.code === "not_running") {
        // It had already ended; show how it went.
        this.setRunning(id, false);
        this.toast("info", "That backup had already stopped. Activity shows how it went.");
        await this.afterRun();
      } else this.toast("bad", (e as Error).message);
    }
  }

  /** Import a Flux profile from a file the user picks, then open its settings (a drive
   * may have another name on this computer). */
  async importProfile(system = false) {
    const path = await pickFile("Import a Flux profile (.toml)");
    if (!path) return;
    try {
      const r = await api.importProfile(path, system);
      await this.refresh();
      this.open(r.id, "settings");
      const errors = r.issues.filter((i) => i.severity === "error").length;
      this.toast(
        errors ? "info" : "ok",
        errors
          ? `Imported. ${errors === 1 ? "One setting needs" : `${errors} settings need`} attention before it can run.`
          : "Imported. Check where it backs up to, then Save to turn on its schedule.",
      );
    } catch (e) {
      this.toast("bad", (e as Error).message);
    }
  }

  /** Copy a profile ("Name (copy)") and open the copy's settings. */
  async duplicateProfile(id: string) {
    try {
      const copy = await api.duplicateProfile(id);
      await this.refresh();
      this.open(copy, "settings");
      this.toast("ok", "Copied. Give the copy its own destination or folders, then Save.");
    } catch (e) {
      this.toast("bad", (e as Error).message);
    }
  }

  /** Delete a profile (its backups stay where they are). */
  async deleteProfile(id: string) {
    const p = this.profiles.find((x) => x.id === id);
    await api.deleteProfile(id);
    this.toast("ok", `Deleted “${p?.name ?? id}”. Its backups were left where they are.`);
    await this.refresh();
    if (this.selectedId === id || !this.selected) {
      this.selectedId = this.profiles.find((x) => x.scope === p?.scope)?.id ?? this.profiles[0]?.id ?? null;
      this.tab = "overview";
      this.openHome();
    }
  }

  async unlockSystem() {
    this.unlockingSystem = true;
    try {
      await api.systemUnlock();
      this.system = { ...this.system, unlocked: true };
      await this.refresh();
      this.toast("ok", "System backups unlocked. They lock again after 15 idle minutes.");
    } catch (e) {
      if (!(e instanceof ApiError && e.code === "auth_cancelled")) this.toast("bad", (e as Error).message);
    } finally {
      this.unlockingSystem = false;
    }
  }

  async lockSystem() {
    await api.systemLock();
    this.onSystemLocked();
  }

  private onSystemLocked() {
    this.system = { ...this.system, unlocked: false };
    this.profiles = this.profiles.filter((p) => p.scope !== "system");
    if (this.selected == null) this.selectedId = this.profiles[0]?.id ?? null;
    if (this.creatingSystem) this.creating = false;
  }

  private applyTheme() {
    document.documentElement.dataset.theme = this.resolvedTheme;
  }

  setTheme(t: Theme) {
    this.theme = t;
    this.applyTheme();
    try {
      localStorage.setItem("flux.theme", t);
    } catch {
      /* storage unavailable */
    }
  }

  toast(kind: "ok" | "bad" | "info", text: string) {
    const id = Date.now() + Math.random();
    this.toasts = [...this.toasts, { id, kind, text }];
    setTimeout(() => (this.toasts = this.toasts.filter((t) => t.id !== id)), kind === "bad" ? 9000 : 5000);
  }

  private onJob({ profile, message }: JobEventPayload) {
    const job: Job = this.jobs[profile] ?? { profile, progress: null, warnings: [], finished: null, startedAt: Date.now() };
    switch (message.type) {
      case "hello":
        this.jobs[profile] = { profile, progress: null, warnings: [], finished: null, startedAt: Date.now() };
        this.setRunning(profile, true);
        return;
      case "progress":
        job.progress = message.progress;
        job.shutdownAfter = message.shutdown_after ?? false;
        break;
      case "event": {
        const ev: JobEvent = message.event;
        if (ev.event === "warning") job.warnings = [...job.warnings, { path: ev.path, message: ev.message }].slice(-200);
        break;
      }
      case "finished": {
        job.finished = { status: message.status, error: message.error };
        this.setRunning(profile, false);
        if (message.shutdown) this.shutdownAt = Date.now() + 60_000;
        const name = this.profiles.find((p) => p.id === profile)?.name ?? profile;
        if (message.status === "failed") this.toast("bad", `Backup of “${name}” failed: ${message.error ?? "unknown error"}`);
        else if (message.status === "completed" || message.status === "unchanged") this.toast("ok", `Backup of “${name}” finished.`);
        else if (message.status === "completed_with_errors") this.toast("info", `Backup of “${name}” finished with problems.`);
        setTimeout(() => {
          if (this.jobs[profile]?.finished) delete this.jobs[profile];
        }, 4000);
        void this.refresh();
        break;
      }
    }
    this.jobs[profile] = job;
  }

  private setRunning(profile: string, running: boolean) {
    const p = this.profiles.find((x) => x.id === profile);
    if (p) p.running = running;
    // "Stopping…" until the backup has actually stopped.
    if (!running) this.stopping[profile] = false;
  }

  async start() {
    this.applyTheme();
    prefersDark?.addEventListener("change", (e) => {
      this.systemDark = e.matches;
      this.applyTheme();
    });
    await listen<JobEventPayload>("flux://job", (p) => this.onJob(p));
    await listen<{ profile: string }>("flux://changed", () => {
      this.revision++;
    });
    await listen("flux://pause", () => void this.loadPause());
    // A Vault unused for its "lock again after" time: its password was forgotten.
    await listen<{ profile: string }>("flux://locked", (e) => {
      const name = this.profiles.find((p) => p.id === e.profile)?.name ?? e.profile;
      this.toast("info", `“${name}” is locked again: it wasn't used for a while.`);
      this.revision++;
    });
    await listen<{ unlocked: boolean }>("flux://system", (e) => {
      if (!e.unlocked && this.system.unlocked) {
        this.onSystemLocked();
        this.toast("info", "System backups are locked again.");
      }
    });
    try {
      this.system = await api.systemStatus();
    } catch {
      /* older backend: no system support */
    }
    await this.refresh();
    await this.loadPause();
  }
}

/** "until 7:00 tomorrow", "until 15:20", "until you resume them". */
export function pauseUntil(p: Pause): string {
  if (!p.until) return "until you resume them";
  const d = new Date(p.until);
  const t = d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  const today = new Date();
  const tomorrow = new Date(today.getFullYear(), today.getMonth(), today.getDate() + 1);
  if (d.toDateString() === today.toDateString()) return `until ${t}`;
  if (d.toDateString() === tomorrow.toDateString()) return `until ${t} tomorrow`;
  return `until ${d.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" })}, ${t}`;
}

export const app = new AppState();
