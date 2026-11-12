import { test } from "node:test";
import assert from "node:assert/strict";
import { timeLeft } from "../src/lib/timeleft.ts";

test("time left in words, as the tray says it", () => {
  assert.equal(timeLeft(12), "less than a minute left");
  assert.equal(timeLeft(70), "about 1 minute left");
  assert.equal(timeLeft(359), "about 6 minutes left");
  assert.equal(timeLeft(3600), "about 1 hour left");
  assert.equal(timeLeft(4210), "about 1 hour 10 minutes left");
  assert.equal(timeLeft(11.5 * 3600), "about 11 hours left");
});

test("time left, short, for a narrow column", () => {
  assert.equal(timeLeft(12, true), "<1 min left");
  assert.equal(timeLeft(359, true), "6 min left");
  assert.equal(timeLeft(4210, true), "1 h 10 min left");
});
