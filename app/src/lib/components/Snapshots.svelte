<script lang="ts">
  import { ramp } from "../ramp";
  import { api } from "../api";
  import { app } from "../state.svelte";
  import { ago, bytes, count, dateTime, dayLabel, monthLabel, time } from "../format";
  import { ApiError, type SnapshotMeta } from "../types";
  import CompareDialog from "./CompareDialog.svelte";
  import ExportSnapshotDialog from "./ExportSnapshotDialog.svelte";
  import SnapshotCalendar, { dayKey } from "./SnapshotCalendar.svelte";
  import FileBrowser from "./FileBrowser.svelte";
  import FindResults from "./FindResults.svelte";
  import Icon from "./Icon.svelte";
  import Callout from "./Callout.svelte";
  import MenuButton from "./MenuButton.svelte";
  import SizeLimitNotice from "./SizeLimitNotice.svelte";
  import { contextMenu, type MenuEntry } from "../menu.svelte";

  let { id, onLocked }: { id: string; onLocked: (retry: () => void) => void } = $props();
  let snaps = $state<SnapshotMeta[] | null>(null);
  let error = $state<string | null>(null);
  let selectedId = $state<string | null>(null);

  function load() {
    const current = id;
    api.snapshots(current).then(
      (s) => {
        if (current !== id) return;
        snaps = s;
        error = null;
        if (app.showSnapshot && s.some((x) => x.id === app.showSnapshot)) {
          selectedId = app.showSnapshot;
          app.showSnapshot = null;
        } else if (!selectedId || !s.some((x) => x.id === selectedId)) selectedId = s.at(-1)?.id ?? null;
      },
      (e) => {
        if (e instanceof ApiError && e.code === "password_required") onLocked(load);
        else if (e instanceof ApiError && e.code === "empty") snaps = [];
        else error = (e as Error).message;
      },
    );
  }

  // Another backup: start over. Something changed (a backup finished somewhere, a Vault
  // locked itself): read the list again in place, keeping what is selected and open. A
  // restore running in its dialog here must not be torn down by an unrelated backup.
  let shownFor: string | null = null;
  $effect(() => {
    void app.revision;
    if (shownFor !== id) {
      shownFor = id;
      snaps = null;
      selectedId = null;
    }
    load();
  });

  const selected = $derived(snaps?.find((s) => s.id === selectedId) ?? null);

  // Searching every snapshot: the results replace the browser until closed.
  let queryDraft = $state("");
  let query = $state<string | null>(null);
  // An item to show (its folder open, it selected) in the browser.
  let show = $state<string | null>(null);
  $effect(() => {
    void id;
    query = null;
    queryDraft = "";
    show = null;
  });
  function showIn(snapshot: string, path: string) {
    selectedId = snapshot;
    show = path;
    query = null;
  }

  let renaming = $state(false);
  let nameDraft = $state("");
  // Note and tags, edited in the snapshot's header.
  let labelling = $state(false);
  let noteDraft = $state("");
  let tagsDraft = $state("");
  /** Only snapshots with this tag in the list (`null`: all). */
  let tagFilter = $state<string | null>(null);
  let confirmingDelete = $state(false);
  let comparing = $state<{ old: string; newer: string | null; title: string } | null>(null);
  let exporting = $state<SnapshotMeta | null>(null);
  // The calendar: open or not, the day picked, and the days a backup failed.
  let calendarOpen = $state(false);
  let dayFilter = $state<string | null>(null);
  let failedDays = $state<Set<string>>(new Set());
  $effect(() => {
    if (!calendarOpen) return;
    const want = id;
    api.runs(want, 1000).then(
      (runs) => want === id && (failedDays = new Set(runs.filter((r) => r.status === "failed").map((r) => dayKey(r.started)))),
      () => {},
    );
  });
  function pickDay(d: string | null) {
    dayFilter = d;
    const first = d ? newestFirst.find((s) => dayKey(s.started) === d) : null;
    if (first) (selectedId = first.id), (confirmingDelete = false), (renaming = false), (labelling = false);
  }

  $effect(() => {
    void selectedId;
    renaming = false;
    labelling = false;
    confirmingDelete = false;
  });
  $effect(() => {
    void id;
    tagFilter = null;
  });

  function startLabelling(s: SnapshotMeta) {
    noteDraft = s.note ?? "";
    tagsDraft = (s.tags ?? []).join(", ");
    labelling = true;
  }

  async function saveLabels() {
    if (!selected) return;
    try {
      const tags = tagsDraft.split(",").map((t) => t.trim()).filter(Boolean);
      replace(await api.updateSnapshot(id, selected.id, { note: noteDraft.trim() || null, tags }));
      labelling = false;
    } catch (e) {
      app.toast("bad", (e as Error).message);
    }
  }

  /** Every tag in use, most used first. */
  const allTags = $derived.by(() => {
    const n = new Map<string, number>();
    for (const s of snaps ?? []) for (const t of s.tags ?? []) n.set(t, (n.get(t) ?? 0) + 1);
    return [...n.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0])).map(([t]) => t);
  });
  const hasTag = (s: SnapshotMeta, t: string) => (s.tags ?? []).some((x) => x.toLowerCase() === t.toLowerCase());

  function replace(updated: SnapshotMeta) {
    if (snaps) snaps = snaps.map((s) => (s.id === updated.id ? updated : s));
  }

  async function saveName() {
    if (!selected) return;
    try {
      replace(await api.updateSnapshot(id, selected.id, { name: nameDraft.trim() || null }));
      renaming = false;
    } catch (e) {
      app.toast("bad", (e as Error).message);
    }
  }

  async function togglePin(s: SnapshotMeta) {
    try {
      replace(await api.updateSnapshot(id, s.id, { pinned: !s.pinned }));
      app.toast("ok", s.pinned ? "Unpinned: retention may remove it later." : "Pinned: it will never be removed automatically.");
    } catch (e) {
      app.toast("bad", (e as Error).message);
    }
  }

  async function remove(s: SnapshotMeta) {
    try {
      const freed = await api.deleteSnapshot(id, s.id);
      confirmingDelete = false;
      app.toast("ok", freed == null ? "Snapshot deleted." : `Snapshot deleted; ${bytes(freed)} freed.`);
      load();
    } catch (e) {
      app.toast("bad", (e as Error).message);
    }
  }
  const newestFirst = $derived(snaps ? [...snaps].reverse().filter((s) => (!tagFilter || hasTag(s, tagFilter)) && (!dayFilter || dayKey(s.started) === dayFilter)) : []);
  /** Vault and Borg count what they stored; Mirror and rsync what they copied. */
  const stored = $derived.by(() => {
    const e = app.profiles.find((p) => p.id === id)?.engine;
    return e === "vault" || e === "borg";
  });
  const month = (t: string) => monthLabel(t);
  /** The history strip's bar for a snapshot: 4 to 12px tall on the same fixed ramp, standing
   * on the text's baseline. */
  function mark(s: SnapshotMeta): number {
    return Math.round(4 + ramp(stored ? s.stats.stored_bytes : s.stats.copied_bytes) * 8);
  }


  // Arrow keys move through the list (newest at the top).
  function onKey(e: KeyboardEvent) {
    if (!snaps?.length || !selectedId) return;
    const i = newestFirst.findIndex((s) => s.id === selectedId);
    if (e.key === "ArrowDown" && i < newestFirst.length - 1) selectedId = newestFirst[i + 1]!.id;
    else if (e.key === "ArrowUp" && i > 0) selectedId = newestFirst[i - 1]!.id;
    else return;
    e.preventDefault();
  }

  // Comparing, and the menu of a snapshot (its ⋯ button and right-click).
  const prevOf = (s: SnapshotMeta) => snaps?.[snaps.findIndex((x) => x.id === s.id) - 1];
  function compareWithPrev(s: SnapshotMeta) {
    const p = prevOf(s);
    if (p) comparing = { old: p.id, newer: s.id, title: `Changes from ${dateTime(p.started)} to ${dateTime(s.started)}` };
  }
  function compareWithNow(s: SnapshotMeta) {
    comparing = { old: s.id, newer: null, title: `Changes since ${dateTime(s.started)}` };
  }
  function snapshotMenu(s: SnapshotMeta): MenuEntry[] {
    const select = () => {
      if (selectedId !== s.id) (selectedId = s.id), (confirmingDelete = false), (renaming = false), (labelling = false);
    };
    return [
      ...(selectedId !== s.id ? [{ label: "Browse files", icon: "folder" as const, onselect: select }] : []),
      ...(prevOf(s) ? [{ label: "What changed since the one before", icon: "history" as const, onselect: () => compareWithPrev(s) }] : []),
      { label: "Compare with files now", icon: "refresh", onselect: () => compareWithNow(s) },
      { label: "Export…", icon: "download", hint: "Save it as a .tar.zst or .zip archive", onselect: () => (exporting = s) },
      "separator",
      { label: s.name ? "Rename…" : "Name…", icon: "tag", onselect: () => (select(), (nameDraft = s.name ?? ""), (renaming = true)) },
      { label: s.note || s.tags?.length ? "Edit note and tags…" : "Note and tags…", icon: "edit", hint: "For finding it again; doesn't change what's kept", onselect: () => (select(), startLabelling(s)) },
      { label: s.pinned ? "Unpin" : "Pin (keep forever)", icon: "pin", onselect: () => void togglePin(s) },
      "separator",
      { label: "Delete…", icon: "trash", danger: true, disabled: s.pinned, hint: s.pinned ? "Pinned snapshots can't be deleted; unpin it first" : "", onselect: () => (select(), (confirmingDelete = true)) },
    ];
  }
