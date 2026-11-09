// Is a backup protecting you, in words? One answer used by the sidebar, All backups and a
// backup's own overview, so they never disagree.

import { ago } from "./format";
import type { BackupStatus, ProfileSummary } from "./types";

export type Tone = "ok" | "warn" | "bad" | "run" | "idle";

export interface Protection {
  tone: Tone;
  /** Counts as protected in "4 of 5 backups are protected". */
  protected: boolean;
  /** Short, for the sidebar: "Protected · 59 min ago". */
  short: string;
  /** One plain sentence about what's wrong and what to do, when something is. */
  detail: string | null;
}

/** `percent`: a running backup's transfer, when known; `waiting`: it waits for another
 * backup to finish ("one backup at a time"). */
export function protection(p: ProfileSummary, s: BackupStatus | undefined, percent: number | null = null, waiting = false): Protection {
  const r = p.last_run;
  const lastGood = s?.last_good ?? (r && r.status !== "failed" && r.status !== "cancelled" ? r.finished : null);
  const danger = s?.health.find((h) => h.level === "danger");
  const warning = s?.health.find((h) => h.level === "warning");
  if (p.running) {
    return {
      tone: "run",
      protected: lastGood != null,
      short: waiting ? "Waiting for its turn…" : percent != null ? `Backing up · ${percent}%` : "Backing up…",
      detail: null,
    };
  }
  if (p.errors.length) {
    return { tone: "bad", protected: false, short: "Can't run: needs fixing", detail: `Its settings have a problem: ${p.errors[0]}` };
  }
  if (!r && !lastGood) {
    return { tone: "idle", protected: false, short: "Not backed up yet", detail: "It hasn't made its first backup yet." };
  }
  if (r?.status === "failed") {
    // A health warning already says it in words ("Archive isn't connected. The last
    // backup to it was 9 days ago."); otherwise the error, and how old the last copy is.
    const h = warning ?? danger;
    const why = h ? withHint(h) : `The last backup failed: ${r.error ?? "unknown error"}.`;
    return {
      tone: "bad",
      protected: false,
      short: `Failed ${ago(r.finished)}`,
      detail: h || !lastGood ? why : `${why} The last good copy is from ${ago(lastGood)}.`,
    };
  }
  if (danger) return { tone: "bad", protected: false, short: shortOf(danger.text, lastGood), detail: withHint(danger) };
  if (warning) return { tone: "warn", protected: true, short: `Check · ${lastGood ? ago(lastGood) : "never"}`, detail: withHint(warning) };
  if (r?.status === "completed_with_errors") {
    return { tone: "warn", protected: true, short: `Some files skipped · ${ago(r.finished)}`, detail: "The last backup skipped files it couldn't read; they're listed in Activity." };
  }
  return { tone: "ok", protected: true, short: `Protected · ${ago(lastGood)}`, detail: null };
}

function withHint(h: { text: string; hint: string | null }) {
  return h.hint ? `${h.text} ${h.hint}` : h.text;
}

function shortOf(text: string, lastGood: string | null) {
  return lastGood ? `At risk · ${ago(lastGood)}` : text.split(".")[0]!;
}

/** When a backup runs next, in words: the same everywhere ("in 5 hours", "when its drive
 * is connected", "when you start it"). `pausedUntil`: schedules are paused. */
export function nextRun(p: ProfileSummary, s: BackupStatus | undefined, pausedUntil: string | null = null): string {
  if (p.running) return "running now";
  if (pausedUntil) return `paused ${pausedUntil}`;
  const at = s?.next_run;
  if (at && Number.isFinite(Date.parse(at))) return ago(at);
  if (p.schedule.includes("connected")) return "when its drive is connected";
  if (p.schedule.startsWith("manual")) return "when you start it";
  return "not scheduled";
}

/** How old the newest copy is ("9 days ago"), or null when there is none. */
export function lastGood(p: ProfileSummary, s: BackupStatus | undefined): string | null {
  const r = p.last_run;
  const t = s?.last_good ?? (r && r.status !== "failed" && r.status !== "cancelled" ? r.finished : null);
  return t ? ago(t) : null;
}
