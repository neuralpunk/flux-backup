<script lang="ts">
  // Files found in any snapshot. Click to show one in its newest snapshot; double-click a
  // file to open a read-only copy; Versions and Restore are on each row.
  import { untrack } from "svelte";
  import { api } from "../api";
  import { app } from "../state.svelte";
  import { bytes, dayLabel, shortDateTime, dateTime } from "../format";
  import { ApiError, type FindResult, type Found, type SnapshotMeta } from "../types";
  import { contextMenu, type MenuEntry } from "../menu.svelte";
  import { copyText } from "../clipboard";
  import Icon from "./Icon.svelte";
  import RestoreDialog from "./RestoreDialog.svelte";
  import VersionsDialog from "./VersionsDialog.svelte";

  let {
    profile,
    query,
    snaps,
    onshow,
    onclose,
    onLocked,
  }: {
    profile: string;
    query: string;
    snaps: SnapshotMeta[];
    onshow: (snapshot: string, path: string) => void;
    onclose: () => void;
    onLocked: (retry: () => void) => void;
  } = $props();

  let result = $state<FindResult | null>(null);
  let error = $state<string | null>(null);
  let versionsOf = $state<string | null>(null);
  let restoring = $state<{ snapshot: SnapshotMeta; paths: string[] } | null>(null);

  /** `quiet`: search again keeping the results on screen (after a restore). */
  function search(q: string, quiet = false) {
    if (!quiet) {
      result = null;
      error = null;
    }
    api.find(profile, q).then(
      (r) => {
        if (q === query) result = r;
      },
      (e) => {
        if (quiet) return;
        if (e instanceof ApiError && e.code === "password_required") onLocked(() => search(q));
        else error = (e as Error).message;
      },
    );
  }
  $effect(() => search(query));
  // Whether each result is on the computer is checked on every search: after a restore,
  // search again so what came back isn't still marked deleted.
  let seenDisk = untrack(() => app.diskRevision);
  $effect(() => {
    const r = app.diskRevision;
    if (r !== seenDisk) {
      seenDisk = r;
      untrack(() => search(query, true));
    }
  });

  const newest = $derived(snaps.at(-1)?.id);
  const snap = (id: string) => snaps.find((s) => s.id === id);
  const nameOf = (p: string) => p.slice(p.lastIndexOf("/") + 1);
  const folderOf = (p: string) => p.slice(0, p.lastIndexOf("/")) || "/";
  const icon = (f: Found) => (f.kind === "dir" ? "folder" : f.kind === "symlink" ? "link" : "file");

  async function openCopy(f: Found) {
    try {
      await api.openFile(profile, f.latest, f.path);
      app.toast("info", `Opened a read-only copy of “${nameOf(f.path)}”. To keep it, restore it.`);
    } catch (e) {
      app.toast("bad", (e as Error).message);
    }
  }
  const restoreLatest = (f: Found) => {
    const s = snap(f.latest);
    if (s) restoring = { snapshot: s, paths: [f.path] };
  };

  function menu(f: Found): MenuEntry[] {
    return [
      { label: "Show in snapshot", icon: "folder", onselect: () => onshow(f.latest, f.path) },
      ...(f.kind === "file"
        ? [
            { label: "Open a copy", icon: "file" as const, hint: "A read-only copy in its usual program", onselect: () => void openCopy(f) },
            { label: "Versions of this file…", icon: "history" as const, onselect: () => (versionsOf = f.path) },
          ]
        : []),
      { label: "Restore…", icon: "restore", onselect: () => restoreLatest(f) },
      "separator",
      { label: "Copy path", icon: "copy", onselect: () => void copyText(f.path).then(() => app.toast("info", `Copied ${f.path}`)) },
    ];
  }
</script>

