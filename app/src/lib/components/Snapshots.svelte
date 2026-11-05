<script lang="ts">
  import { api } from "../api";
  import { app } from "../state.svelte";
  import { ago, bytes, count, dateTime, dayLabel, time } from "../format";
  import { ApiError, type SnapshotMeta } from "../types";
  import CompareDialog from "./CompareDialog.svelte";
  import FileBrowser from "./FileBrowser.svelte";
  import Icon from "./Icon.svelte";

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
        if (!selectedId || !s.some((x) => x.id === selectedId)) selectedId = s.at(-1)?.id ?? null;
      },
      (e) => {
        if (e instanceof ApiError && e.code === "password_required") onLocked(load);
        else if (e instanceof ApiError && e.code === "empty") snaps = [];
        else error = (e as Error).message;
      },
    );
  }

  $effect(() => {
    void app.revision;
    void id;
    snaps = null;
    selectedId = null;
    load();
  });

  const selected = $derived(snaps?.find((s) => s.id === selectedId) ?? null);

  let renaming = $state(false);
  let nameDraft = $state("");
  let confirmingDelete = $state(false);
  let comparing = $state<{ old: string; newer: string | null; title: string } | null>(null);

  $effect(() => {
    void selectedId;
    renaming = false;
    confirmingDelete = false;
  });

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
  const newestFirst = $derived(snaps ? [...snaps].reverse() : []);


  // Arrow keys move through the list (newest at the top).
  function onKey(e: KeyboardEvent) {
    if (!snaps?.length || !selectedId) return;
    const i = newestFirst.findIndex((s) => s.id === selectedId);
    if (e.key === "ArrowDown" && i < newestFirst.length - 1) selectedId = newestFirst[i + 1]!.id;
    else if (e.key === "ArrowUp" && i > 0) selectedId = newestFirst[i - 1]!.id;
    else return;
    e.preventDefault();
  }

  // The "⋯" menu with the less frequent actions.
  let menuOpen = $state(false);
  $effect(() => {
    void selectedId;
    menuOpen = false;
  });
  function act(f: () => void) {
    return () => {
      menuOpen = false;
      f();
    };
  }
</script>

<svelte:window onclick={() => (menuOpen = false)} onkeydown={(e) => e.key === "Escape" && (menuOpen = false)} />

