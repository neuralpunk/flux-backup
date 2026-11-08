<script lang="ts">
  // Asking before something that can't be undone, or before the system asks for a
  // password. Says what will happen and what won't; the confirm button names the action.
  import type { Snippet } from "svelte";
  import Icon, { type IconName } from "./Icon.svelte";
  import Modal from "./Modal.svelte";

  let {
    title,
    icon = "alert",
    confirm,
    danger = false,
    busy = false,
    onconfirm,
    onclose,
    children,
  }: {
    title: string;
    icon?: IconName;
    confirm: string;
    danger?: boolean;
    busy?: boolean;
    onconfirm: () => void;
    onclose: () => void;
    children: Snippet;
  } = $props();
</script>

<Modal {title} {onclose} width="max-w-md">
  <div class="flex flex-col gap-5">
    <div class="flex gap-4">
      <div class="grid size-10 shrink-0 place-items-center rounded-lg {danger ? 'bg-danger/12 text-danger' : 'bg-surface-3 text-fg-2'}"><Icon name={icon} /></div>
      <div class="min-w-0 space-y-2 text-sm text-fg-2">{@render children()}</div>
    </div>
    <div class="flex justify-end gap-2">
      <button class="btn btn-ghost" onclick={onclose}>Cancel</button>
      <!-- svelte-ignore a11y_autofocus -->
      <button class="btn {danger ? 'btn-danger-filled' : 'btn-primary'}" disabled={busy} onclick={onconfirm} autofocus={!danger}>{confirm}</button>
    </div>
  </div>
</Modal>
