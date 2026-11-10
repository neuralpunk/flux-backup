<script lang="ts">
  // Where a run's time went: each phase (wall clock), the stages of the work (summed
  // over threads), throughput and the storage's write time. Folded away: for when a
  // backup is slower than it should be.
  import { bytes } from "../format";
  import type { Performance } from "../types";
  import Icon from "./Icon.svelte";

  let { perf }: { perf: Performance } = $props();

  const secs = (s: number) => (s < 1 ? `${Math.round(s * 1000)} ms` : s < 90 ? `${s.toFixed(1)} s` : `${Math.floor(s / 60)} min ${Math.round(s % 60)} s`);
  const total = $derived(perf.phases.reduce((a, p) => a + p.secs, 0) || 1);
  const stageMax = $derived(Math.max(...(perf.stages ?? []).map((s) => s.secs), 0.000001));
</script>

<details class="group mb-4">
  <summary class="flex cursor-pointer list-none items-center gap-2 text-[13px] font-semibold text-fg-2 select-none hover:text-fg [&::-webkit-details-marker]:hidden">
    <Icon name="chevronRight" size={14} class="transition-transform group-open:rotate-90" />
    <span class="whitespace-nowrap">Where the time went</span>
    {#if perf.throughput}<span class="truncate font-normal text-fg-muted">{bytes(perf.throughput)}/s while copying</span>{/if}
  </summary>
  <div class="mt-3 space-y-4 pl-5">
    <div>
      <h4 class="label mb-1.5">Phases</h4>
      <div class="flex h-2.5 overflow-hidden rounded-full bg-surface-3" aria-hidden="true">
        {#each perf.phases as p, i (p.name)}
          <span class="h-full {['bg-primary', 'bg-success', 'bg-warning', 'bg-fg-muted', 'bg-danger'][i % 5]}" style="width: {(p.secs / total) * 100}%" title="{p.name}: {secs(p.secs)}"></span>
        {/each}
      </div>
      <ul class="mt-2 grid grid-cols-[1fr_auto] gap-x-6 gap-y-0.5 text-[12.5px]">
        {#each perf.phases as p, i (p.name)}
          <li class="contents">
            <span class="flex items-center gap-2"><span class="size-2 rounded-full {['bg-primary', 'bg-success', 'bg-warning', 'bg-fg-muted', 'bg-danger'][i % 5]}"></span>{p.name}</span>
            <span class="num text-right font-mono text-fg-2">{secs(p.secs)}</span>
          </li>
        {/each}
      </ul>
    </div>
    {#if perf.stages?.length}
      <div>
        <h4 class="label mb-1">The work</h4>
        <p class="mb-1.5 text-[12px] text-fg-muted">Time in each step, added up over the threads doing it at once (so it can be longer than the backup took).</p>
        <ul class="space-y-1.5 text-[12.5px]">
          {#each perf.stages as s (s.name)}
            <li class="grid grid-cols-[11rem_1fr_auto] items-center gap-3">
              <span class="truncate">{s.name}</span>
              <span class="h-1.5 overflow-hidden rounded-full bg-surface-3"><span class="block h-full rounded-full bg-primary" style="width: {(s.secs / stageMax) * 100}%"></span></span>
              <span class="num w-20 text-right font-mono text-fg-2">{secs(s.secs)}</span>
            </li>
          {/each}
        </ul>
      </div>
    {/if}
    {#if perf.writes}
      <p class="text-[12.5px] text-fg-2">
        {perf.writes} writes to the storage ({bytes(perf.bytes_written)}){#if perf.write_ms != null}, {perf.write_ms < 1 ? perf.write_ms.toFixed(2) : Math.round(perf.write_ms)} ms each on average{/if}.
      </p>
    {/if}
  </div>
</details>
