<script lang="ts">
  // Every backup's settings side by side: one row each, the columns that tell backups
  // apart (when, how stored, the password, where, what's kept, tests, notifications).
  // Sortable by any column; a cell opens that setting, the name opens the backup.
  import { api } from "../api";
  import { uses24h } from "../clock.svelte";
  import { glance, type GlanceItem, type GlanceKey } from "../glance";
  import { nextRun } from "../protection";
  import { app, pauseUntil } from "../state.svelte";
  import { isSystem, type Profile, type ProfileSummary } from "../types";
  import Icon from "./Icon.svelte";
  import ProfileEditor from "./ProfileEditor.svelte";
  import { openMenuBelow } from "../menu.svelte";

  let { profiles }: { profiles: ProfileSummary[] } = $props();

  /** Each backup's full settings (`null`: couldn't be read, e.g. system backups locked). */
  let full = $state<Record<string, Profile | null>>({});
  $effect(() => {
    void app.revision;
    const ids = profiles.map((p) => p.id);
    void Promise.allSettled(ids.map((id) => api.getProfile(id))).then((rs) => {
      const next: Record<string, Profile | null> = {};
      rs.forEach((r, i) => (next[ids[i]!] = r.status === "fulfilled" ? r.value.profile : null));
      full = next;
    });
  });

  type Col = { key: GlanceKey | "next"; label: string };
  /** Every column there is; the visible ones are chosen with "Columns" (remembered). */
  const ALL: Col[] = [
    { key: "schedule", label: "Schedule" },
    { key: "next", label: "Next" },
    { key: "sources", label: "Backs up" },
    { key: "excluded", label: "Excluded" },
    { key: "where", label: "Where" },
    { key: "places", label: "Other copies" },
    { key: "stored", label: "Stored as" },
    { key: "password", label: "Password" },
    { key: "keeps", label: "Keeps" },
    { key: "tests", label: "Restore tests" },
    { key: "upkeep", label: "Upkeep" },
    { key: "unusual", label: "Unusual changes" },
    { key: "notify", label: "Notifications" },
    { key: "network", label: "Network" },
    { key: "hooks", label: "Hooks" },
  ];
  const DEFAULT: Col["key"][] = ["schedule", "next", "where", "stored", "password", "keeps", "tests", "notify"];
  let shownKeys = $state<Col["key"][]>(DEFAULT);
  try {
    const saved = JSON.parse(localStorage.getItem("flux.tableColumns") ?? "null") as Col["key"][] | null;
    if (Array.isArray(saved) && saved.length) shownKeys = saved.filter((k) => ALL.some((c) => c.key === k));
  } catch {
    /* storage unavailable, or something else stored there */
  }
  const cols = $derived(ALL.filter((c) => shownKeys.includes(c.key)));
  function toggleColumn(k: Col["key"]) {
    const next = shownKeys.includes(k) ? shownKeys.filter((x) => x !== k) : [...shownKeys, k];
    if (!next.length) return; // at least one
    shownKeys = next;
    try {
      localStorage.setItem("flux.tableColumns", JSON.stringify(next));
    } catch {
      /* storage unavailable */
    }
  }
  /** Choose the columns, from a menu below `anchor` (All backups' "Columns" button). */
  export function chooseColumns(anchor: HTMLElement) {
    openMenuBelow(
      anchor,
      [
        ...ALL.map((c) => ({ label: c.label, icon: shownKeys.includes(c.key) ? ("check" as const) : undefined, onselect: () => toggleColumn(c.key) })),
        "separator" as const,
        {
          label: "Reset columns",
          onselect: () => {
            shownKeys = DEFAULT;
            try {
              localStorage.removeItem("flux.tableColumns");
            } catch {
              /* storage unavailable */
            }
          },
        },
      ],
      "Columns",
    );
  }

  type Row = { p: ProfileSummary; cells: Partial<Record<Col["key"], GlanceItem>> | null };
  const rows = $derived<Row[]>(
    profiles.map((p) => {
      const prof = full[p.id];
      if (!prof) return { p, cells: prof === null ? null : {} };
      const items = Object.fromEntries(glance(prof, uses24h(), null, isSystem(p.id)).map((i) => [i.key, i])) as Partial<Record<Col["key"], GlanceItem>>;
      const st = app.statuses[p.id];
      const next = p.running ? "Running now" : nextRun(p, st, app.pause ? pauseUntil(app.pause) : null);
      const shortNext = next.includes("connected") ? "On plug-in" : next === "when you start it" ? "By hand" : undefined;
      items.next = { key: "schedule", label: "Next", value: next, short: shortNext, page: "when", tone: !p.running && !st?.next_run ? "off" : undefined };
      // The other places, with the main one: "BackupDisk +2".
      // With no Other copies column, the main place says how many more there are.
      const others = prof.destinations?.length ?? 0;
      if (items.where && others && !shownKeys.includes("places")) items.where = { ...items.where, value: `${items.where.value} +${others}`, detail: `${items.where.detail ?? ""}; also ${items.places?.value ?? ""}. ${items.places?.detail ?? ""}` };
      return { p, cells: items };
    }),
  );

  let sortBy = $state<Col["key"] | "name" | null>(null);
  let descending = $state(false);
  function sort(k: Col["key"] | "name") {
    if (sortBy === k) {
      if (descending) (sortBy = null), (descending = false);
      else descending = true;
    } else (sortBy = k), (descending = false);
  }
  const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });
  const sorted = $derived.by(() => {
    if (!sortBy) return rows;
    const k = sortBy;
    const val = (r: Row) => (k === "name" ? r.p.name : (r.cells?.[k]?.value ?? ""));
    const out = [...rows].sort((a, b) => collator.compare(val(a), val(b)));
    return descending ? out.reverse() : out;
  });
  const ariaSort = (k: Col["key"] | "name") => (sortBy === k ? (descending ? "descending" : "ascending") : "none");

  let scroller = $state<HTMLDivElement>();
  let more = $state(false);
  function measure() {
    if (scroller) more = scroller.scrollLeft + scroller.clientWidth < scroller.scrollWidth - 2;
  }
  $effect(() => {
    if (!scroller) return;
    void sorted;
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(scroller);
    return () => ro.disconnect();
  });

  /** A setting being changed: its section of the settings, in a dialog over this page. */
  let editing = $state<{ id: string; page: string } | null>(null);
  function show(id: string, i: GlanceItem | undefined) {
    if (i?.page) editing = { id, page: i.page };
  }
