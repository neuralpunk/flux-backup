import { test } from "node:test";
import assert from "node:assert/strict";
import { glance, type GlanceItem } from "../src/lib/glance.ts";
import type { Profile } from "../src/lib/types.ts";

function profile(): Profile {
  return {
    version: 1,
    name: "Home",
    engine: "vault",
    target: { kind: "local", path: "/run/media/me/BackupDisk", fs_uuid: "7A1C" },
    sources: { paths: ["/home/me", "/home/me/Work"], exclude: ["~/.cache/", "*.iso"], exclude_larger_than: "4GB", exclude_caches: true, one_file_system: false },
    retention: { enabled: true, keep_last: 1, keep_hourly: 0, keep_daily: 7, keep_weekly: 4, keep_monthly: 12, keep_yearly: 5, keep_named: true },
    schedule: { trigger: "calendar", on_calendar: "*-*-* 20:00:00", catch_up: true, skip_on_battery: true, on_connect: false },
    options: { preserve_acl: true, preserve_xattr: true, symlinks: "preserve", change_detection: "metadata", continue_on_errors: true, snapshot_if_unchanged: false, nice: 10, io_priority: "best_effort_low", notify: true, backup_before_restore: true, threads: 0 },
    vault: { compression_level: 3, encryption: true, password_source: { from: "keyring" } },
  };
}
const at = (items: GlanceItem[], key: GlanceItem["key"]) => items.find((i) => i.key === key)!;

test("a backup's settings, in a few words each", () => {
  const g = glance(profile(), false);
  assert.equal(at(g, "schedule").value, "Every day at 8:00 PM");
  assert.equal(at(g, "schedule").detail, "Catches up if missed, waits for mains power");
  assert.equal(at(g, "sources").value, "Home folder +1");
  assert.equal(at(g, "excluded").value, "2 rules · caches · over 4 GB");
  assert.equal(at(g, "where").value, "BackupDisk");
  assert.equal(at(g, "places").value, "None");
  assert.equal(at(g, "stored").value, "Encrypted (Vault)");
  assert.equal(at(g, "password").value, "Desktop keyring");
  assert.equal(at(g, "keeps").value, "7 daily · 4 weekly · 12 monthly · 5 yearly");
  assert.equal(at(g, "tests").value, "Only when you ask");
  assert.equal(at(g, "tests").tone, "off");
  assert.equal(at(g, "notify").value, "On this computer");
  assert.equal(g.find((i) => i.key === "network"), undefined, "no server: no network line");
  assert.equal(glance(profile(), true).find((i) => i.key === "schedule")!.value, "Every day at 20:00");
});

test("a password asked for each time is flagged when backups start by themselves", () => {
  const p = profile();
  p.vault!.password_source = { from: "prompt" };
  assert.equal(at(glance(p, false), "password").tone, "warn");
  p.schedule.trigger = "manual";
  assert.equal(at(glance(p, false), "password").tone, undefined, "by hand: it can ask");
  assert.equal(at(glance(p, false), "schedule").value, "Only by hand");
  p.schedule.on_connect = true;
  assert.equal(at(glance(p, false), "password").tone, "warn", "plugged in: nobody to ask");
});

