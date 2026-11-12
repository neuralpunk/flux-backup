<script lang="ts">
  // A backup's settings at a glance, on its Overview: schedule, password, what's kept,
  // tests… one compact cell each, so nobody has to dig through Settings to see whether a
  // schedule is set. A cell opens that setting.
  import { api } from "../api";
  import { uses24h } from "../clock.svelte";
  import { GLANCE_GROUPS, glance, type GlanceItem, type SentryKnows } from "../glance";
  import { app } from "../state.svelte";
  import { isSystem, type Profile } from "../types";
  import Icon from "./Icon.svelte";
  import ProfileEditor from "./ProfileEditor.svelte";
  import SentryDialog from "./SentryDialog.svelte";

  let {
    id,
    sentry = null,
    column = false,
  }: {
    id: string;
    /** What Flux Sentry knows of this backup. */
    sentry?: SentryKnows | null;
    /** One cell a line, for the Compact layout's side column. */
    column?: boolean;
  } = $props();

  let profile = $state<Profile | null>(null);
  $effect(() => {
    const want = id;
    void app.revision; // saved settings come back here
    api.getProfile(want).then(
      (e) => want === id && (profile = e.profile),
      () => want === id && (profile = null),
    );
  });
  const items = $derived(profile ? glance(profile, uses24h(), sentry, isSystem(id)) : []);
  // In groups, without the cells that say nothing about this backup (no hooks, say).
  const groups = $derived(
    GLANCE_GROUPS.map((g) => {
      const its = g.keys.map((k) => items.find((i) => i.key === k && !i.idle)).filter((i): i is GlanceItem => !!i);
      const cols = column ? 1 : its.length === 4 ? 2 : Math.min(its.length, 3);
      return { title: g.title, items: its, cols, left: its.length % Math.max(cols, 1) };
    }).filter((g) => g.items.length),
  );

  // Open unless closed here before (remembered on this computer). The Compact layout's
  // side column remembers its own: it starts open there even if it was closed in Full.
  // svelte-ignore state_referenced_locally (a layout change mounts the glance anew)
  const closedKey = column ? "flux.glanceClosedCompact" : "flux.glanceClosed";
  let open = $state(true);
  try {
    open = localStorage.getItem(closedKey) !== "1";
  } catch {
    /* storage unavailable */
  }
  function toggled(e: Event) {
    open = (e.currentTarget as HTMLDetailsElement).open;
    try {
      localStorage.setItem(closedKey, open ? "0" : "1");
    } catch {
      /* storage unavailable */
    }
  }
  /** Compact's side column: the full wording while it fits on the line, the short one
   * (`tight`) when it doesn't. Measured again when the column's width changes. */
  function fitShort(node: HTMLElement, _value: string) {
    const cell = node.closest<HTMLElement>(".cell") ?? node;
    const measure = () => {
      cell.classList.remove("tight");
      const full = cell.querySelector<HTMLElement>(".full");
      if (full) cell.classList.toggle("tight", full.scrollWidth > full.clientWidth + 1);
    };
    const ro = new ResizeObserver(measure);
    ro.observe(cell);
    void document.fonts?.ready.then(measure);
    return { update: () => queueMicrotask(measure), destroy: () => ro.disconnect() };
  }

  const warnings = $derived(items.filter((i) => i.tone === "warn").length);

  /** The section of the settings being changed, in its dialog over this page. */
  let editing = $state<string | null>(null);
  /** Flux Sentry has a dialog of its own (its settings sit among the retention rules). */
  let sentryOpen = $state(false);
  function show(i: GlanceItem) {
    // Its dialog, here: the Overview stays where it is. (Settings has its own link.)
    if (i.key === "unusual") sentryOpen = true;
    else if (i.page) editing = i.page;
  }
</script>

