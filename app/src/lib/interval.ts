/** A backup's interval ("every 90 minutes"), between the text a profile stores and the
 * editor's number-and-unit control.
 *
 * The backend writes durations the way `humantime` formats them: `90m` comes back as
 * `1h 30m`, `45d` as `1month 14days 13h 26m 24s`. Reading only the first number and unit
 * of that (as the editor once did) turns 90 minutes into 1 hour and 45 days into 1
 * minute, so the whole text is read. */

export type IntervalUnit = "min" | "h" | "d";

/** Seconds per unit, as humantime counts them (a month is 30.44 days, a year 365.25).
 * `m` is minutes and `M` months, so the match is case-sensitive. */
const UNIT_SECONDS: Record<string, number> = {
  s: 1, sec: 1, secs: 1, second: 1, seconds: 1,
  m: 60, min: 60, mins: 60, minute: 60, minutes: 60,
  h: 3600, hr: 3600, hrs: 3600, hour: 3600, hours: 3600,
  d: 86_400, day: 86_400, days: 86_400,
  w: 604_800, week: 604_800, weeks: 604_800,
  M: 2_630_016, month: 2_630_016, months: 2_630_016,
  y: 31_557_600, year: 31_557_600, years: 31_557_600,
};

/** The length in seconds of a duration such as `1h 30m` or `2days`; `null` when it
 * can't be read (or has parts smaller than a second). */
export function durationSeconds(text: string): number | null {
  const s = text.trim();
  if (!s) return null;
  let total = 0;
  let rest = s;
  while (rest) {
    const m = rest.match(/^(\d+)\s*([A-Za-z]+)\s*/);
    if (!m) return null;
    const per = UNIT_SECONDS[m[2]!];
    if (per === undefined) return null;
    total += Number(m[1]) * per;
    rest = rest.slice(m[0].length);
  }
  return Number.isSafeInteger(total) ? total : null;
}

/** A duration as the editor shows it: a whole number of days, hours or minutes (the
 * largest unit that fits exactly). `null` when it isn't a whole number of minutes, or is
 * shorter than one: the editor then shows the text itself and leaves it alone. */
export function intervalParts(text: string): { n: number; unit: IntervalUnit } | null {
  const secs = durationSeconds(text);
  if (secs === null || secs < 60 || secs % 60 !== 0) return null;
  if (secs % 86_400 === 0) return { n: secs / 86_400, unit: "d" };
  if (secs % 3600 === 0) return { n: secs / 3600, unit: "h" };
  return { n: secs / 60, unit: "min" };
}

/** What the profile stores for the editor's number and unit. */
export function intervalText(n: number, unit: IntervalUnit): string {
  return `${Math.max(1, Math.round(n))}${unit}`;
}

/** A duration in words: `4h` → "4 hours", `1h 30m` → "1 hour 30 minutes", `1d` → "1 day".
 * Text that isn't a duration comes back as it is. */
export function durationWords(text: string): string {
  const secs = durationSeconds(text);
  if (secs === null || secs === 0) return text.trim();
  const parts: string[] = [];
  let rest = secs;
  for (const [n, one] of [[86_400, "day"], [3600, "hour"], [60, "minute"], [1, "second"]] as const) {
    const k = Math.floor(rest / n);
    if (k) parts.push(`${k} ${one}${k === 1 ? "" : "s"}`);
    rest -= k * n;
  }
  return parts.join(" ");
}