test("other engines, places and extras", () => {
  const p = profile();
  p.engine = "mirror";
  p.vault = null;
  p.target = { kind: "ssh", host: "nas", user: "me", path: "/srv/flux" };
  p.destinations = [{ name: "usb", label: "USB stick", engine: "vault", target: { kind: "local", path: "/run/media/me/STICK" }, offsite: true }];
  p.retention.enabled = false;
  p.restore_test = { every: "month", mode: "rolling" };
  p.webhooks = [{ url: "https://ntfy.sh/x" }];
  p.hooks = { pre_backup: ["mount /x"], post_backup: [], on_error: ["notify"] };
  p.schedule = { trigger: "interval", every: "4h", catch_up: false, skip_on_battery: false, skip_on_metered: true, on_connect: false };
  const g = glance(p, true);
  assert.equal(at(g, "where").value, "me@nas");
  assert.equal(at(g, "places").value, "USB stick");
  assert.match(at(g, "places").detail!, /^A copy is kept elsewhere/);
  assert.equal(at(g, "stored").value, "Plain folders");
  assert.equal(at(g, "password").value, "None (not encrypted)");
  assert.equal(at(g, "keeps").value, "Everything");
  assert.equal(at(g, "tests").value, "Monthly · every file monthly");
  const watched = at(glance(p, true, { usual: 0.0042, warns_at: 0.05, runs: 31 }), "unusual");
  assert.equal(watched.value, "Flux Sentry on watch");
  assert.equal(watched.detail, "Knows this backup from 31 backups: 0.42% of its files usually change; it would warn at about 5.0%, or at the marks of ransomware");
  assert.match(at(glance(p, true), "unusual").detail!, /^Learns what's normal for this backup/);
  const v = profile();
  v.vault = { ...v.vault!, parity: 5 };
  assert.match(at(glance(v, true), "stored").detail!, /compression level \d+, 5% repair data$/);
  p.restore_test = { every: "week", mode: "confidence", damage: "0.5%" };
  const sure = at(glance(p, true), "tests");
  assert.equal(sure.value, "Weekly · catches 0.5% damage");
  assert.equal(sure.detail, "Enough random files to catch damage to 0.5% of them or more, 99% certain");
  assert.equal(at(g, "notify").value, "On this computer + 1 webhook");
  assert.equal(at(g, "hooks").value, "1 before · 1 after");
  assert.equal(at(g, "schedule").value, "Every 4 hours");
  assert.equal(at(g, "schedule").detail, "Not on metered networks");
  assert.equal(at(g, "network").value, "Any, full speed");
});

test("short forms, idle cells and groups", async () => {
  const { GLANCE_GROUPS } = await import("../src/lib/glance.ts");
  const g = glance(profile(), false);
  assert.equal(at(g, "keeps").short, "7d · 4w · 12m · 5y");
  assert.equal(at(g, "schedule").short, "Daily 8:00 PM");
  const plugged = profile();
  plugged.schedule = { ...plugged.schedule, trigger: "manual", on_connect: true };
  assert.equal(at(glance(plugged, false), "schedule").short, "On plug-in");
  assert.equal(at(g, "hooks").idle, true, "no hooks: nothing to say in the grid");
  assert.equal(at(g, "password").idle, undefined, "a Vault's password is worth seeing");
  const p = profile();
  p.engine = "mirror";
  p.vault = null;
  const m = glance(p, false);
  assert.equal(at(m, "password").idle, true);
  assert.equal(at(m, "upkeep").idle, true, "plain folders: no upkeep unless verifying");
  // Every item belongs to exactly one group.
  const grouped = GLANCE_GROUPS.flatMap((x) => x.keys);
  for (const i of glance(p, false)) assert.equal(grouped.filter((k) => k === i.key).length, 1, i.key);
});

test("short forms for one line: lists cut to their first, tests and passwords in brief", () => {
  const p = profile();
  p.sources.one_file_system = true;
  p.destinations = [
    { name: "usb", label: "USB stick", engine: "vault", target: { kind: "local", path: "/run/media/me/STICK" }, offsite: false },
    { name: "office", label: "Office NAS", engine: "rsync", target: { kind: "ssh", host: "office-nas", path: "/srv" }, offsite: true },
  ];
  p.restore_test = { every: "backup", mode: "confidence", damage: "1%" };
  p.webhooks = [{ url: "https://ntfy.sh/x" }, { url: "https://ntfy.sh/y" }];
  p.vault = { ...p.vault!, unlock_file: "/media/key/unlock" };
  p.network = { wifi_only: ["HomeWifi"], bandwidth: [{ from: "08:00", to: "18:00", limit: "1MB" }] } as Profile["network"];
  const g = glance(p, false);
  assert.equal(at(g, "excluded").value, "2 rules · caches · over 4 GB · one disk");
  assert.equal(at(g, "excluded").short, "2 rules · caches +2");
  assert.equal(at(g, "places").short, "USB stick +1");
  assert.equal(at(g, "tests").short, "Each backup · 1% damage");
  assert.equal(at(g, "notify").short, "Desktop + 2 webhooks");
  assert.equal(at(g, "password").value, "Desktop keyring, plus an unlock file");
  assert.equal(at(g, "password").short, "Keyring + unlock file");
  assert.equal(at(g, "network").short, "Only on HomeWifi +1");
  // Short enough already: no short form.
  const plain = glance(profile(), false);
  assert.equal(at(plain, "places").short, undefined);
  assert.equal(at(plain, "excluded").short, "2 rules · caches +1");
  assert.equal(at(plain, "notify").short, undefined);
});
