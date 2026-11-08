<script lang="ts">
  // A list of paths or patterns as a grouped list: one row per item (remove on the right),
  // and a last row to add one (typed, or picked with Browse…). `after`: more rows for the
  // same group (e.g. the row that opens the exclusions).
  import type { Snippet } from "svelte";
  import Icon, { type IconName } from "../Icon.svelte";

  let {
    items = $bindable(),
    placeholder = "",
    mono = true,
    pick,
    addLabel = "Add",
    icon,
    label = "",
    after,
  }: {
    items: string[];
    placeholder?: string;
    mono?: boolean;
    pick?: () => Promise<string | null>;
    addLabel?: string;
    /** Shown before each item. */
    icon?: IconName;
    /** What the list is, for screen readers ("Folders to back up"). */
    label?: string;
    after?: Snippet;
  } = $props();
  let draft = $state("");

  function add(v: string) {
    const t = v.trim();
    if (t && !items.includes(t)) items = [...items, t];
    draft = "";
  }
  async function choose() {
    const v = await pick?.();
    if (v) add(v);
  }
</script>

<div class="rows">
  {#if items.length}
    <ul class="contents" aria-label={label || undefined}>
      {#each items as item, i (item)}
        <li class="row min-h-11 py-1.5 pr-2">
          {#if icon}<Icon name={icon} size={17} class="shrink-0 text-fg-2" />{/if}
          <span class="selectable min-w-0 flex-1 truncate {mono ? 'font-mono text-[13px]' : 'text-[14px]'}" title={item}>{item}</span>
          <button type="button" class="btn-icon size-8 hover:text-danger" aria-label="Remove {item}" title="Remove" onclick={() => (items = items.filter((_, j) => j !== i))}>
            <Icon name="x" size={15} />
          </button>
        </li>
      {/each}
    </ul>
  {/if}
  <div class="row min-h-12 gap-2 py-2">
    <input
      class="input h-8 flex-1 {mono ? 'font-mono text-[13px]' : ''}"
      {placeholder}
      aria-label={label ? `Add to ${label.toLowerCase()}` : "Add"}
      bind:value={draft}
      onkeydown={(e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          add(draft);
        }
      }}
    />
    <button type="button" class="btn h-8" disabled={!draft.trim()} onclick={() => add(draft)}><Icon name="plus" size={14} /> {addLabel}</button>
    {#if pick}
      <button type="button" class="btn h-8" onclick={choose}><Icon name="folder" size={14} /> Browse…</button>
    {/if}
  </div>
  {@render after?.()}
</div>
