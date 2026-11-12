import { hourCycle } from "./clock.svelte";

export { timeLeft } from "./timeleft";

const UNITS = ["B", "KiB", "MiB", "GiB", "TiB"];

export function bytes(n: number | null | undefined): string {
  if (n == null) return "—";
  let v = n;
  let i = 0;
  while (v >= 1024 && i < UNITS.length - 1) {
    v /= 1024;
    i++;
  }
  return i === 0 ? `${n} B` : `${v < 10 ? v.toFixed(1) : Math.round(v)} ${UNITS[i]}`;
}

const numbers = new Intl.NumberFormat();
export function count(n: number): string {
  return numbers.format(n);
}

/**
 * Date formatters, made once each. `toLocaleString` and its kin build a new formatter on
 * every call, which is the costly part: a list of a few thousand snapshots, each row
 * with several dates, spent most of a second on it every time the list was drawn.
 */
const formatters = new Map<string, Intl.DateTimeFormat>();
function fmt(options: Intl.DateTimeFormatOptions): Intl.DateTimeFormat {
  const key = JSON.stringify(options);
  let f = formatters.get(key);
  if (!f) {
    f = new Intl.DateTimeFormat(undefined, options);
    formatters.set(key, f);
  }
  return f;
}
const format = (when: string | number | Date, options: Intl.DateTimeFormatOptions): string => {
  const d = when instanceof Date ? when : new Date(when);
  return Number.isFinite(d.getTime()) ? fmt(options).format(d) : "Invalid Date";
};

/** "November 2026": the month a day falls in. */
export function monthLabel(when: string | number | Date): string {
  return format(when, { month: "long", year: "numeric" });
}

const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });

export function ago(when: string | number | Date | null | undefined): string {
  if (when == null) return "never";
  const t = new Date(when).getTime();
  // An unparseable date must never throw (RelativeTimeFormat rejects NaN, and a throw
  // while rendering leaves the whole view blank): show it as given instead.
  if (!Number.isFinite(t)) return typeof when === "string" ? when : "unknown";
  const s = Math.round((t - Date.now()) / 1000);
  const a = Math.abs(s);
  if (a < 45) return s <= 0 ? "just now" : "in a moment";
  // 59.5 minutes and up is "in 1 hour", not "in 60 minutes".
  if (a < 3600 && Math.round(a / 60) < 60) return rtf.format(Math.round(s / 60), "minute");
  if (a < 3600) return rtf.format(Math.sign(s), "hour");
  if (a < 86_400) return rtf.format(Math.round(s / 3600), "hour");
  if (a < 30 * 86_400) return rtf.format(Math.round(s / 86_400), "day");
  if (a < 365 * 86_400) return rtf.format(Math.round(s / (30 * 86_400)), "month");
  return rtf.format(Math.round(s / (365 * 86_400)), "year");
}

/** The current year, looked up at most once a minute (it's asked for in every row). */
let yearNow = { at: 0, year: 0 };
function thisYear(): number {
  const now = Date.now();
  if (now - yearNow.at > 60_000) yearNow = { at: now, year: new Date(now).getFullYear() };
  return yearNow.year;
}

export function dateTime(when: string | number | Date): string {
  return format(when, { dateStyle: "medium", timeStyle: "short", ...hourCycle() });
}

/** Compact date and time for tables: "Nov 1, 18:25" this year, "Nov 1 2025" before. */
export function shortDateTime(when: string | number | Date): string {
  const d = new Date(when);
  if (d.getFullYear() !== thisYear()) return format(d, { dateStyle: "medium" });
  return `${format(d, { month: "short", day: "numeric" })}, ${format(d, { timeStyle: "short", ...hourCycle() })}`;
}

/** "Oct 6" this year, "Oct 6, 2025" before. */
export function shortDate(when: string | number | Date): string {
  const d = new Date(when);
  if (!Number.isFinite(d.getTime())) return String(when);
  const sameYear = d.getFullYear() === thisYear();
  return format(d, sameYear ? { month: "short", day: "numeric" } : { dateStyle: "medium" });
}

export function dayLabel(when: string | number | Date): string {
  return format(when, { weekday: "short", month: "short", day: "numeric" });
}

export function time(when: string | number | Date): string {
  return format(when, { hour: "2-digit", minute: "2-digit", ...hourCycle() });
}

export function duration(secs: number): string {
  if (secs < 1) return "<1s";
  if (secs < 60) return `${Math.round(secs)}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${String(Math.round(secs % 60)).padStart(2, "0")}s`;
  return `${Math.floor(secs / 3600)}h ${String(Math.floor((secs % 3600) / 60)).padStart(2, "0")}m`;
}

export function between(a: string, b: string): string {
  return duration((new Date(b).getTime() - new Date(a).getTime()) / 1000);
}

export const phaseLabel: Record<string, string> = {
  preparing: "Preparing",
  scanning: "Scanning files",
  planning: "Comparing",
  transferring: "Copying",
  finalizing: "Finishing",
  done: "Done",
  waiting: "Waiting",
};

export const runStatusLabel: Record<string, string> = {
  completed: "Completed",
  completed_with_errors: "Completed with problems",
  unchanged: "No changes",
  dry_run: "Dry run",
  skipped: "Skipped",
  failed: "Failed",
  cancelled: "Cancelled",
};