</script>

<style>
  .table {
    border-collapse: separate;
    border-spacing: 0;
    width: 100%;
    font-size: 12.5px;
  }
  th {
    position: sticky;
    top: 0;
    z-index: 1;
    /* Its own band, a clear step from the rows below, with a firmer line under it. */
    background: color-mix(in oklab, var(--c-fg) 9%, var(--c-surface-2));
    box-shadow: inset 0 -1px 0 color-mix(in oklab, var(--c-fg) 18%, transparent);
    font-weight: 600;
    color: var(--c-fg-2);
    text-align: left;
    white-space: nowrap;
  }
  td {
    box-shadow: inset 0 1px 0 var(--c-sep);
    background: var(--c-surface-2);
    vertical-align: top;
  }
  tbody tr:first-child td {
    box-shadow: none;
  }
  /* The backup's name stays in view while the columns scroll under it. */
  .sticky-col {
    position: sticky;
    left: 0;
    z-index: 2;
  }
  th.sticky-col {
    z-index: 3;
  }
  .sticky-col::after {
    content: "";
    position: absolute;
    top: 0;
    right: 0;
    bottom: 0;
    width: 1px;
    background: var(--c-sep);
  }
  tbody tr:hover td {
    background: color-mix(in oklab, var(--c-fg) 3%, var(--c-surface-2));
  }
  .cell {
    display: block;
    width: 100%;
    padding: 0.4375rem 0.625rem;
    text-align: left;
    border-radius: 6px;
  }
  .cell.clickable:hover {
    background: color-mix(in oklab, var(--c-fg) 6%, transparent);
  }
