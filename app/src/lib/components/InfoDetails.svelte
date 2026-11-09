<script lang="ts">
  import type { Snippet } from "svelte";
  import Icon, { type IconName } from "./Icon.svelte";

  /** An explanation people can open when they want it: an information block, closed
   * until clicked. */
  let { summary, icon = "info", children }: { summary: string; icon?: IconName; children: Snippet } = $props();
</script>

<details class="info-block group rounded-card px-4 py-3 text-[13px]">
  <summary class="flex cursor-pointer list-none items-center gap-2.5 font-medium text-fg select-none [&::-webkit-details-marker]:hidden">
    <span class="grid size-6 shrink-0 place-items-center rounded-full bg-primary/15 text-primary"><Icon name={icon} size={14} /></span>
    <span class="min-w-0 flex-1">{summary}</span>
    <Icon name="chevronDown" size={15} class="shrink-0 text-fg-muted transition-transform group-open:rotate-180" />
  </summary>
  <div class="mt-3 border-t border-primary/15 pt-3 pl-[34px] text-fg-2">
    {@render children()}
  </div>
</details>

<style>
  .info-block {
    background: color-mix(in oklab, var(--c-primary) 7%, var(--c-surface-2));
    box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--c-primary) 22%, transparent);
  }
</style>
