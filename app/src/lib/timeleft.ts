// Pure (no Svelte), so it's checked with node.

/** Time left in words, as the tray says it: "less than a minute left", "about 6 minutes
 * left", "about 1 hour 10 minutes left"; `short` for a narrow column: "<1 min left",
 * "6 min left", "1 h 10 min left". Rounded to the minute, so it doesn't tick each second. */
export function timeLeft(secs: number, short = false): string {
  const n = (k: number, one: string) => `${k} ${one}${k === 1 ? "" : "s"}`;
  if (secs < 60) return short ? "<1 min left" : "less than a minute left";
  const mins = Math.round(secs / 60);
  if (mins < 60) return short ? `${mins} min left` : `about ${n(mins, "minute")} left`;
  const h = Math.floor(mins / 60);
  const m = mins % 60;
  if (short) return m === 0 || h >= 10 ? `${h} h left` : `${h} h ${m} min left`;
  return m === 0 || h >= 10 ? `about ${n(h, "hour")} left` : `about ${n(h, "hour")} ${n(m, "minute")} left`;
}
