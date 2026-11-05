<script lang="ts">
  import type { Job } from "../state.svelte";
  import { bytes, count, duration, phaseLabel } from "../format";
  import Icon from "./Icon.svelte";

  let { job }: { job: Job } = $props();

  const p = $derived(job.progress);
  const transferring = $derived(p?.phase === "transferring" && p.bytes_total > 0);
  const pct = $derived(transferring && p ? Math.min(100, Math.round((p.bytes_done / p.bytes_total) * 100)) : null);
  const phase = $derived(job.finished ? "Finished" : p?.phase === "transferring" ? "Copying" : phaseLabel[p?.phase ?? "preparing"]);
</script>

<section class="card flex flex-col gap-4 px-6 py-[22px]" aria-live="polite" aria-label="Backup in progress">
  <div class="flex flex-wrap items-baseline justify-between gap-x-6 gap-y-1">
    <div class="flex items-baseline gap-3">
      <span class="text-lg font-medium">{phase}</span>
      {#if pct != null}<span class="num font-mono text-lg font-medium text-primary">{pct}%</span>{/if}
      {#if job.warnings.length}
        <span class="tag text-warning" title={job.warnings.map((w) => `${w.path}: ${w.message}`).join("\n")}>
          <Icon name="alert" size={12} /> {count(job.warnings.length)} {job.warnings.length === 1 ? "file" : "files"} skipped
        </span>
      {/if}
    </div>
    <span class="num font-mono text-[13px] text-fg-muted">
      {#if p}
        {#if p.phase === "scanning"}
          {count(p.scanned)} items found
        {:else if transferring}
          {bytes(p.bytes_done)} / {bytes(p.bytes_total)} · {bytes(p.bytes_per_sec)}/s · {count(p.entries_done)}/{count(p.entries_total)}{#if p.eta_secs != null}&nbsp;· {duration(p.eta_secs)} left{/if}
        {:else}
          {duration(p.elapsed_secs)} elapsed
        {/if}
      {:else}
        starting…
      {/if}
    </span>
  </div>
  <div class="h-2 overflow-hidden rounded bg-track" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct ?? undefined}>
    {#if pct != null}
      <div class="h-full rounded bg-primary motion-safe:transition-[width] motion-safe:duration-300 motion-safe:ease-out" style:width="{pct}%"></div>
    {:else if job.finished}
      <div class="h-full w-full rounded bg-success"></div>
    {:else}
      <div class="h-full w-1/4 rounded bg-primary/60 motion-safe:animate-pulse"></div>
    {/if}
  </div>
</section>
