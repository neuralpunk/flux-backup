<script lang="ts">
  import { untrack } from "svelte";
  import { api } from "../api";
  import { bytes, dateTime, shortDateTime } from "../format";
  import { ApiError, type DirEntry, type Listing, type SnapshotMeta } from "../types";
  import Icon from "./Icon.svelte";
  import RestoreDialog from "./RestoreDialog.svelte";
  import VersionsDialog from "./VersionsDialog.svelte";
  import { contextMenu, type MenuEntry } from "../menu.svelte";
  import { copyText } from "../clipboard";
  import { app } from "../state.svelte";

  let {
    profile,
    snapshot,
    onLocked,
    show = null,
  }: {
    profile: string;
    snapshot: SnapshotMeta;
    onLocked: (retry: () => void) => void;
    /** Open at this item's folder, with it selected (from a search). */
    show?: string | null;
  } = $props();

  // Start in the first source folder: that's what people look for.
  const initialPath = (() => {
    if (show) return show.slice(0, show.lastIndexOf("/")) || "/";
    const src = snapshot.sources[0];
    return src ?? "/";
  })();
  // Only on opening (the browser is re-created for each item shown).
  let highlight = untrack(() => show);

  let path = $state(initialPath);
  let listing = $state<Listing | null>(null);
  let error = $state<string | null>(null);
  let filter = $state("");
  let selected = $state<Set<string>>(new Set());
  let restoring = $state<{ snapshot: SnapshotMeta; paths: string[] } | null>(null);
  let versionsOf = $state<string | null>(null);
  const restore = (paths: string[]) => (restoring = { snapshot, paths });

  function load(p: string) {
    listing = null;
    error = null;
    api.listDir(profile, snapshot.id, p).then(
      (l) => {
        if (p !== path) return;
        // An ancestor that isn't in the snapshot: fall back to the root.
        listing = l;
        if (highlight && l.entries.some((e) => e.path === highlight)) {
          selected = new Set([highlight]);
          const h = highlight;
          requestAnimationFrame(() => document.querySelector(`[data-path="${CSS.escape(h)}"]`)?.scrollIntoView({ block: "center" }));
        }
        highlight = null;
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

  /** Folders: go in. Files: open a read-only copy in their usual program. */
  function open(e: DirEntry) {
    if (e.kind === "dir") path = e.path;
    else if (e.kind === "file") void openCopy(e);
  }
  let opening = $state<string | null>(null);
  async function openCopy(e: DirEntry) {
    opening = e.path;
    try {
      await api.openFile(profile, snapshot.id, e.path);
      app.toast("info", `Opened a read-only copy of “${e.name}”. To keep it, restore it.`);
    } catch (err) {
      app.toast("bad", (err as Error).message);
    } finally {
      opening = null;
    }
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

  /** Right-click on a file or folder (all of it is also on screen: double-click, the
   * checkbox, the Restore button). */
  function entryMenu(e: DirEntry): MenuEntry[] {
    const sel = selected.has(e.path);
    return [
      ...(e.kind === "dir" ? [{ label: "Open", icon: "folder" as const, onselect: () => open(e) }] : []),
      ...(e.kind === "file" ? [{ label: "Open a copy", icon: "file" as const, hint: "A read-only copy in its usual program", onselect: () => open(e) }] : []),
      ...(e.kind === "file" ? [{ label: "Versions of this file…", icon: "history" as const, onselect: () => (versionsOf = e.path) }] : []),
      { label: `Restore “${e.name}”…`, icon: "restore", onselect: () => restore([e.path]) },
      ...(selected.size > 1 && sel ? [{ label: `Restore the ${selected.size} selected items…`, icon: "restore" as const, onselect: () => restore([...selected]) }] : []),
      "separator",
      { label: sel ? "Unselect" : "Select", icon: "check", onselect: () => toggle(e) },
      {
        label: "Copy path",
        icon: "copy",
        onselect: () => void copyText(e.path).then(() => app.toast("info", `Copied ${e.path}`)),
      },
    ];
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
            <tr data-path={e.path} class="group transition-colors {selected.has(e.path) ? 'bg-selected text-on-selected' : 'hover:bg-surface-3'}" ondblclick={() => open(e)} use:contextMenu={{ items: () => entryMenu(e), label: e.name }}>
              <td class="py-1.5 pl-4"><input type="checkbox" class="accent-[var(--c-primary)]" checked={selected.has(e.path)} onchange={() => toggle(e)} aria-label="Select {e.name}" /></td>
              <td class="py-1.5">
                <button class="flex max-w-full items-center gap-2.5 text-left {e.kind === 'dir' ? 'hover:text-primary' : 'cursor-default'}" onclick={() => e.kind === "dir" && open(e)} title={e.kind === "file" ? "Double-click to open a read-only copy" : ""}>
                  <Icon name={icon(e)} size={16} class={e.kind === "dir" ? "text-primary" : "text-fg-muted"} />
                  <span class="truncate">{e.name}</span>
                  {#if opening === e.path}<span class="text-xs text-fg-muted">opening…</span>{/if}
                  {#if e.link}<span class="truncate text-xs text-fg-muted">→ {e.link}</span>{/if}
                </button>
              </td>
              <td class="py-1.5 pr-4 text-right font-mono text-[12.5px] text-fg-2">{e.kind === "dir" ? (e.children != null ? `${e.children} ${e.children === 1 ? "item" : "items"}` : "") : e.kind === "file" ? bytes(e.size) : ""}</td>
              <td class="py-1.5 pr-4 font-mono text-[12.5px] whitespace-nowrap text-fg-2" title={dateTime(e.mtime * 1000)}>{shortDateTime(e.mtime * 1000)}</td>
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
      {#if selected.size === 1}
        {@const only = entries.find((e) => selected.has(e.path))}
        {#if only?.kind === "file"}<button class="btn btn-ghost" onclick={() => (versionsOf = only.path)}><Icon name="history" size={14} /> Versions</button>{/if}
      {/if}
      <button class="btn" onclick={() => restore([...selected])}>Restore {selected.size} {selected.size === 1 ? "item" : "items"}</button>
    {:else}
      <button class="btn" onclick={() => restore([path])} title="Restore everything in this folder">Restore this folder</button>
    {/if}
  </div>
</div>

{#if restoring}
  <RestoreDialog {profile} snapshot={restoring.snapshot} paths={restoring.paths} onclose={() => (restoring = null)} />
{/if}
{#if versionsOf}
  {@const p = versionsOf}
  <VersionsDialog profile={profile} path={p} onclose={() => (versionsOf = null)} onrestore={(s) => ((restoring = { snapshot: s, paths: [p] }), (versionsOf = null))} />
{/if}
