// A backup's settings at a glance: each one as a short label and value, the same on its
// Overview and in the All backups table. Pure (no Svelte), so it's checked with node.
import { sizeLabel } from "./size.ts";
import { scheduleWords } from "./timeparse.ts";
import type { Profile, Target } from "./types.ts";

/** The section of the Settings page an item belongs to: a click opens that section in a
 * dialog of its own. */
export type GlancePage = "sources" | "destination" | "copies" | "when" | "keep" | "more";

export interface GlanceItem {
  key: GlanceKey;
  label: string;
  /** For a cell of two lines at most. */
  value: string;
  /** For one line (a table cell, Compact's side column) when `value` is longer
   * ("7d · 4w · 12m · 5y"). */
  short?: string;
  /** Says nothing about this backup (no password to keep, no hooks): left out of the grid,
   * shown in the table so its columns stay whole. */
  idle?: boolean;
  /** More, for a tooltip or a second line. */
  detail?: string;
  /** `warn`: something to fix; `off`: not used (shown quieter). */
  tone?: "warn" | "off";
  page: GlancePage;
}

/** The grid's groups, in order, and what's in each. */
export const GLANCE_GROUPS: { title: string; keys: GlanceKey[] }[] = [
  { title: "When and what", keys: ["schedule", "sources", "excluded"] },
  { title: "Where and how", keys: ["where", "places", "stored", "password", "network"] },
  { title: "Safety and alerts", keys: ["keeps", "tests", "upkeep", "unusual", "notify", "hooks"] },
];

export type GlanceKey = "schedule" | "sources" | "excluded" | "where" | "places" | "stored" | "password" | "keeps" | "tests" | "upkeep" | "unusual" | "notify" | "network" | "hooks";

const cap = (s: string) => (s ? s[0]!.toUpperCase() + s.slice(1) : s);
const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;
/** The first `keep` of a list, and how many more: "USB stick +1". `undefined` when the
 * list is short enough already. */
function firstOf(parts: string[], sep: string, keep = 1): string | undefined {
  return parts.length > keep ? `${parts.slice(0, keep).join(sep)} +${parts.length - keep}` : undefined;
}
/** `~/x` for a home folder path; others as they are. */
const tilde = (p: string) => p.replace(/^\/home\/[^/]+(?=\/|$)/, "~");

/** "BackupDisk", "me@nas" — where backups go, short. */
export function placeName(t: Target): string {
  if (t.kind === "ssh") return `${t.user ? `${t.user}@` : ""}${t.host || "a server"}`;
  return t.path.split("/").filter(Boolean).at(-1) ?? (t.path || "not chosen");
}
function placeFull(t: Target): string {
  return t.kind === "ssh" ? `${t.user ? `${t.user}@` : ""}${t.host}:${t.path}` : t.path;
}

/** Does it start backups by itself (on a schedule, or when its drive is plugged in)? */
export function runsOnItsOwn(p: Profile): boolean {
  return p.schedule.trigger !== "manual" || p.schedule.on_connect;
}

/** The schedule as the backend sums it up (`*-*-* 20:00:00 + when drive connected`). */
function scheduleSummary(p: Profile): string {
  const s = p.schedule;
  const main = s.trigger === "manual" ? "manual" : s.trigger === "interval" ? `every ${s.every ?? "?"}` : (s.on_calendar ?? "daily");
  return s.on_connect ? `${main} + when drive connected` : main;
}

/** 0.0042 as "0.42%": a share of the files, to two figures. */
export function shareWords(share: number): string {
  const p = share * 100;
  return p >= 10 ? `${Math.round(p)}%` : p >= 1 ? `${p.toFixed(1)}%` : p >= 0.1 ? `${p.toFixed(2)}%` : "under 0.1%";
}

/** What Flux Sentry has learned of a backup (from its Overview), when known. */
export interface SentryKnows {
  usual: number;
  /** `null`: no amount of change alone would warn. */
  warns_at: number | null;
  runs: number;
}

