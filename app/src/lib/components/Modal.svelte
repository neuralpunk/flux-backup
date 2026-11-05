<script lang="ts">
  import type { Snippet } from "svelte";
  import Icon from "./Icon.svelte";

  let { title, onclose, children, width = "max-w-lg" }: { title: string; onclose: () => void; children: Snippet; width?: string } = $props();
  let dialog: HTMLDialogElement;

  $effect(() => {
    dialog.showModal();
    return () => dialog.close();
  });
</script>

<!-- Native <dialog>: focus trapping, Escape to close and inert background for free. -->
<dialog
  bind:this={dialog}
  class="m-auto w-full {width} rounded-panel bg-surface-1 p-0 text-fg backdrop:bg-black/60"
  onclose={onclose}
  aria-labelledby="modal-title"
>
  <div class="flex items-center justify-between px-7 pt-6 pb-2">
    <h2 id="modal-title" class="text-lg font-medium">{title}</h2>
    <button class="btn btn-ghost pill size-8 justify-center p-0" onclick={onclose} aria-label="Close"><Icon name="x" size={16} /></button>
  </div>
  <div class="px-7 pt-3 pb-7">{@render children()}</div>
</dialog>
