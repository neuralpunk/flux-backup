<script lang="ts">
  import { app, type Job } from "../state.svelte";
  import { bytes, count, duration, phaseLabel, timeLeft } from "../format";
  import Icon from "./Icon.svelte";

  /** `inline`: inside the Compact Overview's answer card, without a card of its own. */
  let { job, inline = false }: { job: Job; inline?: boolean } = $props();

  const p = $derived(job.progress);
  const transferring = $derived(p?.phase === "transferring" && p.bytes_total > 0);
  const pct = $derived(transferring && p ? Math.min(100, Math.round((p.bytes_done / p.bytes_total) * 100)) : null);
  const phase = $derived(job.finished ? "Finished" : p?.phase === "transferring" ? "Copying" : phaseLabel[p?.phase ?? "preparing"]);
</script>

<section class={inline ? "flex flex-col gap-2" : "card flex flex-col gap-3 px-5 py-4"} aria-live="polite" aria-label="Backup in progress">
  <div class="flex flex-wrap items-baseline justify-between gap-x-6 gap-y-1">
    <div class="flex items-baseline gap-3">
      <span class="{inline ? 'text-[14px]' : 'text-[16px]'} font-semibold">{phase}</span>
      {#if pct != null}<span class="num font-mono {inline ? 'text-[14px]' : 'text-[16px]'} font-medium text-primary">{pct}%</span>{/if}
      {#if job.warnings.length}
        <span class="tag text-warning" title={job.warnings.map((w) => `${w.path}: ${w.message}`).join("\n")}>
          <Icon name="alert" size={12} /> {count(job.warnings.length)} {job.warnings.length === 1 ? "file" : "files"} skipped
        </span>
      {/if}
    </div>
    <span class="num font-mono {inline ? 'text-[12px]' : 'text-[13px]'} text-fg-muted">
      {#if p}
        {#if p.phase === "waiting"}
          for another backup to finish (one at a time) · {duration(p.elapsed_secs)}
        {:else if p.phase === "scanning"}
          {count(p.scanned)} items found
        {:else if transferring}
          {bytes(p.bytes_done)} / {bytes(p.bytes_total)} · {bytes(p.bytes_per_sec)}/s · {count(p.entries_done)}/{count(p.entries_total)}{#if p.eta_secs != null}&nbsp;· {timeLeft(p.eta_secs)}{/if}
        {:else}
          {duration(p.elapsed_secs)} elapsed
        {/if}
      {:else}
        starting…
      {/if}
    </span>
  </div>
  {#snippet bar(cls: string)}
  <div class="h-1.5 overflow-hidden rounded-full bg-track {cls}" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct ?? undefined}>
    {#if pct != null}
      <div class="h-full rounded-full bg-primary motion-safe:transition-[width] motion-safe:duration-300 motion-safe:ease-out" style:width="{pct}%"></div>
    {:else if job.finished}
      <div class="h-full w-full rounded-full bg-success"></div>
    {:else}
      <div class="h-full w-1/4 rounded-full bg-primary/60 motion-safe:animate-pulse"></div>
    {/if}
  </div>
  {/snippet}
  {#snippet shutdown(short: boolean)}
    <label class="flex shrink-0 items-center gap-2 self-start {short ? 'text-[12.5px]' : 'text-[13px]'} text-fg-2">
      <input type="checkbox" class="accent-[var(--c-primary)]" checked={job.shutdownAfter ?? false} onchange={(e) => void app.setShutdownAfter(job.profile, e.currentTarget.checked)} />
      <Icon name="power" size={13} class="text-fg-muted" /> {short ? "Shut down when done" : "Shut down the computer when done"}
    </label>
  {/snippet}
  {#if inline}
    <!-- One line: the bar, and shutting down when it's done at its end. -->
    <div class="flex items-center gap-4">
      {@render bar("min-w-0 flex-1")}
      {#if !job.finished}{@render shutdown(true)}{/if}
    </div>
  {:else}
    {@render bar("")}
    {#if !job.finished}{@render shutdown(false)}{/if}
  {/if}
</section>
