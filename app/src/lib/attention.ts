// What needs you, counted one way everywhere: the sidebar, All backups' headline and the
// line under it. Two kinds, never mixed up:
//   * not protected: no good copy to restore from (failed, at risk, never backed up)
//   * needs a look: protected, but something is worth seeing (low space, not tested…)
// Pure (no Svelte), so it's checked with node.

export type Tone = "ok" | "warn" | "bad" | "run" | "idle";

/** A health warning's kind, as a few words for a status line. */
const KIND_LABEL: Record<string, string> = {
  overdue: "Overdue",
  failing: "Failing",
  low_space: "Low on space",
  unplugged: "Drive not seen",
  source_missing: "Source missing",
  destination_failing: "A copy failed",
  restore_test_failed: "Restore test failed",
  unverified: "Not tested lately",
  unusual: "Unusual changes",
};

/** "Low on space", or `null` for a kind without a short label. */
export function kindLabel(kind: string | undefined): string | null {
  return (kind && KIND_LABEL[kind]) || null;
}

export interface Counts {
  total: number;
  protected: number;
  /** Not protected (a running backup is counted once it's done). */
  notProtected: number;
  /** Protected, with something worth a look. */
  toLook: number;
}

export function counts(list: { tone: Tone; protected: boolean }[]): Counts {
  return {
    total: list.length,
    protected: list.filter((x) => x.protected).length,
    notProtected: list.filter((x) => !x.protected && x.tone !== "run").length,
    toLook: list.filter((x) => x.protected && (x.tone === "warn" || x.tone === "bad")).length,
  };
}

/** The sidebar's line under All backups: "2 not protected · 3 to look at". */
export function summary(c: Counts): string {
  if (c.notProtected) return `${c.notProtected} not protected${c.toLook ? ` · ${c.toLook} to look at` : ""}`;
  if (c.toLook) return `${c.toLook} ${c.toLook === 1 ? "needs" : "need"} a look`;
  return c.total === 1 ? "Protected" : "All protected";
}

/** After the first backup that needs you is explained: what else does, in the same two
 * kinds ("And 1 more isn't protected, and 3 need a look below."); `null` when nothing. */
export function remainder(c: Counts, first: { protected: boolean }): string | null {
  const np = c.notProtected - (first.protected ? 0 : 1);
  const tl = c.toLook - (first.protected ? 1 : 0);
  const parts = [np ? `${np} more ${np === 1 ? "isn't" : "aren't"} protected` : "", tl ? `${tl} ${np ? "" : "more "}${tl === 1 ? "needs" : "need"} a look` : ""].filter(Boolean);
  return parts.length ? `And ${parts.join(", and ")} below.` : null;
}
