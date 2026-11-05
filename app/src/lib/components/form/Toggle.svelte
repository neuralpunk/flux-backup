<script lang="ts">
  let {
    checked = $bindable(),
    label,
    hint,
    disabled = false,
  }: { checked: boolean; label: string; hint?: string; disabled?: boolean } = $props();
</script>

<label class="flex cursor-pointer items-start gap-3 {disabled ? 'cursor-not-allowed opacity-50' : ''}">
  <button
    type="button"
    role="switch"
    aria-checked={checked}
    aria-label={label}
    {disabled}
    class="relative mt-0.5 h-5 w-9 shrink-0 rounded-full transition-colors {checked ? 'bg-primary' : 'bg-track'}"
    onclick={() => (checked = !checked)}
  >
    <span class="absolute top-0.5 left-0.5 size-4 rounded-full transition-transform {checked ? 'bg-on-primary' : 'bg-fg-muted'} {checked ? 'translate-x-4' : ''}"></span>
  </button>
  <span class="min-w-0">
    <span class="block text-sm font-medium">{label}</span>
    {#if hint}<span class="block text-xs text-fg-muted">{hint}</span>{/if}
  </span>
</label>
