<script lang="ts">
  // Every distinct version of one file across the snapshots, newest first: open one to
  // look at it, compare two (or one with the file now), or restore it.
  import { api } from "../api";
  import { app } from "../state.svelte";
  import { bytes, dateTime, dayLabel } from "../format";
  import type { FileDiff, SnapshotMeta, Version } from "../types";
  import Icon from "./Icon.svelte";
  import Callout from "./Callout.svelte";
  import Modal from "./Modal.svelte";

  let { profile, path, onrestore, onclose }: { profile: string; path: string; onrestore: (snapshot: SnapshotMeta) => void; onclose: () => void } = $props();

  const name = $derived(path.split("/").pop() || path);
  let versions = $state<Version[] | null>(null);
  let snaps = $state<SnapshotMeta[]>([]);
  let error = $state<string | null>(null);
  let opening = $state<string | null>(null);

  $effect(() => {
    Promise.all([api.versions(profile, path), api.snapshots(profile)]).then(
      ([v, s]) => {
        snaps = s;
        versions = [...v].reverse();
      },
      (e) => (error = (e as Error).message),
    );
  });

  const snap = (id: string) => snaps.find((s) => s.id === id);
  const newest = $derived(snaps.at(-1)?.id);
  const when = (id: string) => {
    const s = snap(id);
    return s ? (s.name ? `“${s.name}”` : dayLabel(s.started)) : id;
  };

  // Comparing: two versions (by the snapshot holding each), or one with the file now ("now").
  let comparing = $state<{ old: string; new: string } | null>(null);
  let diff = $state<FileDiff | null>(null);
  let diffError = $state<string | null>(null);
  let tool = $state<[string, string] | null>(null);
  $effect(() => {
    void api.preferences().then(
      (p) => (tool = p.diff_tool_used ?? null),
      () => {},
    );
  });
  $effect(() => {
    const c = comparing;
    diff = null;
    diffError = null;
    if (!c) return;
    let current = true;
    api.diffFile(profile, path, c.old, c.new === "now" ? null : c.new).then(
      (d) => current && (diff = d),
      (e) => current && (diffError = (e as Error).message),
    );
    return () => {
      current = false;
    };
  });
  /** Compare version `v` with the file now, or (if it's gone) with the newest version. */
  function compare(v: Version) {
    const gone = versions?.[0]?.last !== newest;
    const other = gone ? versions?.find((x) => x.last !== v.last)?.last : "now";
    comparing = { old: v.last, new: other ?? "now" };
  }
  async function openInTool() {
    if (!diff) return;
    try {
      await api.openDiffTool(diff.old_file, diff.new_file);
    } catch (e) {
      app.toast("bad", (e as Error).message);
    }
  }

  async function open(v: Version) {
    opening = v.last;
    try {
      await api.openFile(profile, v.last, path);
      app.toast("info", `Opened a read-only copy of “${name}” from ${when(v.last)}.`);
    } catch (e) {
      app.toast("bad", (e as Error).message);
    } finally {
      opening = null;
    }
  }
</script>

