<script lang="ts">
  import type { Snippet } from "svelte";
  import type { Issue } from "../../types";

  let {
    label,
    hint,
    issues = [],
    children,
    id,
  }: { label: string; hint?: string; issues?: Issue[]; children: Snippet; id?: string } = $props();
  const error = $derived(issues.find((i) => i.severity === "error"));
  const warning = $derived(issues.find((i) => i.severity === "warning"));
</script>

<div class="space-y-1.5">
  {#if label}<label class="label block" for={id}>{label}</label>{/if}
  {@render children()}
  {#if error}
    <p class="text-[12.5px] text-danger" role="alert">{error.message}</p>
  {:else if warning}
    <p class="text-[12.5px] text-warning">{warning.message}</p>
  {:else if hint}
    <p class="text-[12.5px] text-fg-muted">{hint}</p>
  {/if}
</div>
