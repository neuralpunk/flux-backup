// The clock the app shows times in: like the system, 12-hour or 24-hour. A preference of
// this computer's app (like the theme), so it's kept in local storage. Reading it from a
// formatter makes whatever shows that time follow a change at once.
import { localeUses24h, scheduleWords, showTime, type Minutes } from "./timeparse";

export type Clock = "auto" | "12" | "24";

function load(): Clock {
  try {
    const c = localStorage.getItem("flux.clock");
    if (c === "12" || c === "24" || c === "auto") return c;
  } catch {
    /* storage unavailable */
  }
  return "auto";
}

export const clock = $state<{ pref: Clock }>({ pref: load() });

export function setClock(c: Clock) {
  clock.pref = c;
  try {
    localStorage.setItem("flux.clock", c);
  } catch {
    /* storage unavailable */
  }
}

/** Are times shown on the 24-hour clock? */
export function uses24h(): boolean {
  return clock.pref === "auto" ? localeUses24h() : clock.pref === "24";
}

/** The `hourCycle` for Intl formatters: none for the system's own. */
export function hourCycle(): { hourCycle?: "h12" | "h23" } {
  return clock.pref === "auto" ? {} : { hourCycle: clock.pref === "24" ? "h23" : "h12" };
}

/** A time of day in the chosen clock: "9:30 PM" or "21:30". */
export function clockTime(t: Minutes): string {
  return showTime(t, uses24h());
}

/** A backup's schedule summary in words, in the chosen clock ("every day at 8:00 PM"). */
export function scheduleText(summary: string): string {
  return scheduleWords(summary, uses24h());
}