<style>
  /* Cells share hairlines: the grid's gap shows the separator colour behind them. */
  .glance {
    display: grid;
    gap: 1px;
    background: var(--c-sep);
    border-radius: var(--radius-card);
    box-shadow: 0 0 0 1px var(--c-group-edge), var(--group-shadow);
    overflow: hidden;
  }
  .cell {
    position: relative;
    display: flex;
    min-width: 0;
    flex-direction: column;
    gap: 0.125rem;
    padding: 0.5rem 1.75rem 0.5625rem 0.875rem;
    background: var(--c-surface-2);
    text-align: left;
    transition: background-color 120ms;
  }
  /* Compact's side column: label and value on one line, the value on the right. */
  .cell.inline {
    flex-direction: row;
    align-items: baseline;
    justify-content: space-between;
    gap: 0.75rem;
    padding: 0.5rem 0.875rem;
  }
  /* There, a group's title is the first line of its box (so the column's first box
     starts level with the page's first card). */
  .gtitle {
    grid-column: 1 / -1;
    padding: 0.375rem 0.875rem;
    background: color-mix(in oklab, var(--c-fg) 3%, var(--c-surface-2));
    font-size: 12px;
    font-weight: 500;
    color: var(--c-fg-muted);
  }
  /* There, the hover lift says it opens; no chevron over the value. */
  /* The short wording, shown only when the full one doesn't fit; the full one stays for
     screen readers. */
  .brief {
    display: none;
  }
  .cell:global(.tight) .brief {
    display: -webkit-box;
  }
  .cell:global(.tight) .full {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }
  .cell.inline :global(.go) {
    display: none;
  }
  .cell.clickable:hover {
    background: color-mix(in oklab, var(--c-fg) 5%, var(--c-surface-2));
  }
  .cell :global(.go) {
    position: absolute;
    top: 50%;
    right: 0.5rem;
    translate: 0 -50%;
    opacity: 0;
    transition: opacity 120ms;
  }
  .cell:hover :global(.go),
  .cell:focus-visible :global(.go) {
    opacity: 1;
  }
</style>

{#if items.length}
  <details class="group" {open} ontoggle={toggled}>
    <summary class="flex cursor-pointer list-none items-center gap-2 rounded-md px-0.5 {column ? 'h-7' : 'py-1'} text-[13px] font-semibold text-fg-2 select-none hover:text-fg [&::-webkit-details-marker]:hidden">
      <Icon name="chevronRight" size={14} class="transition-transform group-open:rotate-90" />
      Settings at a glance
      {#if warnings}<span class="font-normal text-warning">{warnings === 1 ? "1 needs a look" : `${warnings} need a look`}</span>{/if}
      <span class="ml-auto font-normal">
        <button class="text-[12.5px] text-fg-muted hover:text-fg hover:underline" onclick={(e) => (e.preventDefault(), app.open(id, "settings"))}>All settings</button>
      </span>
    </summary>
    <div class="mt-2 flex flex-col {column ? 'gap-2' : 'gap-3'}">
      {#each groups as g (g.title)}
        <section aria-label={g.title}>
          {#if !column}<h3 class="mb-1.5 px-0.5 text-[12px] font-medium text-fg-muted">{g.title}</h3>{/if}
          <!-- Columns to fit the group (four make two rows of two); a short last row's final
               cell stretches, so there's never an empty one. -->
          <div class="glance" style="grid-template-columns: repeat({g.cols}, minmax(0, 1fr))">
            {#if column}<h3 class="gtitle">{g.title}</h3>{/if}
            {#each g.items as i, n (i.key)}
              <!-- A setting with its own dialog opens it here; the rest (where, what) is
                   information, changed on the Settings page. -->
              <svelte:element
                this={i.page ? "button" : "div"}
                class="cell {i.page ? 'clickable' : ''} {column ? 'inline' : ''}"
                style={g.left && n === g.items.length - 1 ? `grid-column: span ${g.cols - g.left + 1}` : ""}
                title={column && i.short ? [i.value, i.detail].filter(Boolean).join(". ") : (i.detail ?? "")}
                role={i.page ? undefined : "group"}
                onclick={i.page ? () => show(i) : undefined}
              >
                <span class="{column ? 'shrink-0 text-[12.5px]' : 'truncate text-[12px]'} text-fg-muted">{i.label}</span>
                <span class="flex min-w-0 items-start gap-1.5 {column ? 'text-right text-[13px]' : 'text-[13.5px]'} leading-snug {i.tone === 'warn' ? 'font-medium text-warning' : i.tone === 'off' ? 'text-fg-muted' : 'text-fg'}">
                  {#if i.tone === "warn"}<Icon name="alert" size={13} class="mt-[3px] shrink-0" />{/if}
                  {#if column && i.short}
                    <span class="flex min-w-0 justify-end" use:fitShort={i.value}>
                      <span class="full min-w-0 truncate">{i.value}</span>
                      <span class="brief line-clamp-2" aria-hidden="true">{i.short}</span>
                    </span>
                  {:else}
                    <span class="line-clamp-2">{i.value}</span>
                  {/if}
                </span>
                {#if i.page}<Icon name="chevronRight" size={14} class="go text-fg-muted" />{/if}
              </svelte:element>
            {/each}
          </div>
        </section>
      {/each}
    </div>
  </details>
{/if}

{#if sentryOpen}
  <SentryDialog {id} knows={sentry} onclose={() => (sentryOpen = false)} />
{/if}

{#if editing}
  <ProfileEditor
    {id}
    dialogOnly={`section:${editing}`}
    onsaved={() => {
      editing = null;
      app.revision++;
    }}
    oncancel={() => (editing = null)}
  />
{/if}
