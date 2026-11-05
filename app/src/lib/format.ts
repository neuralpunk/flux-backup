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

export function count(n: number): string {
  return n.toLocaleString();
}

const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });

export function ago(when: string | number | Date | null | undefined): string {
  if (when == null) return "never";
  const t = new Date(when).getTime();
  const s = Math.round((t - Date.now()) / 1000);
  const a = Math.abs(s);
  if (a < 45) return s <= 0 ? "just now" : "in a moment";
  if (a < 3600) return rtf.format(Math.round(s / 60), "minute");
  if (a < 86_400) return rtf.format(Math.round(s / 3600), "hour");
  if (a < 30 * 86_400) return rtf.format(Math.round(s / 86_400), "day");
  if (a < 365 * 86_400) return rtf.format(Math.round(s / (30 * 86_400)), "month");
  return rtf.format(Math.round(s / (365 * 86_400)), "year");
}

export function dateTime(when: string | number | Date): string {
  return new Date(when).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

/** Compact date and time for tables: "Nov 1, 18:25" this year, "Nov 1 2025" before. */
export function shortDateTime(when: string | number | Date): string {
  const d = new Date(when);
  if (d.getFullYear() !== new Date().getFullYear()) return d.toLocaleDateString(undefined, { dateStyle: "medium" });
  return `${d.toLocaleDateString(undefined, { month: "short", day: "numeric" })}, ${d.toLocaleTimeString(undefined, { timeStyle: "short" })}`;
}

export function dayLabel(when: string | number | Date): string {
  return new Date(when).toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });
}

export function time(when: string | number | Date): string {
  return new Date(when).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
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
