<script lang="ts">
  // A message that needs reading: one shape for every warning, error and note in a page or
  // a dialog. An icon, an optional title, the text, and its actions under it.
  //   warn  something to fix or to know before going on (amber)
  //   bad   something went wrong (red)
  //   info  worth knowing (no colour)
  //   ok    it went well (a check's result)
  import type { Snippet } from "svelte";
  import Icon, { type IconName } from "./Icon.svelte";

  let {
    tone = "warn",
    title,
    icon,
    compact = false,
    class: cls = "",
    children,
    actions,
  }: {
    tone?: "warn" | "bad" | "info" | "ok";
    title?: string;
    icon?: IconName;
    /** One line, actions beside the text (a banner in a page). */
    compact?: boolean;
    class?: string;
    children?: Snippet;
    actions?: Snippet;
  } = $props();

  const frame = $derived({ warn: "bg-warning/10 ring-warning/30", bad: "bg-danger/8 ring-danger/30", info: "bg-surface-2 ring-[var(--c-sep)]", ok: "bg-success/10 ring-success/30" }[tone]);
  const mark = $derived({ warn: "text-warning", bad: "text-danger", info: "text-fg-2", ok: "text-success" }[tone]);
</script>

<div class="flex items-start gap-3 rounded-card px-4 py-3 text-[13.5px] ring-1 ring-inset {frame} {compact ? 'sm:items-center' : ''} {cls}" role={tone === "info" ? "note" : tone === "ok" ? "status" : "alert"}>
  <Icon name={icon ?? (tone === "info" ? "info" : tone === "ok" ? "check" : "alert")} size={16} class="mt-0.5 shrink-0 {mark} {compact ? 'sm:mt-0' : ''}" />
  <div class="min-w-0 flex-1 {compact ? 'sm:flex sm:items-center sm:gap-3' : ''}">
    <div class="min-w-0 flex-1">
      {#if title}<p class="font-medium text-fg">{title}</p>{/if}
      {#if children}<div class="text-fg-2 {title ? 'mt-0.5 text-[13px]' : ''}">{@render children()}</div>{/if}
    </div>
    {#if actions}<div class="flex shrink-0 flex-wrap gap-2 {compact ? 'mt-2.5 sm:mt-0' : 'mt-2.5'}">{@render actions()}</div>{/if}
  </div>
</div>
