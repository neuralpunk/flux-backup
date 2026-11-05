<script lang="ts">
  import { api } from "../api";
  import { app } from "../state.svelte";
  import { ago, between, bytes, count, dateTime, phaseLabel, runStatusLabel, time } from "../format";
  import type { LoggedEvent, RunRecord } from "../types";
  import Icon from "./Icon.svelte";

  let { id }: { id: string } = $props();
  let runs = $state<RunRecord[] | null>(null);
  let error = $state<string | null>(null);
  let selected = $state<RunRecord | null>(null);
  let events = $state<LoggedEvent[] | null>(null);

  $effect(() => {
    void app.revision;
    const current = id;
    api.runs(current).then(
      (r) => {
        if (current !== id) return;
        runs = r;
        if (!selected || !r.some((x) => x.run_id === selected!.run_id)) select(r[0] ?? null);
      },
      (e) => (error = (e as Error).message),
    );
  });

  function select(r: RunRecord | null) {
    selected = r;
    events = null;
    if (r?.run_id) api.runEvents(id, r.run_id).then((ev) => (events = ev), () => (events = []));
  }

  const tone: Record<string, string> = {
    completed: "bg-success",
    unchanged: "bg-fg-muted",
    completed_with_errors: "bg-warning",
    failed: "bg-danger",
    cancelled: "bg-fg-muted",
    skipped: "bg-fg-muted",
    dry_run: "bg-fg-muted",
  };
</script>

{#if error}
  <div class="card flex items-center gap-3 p-5 text-danger"><Icon name="alert" /> {error}</div>
{:else if !runs}
  <div class="card h-64 animate-pulse"></div>
{:else if runs.length === 0}
  <div class="card grid place-items-center gap-2 p-16 text-center">
    <div class="title">No backups have run yet</div>
    <p class="max-w-sm text-sm text-fg-muted">Every run, manual or scheduled, is recorded here with its warnings and hook output.</p>
  </div>
{:else}
  <div class="flex h-full min-h-0 gap-3">
    <div class="card flex w-72 shrink-0 flex-col overflow-hidden">
      <ul class="overflow-y-auto p-2" aria-label="Backup runs">
        {#each runs as r (r.run_id + r.started)}
          <li>
            <button
              class="pill flex w-full items-center gap-3 px-4 py-2.5 text-left text-sm transition-colors {selected === r ? 'bg-primary-container text-on-primary-container' : 'text-fg-2 hover:bg-surface-3 hover:text-fg'}"
              title="{dateTime(r.started)} · {r.trigger}"
              onclick={() => select(r)}
            >
              <span class="size-2 shrink-0 rounded-full {tone[r.status]}" aria-hidden="true"></span>
              <span class="min-w-0 flex-1 truncate">{runStatusLabel[r.status]}</span>
              <span class="shrink-0 text-[12.5px] opacity-75">{ago(r.started)}</span>
            </button>
          </li>
        {/each}
      </ul>
    </div>

    {#if selected}
      {@const r = selected}
      <div class="card flex min-w-0 flex-1 flex-col overflow-hidden">
        <div class="px-6 pt-5 pb-2">
          <div class="flex flex-wrap items-baseline gap-x-3 gap-y-1">
            <h2 class="text-lg font-medium">{runStatusLabel[r.status]}</h2>
            <span class="text-[13px] text-fg-muted">{dateTime(r.started)} · {r.trigger} · took {between(r.started, r.finished)}</span>
          </div>
          {#if r.error}<p class="mt-2 text-sm break-words text-danger">{r.error}</p>{/if}
          {#if r.stats}
            <p class="num mt-1 text-sm text-fg-2">
              {count(r.stats.files)} files · {count(r.stats.new + r.stats.changed)} new or changed · {bytes(r.stats.stored_bytes || r.stats.copied_bytes)} written
            </p>
          {/if}
          {#if r.pruned.length}<p class="mt-1 text-[12.5px] text-fg-muted" title={r.pruned.join(", ")}>Removed {r.pruned.length} old {r.pruned.length === 1 ? "snapshot" : "snapshots"}</p>{/if}
        </div>
        <div class="min-h-0 flex-1 overflow-y-auto px-6 pt-3 pb-5">
          <h3 class="label mb-2">Log</h3>
          {#if !events}
            <div class="h-24 animate-pulse rounded-lg bg-surface-3"></div>
          {:else if events.length === 0}
            <p class="text-sm text-fg-muted">No detailed log for this run (only the 50 most recent runs keep one).</p>
          {:else}
            <ol class="space-y-1.5 font-mono text-xs">
              {#each events as e, i (i)}
                <li class="flex gap-3">
                  <span class="shrink-0 text-fg-muted">{time(e.t)}</span>
                  {#if e.event === "phase"}
                    <span class="text-fg-2">── {phaseLabel[e.phase ?? ""] ?? e.phase}</span>
                  {:else if e.event === "warning"}
                    <span class="break-all text-warning">{e.path}: {e.message}</span>
                  {:else}
                    <span class="whitespace-pre-wrap break-all">{e.message}</span>
                  {/if}
                </li>
              {/each}
            </ol>
          {/if}
        </div>
      </div>
    {/if}
  </div>
{/if}