<div class="card flex min-h-0 flex-1 flex-col overflow-hidden">
  <div class="flex items-center gap-3 px-6 py-4">
    <div class="min-w-0 flex-1">
      <div class="title truncate">Search results for “{query}”</div>
      <div class="num mt-0.5 text-[13px] text-fg-muted">
        {#if result}
          {result.total === 0 ? "Nothing found" : result.total === 1 ? "1 item" : `${result.total.toLocaleString()} items`} in {result.snapshots} snapshots{result.total > result.hits.length ? ` (showing the first ${result.hits.length}: try a longer name)` : ""}
        {:else if !error}
          Searching {snaps.length} snapshots…
        {/if}
      </div>
    </div>
    <button class="btn" onclick={onclose}><Icon name="x" size={14} /> Close search</button>
  </div>

  <div class="min-h-0 flex-1 overflow-y-auto">
    {#if error}
      <div class="flex items-center gap-2 p-5 text-sm text-danger"><Icon name="alert" size={16} /> {error}</div>
    {:else if !result}
      <div class="space-y-2 px-4 pb-4">
        {#each [0, 1, 2, 3] as i (i)}<div class="h-12 rounded-lg bg-surface-3 motion-safe:animate-pulse"></div>{/each}
      </div>
    {:else if result.hits.length === 0}
      <div class="grid place-items-center gap-2 p-12 text-center">
        <Icon name="search" size={22} class="text-fg-muted" />
        <p class="max-w-sm text-sm text-fg-2">No file or folder named like “{query}” in any snapshot.</p>
        <p class="max-w-sm text-[13px] text-fg-muted">Search matches part of a name. Use <code class="font-mono">*</code> as a wildcard (<code class="font-mono">*.pdf</code>), or include a <code class="font-mono">/</code> to match folders too.</p>
      </div>
    {:else}
      <ul class="px-2 pb-2">
        {#each result.hits as f (f.path)}
          <!-- Not in the newest snapshot: "last in …" rather than a version count. -->
          {@const gone = f.latest !== newest}
          <!-- Deleted means not on the computer now (when that can't be checked: not in the
               newest snapshot). Something restored since is on the computer again. -->
          {@const deleted = f.on_disk === false || (f.on_disk == null && gone)}
          <li use:contextMenu={{ items: () => menu(f), label: nameOf(f.path) }}>
            <div class="group flex items-center gap-3 rounded-lg px-3 py-2 hover:bg-surface-3">
              <button class="flex min-w-[9rem] flex-1 items-center gap-3 text-left" onclick={() => onshow(f.latest, f.path)} ondblclick={() => f.kind === "file" && openCopy(f)} title="Show in its snapshot{f.kind === 'file' ? ' (double-click to open a copy)' : ''}">
                <Icon name={icon(f)} size={16} class="shrink-0 {f.kind === 'dir' ? 'text-primary' : 'text-fg-muted'}" />
                <span class="min-w-0 flex-1">
                  <span class="flex items-center gap-2">
                    <span class="truncate text-[14px]">{nameOf(f.path)}</span>
                    {#if deleted}<span class="tag tag-warn shrink-0" title="Not on your computer any more: deleted or moved since it was backed up">Deleted</span>{/if}
                  </span>
                  <span class="block truncate font-mono text-[11.5px] text-fg-muted">{folderOf(f.path)}</span>
                  <span class="num block truncate text-[12px] text-fg-2 2xl:hidden">
                    {f.kind === "file" ? `${bytes(f.latest_size)} · ` : ""}{gone ? `last in ${snap(f.latest) ? dayLabel(snap(f.latest)!.started) : f.latest}` : f.versions === 1 ? "1 version" : `${f.versions} versions`}
                  </span>
                </span>
              </button>
              <span class="num hidden w-44 shrink-0 text-right text-[12.5px] text-fg-2 2xl:block" title={dateTime(f.latest_mtime * 1000)}>
                {f.kind === "file" ? `${bytes(f.latest_size)} · ` : ""}{shortDateTime(f.latest_mtime * 1000)}
              </span>
              <span class="num hidden w-32 shrink-0 text-right text-[12.5px] text-fg-muted 2xl:block">
                {#if gone}last in {snap(f.latest) ? dayLabel(snap(f.latest)!.started) : f.latest}{:else}{f.versions === 1 ? "1 version" : `${f.versions} versions`}{/if}
              </span>
              {#if f.kind === "file"}
                <button class="btn btn-ghost h-8 shrink-0" onclick={() => (versionsOf = f.path)} title="Every version of this file"><Icon name="history" size={14} /> Versions</button>
              {/if}
              <button class="btn h-8 shrink-0" onclick={() => restoreLatest(f)} title="Restore the newest copy"><Icon name="restore" size={14} /> Restore…</button>
            </div>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</div>

{#if versionsOf}
  {@const p = versionsOf}
  <VersionsDialog {profile} path={p} onclose={() => (versionsOf = null)} onrestore={(s) => ((restoring = { snapshot: s, paths: [p] }), (versionsOf = null))} />
{/if}
{#if restoring}
  <RestoreDialog {profile} snapshot={restoring.snapshot} paths={restoring.paths} onclose={() => (restoring = null)} />
{/if}
