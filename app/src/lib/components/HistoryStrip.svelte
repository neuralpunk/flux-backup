<script lang="ts">
  // A backup's last 30 days, one mark per day, on a fixed magnitude ramp: the more a day's
  // backups copied (or stored), the bigger its mark. The ramp is fixed (not relative to the
  // backup), so marks compare across backups. Red: only failures that day. Hollow: nothing.
  import { bytes, dayLabel } from "../format";
  import type { Day } from "../types";

  /** `fill`: spread across the width, with the first day and today named underneath. */
  let { days, fill = false, class: klass = "" }: { days: Day[]; fill?: boolean; class?: string } = $props();

  /** Diameter in px for a day that kept a copy: 4px (nothing changed, or up to 1 MiB) to
   * 11px (10 GiB or more), on a log scale, in whole pixels; at most the 13px cell minus 2,
   * so neighbours never touch. Fixed, so marks compare across backups. The snapshot list
   * uses the same ramp. */
  const LO = Math.log10(1024 ** 2);
  const HI = Math.log10(10 * 1024 ** 3);
  function size(b: number): number {
    if (b <= 1024 ** 2) return 4;
    const t = Math.min(1, (Math.log10(b) - LO) / (HI - LO));
    return Math.round(4 + t * 7);
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
<div class="flex h-4 items-center" role="img" aria-label={summary}>
  {#each days as d (d.date)}
    <span class="grid h-4 shrink-0 place-items-center {fill ? 'min-w-[13px] flex-1' : 'w-[13px]'}" title={describe(d)}>
      {#if d.state === "ok"}
        <span class="block rounded-full bg-success" style:width="{size(d.bytes)}px" style:height="{size(d.bytes)}px"></span>
      {:else if d.state === "failed"}
        <span class="block size-2 rounded-full bg-danger"></span>
      {:else}
        <span class="block size-1 rounded-full ring-1 ring-fg-muted"></span>
      {/if}
    </span>
  {/each}
</div>
{#if fill && days.length}
  <div class="mt-1.5 flex justify-between text-[11.5px] text-fg-muted">
    <span>{dayLabel(`${days[0]!.date}T12:00:00`)}</span>
    <span>Today</span>
  </div>
{/if}
</div>
