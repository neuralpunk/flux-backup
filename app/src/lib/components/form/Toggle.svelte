<script lang="ts">
  import Icon, { type IconName } from "../Icon.svelte";
  // A switch with its label: label on the left, switch on the right, the whole line
  // clickable — the convention of GNOME, macOS and phone settings. `row`: drawn as a
  // settings row (inside a .rows group).
  let {
    checked = $bindable(),
    label,
    hint,
    disabled = false,
    reason = "",
    row = false,
    onchange,
    icon,
  }: {
    checked: boolean;
    label: string;
    hint?: string;
    disabled?: boolean;
    reason?: string;
    row?: boolean;
    /** Instead of flipping `checked` itself: for switches whose effect is more than a flag. */
    onchange?: (on: boolean) => void;
    /** Rows beside navigation rows carry an icon too, so their text lines up. */
    icon?: IconName;
  } = $props();
  const id = `sw-${Math.random().toString(36).slice(2, 9)}`;
</script>

<label for={id} class="{row ? 'row row-link' : 'flex items-center gap-4'} {disabled ? 'cursor-not-allowed opacity-60' : 'cursor-pointer'}">
  {#if icon}
    <span class="grid w-5 shrink-0 place-items-center text-fg-2"><Icon name={icon} size={17} /></span>
  {/if}
  <span class="min-w-0 flex-1">
    <span class="block text-[14px] font-medium">{label}</span>
    {#if disabled && reason}
      <span class="mt-0.5 block text-[12.5px] text-fg-muted">{reason}</span>
    {:else if hint}
      <span class="mt-0.5 block text-[12.5px] text-fg-muted">{hint}</span>
    {/if}
  </span>
  <button
    {id}
    type="button"
    role="switch"
    aria-label={label}
    aria-checked={checked}
    {disabled}
    class="switch"
    onclick={() => (onchange ? onchange(!checked) : (checked = !checked))}
  >
    <span class="switch-knob"></span>
  </button>
</label>
