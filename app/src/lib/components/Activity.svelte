<script lang="ts">
  import PerformanceView from "./PerformanceView.svelte";
  import { api } from "../api";
  import { app } from "../state.svelte";
  import { ago, between, bytes, count, dateTime, phaseLabel, runStatusLabel, time } from "../format";
  import type { LoggedEvent, RunRecord } from "../types";
  import Icon from "./Icon.svelte";
  import { contextMenu, type MenuEntry } from "../menu.svelte";
  import { copyText } from "../clipboard";

  let { id }: { id: string } = $props();
  let runs = $state<RunRecord[] | null>(null);
  let error = $state<string | null>(null);
  let selected = $state<RunRecord | null>(null);
  let events = $state<LoggedEvent[] | null>(null);
  /** Show only runs that went wrong. */
  let problemsOnly = $state(false);
  const problem = (r: RunRecord) => r.status === "failed" || r.status === "completed_with_errors";
  const shown = $derived((runs ?? []).filter((r) => !problemsOnly || problem(r)));
  const problemCount = $derived((runs ?? []).filter(problem).length);

  let copying = $state(false);
  async function copyDiagnostics() {
    copying = true;
    try {
      await copyText(await api.diagnostics(id));
      app.toast("ok", "Diagnostics copied (password commands and hooks left out; folder paths included).");
    } catch (e) {
      app.toast("bad", (e as Error).message);
    } finally {
      copying = false;
    }
  }

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

  /** By what the run is, not by object: `runs` holds reactive proxies, `selected` may not. */
  const isSelected = (r: RunRecord) => !!selected && selected.run_id === r.run_id && selected.started === r.started;

  function select(r: RunRecord | null) {
    selected = r;
    events = null;
    if (r?.run_id) api.runEvents(id, r.run_id).then((ev) => (events = ev), () => (events = []));
  }

  function runMenu(r: RunRecord): MenuEntry[] {
    return [
      { label: "Show details", icon: "activity", onselect: () => select(r) },
      ...(r.error ? [{ label: "Copy the error", icon: "copy" as const, onselect: () => void copyText(r.error!).then(() => app.toast("info", "Copied the error message.")) }] : []),
      "separator",
      { label: "Back up again", icon: "play", onselect: () => void app.backupNow(id) },
    ];
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
  <div class="card h-64 motion-safe:animate-pulse"></div>
{:else if runs.length === 0}
  <div class="card grid place-items-center gap-2 p-16 text-center">
    <div class="title">No backups have run yet</div>
    <p class="max-w-sm text-sm text-fg-muted">Every run, manual or scheduled, is recorded here with its warnings and hook output.</p>
  </div>
{:else}
  <div class="flex h-full min-h-0 gap-3">
    <div class="card flex w-60 shrink-0 flex-col overflow-hidden {app.layout === 'compact' ? 'min-[1100px]:w-[19.5rem]' : 'min-[1100px]:w-72'}">
      <div class="flex items-center gap-2 px-3 pt-3">
        <div class="segmented flex flex-1 [&>button]:flex-1" role="radiogroup" aria-label="Show">
          <button role="radio" aria-checked={!problemsOnly} onclick={() => (problemsOnly = false)}>All</button>
          <button role="radio" aria-checked={problemsOnly} onclick={() => (problemsOnly = true)}>Problems{problemCount ? ` (${problemCount})` : ""}</button>
        </div>
      </div>
      <ul class="min-h-0 flex-1 overflow-y-auto px-1.5 py-2" aria-label="Backup runs">
        {#each shown as r (r.run_id + r.started)}
          <li>
            <button
              class="flex w-full items-center gap-3 rounded-lg px-3 py-2 text-left text-[13.5px] transition-colors {isSelected(r) ? 'bg-selected font-semibold text-on-selected' : 'text-fg-2 hover:bg-surface-3 hover:text-fg'}"
              title="{dateTime(r.started)} · {r.trigger}"
              onclick={() => select(r)}
              use:contextMenu={{ items: () => runMenu(r), label: `Backup run ${dateTime(r.started)}` }}
            >
              <span class="size-2 shrink-0 rounded-full {tone[r.status]}" aria-hidden="true"></span>
              <span class="min-w-0 flex-1 truncate">{runStatusLabel[r.status]}</span>
              <span class="num shrink-0 text-[12.5px] font-normal text-fg-muted">{ago(r.started)}</span>
            </button>
          </li>
        {:else}
          <li class="px-4 py-6 text-center text-[13px] text-fg-muted">No problems in the recent runs.</li>
        {/each}
      </ul>
      <div class="px-3 pb-3">
        <button class="btn btn-ghost w-full justify-center text-[13px]" onclick={copyDiagnostics} disabled={copying} title="Versions, tools, this profile (secrets hidden) and its recent runs, for a bug report">
          <Icon name="copy" size={14} /> {copying ? "Collecting…" : "Copy diagnostics"}
        </button>
      </div>
    </div>

    {#if selected}
      {@const r = selected}
      <div class="card flex min-w-0 flex-1 flex-col overflow-hidden">
        <div class="px-6 pt-5 pb-2">
          <div class="flex flex-wrap items-baseline gap-x-3 gap-y-1">
            <h2 class="text-[17px] font-semibold">{runStatusLabel[r.status]}</h2>
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
          {#if r.performance}<PerformanceView perf={r.performance} />{/if}
          <h3 class="label mb-2">Log</h3>
          {#if !events}
            <div class="h-24 rounded-lg bg-surface-3 motion-safe:animate-pulse"></div>
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
                    <span class="wrap-anywhere text-warning">{e.path}: {e.message}</span>
                  {:else}
                    <span class="whitespace-pre-wrap wrap-anywhere">{e.message}</span>
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
