<script lang="ts">
  import { api } from "../api";
  import { bytes, count } from "../format";
  import type { Change, Comparison } from "../types";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";

  let { profile, old, newer, title, onclose }: { profile: string; old: string; newer: string | null; title: string; onclose: () => void } = $props();
  let data = $state<Comparison | null>(null);
  let error = $state<string | null>(null);
  let show = $state<Record<Change["change"], boolean>>({ added: true, removed: true, modified: true, type_changed: true });
  let filter = $state("");

  $effect(() => {
    api.compare(profile, old, newer).then((d) => (data = d), (e) => (error = (e as Error).message));
  });

  const visible = $derived(
    data?.changes.filter((c) => show[c.change] && (!filter || c.path.toLowerCase().includes(filter.toLowerCase()))) ?? [],
  );

  const meta = {
    added: { mark: "+", tone: "text-success", label: "added" },
    removed: { mark: "−", tone: "text-danger", label: "removed" },
    modified: { mark: "~", tone: "text-warning", label: "changed" },
    type_changed: { mark: "T", tone: "text-primary", label: "changed type" },
  } as const;
</script>

<Modal {title} {onclose} width="max-w-4xl">
  {#if error}
    <div class="flex items-center gap-2 text-sm text-danger"><Icon name="alert" size={16} /> {error}</div>
  {:else if !data}
    <div class="flex items-center gap-3 text-sm text-fg-2"><Icon name="refresh" class="animate-spin" /> Comparing{newer ? "" : " with the files on disk"}…</div>
  {:else}
    {@const s = data.summary}
    <div class="flex flex-wrap items-center gap-2">
      {#each Object.entries(meta) as [k, m] (k)}
        {@const key = k as Change["change"]}
        {@const n = key === "added" ? s.added : key === "removed" ? s.removed : key === "modified" ? s.modified : s.type_changed}
        <button
          class="rounded-lg px-3 py-1.5 text-[13px] font-medium transition-colors {show[key] ? 'bg-surface-3 text-fg' : 'bg-surface-3 text-fg-muted line-through'}"
          aria-pressed={show[key]}
          onclick={() => (show[key] = !show[key])}
        >
          <span class={m.tone}>{m.mark}</span> {count(n)} {m.label}
        </button>
      {/each}
      <span class="tabular ml-auto text-sm text-fg-2">{s.bytes_delta < 0 ? "−" : "+"}{bytes(Math.abs(s.bytes_delta))}</span>
    </div>
    <label class="relative mt-4 block">
      <span class="sr-only">Filter changes</span>
      <Icon name="search" size={15} class="pointer-events-none absolute top-1/2 left-2.5 -translate-y-1/2 text-fg-muted" />
      <input class="input pl-8" placeholder="Filter by path" bind:value={filter} />
    </label>
    <ul class="mt-3 max-h-[50vh] space-y-px overflow-y-auto rounded-card bg-surface-3 p-1.5 text-sm">
      {#each visible as c (c.path)}
        {@const m = meta[c.change]}
        <li class="flex items-center gap-3 px-3 py-1.5">
          <span class="w-4 shrink-0 text-center font-mono font-semibold {m.tone}">{m.mark}</span>
          <span class="min-w-0 flex-1 truncate font-mono text-[13px]" title={c.path}>{c.path}{c.kind === "dir" ? "/" : ""}</span>
          {#if c.change === "modified"}<span class="shrink-0 text-xs text-fg-muted">{c.fields.join(", ")}</span>{/if}
          <span class="tabular w-36 shrink-0 text-right text-xs text-fg-2">
            {#if c.old_size != null && c.new_size != null && c.old_size !== c.new_size}{bytes(c.old_size)} → {bytes(c.new_size)}{:else}{bytes(c.new_size ?? c.old_size)}{/if}
          </span>
        </li>
      {:else}
        <li class="p-6 text-center text-fg-muted">{data.changes.length ? "Nothing matches." : "No differences."}</li>
      {/each}
    </ul>
    {#if data.truncated}<p class="mt-2 text-xs text-fg-muted">Showing the first 5,000 changes. The CLI (`flux diff`) lists all of them.</p>{/if}
  {/if}
</Modal>
