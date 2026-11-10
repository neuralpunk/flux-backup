// `npm test`: node's own test runner (no dependencies), for the app's pure modules.
import { test } from "node:test";
import assert from "node:assert/strict";
import { fromHHMM, parseTime, showTime, toHHMM } from "../src/lib/timeparse.ts";
import { intervalParts, intervalText } from "../src/lib/interval.ts";

test("times are read the way people type them", () => {
  const cases: [string, string][] = [
    ["9", "09:00"], ["09", "09:00"], ["21", "21:00"], ["0", "00:00"],
    ["930", "09:30"], ["0930", "09:30"], ["2130", "21:30"], ["2400", "00:00"],
    ["9:30", "09:30"], ["9.30", "09:30"], ["9h30", "09:30"], ["9 30", "09:30"], ["21:30", "21:30"],
    ["9pm", "21:00"], ["9 PM", "21:00"], ["9:30 p.m.", "21:30"], ["9:30pm", "21:30"], ["7a", "07:00"],
    ["12am", "00:00"], ["12:15 AM", "00:15"], ["12pm", "12:00"],
    ["noon", "12:00"], ["Midnight", "00:00"], ["24:00", "00:00"],
  ];
  for (const [typed, want] of cases) assert.equal(toHHMM(parseTime(typed)!), want, typed);
});

test("what isn't a time is refused, not guessed", () => {
  for (const typed of ["", "25", "9:3", "9:60", "13pm", "0am", "abc", "2401", "12345", "pm", "9::30"]) {
    assert.equal(parseTime(typed), null, typed);
  }
});

test("times are shown in the chosen clock", () => {
  assert.equal(showTime(0, false), "12:00 AM");
  assert.equal(showTime(0, true), "00:00");
  assert.equal(showTime(750, false), "12:30 PM");
  assert.equal(showTime(1290, false), "9:30 PM");
  assert.equal(showTime(1290, true), "21:30");
  assert.equal(showTime(545, true), "09:05");
});

test("every minute survives being shown and read back, in both clocks", () => {
  for (let t = 0; t < 1440; t++) {
    for (const h24 of [true, false]) assert.equal(parseTime(showTime(t, h24)), t, showTime(t, h24));
    assert.equal(fromHHMM(toHHMM(t)), t);
  }
  assert.equal(fromHHMM("20:00:00"), 1200, "systemd's seconds");
  assert.equal(fromHHMM("24:00"), null);
});

test("intervals: the whole stored text is read", () => {
  assert.deepEqual(intervalParts("1h 30m"), { n: 90, unit: "min" });
  assert.deepEqual(intervalParts("4h"), { n: 4, unit: "h" });
  assert.equal(intervalText(90, "min"), "90min");
});

test("schedules in words, in the chosen clock", async () => {
  const { scheduleWords } = await import("../src/lib/timeparse.ts");
  assert.equal(scheduleWords("*-*-* 20:00:00", false), "every day at 8:00 PM");
  assert.equal(scheduleWords("*-*-* 20:00:00", true), "every day at 20:00");
  assert.equal(scheduleWords("Mon..Fri *-*-* 07:30:00", true), "weekdays at 07:30");
  assert.equal(scheduleWords("Sun *-*-* 09:05", false), "every Sunday at 9:05 AM");
  assert.equal(scheduleWords("hourly", false), "every hour");
  assert.equal(scheduleWords("every 4h", false), "every 4 hours");
  assert.equal(scheduleWords("manual + when drive connected", false), "manual + when drive connected");
  assert.equal(scheduleWords("*-*-* 06:00:00 + when drive connected", true), "every day at 06:00 + when drive connected");
  assert.equal(scheduleWords("Mon,Wed *-*-* 07:30", true), "Mon,Wed *-*-* 07:30", "others as they are");
});

test("durations in words", async () => {
  const { durationWords } = await import("../src/lib/interval.ts");
  const { scheduleWords } = await import("../src/lib/timeparse.ts");
  assert.equal(durationWords("4h"), "4 hours");
  assert.equal(durationWords("1h 30m"), "1 hour 30 minutes");
  assert.equal(durationWords("90min"), "1 hour 30 minutes");
  assert.equal(durationWords("1d"), "1 day");
  assert.equal(durationWords("weird"), "weird");
  assert.equal(scheduleWords("every 4h", false), "every 4 hours");
  assert.equal(scheduleWords("every 12h + when drive connected", true), "every 12 hours + when drive connected");
});
