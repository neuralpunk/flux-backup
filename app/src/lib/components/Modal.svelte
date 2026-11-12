<script lang="ts">
  import type { Snippet } from "svelte";
  import Icon from "./Icon.svelte";

  /** `locked`: it can't be dismissed just now (something it started is still running, and
   * its outcome is shown here). `required`: a step that has to be done before going on;
   * no Close at all. */
  let {
    title,
    onclose,
    children,
    width = "max-w-lg",
    locked = false,
    required = false,
  }: { title: string; onclose: () => void; children: Snippet; width?: string; locked?: boolean; required?: boolean } = $props();
  let dialog: HTMLDialogElement;

  $effect(() => {
    dialog.showModal();
    return () => dialog.close();
  });
</script>

<!-- Native <dialog>: focus trapping, Escape to close and inert background for free. -->
<dialog
  bind:this={dialog}
  class="relative m-auto max-h-[calc(100dvh-2rem)] w-full {width} overflow-y-auto rounded-panel bg-surface-1 p-0 text-fg shadow-[0_24px_64px_-16px_rgb(0_0_0/0.55),0_0_0_1px_var(--c-group-edge)] backdrop:bg-black/55"
  onclose={onclose}
  oncancel={(e) => (locked || required) && e.preventDefault()}
  aria-labelledby="modal-title"
>
  <div class="px-6 pt-5 pb-1 {required ? '' : 'pr-14'}">
    <h2 id="modal-title" class="text-[17px] font-semibold">{title}</h2>
  </div>
  <div class="px-6 pt-3 pb-6">{@render children()}</div>
  <!-- Last in the DOM, so opening the dialog focuses its first real control, not Close. -->
  {#if !required}
    <button class="btn-icon absolute top-4 right-4 size-8" onclick={onclose} disabled={locked} aria-label="Close" title={locked ? "Still working…" : "Close (Esc)"}><Icon name="x" size={16} /></button>
  {/if}
</dialog>
