<script lang="ts">
  // A backup's last 30 days, one bar per day, its height on the fixed magnitude ramp: the
  // more a day's backups copied (or stored), the taller its bar. The ramp is fixed (not
  // relative to the backup), so bars compare across backups. A day with only failures is a
  // full-height red bar; a day with nothing is a stub on the baseline.
  import { bytes, dayLabel } from "../format";
  import { ramp } from "../ramp";
  import type { Day } from "../types";

  /** `fill`: spread across the width as a small bar chart, with the first day and today
   * named underneath. */
  let { days, fill = false, class: klass = "" }: { days: Day[]; fill?: boolean; class?: string } = $props();

  /** Bar height in px: 4 to 16 in a row, 8 to 40 as a chart; failures fill the height. */
  const full = $derived(fill ? 40 : 16);
  function height(d: Day): number {
    if (d.state === "failed") return full;
    if (d.state !== "ok") return 2;
    return Math.round(fill ? 8 + ramp(d.bytes) * 32 : 4 + ramp(d.bytes) * 12);
  }

  function describe(d: Day): string {
    const when = dayLabel(`${d.date}T12:00:00`);
    if (d.state === "ok") return `${when}: backed up${d.bytes ? `, ${bytes(d.bytes)}` : ", nothing had changed"}`;
    if (d.state === "failed") return `${when}: the backup failed`;
    return `${when}: no backup`;
  }

  const kept = $derived(days.filter((d) => d.state === "ok").length);
  const failed = $derived(days.filter((d) => d.state === "failed").length);
  const summary = $derived(
    `Last ${days.length} days: backed up on ${kept}${failed ? `, failed on ${failed}` : ""}, nothing on ${days.length - kept - failed}`,
  );
</script>

<div class={klass}>
  <div class="flex items-end {fill ? 'h-10 gap-1' : 'h-4'}" role="img" aria-label={summary}>
    {#each days as d (d.date)}
      <!-- Each day's whole column takes the hover title, so thin bars are easy to point at. -->
      <span class="flex h-full shrink-0 items-end {fill ? 'min-w-1 flex-1' : 'w-2 justify-center'}" title={describe(d)}>
        <span
          class="block rounded-[2px] {fill ? 'w-full' : 'w-[5px]'} {d.state === 'ok' ? 'bg-success' : d.state === 'failed' ? 'bg-danger' : 'empty'}"
          style:height="{height(d)}px"
        ></span>
      </span>
    {/each}
  </div>
  {#if fill && days.length}
    <div class="mt-2 flex justify-between text-[11.5px] text-fg-muted">
      <span>{dayLabel(`${days[0]!.date}T12:00:00`)}</span>
      <span>Today</span>
    </div>
  {/if}
</div>

<style>
  .empty {
    background: color-mix(in oklab, var(--c-fg) 16%, transparent);
  }
</style>
