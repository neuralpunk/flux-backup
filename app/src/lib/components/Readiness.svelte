<script lang="ts">
  // Could this backup be recovered on another computer? A short checklist, folded away:
  // what Flux can see (settings kept with the backups, a restore test passed, a copy kept
  // elsewhere), and what only you can tell (the password kept somewhere else).
  import { api } from "../api";
  import { app } from "../state.svelte";
  import type { ReadinessCheck } from "../types";
  import Icon from "./Icon.svelte";

  let { id }: { id: string } = $props();

  let checks = $state<ReadinessCheck[] | null>(null);
  $effect(() => {
    const want = id;
    checks = null;
    api.readiness(want).then(
      (c) => want === id && (checks = c),
      () => (checks = []),
    );
  });
  const ready = $derived(checks?.filter((c) => c.ok).length ?? 0);

  async function answer(c: ReadinessCheck) {
    try {
      checks = await api.setReadiness(id, c.key, !c.ok);
    } catch (e) {
      app.toast("bad", (e as Error).message);
    }
  }
</script>

{#if checks?.length}
  <details class="group">
    <summary class="flex cursor-pointer list-none items-center gap-2 rounded-md px-0.5 py-1 text-[13px] font-semibold text-fg-2 select-none hover:text-fg [&::-webkit-details-marker]:hidden">
      <Icon name="chevronRight" size={14} class="transition-transform group-open:rotate-90" />
      Could you recover it on another computer?
      <span class="font-normal text-fg-muted">{ready} of {checks.length} ready</span>
    </summary>
    <div class="rows mt-2">
      {#each checks as c (c.key)}
        <div class="row items-start gap-3 py-3">
          <span class="mt-0.5 grid size-5 shrink-0 place-items-center rounded-full {c.ok ? 'bg-success/15 text-success' : 'bg-surface-3 text-fg-muted'}">
            <Icon name={c.ok ? "check" : c.asked ? "help" : "x"} size={12} />
          </span>
          <span class="min-w-0 flex-1">
            <span class="block font-medium">{c.title}</span>
            <span class="block text-[12.5px] text-fg-muted">{c.detail}</span>
          </span>
          {#if c.asked}
            <button class="btn shrink-0" aria-pressed={c.ok} onclick={() => void answer(c)}>{c.ok ? "Not any more" : "I have"}</button>
          {/if}
        </div>
      {/each}
    </div>
  </details>
{/if}
