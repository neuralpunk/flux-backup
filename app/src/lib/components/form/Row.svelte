<script lang="ts">
  // A settings row: what it is on the left, its current value and a way to change it on
  // the right. The shape tells what it does (docs/UX_PRINCIPLES.md):
  //   * `onclick` set: the whole row is a button; a chevron says "opens something"
  //   * `children`: the control on the right (a button, a select…)
  //   * neither: information only
  // A disabled row says why, in place of its subtitle.
  import type { Snippet } from "svelte";
  import Icon, { type IconName } from "../Icon.svelte";

  let {
    title,
    subtitle = "",
    icon,
    value = "",
    onclick,
    disabled = false,
    reason = "",
    children,
    tone = "normal",
  }: {
    title: string;
    subtitle?: string;
    icon?: IconName;
    value?: string;
    onclick?: () => void;
    disabled?: boolean;
    reason?: string;
    children?: Snippet;
    tone?: "normal" | "accent" | "danger";
  } = $props();
</script>

{#snippet body()}
  {#if icon}
    <span class="grid w-5 shrink-0 place-items-center {tone === 'accent' ? 'text-primary' : tone === 'danger' ? 'text-danger' : 'text-fg-2'}">
      <Icon name={icon} size={17} />
    </span>
  {/if}
  <span class="min-w-0 flex-1">
    <span class="block text-[14px] font-medium {tone === 'danger' ? 'text-danger' : ''}">{title}</span>
    {#if disabled && reason}
      <span class="mt-0.5 flex items-center gap-1 text-[12.5px] text-fg-muted"><Icon name="lock" size={11} class="shrink-0" /> {reason}</span>
    {:else if subtitle}
      <span class="mt-0.5 block text-[12.5px] text-fg-muted">{subtitle}</span>
    {/if}
  </span>
  {#if value}<span class="max-w-[45%] shrink-0 truncate text-right text-[13px] {tone === 'danger' ? 'text-danger' : 'text-fg-2'}">{value}</span>{/if}
{/snippet}

{#if onclick}
  <button type="button" class="row row-link" {disabled} {onclick}>
    {@render body()}
    <Icon name="chevronRight" size={16} class="shrink-0 text-fg-muted" />
  </button>
{:else}
  <div class="row {disabled ? 'opacity-60' : ''}">
    {@render body()}
    {#if children}<span class="flex shrink-0 items-center gap-2">{@render children()}</span>{/if}
  </div>
{/if}
