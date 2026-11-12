import { test } from "node:test";
import assert from "node:assert/strict";
import { changeMarks, changeWords, snapshotBefore } from "../src/lib/changes.ts";

test("what a backup changed, in a few words", () => {
  assert.equal(changeWords({ new: 214, changed: 38, deleted: 2 }), "214 new · 38 changed · 2 deleted");
  assert.equal(changeWords({ new: 0, changed: 1200, deleted: 0 }), "1,200 changed");
  assert.equal(changeWords({ new: 0, changed: 0, deleted: 0 }), "No changes");
  assert.equal(changeMarks({ new: 214, changed: 38, deleted: 2 }), "+214 ~38 −2");
  assert.equal(changeMarks({ new: 0, changed: 0, deleted: 3 }), "−3");
  assert.equal(changeMarks({ new: 0, changed: 0, deleted: 0 }), null);
});

test("the snapshot before a run's, skipping runs that made none", () => {
  const runs = [{ snapshot: "c" }, { snapshot: null }, { snapshot: "b" }, { snapshot: "a" }];
  assert.equal(snapshotBefore(runs, 0), "b");
  assert.equal(snapshotBefore(runs, 2), "a");
  assert.equal(snapshotBefore(runs, 3), null);
});
