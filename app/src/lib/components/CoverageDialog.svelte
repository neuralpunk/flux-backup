<script lang="ts">
  // The folders a backup takes, browsed with each item marked backed up or left out (and
  // why), and how much is in and out below each folder. Uses the rules in the editor.
  import { api } from "../api";
  import { bytes, count } from "../format";
  import type { Coverage, Profile } from "../types";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";

  let { profile, onclose }: { profile: Profile; onclose: () => void } = $props();

  const sources = $derived(profile.sources.paths);
  let dir = $state<string | null>(null);
  let data = $state<Coverage | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);

  $effect(() => {
    if (dir === null && sources.length === 1) dir = sources[0]!;
  });

  $effect(() => {
    const d = dir;
    if (d === null) return;
    const draft = $state.snapshot(profile) as Profile;
    loading = true;
    error = null;
    api.coverage(draft, d).then(
      (c) => d === dir && ((data = c), (loading = false)),
      (e) => d === dir && ((error = (e as Error).message), (data = null), (loading = false)),
    );
  });

  /** The source folder `dir` is in. */
  const source = $derived(sources.filter((s) => dir !== null && (dir === s || dir.startsWith(s.endsWith("/") ? s : `${s}/`))).sort((a, b) => b.length - a.length)[0] ?? null);
  const up = $derived.by(() => {
    if (dir === null || source === null) return null;
    if (dir === source) return sources.length > 1 ? "" : null;
    return dir.slice(0, dir.lastIndexOf("/")) || "/";
  });
  const crumbs = $derived.by(() => {
    if (dir === null || source === null) return [];
    const rest = dir.slice(source.length).split("/").filter(Boolean);
    const out = [{ name: source, path: source }];
    let at = source;
    for (const r of rest) {
      at = `${at.replace(/\/$/, "")}/${r}`;
      out.push({ name: r, path: at });
    }
    return out;
  });
  const totals = $derived.by(() => {
    if (!data) return null;
    let [fi, bi, fo, bo] = [0, 0, 0, 0];
    for (const i of data.items) (fi += i.files_in), (bi += i.bytes_in), (fo += i.files_out), (bo += i.bytes_out);
    return { fi, bi, fo, bo };
  });
</script>

<Modal title="What's backed up" {onclose} width="max-w-3xl">
  <p class="-mt-1 mb-4 text-[13.5px] text-fg-2">With the rules as they are in the editor now. Left-out items say why; folders show how much below them is in and out.</p>
  {#if dir === null}
    <h2 class="group-title">The folders this backup takes</h2>
    <div class="rows">
      {#each sources as s (s)}
        <button class="row row-link gap-3 text-left" onclick={() => (dir = s)}>
          <Icon name="folder" size={17} class="shrink-0 text-fg-2" />
          <span class="min-w-0 flex-1 truncate font-mono text-[13px]">{s}</span>
          <Icon name="chevronRight" size={16} class="text-fg-muted" />
        </button>
      {/each}
    </div>
  {:else}
    <div class="mb-2 flex items-center gap-2">
      <button class="btn-icon size-8" disabled={up === null} aria-label="Up" title="Up" onclick={() => up !== null && (dir = up === "" ? null : up)}>
        <Icon name="chevronRight" size={16} class="rotate-180" />
      </button>
      <nav class="flex min-w-0 flex-wrap items-center gap-1 font-mono text-[12.5px]" aria-label="Folder">
        {#each crumbs as c, i (c.path)}
          {#if i > 0}<span class="text-fg-muted">/</span>{/if}
          <button class="rounded px-1 hover:bg-surface-3 {i === crumbs.length - 1 ? 'font-semibold' : 'text-fg-2'}" onclick={() => (dir = c.path)}>{c.name}</button>
        {/each}
      </nav>
    </div>
    {#if error}
      <p class="text-[13px] text-danger">{error}</p>
    {:else if !data}
      <p class="py-6 text-center text-[13px] text-fg-muted">Counting…</p>
    {:else}
      {#if !data.backed_up}
        <p class="mb-2 rounded-lg bg-surface-3 px-3 py-2 text-[13px] text-fg-2">{data.why}</p>
      {/if}
      {#if totals}
        <p class="mb-2 px-0.5 text-[12.5px] text-fg-muted tabular">
          Here: {count(totals.fi)} files backed up ({bytes(totals.bi)}){#if totals.fo}, {count(totals.fo)} left out ({bytes(totals.bo)}){/if}{data.partial ? " · counted for 10 seconds, so far" : ""}{loading ? " · updating…" : ""}
        </p>
      {/if}
      <div class="rows max-h-[52vh] overflow-y-auto">
        {#each data.items as i (i.path)}
          {@const label = i.dir ? `${bytes(i.bytes_in)} in${i.bytes_out || i.files_out ? ` · ${bytes(i.bytes_out)} out` : ""}` : bytes(i.size)}
          <svelte:element
            this={i.dir ? "button" : "div"}
            class="row shrink-0 items-start gap-3 text-left {i.dir ? 'row-link' : ''}"
            onclick={i.dir ? () => (dir = i.path) : undefined}
            role={i.dir ? "button" : undefined}
          >
            <span class="mt-0.5 grid size-5 shrink-0 place-items-center rounded-full {i.backed_up ? 'bg-success/15 text-success' : 'bg-surface-3 text-fg-muted'}" title={i.backed_up ? "Backed up" : "Left out"}>
              <Icon name={i.backed_up ? "check" : "x"} size={12} />
            </span>
            <Icon name={i.dir ? "folder" : "file"} size={16} class="mt-0.5 shrink-0 text-fg-2" />
            <span class="min-w-0 flex-1">
              <span class="block truncate {i.backed_up ? '' : 'text-fg-2'}">{i.name}</span>
              {#if !i.backed_up}<span class="block text-[12.5px] text-fg-muted">{i.why}</span>{/if}
            </span>
            <span class="shrink-0 font-mono text-[12px] text-fg-muted tabular">{label}</span>
          </svelte:element>
        {:else}
          <p class="row text-[13px] text-fg-muted">Empty folder.</p>
        {/each}
      </div>
    {/if}
  {/if}
  <div class="mt-4 flex justify-end"><button class="btn btn-primary" onclick={onclose}>Done</button></div>
</Modal>
