import { test } from "node:test";
import assert from "node:assert/strict";
import { backupIcon } from "../src/lib/backupIcon.ts";

const user = (...sources: string[]) => backupIcon({ scope: "user", sources });

test("a backup's icon, by what it backs up", () => {
  assert.equal(user("~"), "home");
  assert.equal(user("/home/me/"), "home");
  assert.equal(user("~/Documents"), "document");
  assert.equal(user("~/Pictures", "~/Videos"), "image");
  assert.equal(user("~/Music"), "music");
  assert.equal(user("~/.local/share/mail", "~/Notes"), "mail");
  assert.equal(user("~/Projects"), "code");
  assert.equal(user("~/Videos"), "film");
  assert.equal(user("/mnt/scans"), "folder");
  assert.equal(user("/"), "drive");
  assert.equal(user(), "folder");
  assert.equal(backupIcon({ scope: "system", sources: ["/etc"] }), "settings");
  assert.equal(backupIcon({ scope: "system", sources: ["/home/me"] }), "home");
});
