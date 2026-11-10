<script lang="ts">
  // Where setting up a backup starts: a template that fills in the folders, exclusions,
  // schedule and what to keep (all changeable next), or nothing.
  import { api } from "../api";
  import { app } from "../state.svelte";
  import type { Template } from "../types";
  import Icon, { type IconName } from "./Icon.svelte";

  let { system = false, onpick }: { system?: boolean; onpick: (id: string) => void } = $props();

  let templates = $state<Template[] | null>(null);
  $effect(() => {
    api.templates(system).then(
      (t) => (templates = t),
      () => (templates = []),
    );
  });

  async function remove(t: Template) {
    try {
      templates = await api.deleteTemplate(t.id, system);
      app.toast("info", `Removed the template “${t.name}”.`);
    } catch (e) {
      app.toast("bad", (e as Error).message);
    }
  }
</script>

{#if templates === null}
  <p class="py-10 text-center text-[13px] text-fg-muted">Loading…</p>
{:else}
  {@const builtin = templates.filter((t) => !t.own)}
  {@const own = templates.filter((t) => t.own)}
  <h2 class="group-title">What do you want to back up?</h2>
  <div class="grid gap-3 sm:grid-cols-2">
    {#each builtin as t (t.id)}
      <button class="card flex items-start gap-3 px-4 py-3.5 text-left transition-colors hover:bg-surface-3" onclick={() => onpick(t.id)}>
        <span class="grid size-9 shrink-0 place-items-center rounded-lg bg-surface-3 text-fg-2"><Icon name={t.icon as IconName} size={18} /></span>
        <span class="min-w-0">
          <span class="block font-medium">{t.name}</span>
          <span class="block text-[12.5px] text-fg-muted">{t.line}</span>
        </span>
      </button>
    {/each}
  </div>
  {#if own.length}
    <h2 class="group-title mt-6">Your templates</h2>
    <div class="rows">
      {#each own as t (t.id)}
        <div class="row gap-3">
          <button class="flex min-w-0 flex-1 items-start gap-3 text-left" onclick={() => onpick(t.id)}>
            <Icon name="copy" size={17} class="mt-0.5 shrink-0 text-fg-2" />
            <span class="min-w-0">
              <span class="block font-medium">{t.name}</span>
              <span class="block truncate text-[12.5px] text-fg-muted" title={t.line}>{t.line}</span>
            </span>
          </button>
          <button class="btn-icon size-8 shrink-0" aria-label="Remove the template {t.name}" title="Remove" onclick={() => void remove(t)}><Icon name="trash" size={15} /></button>
        </div>
      {/each}
    </div>
  {/if}
  <p class="mt-4 px-0.5 text-[12.5px] text-fg-muted">To make a template of your own, choose Save as a template in a backup's ⋯ menu.</p>
{/if}
