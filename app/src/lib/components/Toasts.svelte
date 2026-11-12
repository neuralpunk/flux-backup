<script lang="ts">
  import { app, UNDO_MS } from "../state.svelte";
  import Icon from "./Icon.svelte";

  const tone = { ok: "text-success", bad: "text-danger", info: "text-fg-2" } as const;
  // A warning sign only for what went wrong; news ("Timer updated") gets an "i".
  const icon = { ok: "check", bad: "alert", info: "info" } as const;

  // Ctrl+Z undoes a deletion that's waiting (not while typing, where it undoes typing).
  function onkeydown(e: KeyboardEvent) {
    if (!app.deleting || !(e.ctrlKey || e.metaKey) || e.key.toLowerCase() !== "z" || e.shiftKey) return;
    const t = e.target as HTMLElement | null;
    if (t?.closest("input, textarea, select, [contenteditable]")) return;
    e.preventDefault();
    app.undoDelete();
  }
</script>

<svelte:window {onkeydown} />

<style>
  /* The time left to undo, as a line running out along the bottom. */
  .left {
    transform-origin: left;
    animation: run-out linear forwards;
  }
  @keyframes run-out {
    to {
      transform: scaleX(0);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .left {
      animation: none;
    }
  }
</style>

<!-- Above a bar along the bottom (an editor's Save), when there is one: see lift.ts. -->
<div class="pointer-events-none fixed right-6 z-50 flex w-96 flex-col gap-2 transition-[bottom] duration-150" style="bottom: calc(1.5rem + var(--toast-lift, 0px))" aria-live="polite">
  {#if app.deleting}
    {#key app.deleting}
      {@const n = app.deleting.ids.length}
      <div class="pointer-events-auto relative flex items-center gap-3 overflow-hidden rounded-card bg-surface-3 py-2.5 pr-2.5 pl-4 text-sm shadow-[0_12px_32px_-12px_rgb(0_0_0/0.5),0_0_0_1px_var(--c-group-edge)]" role="status">
        <Icon name="trash" class="text-fg-2" />
        <span class="min-w-0 flex-1">{n === 1 ? "Deleting the snapshot" : `Deleting ${n} snapshots`}</span>
        <button class="btn h-8 px-3" title="Undo (Ctrl+Z)" onclick={() => app.undoDelete()}>Undo</button>
        <span class="left absolute inset-x-0 bottom-0 h-0.5 bg-primary" style="animation-duration: {UNDO_MS}ms" aria-hidden="true"></span>
      </div>
    {/key}
  {/if}
  {#each app.toasts as t (t.id)}
    <div class="pointer-events-auto flex items-start gap-3 rounded-card bg-surface-3 px-4 py-3.5 text-sm shadow-[0_12px_32px_-12px_rgb(0_0_0/0.5),0_0_0_1px_var(--c-group-edge)]">
      <Icon name={icon[t.kind]} class={tone[t.kind]} />
      <span class="min-w-0 flex-1 break-words">{t.text}</span>
      <button class="btn-icon -my-1 -mr-1 size-7" aria-label="Dismiss" onclick={() => (app.toasts = app.toasts.filter((x) => x.id !== t.id))}>
        <Icon name="x" size={14} />
      </button>
    </div>
  {/each}
</div>
