// Times of day as people type them, and as the chosen clock shows them. Pure functions
// (no Svelte), so they can be checked with node on their own.
import { durationWords } from "./interval.ts";

/** A time of day in minutes after midnight (0–1439). */
export type Minutes = number;

/** "21:30" (how profiles store a time) → minutes; `null` if it isn't one. */
export function fromHHMM(s: string): Minutes | null {
  const m = /^(\d{1,2}):(\d{2})(?::\d{2})?$/.exec(s.trim());
  if (!m) return null;
  const h = Number(m[1]);
  const min = Number(m[2]);
  return h < 24 && min < 60 ? h * 60 + min : null;
}

/** Minutes → "21:30" (how profiles store a time). */
export function toHHMM(t: Minutes): string {
  const v = ((Math.round(t) % 1440) + 1440) % 1440;
  return `${String(Math.floor(v / 60)).padStart(2, "0")}:${String(v % 60).padStart(2, "0")}`;
}

/** Minutes → "9:30 PM" (12-hour) or "21:30" (24-hour). */
export function showTime(t: Minutes, h24: boolean): string {
  const v = ((Math.round(t) % 1440) + 1440) % 1440;
  const h = Math.floor(v / 60);
  const m = String(v % 60).padStart(2, "0");
  if (h24) return `${String(h).padStart(2, "0")}:${m}`;
  return `${h % 12 === 0 ? 12 : h % 12}:${m} ${h < 12 ? "AM" : "PM"}`;
}

/**
 * What was typed → minutes, or `null` when it can't be read. Forgiving: "9", "09",
 * "930", "0930", "2130", "9:30", "9.30", "9h30", "9 30", "21:30", "9pm", "9:30 p.m.",
 * "12am" (midnight), "noon", "midnight". Without AM/PM, an hour is read on the 24-hour
 * clock ("9" is 9:00 in the morning, "21" the evening), as most time fields do.
 */
export function parseTime(text: string): Minutes | null {
  let s = text.trim().toLowerCase().replace(/\s+/g, " ");
  if (!s) return null;
  if (s === "noon" || s === "midday") return 12 * 60;
  if (s === "midnight") return 0;
  // AM/PM, in any of the usual spellings, at the end.
  let half: "am" | "pm" | null = null;
  const ap = /\s*([ap])\.?\s*m?\.?$/.exec(s);
  if (ap && /\d/.test(s.slice(0, ap.index))) {
    half = ap[1] === "a" ? "am" : "pm";
    s = s.slice(0, ap.index).trim();
  }
  let h: number;
  let m = 0;
  let parts: RegExpExecArray | null;
  if ((parts = /^(\d{1,2})\s*[:.h ]\s*(\d{1,2})$/.exec(s))) {
    if (parts[2]!.length !== 2) return null; // "9:3" is ambiguous: 9:03 or 9:30?
    h = Number(parts[1]);
    m = Number(parts[2]);
  } else if (/^\d{1,4}$/.test(s)) {
    // Digits alone: an hour ("9", "21"), or an hour and minutes ("930", "2130").
    if (s.length <= 2) h = Number(s);
    else {
      h = Number(s.slice(0, -2));
      m = Number(s.slice(-2));
    }
  } else return null;
  if (m > 59) return null;
  if (half) {
    if (h < 1 || h > 12) return null;
    h = (h % 12) + (half === "pm" ? 12 : 0);
  } else if (h > 23) {
    if (h === 24 && m === 0) h = 0; // "24:00": midnight
    else return null;
  }
  return h * 60 + m;
}

/** Does this locale write times on the 24-hour clock? */
export function localeUses24h(locale?: string): boolean {
  try {
    const hc = new Intl.DateTimeFormat(locale, { hour: "numeric" }).resolvedOptions().hourCycle;
    return hc === "h23" || hc === "h24";
  } catch {
    return false;
  }
}

const DAY_NAMES: Record<string, string> = { Mon: "Monday", Tue: "Tuesday", Wed: "Wednesday", Thu: "Thursday", Fri: "Friday", Sat: "Saturday", Sun: "Sunday" };

/**
 * A schedule as the backend sums it up (`*-*-* 20:00:00`, `Mon..Fri *-*-* 07:30:00`,
 * `every 4h`, `manual + when drive connected`) in words, with times in the chosen clock:
 * "every day at 8:00 PM", "weekdays at 07:30", "every 4h". Other calendar expressions are
 * left as they are.
 */
export function scheduleWords(summary: string, h24: boolean): string {
  const [main = "", ...rest] = summary.split(" + ");
  const s = main.trim();
  let m: RegExpExecArray | null;
  let out = s;
  const at = (hh: string, mm: string) => showTime(Number(hh) * 60 + Number(mm), h24);
  if (s === "daily") out = "every day at midnight";
  else if (s === "hourly") out = "every hour";
  else if (s === "weekly") out = "every Monday at midnight";
  else if ((m = /^every (.+)$/.exec(s))) out = `every ${durationWords(m[1]!)}`;
  else if ((m = /^\*-\*-\* (\d\d):(\d\d)(?::00)?$/.exec(s))) out = `every day at ${at(m[1]!, m[2]!)}`;
  else if ((m = /^Mon\.\.Fri \*-\*-\* (\d\d):(\d\d)(?::00)?$/.exec(s))) out = `weekdays at ${at(m[1]!, m[2]!)}`;
  else if ((m = /^(Mon|Tue|Wed|Thu|Fri|Sat|Sun) \*-\*-\* (\d\d):(\d\d)(?::00)?$/.exec(s))) out = `every ${DAY_NAMES[m[1]!]} at ${at(m[2]!, m[3]!)}`;
  return [out, ...rest].join(" + ");
}
