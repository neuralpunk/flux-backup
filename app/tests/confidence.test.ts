import assert from "node:assert/strict";
import { test } from "node:test";
import { guaranteeWords, sampleBound } from "../src/lib/confidence.ts";

test("how many files a certainty takes", () => {
  assert.equal(sampleBound("1%", "99%"), 459);
  assert.equal(sampleBound(undefined, undefined), 459, "the defaults");
  assert.equal(sampleBound("5%", "95%"), 59);
  assert.equal(sampleBound("0.1%", "99.9%"), 6905);
  assert.equal(sampleBound("50%", "50%"), 1);
});

test("what passing proves, in words", () => {
  assert.equal(
    guaranteeWords({ files: 120000, damaged: 1200, damage: "1%", certainty: "99%" }),
    "If 1% or more of its 120,000 files were damaged, this test would have found it (99% certain).",
  );
  assert.match(guaranteeWords({ files: 8, damaged: 1, damage: "12.5%", certainty: "99%" }), /^If even one of its 8 files/);
});
