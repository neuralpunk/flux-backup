<script lang="ts">
  // Every distinct version of one file across the snapshots, newest first: open one to
  // look at it, or restore it.
  import { api } from "../api";
  import { app } from "../state.svelte";
  import { bytes, dateTime, dayLabel } from "../format";
  import type { SnapshotMeta, Version } from "../types";
  import Icon from "./Icon.svelte";
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

<Modal title="Versions of “{name}”" {onclose} width="max-w-2xl">
  <p class="-mt-1 mb-4 truncate font-mono text-xs text-fg-muted" title={path}>{path}</p>
  {#if error}
    <div class="flex items-center gap-3 rounded-card bg-danger/10 p-4 text-sm text-danger"><Icon name="alert" /> {error}</div>
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
