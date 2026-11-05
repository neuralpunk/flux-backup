// Application state shared across components (Svelte 5 runes).

import { api, listen, type JobEventPayload } from "./api";
import { ApiError, type JobEvent, type ProfileSummary, type ProgressSnapshot, type RunStatus, type SystemStatus } from "./types";

export interface Job {
  profile: string;
  progress: ProgressSnapshot | null;
  warnings: { path: string; message: string }[];
  finished: { status: RunStatus; error: string | null } | null;
  startedAt: number;
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

/** The desktop's preference (WebKitGTK follows the GTK theme). */
const prefersDark = typeof window !== "undefined" ? window.matchMedia("(prefers-color-scheme: dark)") : null;

class AppState {
  profiles = $state<ProfileSummary[]>([]);
  loading = $state(true);
  loadError = $state<string | null>(null);
  selectedId = $state<string | null>(null);
  tab = $state<Tab>("overview");
  /** The new-profile editor is open. */
  creating = $state(false);
  /** …for a system profile. */
  creatingSystem = $state(false);
  /** System (root) profiles: helper installed / unlocked by an administrator. */
  system = $state<SystemStatus>({ available: false, unlocked: false });
  unlockingSystem = $state(false);
  showShortcuts = $state(false);
  /** The profile editor shows its advanced options (remembered while the app runs). */
  editorAdvanced = $state(false);
  jobs = $state<Record<string, Job>>({});
  theme = $state<Theme>(loadTheme());
  private systemDark = $state(prefersDark?.matches ?? true);
  /** What's actually shown. */
  resolvedTheme = $derived<"dark" | "light">(this.theme === "auto" ? (this.systemDark ? "dark" : "light") : this.theme);
  /** Bumped when a profile's snapshots change, so views reload. */
  revision = $state(0);
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
    } catch (e) {
      this.loadError = (e as Error).message;
    } finally {
      this.loading = false;
    }
  }

  newProfile(system = false) {
    this.creatingSystem = system;
    this.creating = true;
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
        break;
      case "event": {
        const ev: JobEvent = message.event;
        if (ev.event === "warning") job.warnings = [...job.warnings, { path: ev.path, message: ev.message }].slice(-200);
        break;
      }
      case "finished": {
        job.finished = { status: message.status, error: message.error };
        this.setRunning(profile, false);
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
  }
}

export const app = new AppState();