<Modal title={comparing ? `Compare “${name}”` : `Versions of “${name}”`} {onclose} width={comparing ? "max-w-4xl" : "max-w-2xl"}>
  <p class="-mt-1 mb-4 truncate font-mono text-xs text-fg-muted" title={path}>{path}</p>
  {#if comparing && versions}
    {@const c = comparing}
    <div class="mb-3 flex flex-wrap items-center gap-2 text-sm">
      <button class="btn btn-ghost h-8" onclick={() => (comparing = null)}><Icon name="chevronLeft" size={14} /> Versions</button>
      <span class="flex-1"></span>
      <label class="flex items-center gap-1.5">
        <span class="text-fg-muted">From</span>
        <select class="input h-8 py-0" value={c.old} onchange={(e) => (comparing = { ...c, old: e.currentTarget.value })} aria-label="Older version">
          {#each versions as v (v.last)}<option value={v.last}>Modified {dateTime(v.mtime * 1000)}</option>{/each}
        </select>
      </label>
      <label class="flex items-center gap-1.5">
        <span class="text-fg-muted">to</span>
        <select class="input h-8 py-0" value={c.new} onchange={(e) => (comparing = { ...c, new: e.currentTarget.value })} aria-label="Newer version">
          <option value="now">Now, on your computer</option>
          {#each versions as v (v.last)}<option value={v.last}>Modified {dateTime(v.mtime * 1000)}</option>{/each}
        </select>
      </label>
    </div>
    {#if diffError}
      <Callout tone="bad">{diffError}</Callout>
    {:else if !diff}
      <div class="h-64 rounded-card bg-surface-2 motion-safe:animate-pulse"></div>
    {:else}
      <div class="mb-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-sm">
        {#if diff.identical}
          <span class="flex items-center gap-2"><Icon name="check" size={14} class="text-success" /> The two are identical.</span>
        {:else if diff.binary || diff.too_large}
          <span class="text-fg-2">
            {diff.too_large ? "Too large to compare here" : "Not a text file, so it can't be compared line by line here"}: {bytes(diff.old_size)} → {bytes(diff.new_size)}.
          </span>
        {:else}
          <span class="num font-mono text-success">+{diff.added}</span>
          <span class="num font-mono text-danger">−{diff.removed}</span>
          <span class="text-fg-muted">{diff.added + diff.removed === 1 ? "line" : "lines"} changed</span>
        {/if}
        <span class="flex-1"></span>
        {#if tool && !diff.identical}
          <button class="btn h-8" onclick={openInTool} title="Open both in {tool[1]}"><Icon name="external" size={14} /> Open in {tool[1]}</button>
        {/if}
      </div>
      {#if !diff.identical && !diff.binary && !diff.too_large}
        <div class="diff max-h-[58vh] overflow-auto rounded-card bg-surface-2 py-1 font-mono text-[12.5px] leading-[1.55]" role="table" aria-label="Changes">
          {#each diff.lines as l, i (i)}
            {#if l.kind === "skip"}
              <div class="skip px-3 py-1 text-[12px] text-fg-muted">⋯ {l.count} unchanged {l.count === 1 ? "line" : "lines"}</div>
            {:else}
              <div class="line {l.kind}" role="row">
                <span class="no" role="cell">{l.old ?? ""}</span>
                <span class="no" role="cell">{l.new ?? ""}</span>
                <span class="mark" role="cell" aria-label={l.kind}>{l.kind === "added" ? "+" : l.kind === "removed" ? "−" : ""}</span>
                <span class="text" role="cell">{l.text}</span>
              </div>
            {/if}
          {/each}
          {#if diff.truncated}<div class="px-3 py-1 text-[12px] text-fg-muted">…and {diff.truncated} more changed lines{tool ? `; open in ${tool[1]} to see them all` : ""}.</div>{/if}
        </div>
      {/if}
    {/if}
  {:else if error}
    <Callout tone="bad">{error}</Callout>
  {:else if !versions}
    <div class="space-y-2">
      {#each [0, 1, 2] as i (i)}<div class="h-16 rounded-card bg-surface-2 motion-safe:animate-pulse"></div>{/each}
    </div>
    <p class="mt-3 text-sm text-fg-muted">Looking through {snaps.length || "the"} snapshots…</p>
  {:else if versions.length === 0}
    <p class="text-sm text-fg-2">This file isn't in any snapshot.</p>
  {:else}
    <p class="mb-3 text-sm text-fg-2">
      {versions.length === 1 ? "One version: it hasn't changed in any snapshot." : `${versions.length} different versions, newest first.`}
      {#if versions[0] && versions[0].last !== newest}<span class="text-warning">It isn't in the newest snapshot: it was deleted or moved after {when(versions[0].last)}.</span>{/if}
    </p>
    <div class="rows">
      {#each versions as v, i (v.first)}
        {@const s = snap(v.last)}
        <div class="row items-center">
          <span class="min-w-0 flex-1">
            <span class="flex items-center gap-2 text-[14px] font-medium">
              Modified {dateTime(v.mtime * 1000)}
              {#if i === 0 && v.last === newest}<span class="tag tag-accent">Current</span>{/if}
            </span>
            <span class="num mt-0.5 block text-[12.5px] text-fg-muted">
              {v.kind === "file" ? bytes(v.size) : v.kind} ·
              {v.snapshots === 1 ? `in the snapshot of ${when(v.first)}` : `in ${v.snapshots} snapshots, ${when(v.first)} to ${when(v.last)}`}
            </span>
          </span>
          {#if v.kind === "file"}
            <button class="btn btn-ghost h-8" onclick={() => compare(v)} title="See what changed between this version and the file now, or another version">
              <Icon name="diff" size={14} /> Compare
            </button>
            <button class="btn btn-ghost h-8" onclick={() => open(v)} disabled={opening != null} title="Open a read-only copy in its usual program">
              <Icon name="file" size={14} /> {opening === v.last ? "Opening…" : "Open"}
            </button>
          {/if}
          <button class="btn h-8" disabled={!s} onclick={() => s && onrestore(s)}><Icon name="restore" size={14} /> Restore…</button>
        </div>
      {/each}
    </div>
  {/if}
</Modal>

<style>
  .line {
    display: grid;
    grid-template-columns: 3.2em 3.2em 1.4em 1fr;
    padding-inline: 0.25rem;
  }
  .line .no {
    color: var(--c-fg-muted);
    text-align: right;
    padding-right: 0.6em;
    user-select: none;
  }
  .line .mark {
    text-align: center;
    user-select: none;
  }
  .line .text {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .line.removed {
    background: color-mix(in oklab, var(--c-danger) 13%, transparent);
  }
  .line.removed .mark {
    color: var(--c-danger);
  }
  .line.added {
    background: color-mix(in oklab, var(--c-success) 13%, transparent);
  }
  .line.added .mark {
    color: var(--c-success);
  }
  .skip {
    border-block: 1px solid var(--c-group-edge);
  }
</style>

