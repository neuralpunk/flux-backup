// What a backup changed, in a few words: "214 new · 38 changed · 2 deleted". Pure (no
// Svelte), so it's checked with node.

export interface ChangeCounts {
  new: number;
  changed: number;
  deleted: number;
}

const n = (v: number) => v.toLocaleString("en-US");

/** The non-zero counts, or "No changes". */
export function changeWords(s: ChangeCounts): string {
  const parts = [s.new ? `${n(s.new)} new` : "", s.changed ? `${n(s.changed)} changed` : "", s.deleted ? `${n(s.deleted)} deleted` : ""].filter(Boolean);
  return parts.length ? parts.join(" · ") : "No changes";
}

/** The same, short, as a diff says it: "+214 ~38 −2" (`null`: no changes). */
export function changeMarks(s: ChangeCounts): string | null {
  const parts = [s.new ? `+${n(s.new)}` : "", s.changed ? `~${n(s.changed)}` : "", s.deleted ? `−${n(s.deleted)}` : ""].filter(Boolean);
  return parts.length ? parts.join(" ") : null;
}

/** The snapshot before run `i`'s in a list of runs, newest first (to compare with). */
export function snapshotBefore(runs: { snapshot: string | null }[], i: number): string | null {
  return runs.slice(i + 1).find((r) => r.snapshot)?.snapshot ?? null;
}
