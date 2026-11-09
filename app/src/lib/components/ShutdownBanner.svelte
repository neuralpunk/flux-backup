<script lang="ts">
  // A backup asked to shut the computer down when done: count down, with a way out.
  import { app } from "../state.svelte";
  import Icon from "./Icon.svelte";

  let now = $state(Date.now());
  $effect(() => {
    if (app.shutdownAt == null) return;
    const t = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(t);
  });
  const left = $derived(app.shutdownAt == null ? 0 : Math.max(0, Math.round((app.shutdownAt - now) / 1000)));
</script>

{#if app.shutdownAt != null}
  <div class="fixed inset-x-0 top-14 z-50 flex justify-center px-4" role="alert">
    <div class="flex w-full max-w-xl items-center gap-3 rounded-card bg-surface-3 px-4 py-3 text-sm shadow-[0_12px_32px_-12px_rgb(0_0_0/0.5),0_0_0_1px_color-mix(in_oklab,var(--c-warning)_45%,transparent)]">
      <span class="glyph glyph-warn size-8"><Icon name="power" size={15} /></span>
      <span class="min-w-0 flex-1">
        <span class="block font-medium">{left > 0 ? `Shutting down in ${left} s` : "Shutting down…"}</span>
        <span class="block text-[12.5px] text-fg-muted">The backup finished, and you asked to shut down when it did.</span>
      </span>
      <button class="btn h-8" onclick={() => void app.cancelShutdown()}>Cancel shutdown</button>
      <button class="btn-icon size-7" aria-label="Hide" title="Hide (the shutdown still happens)" onclick={() => (app.shutdownAt = null)}><Icon name="x" size={14} /></button>
    </div>
  </div>
{/if}
