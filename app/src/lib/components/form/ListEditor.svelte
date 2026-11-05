<script lang="ts">
  import Icon from "../Icon.svelte";

  let {
    items = $bindable(),
    placeholder = "",
    mono = true,
    pick,
    addLabel = "Add",
  }: { items: string[]; placeholder?: string; mono?: boolean; pick?: () => Promise<string | null>; addLabel?: string } = $props();
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

<div class="space-y-2">
  {#if items.length}
    <ul class="space-y-1">
      {#each items as item, i (item)}
        <li class="flex items-center gap-2 rounded-xl bg-surface-3 py-1.5 pr-1.5 pl-3.5">
          <span class="min-w-0 flex-1 truncate text-sm {mono ? 'font-mono text-[13px]' : ''}">{item}</span>
          <button type="button" class="rounded-md p-1 text-fg-muted hover:bg-track hover:text-danger" aria-label="Remove {item}" onclick={() => (items = items.filter((_, j) => j !== i))}>
            <Icon name="x" size={14} />
          </button>
        </li>
      {/each}
    </ul>
  {/if}
  <div class="flex gap-2">
    <input
      class="input {mono ? 'font-mono text-[13px]' : ''}"
      {placeholder}
      bind:value={draft}
      onkeydown={(e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          add(draft);
        }
      }}
    />
    <button type="button" class="btn" disabled={!draft.trim()} onclick={() => add(draft)}><Icon name="plus" size={14} /> {addLabel}</button>
    {#if pick}
      <button type="button" class="btn" onclick={choose}><Icon name="folder" size={14} /> Browse…</button>
    {/if}
  </div>
</div>
