<script lang="ts">
  import { api } from "../api";
  import { bytes, dateTime, shortDateTime } from "../format";
  import { ApiError, type DirEntry, type Listing, type SnapshotMeta } from "../types";
  import Icon from "./Icon.svelte";
  import RestoreDialog from "./RestoreDialog.svelte";

  let { profile, snapshot, onLocked }: { profile: string; snapshot: SnapshotMeta; onLocked: (retry: () => void) => void } = $props();

  // Start in the first source folder: that's what people look for.
  const initialPath = (() => {
    const src = snapshot.sources[0];
    return src ?? "/";
  })();

  let path = $state(initialPath);
  let listing = $state<Listing | null>(null);
  let error = $state<string | null>(null);
  let filter = $state("");
  let selected = $state<Set<string>>(new Set());
  let restoring = $state<string[] | null>(null);

  function load(p: string) {
    listing = null;
    error = null;
    api.listDir(profile, snapshot.id, p).then(
      (l) => {
        if (p !== path) return;
        // An ancestor that isn't in the snapshot: fall back to the root.
        listing = l;
      },
      (e) => {
        if (e instanceof ApiError && e.code === "password_required") onLocked(() => load(p));
        else if (p !== "/") {
          path = "/";
        } else error = (e as Error).message;
      },
    );
  }

  $effect(() => {
    selected = new Set();
    filter = "";
    load(path);
  });

  const crumbs = $derived.by(() => {
    const parts = path.split("/").filter(Boolean);
    return [{ name: "/", path: "/" }, ...parts.map((name, i) => ({ name, path: "/" + parts.slice(0, i + 1).join("/") }))];
  });

  const entries = $derived.by(() => {
    if (!listing) return [];
    const f = filter.trim().toLowerCase();
    return listing.entries
      .filter((e) => !f || e.name.toLowerCase().includes(f))
      .sort((a, b) => Number(b.kind === "dir") - Number(a.kind === "dir") || a.name.localeCompare(b.name));
  });

  function open(e: DirEntry) {
    if (e.kind === "dir") path = e.path;
  }

  function toggle(e: DirEntry) {
    const s = new Set(selected);
    if (s.has(e.path)) s.delete(e.path);
    else s.add(e.path);
    selected = s;
  }

  const allSelected = $derived(entries.length > 0 && entries.every((e) => selected.has(e.path)));
  function toggleAll() {
    selected = allSelected ? new Set() : new Set(entries.map((e) => e.path));
  }

  const icon = (e: DirEntry) => (e.kind === "dir" ? "folder" : e.kind === "symlink" ? "link" : "file");
</script>

<div class="card flex min-h-0 flex-1 flex-col overflow-hidden">
  <div class="flex items-center gap-3 px-4 py-3">
    <button class="btn btn-ghost size-8 justify-center p-0" disabled={path === "/"} aria-label="Up one folder" onclick={() => (path = crumbs.at(-2)?.path ?? "/")}>
      <Icon name="chevronLeft" size={16} />
    </button>
    <nav class="flex min-w-0 flex-1 items-center gap-0.5 overflow-x-auto text-sm" aria-label="Folder path">
      {#each crumbs as c, i (c.path)}
        {#if i > 1}<Icon name="chevronRight" size={13} class="shrink-0 text-fg-muted" />{/if}
        <button class="shrink-0 rounded-md px-1.5 py-1 hover:bg-surface-3 {i === crumbs.length - 1 ? 'font-medium text-fg' : 'text-fg-2'}" onclick={() => (path = c.path)}>
          {c.name}
        </button>
      {/each}
    </nav>
    <label class="relative w-56">
      <span class="sr-only">Filter this folder</span>
      <Icon name="search" size={15} class="pointer-events-none absolute top-1/2 left-2.5 -translate-y-1/2 text-fg-muted" />
      <input id="browser-filter" class="input h-8 pl-8" placeholder="Filter" title="Filter this folder (/)" bind:value={filter} />
    </label>
  </div>

  <div class="min-h-0 flex-1 overflow-y-auto">
    {#if error}
      <div class="flex items-center gap-2 p-5 text-sm text-danger"><Icon name="alert" size={16} /> {error}</div>
    {:else if !listing}
      <div class="space-y-2 p-4">
        {#each [0, 1, 2, 3, 4, 5] as i (i)}<div class="h-8 animate-pulse rounded-lg bg-surface-3"></div>{/each}
      </div>
    {:else}
      <table class="w-full text-sm">
        <thead class="sticky top-0 bg-surface-2 text-left text-xs text-fg-muted">
          <tr>
            <th class="w-10 py-2 pl-4"><input type="checkbox" class="accent-[var(--c-primary)]" checked={allSelected} onchange={toggleAll} aria-label="Select all" /></th>
            <th class="py-2 font-medium">Name</th>
            <th class="w-28 py-2 pr-4 text-right font-medium">Size</th>
            <th class="w-48 py-2 pr-4 font-medium">Modified</th>
          </tr>
        </thead>
        <tbody>
          {#each entries as e (e.path)}
            <tr class="group transition-colors {selected.has(e.path) ? 'bg-primary-container text-on-primary-container' : 'hover:bg-surface-3'}" ondblclick={() => open(e)}>
              <td class="py-1.5 pl-4"><input type="checkbox" class="accent-[var(--c-primary)]" checked={selected.has(e.path)} onchange={() => toggle(e)} aria-label="Select {e.name}" /></td>
              <td class="py-1.5">
                <button class="flex max-w-full items-center gap-2.5 text-left {e.kind === 'dir' ? 'hover:text-primary' : 'cursor-default'}" onclick={() => open(e)}>
                  <Icon name={icon(e)} size={16} class={e.kind === "dir" ? "text-primary" : "text-fg-muted"} />
                  <span class="truncate">{e.name}</span>
                  {#if e.link}<span class="truncate text-xs text-fg-muted">→ {e.link}</span>{/if}
                </button>
              </td>
              <td class="tabular py-1.5 pr-4 text-right text-fg-2">{e.kind === "dir" ? (e.children != null ? `${e.children} ${e.children === 1 ? "item" : "items"}` : "") : e.kind === "file" ? bytes(e.size) : ""}</td>
              <td class="tabular py-1.5 pr-4 whitespace-nowrap text-fg-2" title={dateTime(e.mtime * 1000)}>{shortDateTime(e.mtime * 1000)}</td>
            </tr>
          {:else}
            <tr><td colspan="4" class="p-8 text-center text-sm text-fg-muted">{filter ? "Nothing matches the filter." : "This folder is empty."}</td></tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </div>

  <div class="flex items-center justify-end gap-2 px-4 py-3">
    {#if selected.size}
      <button class="btn btn-ghost" onclick={() => (selected = new Set())}>Clear selection</button>
      <button class="btn btn-tonal" onclick={() => (restoring = [...selected])}>Restore {selected.size} {selected.size === 1 ? "item" : "items"}</button>
    {:else}
      <button class="btn" onclick={() => (restoring = [path])} title="Restore everything in this folder">Restore this folder</button>
    {/if}
  </div>
</div>

{#if restoring}
  <RestoreDialog {profile} {snapshot} paths={restoring} onclose={() => (restoring = null)} />
{/if}