</style>

<div class="relative">
<div
  class="overflow-x-auto rounded-card shadow-[0_0_0_1px_var(--c-group-edge),var(--group-shadow)]"
  bind:this={scroller}
  onscroll={measure}
>
  <table class="table">
    <thead>
      <tr>
        <th class="sticky-col p-0" aria-sort={ariaSort("name")}>
          <button class="flex w-full items-center gap-1 px-3 py-2 hover:text-fg" onclick={() => sort("name")}>
            Backup {#if sortBy === "name"}<Icon name="chevronDown" size={12} class={descending ? "rotate-180" : ""} />{/if}
          </button>
        </th>
        {#each cols as c (c.key)}
          <th class="p-0" aria-sort={ariaSort(c.key)}>
            <button class="flex w-full items-center gap-1 px-3 py-2 hover:text-fg" onclick={() => sort(c.key)}>
              {c.label} {#if sortBy === c.key}<Icon name="chevronDown" size={12} class={descending ? "rotate-180" : ""} />{/if}
            </button>
          </th>
        {/each}
      </tr>
    </thead>
    <tbody>
      {#each sorted as r (r.p.id)}
        <tr>
          <td class="sticky-col p-0.5">
            <button class="cell" onclick={() => app.open(r.p.id)} title="Open {r.p.name}">
              <span class="block max-w-52 truncate font-medium whitespace-nowrap text-fg">{r.p.name}</span>
              {#if r.p.scope === "system"}<span class="block text-[12px] text-fg-muted">System backup</span>{/if}
            </button>
          </td>
          {#if r.cells === null}
            <td class="p-0.5" colspan={cols.length}>
              <span class="cell text-fg-muted">{r.p.scope === "system" ? "Unlock system backups to see their settings." : "Its settings can't be read."}</span>
            </td>
          {:else}
            {#each cols as c (c.key)}
              {@const i = r.cells[c.key]}
              <td class="p-0.5">
                {#if i}
                  <!-- One line: the short form, the whole of it on hover. A setting with its
                       own dialog opens it here; where and what are information. -->
                  <svelte:element this={i.page ? "button" : "div"} class="cell {i.page ? 'clickable' : ''}" role={i.page ? undefined : "group"} title={[i.value, i.detail].filter(Boolean).join("\n")} onclick={i.page ? () => show(r.p.id, i) : undefined}>
                    <span class="flex items-center gap-1.5 whitespace-nowrap {i.tone === 'warn' ? 'font-medium text-warning' : i.tone === 'off' ? 'text-fg-muted' : 'text-fg'}">
                      {#if i.tone === "warn"}<Icon name="alert" size={13} class="shrink-0" />{/if}
                      <span class="max-w-60 truncate">{i.short ?? i.value}</span>
                    </span>
                  </svelte:element>
                {:else}
                  <span class="cell"><span class="block h-3.5 w-16 rounded bg-surface-3 motion-safe:animate-pulse"></span></span>
                {/if}
              </td>
            {/each}
          {/if}
        </tr>
      {/each}
    </tbody>
  </table>
</div>
  <!-- More columns to the right: a fade says so. -->
  {#if more}<div class="pointer-events-none absolute inset-y-0 right-0 w-10 rounded-r-card bg-linear-to-l from-[var(--c-surface-2)] to-transparent"></div>{/if}
</div>

{#if editing}
  <ProfileEditor
    id={editing.id}
    dialogOnly={`section:${editing.page}`}
    onsaved={() => {
      editing = null;
      app.revision++;
    }}
    oncancel={() => (editing = null)}
  />
{/if}
