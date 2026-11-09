<script lang="ts">
  import { api } from "../api";
  import { app } from "../state.svelte";
  import { bytes, count } from "../format";
  import { sizeLabel } from "../size";
  import type { Change, Comparison, SnapshotMeta } from "../types";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";
  import RestoreDialog from "./RestoreDialog.svelte";

  let {
    profile,
    old,
    newer,
    title,
    oldSnap = null,
    onclose,
  }: {
    profile: string;
    old: string;
    newer: string | null;
    title: string;
    /** The older snapshot, so removed and changed files can be restored from it. */
    oldSnap?: SnapshotMeta | null;
    onclose: () => void;
  } = $props();
  let data = $state<Comparison | null>(null);
  let error = $state<string | null>(null);
  let show = $state<Record<Change["change"], boolean>>({ added: true, removed: true, modified: true, type_changed: true });
  /** Folders whose only change is their date (any file added or removed in them changes
   * it): noise next to the files themselves, so hidden until asked for. */
  let showFolderDates = $state(false);
  let filter = $state("");
  let restoring = $state<{ paths: string[]; missing: boolean } | null>(null);

  $effect(() => {
    // Compared with the files on the computer: compare again after a restore changes them.
    if (newer == null) void app.diskRevision;
    api.compare(profile, old, newer).then((d) => (data = d), (e) => (error = (e as Error).message));
  });

  const folderDate = (c: Change) => c.kind === "dir" && c.change === "modified";
  const folderDates = $derived(data?.changes.filter(folderDate).length ?? 0);
  const visible = $derived(
    data?.changes.filter((c) => show[c.change] && (showFolderDates || !folderDate(c)) && (!filter || c.path.toLowerCase().includes(filter.toLowerCase()))) ?? [],
  );
  /** Removed items, without those inside a removed folder (restoring it brings them). */
  const removedTops = $derived.by(() => {
    const r = (data?.changes ?? []).filter((c) => c.change === "removed").map((c) => c.path);
    const dirs = new Set((data?.changes ?? []).filter((c) => c.change === "removed" && c.kind === "dir").map((c) => c.path));
    return r.filter((p) => {
      for (let i = p.lastIndexOf("/"); i > 0; i = p.lastIndexOf("/", i - 1)) if (dirs.has(p.slice(0, i))) return false;
      return true;
    });
  });

  const meta = {
    added: { mark: "+", tone: "text-success", label: "added" },
    removed: { mark: "−", tone: "text-danger", label: "removed" },
    modified: { mark: "~", tone: "text-warning", label: "changed" },
    type_changed: { mark: "T", tone: "text-primary", label: "changed type" },
  } as const;
  const fieldName: Record<string, string> = { content: "contents", mtime: "date", mode: "permissions", owner: "owner", xattrs: "attributes", target: "link target" };
  /** What a restore of `c` brings back from the older snapshot, in words. */
  const restoreLabel = (c: Change) => (c.change === "removed" ? "Restore" : "Restore old version");
  const canRestore = (c: Change) => oldSnap != null && (c.change === "removed" || ((c.change === "modified" || c.change === "type_changed") && c.kind !== "dir"));
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
        {@const n = key === "added" ? s.added : key === "removed" ? s.removed : key === "modified" ? s.modified - folderDates : s.type_changed}
        <button
          class="rounded-lg px-3 py-1.5 text-[13px] font-medium transition-colors {show[key] ? 'bg-surface-3 text-fg' : 'bg-surface-3 text-fg-muted line-through'}"
          aria-pressed={show[key]}
          onclick={() => (show[key] = !show[key])}
        >
          <span class={m.tone}>{m.mark}</span> {count(n)} {m.label}
        </button>
      {/each}
      {#if folderDates}
        <button
          class="rounded-lg px-3 py-1.5 text-[13px] transition-colors {showFolderDates ? 'bg-surface-3 font-medium text-fg' : 'bg-surface-3 text-fg-muted hover:text-fg'}"
          aria-pressed={showFolderDates}
          title="Folders whose only change is their date (adding or removing a file inside changes it)"
          onclick={() => (showFolderDates = !showFolderDates)}
        >
          {count(folderDates)} folder {folderDates === 1 ? "date" : "dates"}
        </button>
      {/if}
      {#if s.bytes_delta !== 0}<span class="tabular ml-auto text-sm text-fg-2">Size {s.bytes_delta < 0 ? "−" : "+"}{bytes(Math.abs(s.bytes_delta))}</span>{/if}
    </div>

    {#if data.left_out_by_size && data.size_limit}
      <div class="mt-3 flex gap-3 rounded-card bg-surface-3 px-4 py-3 text-sm" role="note">
        <Icon name="info" size={15} class="mt-0.5 shrink-0 text-fg-muted" />
        <span class="text-fg-2">
          <span class="font-medium text-fg">{count(data.left_out_by_size)} {data.left_out_by_size === 1 ? "file" : "files"} on disk {data.left_out_by_size === 1 ? "isn't" : "aren't"} compared:</span>
          {data.left_out_by_size === 1 ? "it's" : "they're"} larger than this backup's size limit ({sizeLabel(data.size_limit)}), so no backup holds {data.left_out_by_size === 1 ? "it" : "them"}.
        </span>
      </div>
    {/if}

    <div class="mt-4 flex items-center gap-2">
      <label class="relative block flex-1">
        <span class="sr-only">Filter changes</span>
        <Icon name="search" size={15} class="pointer-events-none absolute top-1/2 left-2.5 -translate-y-1/2 text-fg-muted" />
        <input class="input pl-8" placeholder="Filter by path" bind:value={filter} />
      </label>
      {#if oldSnap && removedTops.length}
        <button class="btn shrink-0" onclick={() => (restoring = { paths: removedTops, missing: newer == null })}>
          <Icon name="restore" size={14} /> Restore all removed
        </button>
      {/if}
    </div>
    <ul class="mt-3 max-h-[50vh] space-y-px overflow-y-auto rounded-card bg-surface-3 p-1.5 text-sm">
      {#each visible as c (c.path)}
        {@const m = meta[c.change]}
        <li class="group flex min-h-[42px] items-center gap-3 rounded-md px-3 py-1 hover:bg-track/50">
          <span class="w-4 shrink-0 text-center font-mono font-semibold {m.tone}">{m.mark}</span>
          <span class="min-w-0 flex-1 truncate font-mono text-[13px]" title={c.path}>{c.path}{c.kind === "dir" ? "/" : ""}</span>
          {#if c.change === "modified"}<span class="shrink-0 text-xs text-fg-muted">{c.fields.map((f) => fieldName[f] ?? f).join(", ")}</span>{/if}
          <span class="tabular w-36 shrink-0 text-right text-xs text-fg-2">
            {#if c.kind === "dir"}{:else if c.old_size != null && c.new_size != null && c.old_size !== c.new_size}{bytes(c.old_size)} → {bytes(c.new_size)}{:else}{bytes(c.new_size ?? c.old_size)}{/if}
          </span>
          <span class="flex w-36 shrink-0 justify-end">
            {#if canRestore(c)}
              <button
                class="btn btn-ghost h-7 px-2 text-[12.5px] opacity-0 group-focus-within:opacity-100 group-hover:opacity-100 focus:opacity-100"
                onclick={() => (restoring = { paths: [c.path], missing: newer == null && c.change === "removed" })}
                title="{restoreLabel(c)} from the older snapshot"
              ><Icon name="restore" size={13} /> {restoreLabel(c)}</button>
            {/if}
          </span>
        </li>
      {:else}
        <li class="p-6 text-center text-fg-muted">
          {#if data.changes.length && !filter && folderDates === data.changes.length}
            No files changed; only folder dates differ.
            <button class="ml-1 text-primary hover:underline" onclick={() => (showFolderDates = true)}>Show them</button>
          {:else}
            {data.changes.length ? "Nothing matches." : "No differences."}
          {/if}
        </li>
      {/each}
    </ul>
    {#if data.truncated}<p class="mt-2 text-xs text-fg-muted">Showing the first 5,000 changes. The CLI (`flux diff`) lists all of them.</p>{/if}
  {/if}
</Modal>

{#if restoring && oldSnap}
  <RestoreDialog {profile} snapshot={oldSnap} paths={restoring.paths} missing={restoring.missing} onclose={() => (restoring = null)} />
{/if}