</script>

{#if error}
  <Callout tone="bad">{error}</Callout>
{:else if !snaps}
  <div class="card h-64 motion-safe:animate-pulse"></div>
{:else if snaps.length === 0}
  <div class="card grid place-items-center gap-2 p-16 text-center">
    <div class="title">No snapshots yet</div>
    <p class="max-w-sm text-sm text-fg-muted">Your first backup creates one. Each later backup adds another, storing only what changed.</p>
  </div>
{:else}
  <SizeLimitNotice {id} class="mb-2" />
  <div class="flex min-h-0 flex-1 gap-3">
    <!-- Snapshot list, newest first -->
    <div class="card flex w-56 shrink-0 flex-col overflow-hidden min-[1100px]:w-64">
      <form
        class="relative px-2 pt-2"
        role="search"
        onsubmit={(e) => {
          e.preventDefault();
          if (queryDraft.trim()) query = queryDraft.trim();
        }}
      >
        <label for="find-all" class="sr-only">Find a file in all snapshots</label>
        <Icon name="search" size={15} class="pointer-events-none absolute top-1/2 left-5 mt-1 -translate-y-1/2 text-fg-muted" />
        <input id="find-all" class="input h-9 pl-8" placeholder="Find in all snapshots" title="Search every snapshot by name (Enter)" bind:value={queryDraft} />
      </form>
      <div class="flex items-center gap-1.5 px-2.5 pt-2">
        <button
          class="flex items-center gap-1.5 rounded-md px-2 py-0.5 text-[12px] transition-colors {calendarOpen ? 'bg-surface-3 font-medium text-fg' : 'text-fg-2 hover:bg-surface-3 hover:text-fg'}"
          aria-expanded={calendarOpen}
          onclick={() => {
            calendarOpen = !calendarOpen;
            if (!calendarOpen) dayFilter = null;
          }}
        >
          <Icon name="calendar" size={13} /> Calendar
        </button>
        {#if dayFilter}
          <button class="flex items-center gap-1 rounded-md bg-selected px-2 py-0.5 text-[12px] font-medium text-on-selected" title="Show every day" onclick={() => (dayFilter = null)}>
            {new Date(`${dayFilter}T12:00:00`).toLocaleDateString(undefined, { day: "numeric", month: "short" })} <Icon name="x" size={11} />
          </button>
        {/if}
      </div>
      {#if calendarOpen && snaps}
        <SnapshotCalendar snaps={snaps} failed={failedDays} day={dayFilter} anchor={snaps.find((s) => s.id === selectedId)?.started ?? snaps.at(-1)?.started ?? null} onpick={pickDay} />
      {/if}
      {#if allTags.length}
        <div class="flex flex-wrap gap-1.5 px-2.5 pt-2" role="group" aria-label="Show only snapshots tagged">
          {#each allTags as t (t)}
            {@const on = tagFilter?.toLowerCase() === t.toLowerCase()}
            <button
              class="chip-toggle px-2 py-0.5 text-[12px]"
              aria-pressed={on}
              title={on ? "Show all snapshots" : `Show only snapshots tagged ${t}`}
              onclick={() => (tagFilter = on ? null : t)}
            >{#if on}<Icon name="check" size={12} />{/if}#{t}</button>
          {/each}
        </div>
      {/if}
      <div class="overflow-y-auto px-1.5 pt-1 pb-2" role="listbox" tabindex="0" aria-label="Snapshots (arrow keys move)" onkeydown={onKey}>
        {#if tagFilter && !newestFirst.length}
          <p class="px-2.5 py-3 text-[12.5px] text-fg-muted">No snapshot is tagged #{tagFilter} any more.</p>
        {/if}
        {#each newestFirst as s, i (s.id)}
          {#if i === 0 || month(s.started) !== month(newestFirst[i - 1]!.started)}
            <div class="px-2.5 pt-3 pb-1 text-[12px] font-semibold text-fg-muted first:pt-1" role="presentation">{month(s.started)}</div>
          {/if}
          <button
            role="option"
            aria-selected={s.id === selectedId}
            tabindex="-1"
            title="{dateTime(s.started)} · {bytes(stored ? s.stats.stored_bytes : s.stats.copied_bytes)} {stored ? 'stored' : 'copied'}{s.tags?.length ? `\n${s.tags.map((t) => `#${t}`).join(' ')}` : ''}{s.note ? `\n${s.note}` : ''}"
            class="flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-left text-[13.5px] transition-colors
              {s.id === selectedId ? 'bg-selected text-on-selected' : 'text-fg-2 hover:bg-surface-3 hover:text-fg'}"
            onclick={() => ((selectedId = s.id), (query = null), (show = null))}
            use:contextMenu={{ items: () => snapshotMenu(s), label: `Snapshot ${dateTime(s.started)}` }}
          >
            <span class="flex h-3 w-3.5 shrink-0 items-end justify-center" aria-hidden="true">
              <span class="block w-[5px] rounded-[2px] {s.status !== 'complete' ? 'bg-warning' : 'bg-success'}" style:height="{mark(s)}px"></span>
            </span>
            <span class="min-w-0 flex-1 truncate {s.id === selectedId ? 'font-semibold text-fg' : ''}">{s.name ?? dayLabel(s.started)}</span>
            {#if s.note || s.tags?.length}<Icon name="edit" size={12} class="shrink-0 text-fg-muted" label="Has a note or tags" />{/if}
            {#if s.pinned}<Icon name="pin" size={13} label="Pinned" />{/if}
            {#if s.status !== "complete"}<Icon name="alert" size={13} class="text-warning" label="Finished with problems" />{/if}
            <span class="shrink-0 font-mono text-[12px] text-fg-muted">{s.name ? dayLabel(s.started) : time(s.started)}</span>
          </button>
        {/each}
      </div>
    </div>

    <div class="flex min-w-0 flex-1 flex-col gap-3">
      {#if query}
        <FindResults profile={id} {query} snaps={snaps} onshow={showIn} onclose={() => (query = null)} {onLocked} />
      {:else if selected}
        {@const sel = selected}
        {@const prev = prevOf(sel)}
        <div class="card flex flex-wrap items-center gap-x-4 gap-y-2 px-5 py-3.5">
          <div class="min-w-[15rem] flex-1">
            <div class="flex items-center gap-2">
              <span class="title">{dateTime(sel.started)}</span>
              {#if sel.name}<span class="tag">“{sel.name}”</span>{/if}
              {#if sel.pinned}<span class="tag"><Icon name="pin" size={11} /> Pinned</span>{/if}
              {#each sel.tags ?? [] as t (t)}<span class="tag">#{t}</span>{/each}
            </div>
            {#if renaming}
              <form class="mt-2 flex gap-2" onsubmit={(e) => { e.preventDefault(); void saveName(); }}>
                <!-- svelte-ignore a11y_autofocus -->
                <input class="input h-8 w-56" bind:value={nameDraft} placeholder="e.g. before-upgrade" autofocus />
                <button class="btn h-8">Save</button>
                <button type="button" class="btn btn-ghost h-8" onclick={() => (renaming = false)}>Cancel</button>
              </form>
            {:else if labelling}
              <form class="mt-2 flex flex-col gap-2" onsubmit={(e) => { e.preventDefault(); void saveLabels(); }}>
                <label class="flex flex-col gap-1 text-[12.5px] text-fg-2">
                  Note
                  <!-- svelte-ignore a11y_autofocus -->
                  <textarea class="input min-h-16 py-1.5 text-[13.5px]" bind:value={noteDraft} placeholder="e.g. Before upgrading to Fedora 46" maxlength="2000" autofocus></textarea>
                </label>
                <label class="flex flex-col gap-1 text-[12.5px] text-fg-2">
                  Tags, separated by commas
                  <input class="input h-8 text-[13.5px]" bind:value={tagsDraft} placeholder="e.g. taxes, release" />
                </label>
                <div class="flex items-center gap-2">
                  <button class="btn h-8">Save</button>
                  <button type="button" class="btn btn-ghost h-8" onclick={() => (labelling = false)}>Cancel</button>
                  <span class="text-[12px] text-fg-muted">Notes and tags never change what's kept; pin or name a snapshot for that.</span>
                </div>
              </form>
            {:else}
              <div class="mt-0.5 font-mono text-[12.5px] text-fg-muted">
                {ago(sel.started)} · {count(sel.stats.files)} files · {bytes(sel.stats.total_bytes)}
                {#if sel.errors_total}· <span class="text-warning" title={sel.errors.map((e) => `${e.path}: ${e.message}`).join("\n")}>{sel.errors_total} unreadable</span>{/if}
              </div>
              {#if sel.note}
                <p class="mt-1.5 max-w-[70ch] text-[13px] whitespace-pre-line text-fg-2">{sel.note}</p>
              {/if}
              {#each sel.sources_missing ?? [] as m (m.path)}
                <p class="mt-1 text-[12.5px] text-warning">
                  <span class="font-mono">{m.path}</span> wasn't connected: {m.kept_from ? "its files are as they were in the snapshot before." : "this snapshot doesn't have it."}
                </p>
              {/each}
            {/if}
          </div>
          {#if confirmingDelete}
            <span class="text-sm text-fg-2">Delete for good?</span>
            <button class="btn btn-danger" onclick={() => remove(sel)}>Delete</button>
            <button class="btn btn-ghost" onclick={() => (confirmingDelete = false)}>Keep</button>
          {:else}
            <button class="btn" onclick={() => (prev ? compareWithPrev(sel) : compareWithNow(sel))} title={prev ? "Compare with the snapshot before" : "Compare with the files on disk now"}>What changed</button>
            <MenuButton items={() => snapshotMenu(sel)} label="More actions for this snapshot" />
          {/if}
        </div>
        {#key `${selected.id}\n${show}`}
          <FileBrowser profile={id} snapshot={selected} {show} {onLocked} />
        {/key}
      {/if}
    </div>
  </div>
{/if}

{#if exporting}
  <ExportSnapshotDialog profile={id} snapshot={exporting} paths={[]} onclose={() => (exporting = null)} />
{/if}
{#if comparing}
  <CompareDialog profile={id} old={comparing.old} newer={comparing.newer} title={comparing.title} oldSnap={snaps?.find((s) => s.id === comparing!.old) ?? null} onclose={() => (comparing = null)} />
{/if}