{#if error}
  <div class="card flex items-center gap-3 bg-danger/10 p-5 text-danger"><Icon name="alert" /> {error}</div>
{:else if !snaps}
  <div class="card h-64 motion-safe:animate-pulse"></div>
{:else if snaps.length === 0}
  <div class="card grid place-items-center gap-2 p-16 text-center">
    <div class="title">No snapshots yet</div>
    <p class="max-w-sm text-sm text-fg-muted">Your first backup creates one. Each later backup adds another, storing only what changed.</p>
  </div>
{:else}
  <div class="flex min-h-0 flex-1 gap-3">
    <!-- Snapshot list, newest first -->
    <div class="card flex w-60 shrink-0 flex-col overflow-hidden">
      <div class="overflow-y-auto p-2" role="listbox" tabindex="0" aria-label="Snapshots (arrow keys move)" onkeydown={onKey}>
        {#each newestFirst as s (s.id)}
          <button
            role="option"
            aria-selected={s.id === selectedId}
            tabindex="-1"
            title={dateTime(s.started)}
            class="pill flex w-full items-center gap-2 px-4 py-2.5 text-left text-sm transition-colors
              {s.id === selectedId ? 'bg-primary-container text-on-primary-container' : 'text-fg-2 hover:bg-surface-3 hover:text-fg'}"
            onclick={() => (selectedId = s.id)}
          >
            <span class="min-w-0 flex-1 truncate {s.id === selectedId ? 'font-medium' : ''}">{s.name ?? dayLabel(s.started)}</span>
            {#if s.pinned}<Icon name="pin" size={13} label="Pinned" />{/if}
            {#if s.status !== "complete"}<Icon name="alert" size={13} class="text-warning" label="Finished with problems" />{/if}
            <span class="num shrink-0 text-[12.5px] opacity-75">{s.name ? dayLabel(s.started) : time(s.started)}</span>
          </button>
        {/each}
      </div>
    </div>

    <div class="flex min-w-0 flex-1 flex-col gap-3">
      {#if selected}
        {@const sel = selected}
        {@const prev = snaps[snaps.findIndex((x) => x.id === sel.id) - 1]}
        {@const compareWithPrev = () => prev && (comparing = { old: prev.id, newer: sel.id, title: `Changes from ${dateTime(prev.started)} to ${dateTime(sel.started)}` })}
        {@const compareWithNow = () => (comparing = { old: sel.id, newer: null, title: `Changes since ${dateTime(sel.started)}` })}
        <div class="card flex items-center gap-4 px-6 py-4">
          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-2">
              <span class="title">{dateTime(sel.started)}</span>
              {#if sel.name}<span class="tag">“{sel.name}”</span>{/if}
              {#if sel.pinned}<span class="tag"><Icon name="pin" size={11} /> Pinned</span>{/if}
            </div>
            {#if renaming}
              <form class="mt-2 flex gap-2" onsubmit={(e) => { e.preventDefault(); void saveName(); }}>
                <!-- svelte-ignore a11y_autofocus -->
                <input class="input h-8 w-56" bind:value={nameDraft} placeholder="e.g. before-upgrade" autofocus />
                <button class="btn h-8">Save</button>
                <button type="button" class="btn btn-ghost h-8" onclick={() => (renaming = false)}>Cancel</button>
              </form>
            {:else}
              <div class="num mt-0.5 text-[13px] text-fg-muted">
                {ago(sel.started)} · {count(sel.stats.files)} files · {bytes(sel.stats.total_bytes)}
                {#if sel.errors_total}· <span class="text-warning" title={sel.errors.map((e) => `${e.path}: ${e.message}`).join("\n")}>{sel.errors_total} unreadable</span>{/if}
              </div>
            {/if}
          </div>
          {#if confirmingDelete}
            <span class="text-sm text-fg-2">Delete for good?</span>
            <button class="btn btn-danger" onclick={() => remove(sel)}>Delete</button>
            <button class="btn btn-ghost" onclick={() => (confirmingDelete = false)}>Keep</button>
          {:else}
            <button class="btn" onclick={() => (prev ? compareWithPrev() : compareWithNow())} title={prev ? "Compare with the snapshot before" : "Compare with the files on disk now"}>What changed</button>
            <div class="relative">
              <button class="btn w-9 justify-center px-0" aria-label="More actions" aria-haspopup="menu" aria-expanded={menuOpen} onclick={(e) => { e.stopPropagation(); menuOpen = !menuOpen; }}>
                <Icon name="more" size={18} class="[stroke-width:3]" />
              </button>
              {#if menuOpen}
                <div role="menu" class="absolute top-11 right-0 z-20 w-56 rounded-2xl bg-surface-3 p-1.5 text-sm shadow-[0_16px_40px_-12px_rgb(0_0_0/0.55)]">
                  <button role="menuitem" class="flex w-full rounded-xl px-3 py-2 text-left hover:bg-track" onclick={act(() => { nameDraft = sel.name ?? ""; renaming = true; })}>{sel.name ? "Rename" : "Name…"}</button>
                  <button role="menuitem" class="flex w-full rounded-xl px-3 py-2 text-left hover:bg-track" onclick={act(() => togglePin(sel))}>{sel.pinned ? "Unpin" : "Pin (keep forever)"}</button>
                  {#if prev}<button role="menuitem" class="flex w-full rounded-xl px-3 py-2 text-left hover:bg-track" onclick={act(compareWithPrev)}>Compare with previous</button>{/if}
                  <button role="menuitem" class="flex w-full rounded-xl px-3 py-2 text-left hover:bg-track" onclick={act(compareWithNow)}>Compare with files now</button>
                  <button role="menuitem" class="flex w-full rounded-xl px-3 py-2 text-left text-danger hover:bg-track disabled:opacity-40" disabled={sel.pinned} title={sel.pinned ? "Unpin it first" : ""} onclick={act(() => (confirmingDelete = true))}>Delete…</button>
                </div>
              {/if}
            </div>
          {/if}
        </div>
        {#key selected.id}
          <FileBrowser profile={id} snapshot={selected} {onLocked} />
        {/key}
      {/if}
    </div>
  </div>
{/if}

{#if comparing}
  <CompareDialog profile={id} old={comparing.old} newer={comparing.newer} title={comparing.title} onclose={() => (comparing = null)} />
{/if}
