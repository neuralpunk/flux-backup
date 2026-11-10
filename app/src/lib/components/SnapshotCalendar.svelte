<script lang="ts" module>
  /** YYYY-MM-DD in local time. */
  export function dayKey(t: string | Date): string {
    const d = typeof t === "string" ? new Date(t) : t;
    return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
  }
</script>

<script lang="ts">
  import { untrack } from "svelte";
  // A month of snapshots: days with one are marked (amber when files were skipped, red
  // for a backup that failed that day); click a day to see only its snapshots.
  import type { SnapshotMeta } from "../types";
  import Icon from "./Icon.svelte";

  let {
    snaps,
    failed,
    day,
    anchor,
    onpick,
  }: {
    snaps: SnapshotMeta[];
    /** Days (YYYY-MM-DD, local) with a failed backup. */
    failed: Set<string>;
    /** The day picked, if any. */
    day: string | null;
    /** A date in the month to show first. */
    anchor: string | null;
    onpick: (day: string | null) => void;
  } = $props();

  // The month shown first; after that, the arrows move it.
  const start = untrack(() => (anchor ? new Date(anchor) : new Date()));
  let year = $state(start.getFullYear());
  let month = $state(start.getMonth());

  const byDay = $derived.by(() => {
    const m = new Map<string, { n: number; skipped: boolean }>();
    for (const s of snaps) {
      const k = dayKey(s.started);
      const e = m.get(k) ?? { n: 0, skipped: false };
      e.n++;
      e.skipped ||= s.status === "complete_with_errors";
      m.set(k, e);
    }
    return m;
  });

  // Weeks start on Monday.
  const cells = $derived.by(() => {
    const first = new Date(year, month, 1);
    const lead = (first.getDay() + 6) % 7;
    const days = new Date(year, month + 1, 0).getDate();
    const out: (Date | null)[] = Array.from({ length: lead }, () => null);
    for (let d = 1; d <= days; d++) out.push(new Date(year, month, d));
    while (out.length % 7) out.push(null);
    return out;
  });
  const title = $derived(new Date(year, month, 1).toLocaleDateString(undefined, { month: "long", year: "numeric" }));
  const weekdays = Array.from({ length: 7 }, (_, i) => new Date(2024, 0, 1 + i).toLocaleDateString(undefined, { weekday: "narrow" }));
  const today = dayKey(new Date());
  const oldest = $derived(snaps.length ? new Date(snaps[0]!.started) : new Date());
  const canBack = $derived(year > oldest.getFullYear() || (year === oldest.getFullYear() && month > oldest.getMonth()));
  const canForward = $derived(new Date(year, month + 1, 1) <= new Date());

  function step(by: number) {
    const d = new Date(year, month + by, 1);
    year = d.getFullYear();
    month = d.getMonth();
  }
</script>

<div class="px-2 pt-2" role="group" aria-label="Snapshots by day">
  <div class="flex items-center justify-between pb-1">
    <button class="btn-icon size-7" disabled={!canBack} aria-label="Previous month" onclick={() => step(-1)}><Icon name="chevronRight" size={14} class="rotate-180" /></button>
    <span class="text-[12.5px] font-semibold">{title}</span>
    <button class="btn-icon size-7" disabled={!canForward} aria-label="Next month" onclick={() => step(1)}><Icon name="chevronRight" size={14} /></button>
  </div>
  <div class="grid grid-cols-7 gap-y-0.5 text-center">
    {#each weekdays as w, i (i)}<span class="pb-0.5 text-[10.5px] font-medium text-fg-muted">{w}</span>{/each}
    {#each cells as c, i (i)}
      {#if c}
        {@const k = dayKey(c)}
        {@const info = byDay.get(k)}
        {@const bad = failed.has(k)}
        {@const label = `${c.toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long" })}: ${info ? `${info.n} snapshot${info.n === 1 ? "" : "s"}` : "no snapshots"}${info?.skipped ? ", files skipped" : ""}${bad ? ", a backup failed" : ""}`}
        <button
          class="relative mx-auto grid size-7 place-items-center rounded-md text-[12px] tabular transition-colors
            {day === k ? 'bg-selected font-semibold text-on-selected' : info ? 'font-medium text-fg hover:bg-surface-3' : 'text-fg-muted'}
            {k === today && day !== k ? 'ring-1 ring-[var(--c-sep)]' : ''}"
          disabled={!info}
          aria-pressed={day === k}
          aria-label={label}
          title={label}
          onclick={() => onpick(day === k ? null : k)}
        >
          {c.getDate()}
          {#if info || bad}
            <span class="absolute bottom-0.5 left-1/2 flex -translate-x-1/2 gap-0.5">
              {#if info}<span class="size-1 rounded-full {info.skipped ? 'bg-warning' : 'bg-success'}"></span>{/if}
              {#if bad}<span class="size-1 rounded-full bg-danger"></span>{/if}
            </span>
          {/if}
        </button>
      {:else}
        <span></span>
      {/if}
    {/each}
  </div>
</div>
