<script lang="ts">
  import { untrack } from "svelte";
  import { api } from "../api";
  import { bytes, count, dateTime, shortDateTime } from "../format";
  import { ApiError, type DirEntry, type Listing, type SnapshotMeta } from "../types";
  import Icon from "./Icon.svelte";
  import RestoreDialog from "./RestoreDialog.svelte";
  import ExportSnapshotDialog from "./ExportSnapshotDialog.svelte";
  import VersionsDialog from "./VersionsDialog.svelte";
  import PreviewDialog from "./PreviewDialog.svelte";
  import { contextMenu, type MenuEntry } from "../menu.svelte";
  import { copyText } from "../clipboard";
  import { app } from "../state.svelte";
  import { sizeLabel } from "../size";

  let {
    profile,
    snapshot,
    onLocked,
    show = null,
    openAt = null,
    onlyDeleted = false,
  }: {
    profile: string;
    snapshot: SnapshotMeta;
    onLocked: (retry: () => void) => void;
    /** Open at this item's folder, with it selected (from a search). */
    show?: string | null;
    /** Open in this folder (from a file manager's request). */
    openAt?: string | null;
    /** Start with *Only deleted* on. */
    onlyDeleted?: boolean;
  } = $props();

  // Start in the first source folder: that's what people look for.
  const initialPath = (() => {
    if (openAt) return openAt;
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
  let restoring = $state<{ snapshot: SnapshotMeta; paths: string[]; missing: boolean } | null>(null);
  let versionsOf = $state<string | null>(null);
  /** A file being looked at before restoring it. */
  let previewing = $state<DirEntry | null>(null);
  /** Whether each item seen so far is still on disk (from the listings). */
  const onDisk = new Map<string, boolean | null>();
  const restore = (paths: string[]) => (restoring = { snapshot, paths, missing: paths.every((p) => onDisk.get(p) === false) });
  let exporting = $state<string[] | null>(null);
  /** Only items deleted (or moved) since this snapshot. */
  let onlyMissing = $state(untrack(() => onlyDeleted));
  const sizeLimit = $derived(app.profiles.find((p) => p.id === profile)?.size_limit ?? null);

  /**
   * List folder `p`. "open": moving to it (placeholder rows until it's in). "again" and
   * "auto": checking again what's on the computer, keeping the rows on screen; "again"
   * was asked for, so it says when it can't, while "auto" (after a restore, or coming
   * back to the window) fails quietly.
   */
  function load(p: string, how: "open" | "again" | "auto" = "open"): Promise<void> {
    if (how === "open") {
      listing = null;
      error = null;
    }
    // The folder is filtered and put in order where the snapshot is read, and comes at
    // most 2,000 items at a time: a folder of a hundred thousand, sent and shown whole,
    // stops the window answering.
    const [f, m] = untrack(() => [filter.trim(), onlyMissing] as const);
    return api.listDir(profile, snapshot.id, p, f || null, m).then(
      (l) => {
        // An answer for a folder, or a filter, that has been left since.
        if (p !== path || f !== filter.trim() || m !== onlyMissing) return;
        error = null;
        listing = l;
        for (const e of l.entries) onDisk.set(e.path, e.on_disk ?? null);
        if (highlight && l.entries.some((e) => e.path === highlight)) {
          selected = new Set([highlight]);
          const h = highlight;
          requestAnimationFrame(() => document.querySelector(`[data-path="${CSS.escape(h)}"]`)?.scrollIntoView({ block: "center" }));
          highlight = null;
        } else if (highlight && l.matching > l.entries.length && !f) {
          // The item to show is past what a big folder sends: ask for it by name.
          filter = highlight.slice(highlight.lastIndexOf("/") + 1);
        } else highlight = null;
      },
      (e) => {
        if (p !== path || how === "auto") return;
        const locked = e instanceof ApiError && e.code === "password_required";
        if (how === "again") {
          if (locked) onLocked(() => void load(p, "again"));
          else app.toast("bad", `Couldn't check this folder again: ${(e as Error).message}`);
        } else if (locked) onLocked(() => void load(p));
        // An ancestor that isn't in the snapshot: fall back to the root.
        else if (p !== "/") path = "/";
        else error = (e as Error).message;
      },
    );
  }

  // The selection stays while moving between folders, so files from several folders
  // can be restored together.
  let shownPath: string | null = null;
  $effect(() => {
    const p = path;
    untrack(() => {
      // Into another folder: its own filter, from nothing. (The item a search asked to
      // be shown is filtered for, when its folder is too big to come whole.)
      if (shownPath !== null && shownPath !== p) filter = "";
      shownPath = p;
      void load(p);
    });
  });
  // The filter and "only deleted" ask the snapshot again, once typing pauses.
  let asked: string | null = null;
  $effect(() => {
    const key = `${filter.trim()}\n${onlyMissing}`;
    if (asked === null) {
      asked = key; // the first listing is the folder effect's
      return;
    }
    if (key === asked) return;
    const t = setTimeout(() => {
      asked = key;
      void load(untrack(() => path), "auto");
    }, 200);
    return () => clearTimeout(t);
  });

  // "Deleted" is checked against the computer each time a folder is listed, so after a
  // restore (or on demand, or coming back to the window) the folder is listed again and
  // what was just brought back isn't still marked.
  let refreshing = $state(false);
  let pending: "again" | "auto" | null = null;
  async function refresh(how: "again" | "auto") {
    if (!listing) return;
    if (refreshing) {
      // One more pass once this one's done: it may have started before the files changed.
      pending = pending === "again" ? "again" : how;
      return;
    }
    refreshing = true;
    try {
      // A refresh someone asked for spins long enough to be seen.
      await Promise.all([load(path, how), how === "again" ? new Promise((r) => setTimeout(r, 400)) : null]);
    } finally {
      refreshing = false;
    }
    const next = pending;
    pending = null;
    if (next) void refresh(next);
  }
  let seenDisk = untrack(() => app.diskRevision);
  $effect(() => {
    const r = app.diskRevision;
    if (r !== seenDisk) {
      seenDisk = r;
      untrack(() => void refresh("auto"));
    }
  });

  const crumbs = $derived.by(() => {
    const parts = path.split("/").filter(Boolean);
    return [{ name: "/", path: "/" }, ...parts.map((name, i) => ({ name, path: "/" + parts.slice(0, i + 1).join("/") }))];
  });

  /** What's shown: already filtered and in order (folders first, then by name). */
  const entries = $derived(listing?.entries ?? []);
  /** How many more there are than a listing brings. */
  const more = $derived(listing ? listing.matching - listing.entries.length : 0);

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

  /** How many items of this folder aren't on disk any more. */
  const missing = $derived(listing?.missing ?? 0);
  $effect(() => {
    if (onlyMissing && listing && missing === 0) onlyMissing = false;
  });
  /** Bring back what was deleted from this folder (all of it, not only what's shown). */
  async function restoreMissing() {
    try {
      const l = await api.listDir(profile, snapshot.id, path, null, true);
      for (const e of l.entries) onDisk.set(e.path, e.on_disk ?? null);
      if (l.matching > l.entries.length) app.toast("info", `Restoring the first ${count(l.entries.length)} of ${count(l.matching)} deleted items; do it again for the rest.`);
      if (l.entries.length) restore(l.entries.map((e) => e.path));
    } catch (e) {
      app.toast("bad", (e as Error).message);
    }
  }
  /** Selected items in other folders (or past what this folder shows). */
  const here = $derived(new Set(entries.map((e) => e.path)));
  const elsewhere = $derived([...selected].filter((p) => !here.has(p)).length);

  const allSelected = $derived(entries.length > 0 && entries.every((e) => selected.has(e.path)));
  /** Select or unselect everything shown in this folder (the rest stays as it is). */
  function toggleAll() {
    const s = new Set(selected);
    for (const e of entries) {
      if (allSelected) s.delete(e.path);
      else s.add(e.path);
    }
    selected = s;
  }

  /** Right-click on a file or folder (all of it is also on screen: double-click, the
   * checkbox, the Restore button). */
  function entryMenu(e: DirEntry): MenuEntry[] {
    const sel = selected.has(e.path);
    return [
      ...(e.kind === "dir" ? [{ label: "Open", icon: "folder" as const, onselect: () => open(e) }] : []),
      ...(e.kind === "file" ? [{ label: "Preview", icon: "eye" as const, shortcut: "Space", onselect: () => (previewing = e) }] : []),
      ...(e.kind === "file" ? [{ label: "Open a copy", icon: "file" as const, hint: "A read-only copy in its usual program", onselect: () => open(e) }] : []),
      ...(e.kind === "file" ? [{ label: "Versions of this file…", icon: "history" as const, onselect: () => (versionsOf = e.path) }] : []),
      { label: `Restore “${e.name}”…`, icon: "restore", onselect: () => restore([e.path]) },
      ...(selected.size > 1 && sel ? [{ label: `Restore the ${selected.size} selected items…`, icon: "restore" as const, onselect: () => restore([...selected]) }] : []),
      selected.size > 1 && sel
        ? { label: `Export the ${selected.size} selected items…`, icon: "download", onselect: () => (exporting = [...selected]) }
        : { label: `Export “${e.name}”…`, icon: "download", hint: "As a .tar.zst or .zip archive", onselect: () => (exporting = [e.path]) },
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

<div class="@container card flex min-h-0 flex-1 flex-col overflow-hidden">
  <div class="flex items-center gap-3 px-4 py-3">
    <button class="btn btn-ghost size-8 justify-center p-0" disabled={path === "/"} aria-label="Up one folder" onclick={() => (path = crumbs.at(-2)?.path ?? "/")}>
      <Icon name="chevronLeft" size={16} />
    </button>
    <nav class="flex min-w-16 flex-1 items-center gap-0.5 overflow-x-auto text-sm" aria-label="Folder path">
      {#each crumbs as c, i (c.path)}
        {#if i > 1}<Icon name="chevronRight" size={13} class="shrink-0 text-fg-muted" />{/if}
        <button class="shrink-0 rounded-md px-1.5 py-1 hover:bg-surface-3 {i === crumbs.length - 1 ? 'font-medium text-fg' : 'text-fg-2'}" onclick={() => (path = c.path)}>
          {c.name}
        </button>
      {/each}
    </nav>
    {#if missing}
      <button
        class="flex h-8 shrink-0 items-center gap-1.5 rounded-lg px-2.5 text-[13px] transition-colors {onlyMissing ? 'bg-warning/15 font-medium text-warning' : 'bg-surface-3 text-fg-2 hover:bg-track hover:text-fg'}"
        aria-pressed={onlyMissing}
        title="Show only what was deleted or moved since this snapshot"
        onclick={() => (onlyMissing = !onlyMissing)}
      >
        <Icon name="filter" size={13} /> Only deleted ({count(missing)})
      </button>
    {/if}
    <button
      id="browser-refresh"
      class="btn btn-ghost size-8 shrink-0 justify-center p-0"
      aria-label="Check again what's on your computer"
      title="Check again what's on your computer (F5)"
      disabled={refreshing || !listing}
      onclick={() => void refresh("again")}
    >
      <Icon name="refresh" size={15} class={refreshing ? "motion-safe:animate-spin" : ""} />
    </button>
    <label class="relative w-32 shrink-0 @min-[560px]:w-56">
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
      <table class="w-full table-fixed text-sm">
        <thead class="sticky top-0 bg-surface-2 text-left text-xs text-fg-muted">
          <tr>
            <th class="w-10 py-2 pl-4"><input type="checkbox" class="accent-[var(--c-primary)]" checked={allSelected} onchange={toggleAll} aria-label="Select all" /></th>
            <th class="py-2 font-medium">Name</th>
            <th class="w-24 py-2 pr-4 text-right font-medium">Size</th>
            <th class="hidden w-36 py-2 pr-2 font-medium @min-[520px]:table-cell">Modified</th>
            <th class="w-32 py-2 pr-4"><span class="sr-only">Preview and restore</span></th>
          </tr>
        </thead>
        <tbody>
          {#each entries as e (e.path)}
            <tr data-path={e.path} class="group transition-colors {selected.has(e.path) ? 'bg-selected text-on-selected' : 'hover:bg-surface-3'}" ondblclick={() => open(e)} use:contextMenu={{ items: () => entryMenu(e), label: e.name }}>
              <td class="py-1.5 pl-4"><input type="checkbox" class="accent-[var(--c-primary)]" checked={selected.has(e.path)} onchange={() => toggle(e)} aria-label="Select {e.name}" /></td>
              <td class="min-w-0 py-1.5">
                <button
                  class="flex max-w-full items-center gap-2.5 text-left {e.kind === 'dir' ? 'hover:text-primary' : 'cursor-default'}"
                  onclick={() => e.kind === "dir" && open(e)}
                  onkeydown={(k) => {
                    // Space previews a file (as in a file manager); Enter opens it.
                    if (e.kind !== "file") return;
                    if (k.key === " ") (k.preventDefault(), (previewing = e));
                    else if (k.key === "Enter") (k.preventDefault(), open(e));
                  }}
                  title={e.kind === "file" ? "Space to preview · double-click to open a read-only copy" : ""}
                >
                  <Icon name={icon(e)} size={16} class={e.kind === "dir" ? "text-primary" : "text-fg-muted"} />
                  <span class="truncate">{e.name}</span>
                  {#if opening === e.path}<span class="text-xs text-fg-muted">opening…</span>{/if}
                  {#if e.link}<span class="truncate text-xs text-fg-muted">→ {e.link}</span>{/if}
                  {#if e.on_disk === false}<span class="tag shrink-0 bg-warning/15 text-warning" title="Not on your computer any more: deleted or moved since this snapshot">Deleted</span>{/if}
                </button>
              </td>
              <td class="truncate py-1.5 pr-4 text-right font-mono text-[12.5px] whitespace-nowrap text-fg-2">{e.kind === "dir" ? (e.children != null ? `${e.children} ${e.children === 1 ? "item" : "items"}` : "") : e.kind === "file" ? bytes(e.size) : ""}</td>
              <td class="hidden truncate py-1.5 pr-2 font-mono @min-[520px]:table-cell text-[12.5px] whitespace-nowrap text-fg-2" title={dateTime(e.mtime * 1000)}>{shortDateTime(e.mtime * 1000)}</td>
              <td class="py-1 pr-3 text-right whitespace-nowrap">
                {#if e.kind === "file"}
                  <button
                    class="btn-icon size-7 align-middle opacity-0 group-focus-within:opacity-100 group-hover:opacity-100"
                    onclick={() => (previewing = e)}
                    aria-label="Preview {e.name}"
                    title="Preview (Space)"
                  ><Icon name="eye" size={14} /></button>
                {/if}
                <button
                  class="btn btn-ghost h-7 px-2 text-[12.5px] {e.on_disk === false ? '' : 'opacity-0 group-focus-within:opacity-100 group-hover:opacity-100'}"
                  onclick={() => restore([e.path])}
                  aria-label="Restore {e.name}"
                  title="Restore just this {e.kind === 'dir' ? 'folder' : 'item'}"
                ><Icon name="restore" size={13} /> Restore</button>
              </td>
            </tr>
          {:else}
            <tr>
              <td colspan="5" class="p-8 text-center text-sm text-fg-muted">
                {#if filter}Nothing matches the filter.
                {:else}
                  This folder is empty in this snapshot.
                  {#if sizeLimit}<span class="mt-1 block">Files larger than {sizeLabel(sizeLimit)} aren't in this backup (its size limit).</span>{/if}
                {/if}
              </td>
            </tr>
          {/each}
          {#if more > 0}
            <tr>
              <td colspan="5" class="px-4 py-3 text-center text-[13px] text-fg-muted">
                The first {count(entries.length)} of {count(listing.matching)} items. Type part of a name in the filter to find the others.
              </td>
            </tr>
          {/if}
        </tbody>
      </table>
    {/if}
  </div>

  <div class="flex items-center justify-end gap-2 px-4 py-3">
    {#if selected.size}
      {#if elsewhere}<span class="mr-auto text-[13px] text-fg-muted">{elsewhere} selected in other folders</span>{/if}
      <button class="btn btn-ghost" onclick={() => (selected = new Set())}>Clear selection</button>
      {#if selected.size === 1}
        {@const only = entries.find((e) => selected.has(e.path))}
        {#if only?.kind === "file"}<button class="btn btn-ghost" onclick={() => (versionsOf = only.path)}><Icon name="history" size={14} /> Versions</button>{/if}
      {/if}
      <button class="btn" onclick={() => restore([...selected])}>Restore {selected.size} {selected.size === 1 ? "item" : "items"}</button>
    {:else}
      <span class="mr-auto hidden text-[13px] text-fg-muted @min-[520px]:inline">Tick files and folders to restore just those.</span>
      <button class="btn" onclick={() => restore([path])} title="Restore everything in this folder; files that are already identical are skipped">Restore this whole folder</button>
      {#if missing}
        <button class="btn btn-primary" onclick={() => void restoreMissing()} title="Bring back only what was deleted from this folder">
          <Icon name="restore" size={14} /> Restore {count(missing)} deleted {missing === 1 ? "item" : "items"}
        </button>
      {/if}
    {/if}
  </div>
</div>

<svelte:window onfocus={() => void refresh("auto")} />

{#if restoring}
  <RestoreDialog {profile} snapshot={restoring.snapshot} paths={restoring.paths} missing={restoring.missing} onclose={() => (restoring = null)} />
{/if}
{#if exporting}
  <ExportSnapshotDialog {profile} {snapshot} paths={exporting} onclose={() => (exporting = null)} />
{/if}
{#if previewing}
  {@const e = previewing}
  <PreviewDialog
    {profile}
    {snapshot}
    entry={e}
    {onLocked}
    onclose={() => (previewing = null)}
    onrestore={() => ((previewing = null), restore([e.path]))}
    onopen={() => ((previewing = null), void openCopy(e))}
    onversions={() => ((previewing = null), (versionsOf = e.path))}
  />
{/if}
{#if versionsOf}
  {@const p = versionsOf}
  <VersionsDialog profile={profile} path={p} onclose={() => (versionsOf = null)} onrestore={(s) => ((restoring = { snapshot: s, paths: [p], missing: onDisk.get(p) === false }), (versionsOf = null))} />
{/if}
