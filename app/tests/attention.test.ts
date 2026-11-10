import { test } from "node:test";
import assert from "node:assert/strict";
import { counts, kindLabel, remainder, summary, type Tone } from "../src/lib/attention.ts";

const b = (tone: Tone, prot: boolean) => ({ tone, protected: prot });

test("the sidebar and the headline count the same way", () => {
  // The demo: two not protected (failed, at risk), three protected with something to see.
  const c = counts([b("bad", false), b("bad", false), b("warn", true), b("warn", true), b("warn", true)]);
  assert.deepEqual(c, { total: 5, protected: 3, notProtected: 2, toLook: 3 });
  assert.equal(summary(c), "2 not protected · 3 to look at");
  assert.equal(remainder(c, { protected: false }), "And 1 more isn't protected, and 3 need a look below.");
  assert.equal(c.total - c.notProtected, c.protected, "the headline's 3 of 5 agrees");
});

test("only looks, or nothing", () => {
  const looks = counts([b("ok", true), b("warn", true)]);
  assert.equal(summary(looks), "1 needs a look");
  assert.equal(remainder(looks, { protected: true }), null);
  const two = counts([b("warn", true), b("warn", true)]);
  assert.equal(remainder(two, { protected: true }), "And 1 more needs a look below.");
  assert.equal(summary(counts([b("ok", true)])), "Protected");
  assert.equal(summary(counts([b("ok", true), b("run", true)])), "All protected");
  assert.equal(summary(counts([b("run", false)])), "Protected", "a first backup running isn't a problem yet");
  assert.equal(summary(counts([b("idle", false), b("ok", true)])), "1 not protected");
});

test("health kinds in a few words", () => {
  assert.equal(kindLabel("low_space"), "Low on space");
  assert.equal(kindLabel("unverified"), "Not tested lately");
  assert.equal(kindLabel("something_new"), null);
  assert.equal(kindLabel(undefined), null);
});