/** `system`: a system backup (no tripwire files are planted for those). */
export function glance(p: Profile, h24: boolean, sentry?: SentryKnows | null, system = false): GlanceItem[] {
  const wires = !p.tripwires ? "" : system ? "; tripwire files aren't planted for system backups" : "; tripwire files planted";
  const out: GlanceItem[] = [];
  const s = p.schedule;

  // When
  const when = s.trigger === "manual" ? (s.on_connect ? "When its drive is plugged in" : "Only by hand") : cap(scheduleWords(scheduleSummary(p), h24)).replace(" + when drive connected", ", and when plugged in");
  const conditions = [s.trigger === "calendar" && s.catch_up ? "catches up if missed" : "", s.skip_on_battery ? "waits for mains power" : "", s.skip_on_metered && p.target.kind === "ssh" ? "not on metered networks" : "", s.release_drive && p.target.kind === "local" ? "unmounts the drive after" : ""].filter(Boolean);
  // For a table cell: "Daily 8:00 PM", "Mondays 07:30", "On plug-in".
  const shortWhen = when
    .replace(/^Every day at /, "Daily ")
    .replace(/^Weekdays at /, "Weekdays ")
    .replace(/^Every (\w+day) at /, "$1s ")
    .replace(/^When its drive is plugged in$/, "On plug-in")
    .replace(/, and when plugged in$/, " + plug-in");
  out.push({ key: "schedule", label: "Schedule", value: when, short: shortWhen !== when ? shortWhen : undefined, detail: conditions.length ? cap(conditions.join(", ")) : undefined, page: "when" });

  // What
  const paths = p.sources.paths;
  out.push({
    key: "sources",
    label: "Backs up",
    value: paths.length ? `${tilde(paths[0]!) === "~" ? "Home folder" : tilde(paths[0]!)}${paths.length > 1 ? ` +${paths.length - 1}` : ""}` : "Nothing yet",
    detail: paths.length > 1 ? paths.map(tilde).join(", ") : undefined,
    tone: paths.length ? undefined : "warn",
    page: "sources",
  });
  const ex = p.sources;
  const excluded = [ex.exclude.length ? plural(ex.exclude.length, "rule") : "", ex.exclude_caches ? "caches" : "", ex.exclude_larger_than ? `over ${sizeLabel(ex.exclude_larger_than)}` : "", ex.one_file_system ? "one disk" : ""].filter(Boolean);
  out.push({
    key: "excluded",
    label: "Excluded",
    value: excluded.length ? excluded.join(" · ") : "Nothing",
    short: firstOf(excluded, " · ", 2),
    detail: ex.exclude.length ? ex.exclude.slice(0, 8).join("  ") + (ex.exclude.length > 8 ? "  …" : "") : undefined,
    tone: excluded.length ? undefined : "off",
    page: "sources",
  });

  // Where
  out.push({ key: "where", label: "Where", value: placeName(p.target), detail: placeFull(p.target) || undefined, page: "destination" });
  const others = p.destinations ?? [];
  const offsite = p.offsite || others.some((d) => d.offsite);
  out.push({
    key: "places",
    label: "Other copies",
    value: others.length ? others.map((d) => d.label || d.name).join(", ") : "None",
    short: firstOf(others.map((d) => d.label || d.name), ", "),
    detail: others.length ? `${offsite ? "A copy is kept elsewhere" : "None is marked as kept elsewhere"}. ${others.map((d) => `${d.label || d.name}: ${placeFull(d.target)}${d.offsite ? " (offsite)" : ""}`).join("; ")}` : offsite ? "This place is kept elsewhere" : "Only one copy: a second place guards against a lost or broken drive",
    tone: others.length ? undefined : "off",
    page: "copies",
  });

  // How
  const v = p.vault;
  const encrypted = p.engine === "vault" ? v?.encryption !== false : p.engine === "borg" ? (p.borg?.encryption ?? "repokey-blake2") !== "none" : false;
  const stored = {
    mirror: "Plain folders",
    vault: encrypted ? "Encrypted (Vault)" : "Compressed (Vault)",
    rsync: "rsync folders",
    borg: encrypted ? `Borg, encrypted` : "Borg, not encrypted",
  }[p.engine];
  const storedDetail = {
    mirror: "Mirror: browsable folders; unchanged files take no extra space",
    vault: `Vault: deduplicated, compression level ${v?.compression_level ?? 3}${v?.parity ? `, ${v.parity}% repair data` : ""}`,
    rsync: "Works without Flux: plain folders written by rsync",
    borg: `Works without Flux: a Borg repository (${p.borg?.encryption ?? "repokey-blake2"}, ${p.borg?.compression ?? "zstd,3"})`,
  }[p.engine];
  out.push({ key: "stored", label: "Stored as", value: stored, detail: storedDetail, page: "destination" });

  const source = p.engine === "borg" ? p.borg?.password_source : p.engine === "vault" ? v?.password_source : undefined;
  if (!encrypted || !source) {
    out.push({ key: "password", label: "Password", value: "None (not encrypted)", short: "None", tone: "off", idle: true, page: "destination" });
  } else {
    const extra = [v?.unlock_file ? "plus an unlock file" : "", v?.hashing?.level && v.hashing.level !== "standard" ? `${v.hashing.level} hashing` : ""].filter(Boolean);
    const asked = source.from === "prompt";
    const value = { keyring: "Desktop keyring", prompt: "Asked each time", file: "Key file", command: "From a command" }[source.from];
    const brief = { keyring: "Keyring", prompt: "Asked each time", file: "Key file", command: "A command" }[source.from];
    const where = source.from === "file" ? tilde(source.path) : source.from === "command" ? source.command : "";
    out.push({
      key: "password",
      label: "Password",
      value: extra.length ? `${value}, ${extra[0]}` : value,
      short: extra.length ? `${brief} + ${extra[0]!.replace(/^plus an /, "")}` : undefined,
      detail: asked && runsOnItsOwn(p) ? "Scheduled backups will fail: nobody is there to type it" : [where, ...extra].filter(Boolean).join(" · ") || undefined,
      tone: asked && runsOnItsOwn(p) ? "warn" : undefined,
      page: "destination",
    });
  }

  // What's kept
  const r = p.retention;
  let keeps: string;
  if (!r.enabled) keeps = "Everything";
  else {
    const parts = [r.keep_hourly && `${r.keep_hourly} hourly`, r.keep_daily && `${r.keep_daily} daily`, r.keep_weekly && `${r.keep_weekly} weekly`, r.keep_monthly && `${r.keep_monthly} monthly`, r.keep_yearly && `${r.keep_yearly} yearly`].filter(Boolean) as string[];
    keeps = parts.length ? parts.join(" · ") : `The newest ${Math.max(1, r.keep_last)}`;
  }
  const short = r.enabled
    ? [r.keep_hourly && `${r.keep_hourly}h`, r.keep_daily && `${r.keep_daily}d`, r.keep_weekly && `${r.keep_weekly}w`, r.keep_monthly && `${r.keep_monthly}m`, r.keep_yearly && `${r.keep_yearly}y`].filter(Boolean).join(" · ") || undefined
    : undefined;
  out.push({
    key: "keeps",
    label: "Keeps",
    value: keeps,
    short,
    detail: r.enabled ? `At least ${plural(r.safety_floor ?? 1, "complete snapshot")} always${(r.after_problems ?? "hold") === "hold" ? "; nothing removed after a backup with problems" : ""}${r.keep_named ? "; named and pinned ones kept" : ""}` : "Nothing is removed automatically",
    page: "keep",
  });

  // Checks
  const t = p.restore_test;
  const every = t?.every ?? "off";
  const sampleShort = t?.mode === "full" ? "all" : t?.mode === "rolling" ? "rolling" : t?.mode === "confidence" ? `${t.damage ?? "1%"} damage` : undefined;
  const sample = t?.mode === "full" ? "everything" : t?.mode === "percent" ? `${t.percent ?? 5}%` : t?.mode === "rolling" ? "every file monthly" : t?.mode === "confidence" ? `catches ${t.damage ?? "1%"} damage` : plural(t?.files ?? 10, "file");
  out.push({
    key: "tests",
    label: "Restore tests",
    value: every === "off" ? "Only when you ask" : `${{ backup: "After each backup", week: "Weekly", month: "Monthly" }[every]} · ${sample}`,
    short: every === "off" ? undefined : `${{ backup: "Each backup", week: "Weekly", month: "Monthly" }[every]} · ${sampleShort ?? sample}`,
    detail: every === "off" ? "Scheduled tests read files back to prove a restore works" : t?.mode === "confidence" ? `Enough random files to catch damage to ${t.damage ?? "1%"} of them or more, ${t.certainty ?? "99%"} certain${t.compare_with_source ? "; also compared with the files now" : ""}` : t?.compare_with_source ? "Also compared with the files now" : undefined,
    tone: every === "off" ? "off" : undefined,
    page: "keep",
  });
  const m = p.maintenance ?? { verify_monthly: false, free_space_weekly: true };
  const upkeep = [m.verify_monthly ? "verifies every byte monthly" : "", m.free_space_weekly && (p.engine === "vault" || p.engine === "borg") ? "frees space weekly" : ""].filter(Boolean);
  out.push({ key: "upkeep", label: "Upkeep", value: upkeep.length ? cap(upkeep.join(" · ")) : "None", short: upkeep.length ? [m.verify_monthly ? "verify monthly" : "", upkeep.length > (m.verify_monthly ? 1 : 0) ? "free weekly" : ""].filter(Boolean).join(" · ") : undefined, tone: upkeep.length ? undefined : "off", idle: !upkeep.length, page: "when" });
  const unusual = p.unusual_changes ?? "normal";
  out.push({
    key: "unusual",
    label: "Unusual changes",
    value: unusual === "off" ? "Not watched" : unusual === "high" ? "Flux Sentry, closely" : "Flux Sentry on watch",
    short: unusual === "off" ? undefined : unusual === "high" ? "Closely" : "Watched",
    detail:
      unusual === "off"
        ? undefined
        : sentry
          ? `Knows this backup from ${sentry.runs} backups: ${shareWords(sentry.usual)} of its files usually change; it would warn ${sentry.warns_at == null ? "at the marks of ransomware (no amount of change alone stands out here)" : `at about ${shareWords(sentry.warns_at)}, or at the marks of ransomware`}${wires}`
          : `Learns what's normal for this backup; warns of ransomware-like changes and keeps the snapshot before${wires}`,
    tone: unusual === "off" ? "off" : undefined,
    page: "keep",
  });

  // Telling you
  const hooks = p.webhooks?.filter((w) => w.enabled !== false).length ?? 0;
  out.push({
    key: "notify",
    label: "Notifications",
    value: [p.options.notify ? "On this computer" : "", hooks ? plural(hooks, "webhook") : ""].filter(Boolean).join(" + ") || "Off",
    short: p.options.notify && hooks ? `Desktop + ${plural(hooks, "webhook")}` : undefined,
    tone: !p.options.notify && !hooks ? "off" : undefined,
    page: "more",
  });

  // Extras, only when they apply
  if (p.target.kind === "ssh" || others.some((d) => d.target.kind === "ssh")) {
    const n = p.network;
    const parts = [n?.wifi_only?.length ? `only on ${n.wifi_only.join(", ")}` : "", n?.wifi_never?.length ? `never on ${n.wifi_never.join(", ")}` : "", n?.bandwidth?.length ? plural(n.bandwidth.length, "upload limit") : ""].filter(Boolean);
    out.push({ key: "network", label: "Network", value: parts.length ? cap(parts.join(" · ")) : "Any, full speed", short: firstOf(parts.map(cap), " · "), tone: parts.length ? undefined : "off", page: "more" });
  }
  const h = p.hooks;
  const before = h?.pre_backup?.length ?? 0;
  const after = (h?.post_backup?.length ?? 0) + (h?.on_error?.length ?? 0);
  out.push({
    key: "hooks",
    label: "Hooks",
    value: before || after ? [before ? `${before} before` : "", after ? `${after} after` : ""].filter(Boolean).join(" · ") : "None",
    detail: before || after ? [...(h?.pre_backup ?? []), ...(h?.post_backup ?? []), ...(h?.on_error ?? [])].join("; ") : undefined,
    tone: before || after ? undefined : "off",
    idle: !(before || after),
    page: "more",
  });
  return out;
}
