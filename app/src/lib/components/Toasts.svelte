<script lang="ts">
  import { app } from "../state.svelte";
  import Icon from "./Icon.svelte";

  const tone = { ok: "text-success", bad: "text-danger", info: "text-fg-2" } as const;
  // A warning sign only for what went wrong; news ("Timer updated") gets an "i".
  const icon = { ok: "check", bad: "alert", info: "info" } as const;
</script>

<!-- Above a bar along the bottom (an editor's Save), when there is one: see lift.ts. -->
<div class="pointer-events-none fixed right-6 z-50 flex w-96 flex-col gap-2 transition-[bottom] duration-150" style="bottom: calc(1.5rem + var(--toast-lift, 0px))" aria-live="polite">
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
