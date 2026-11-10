<script lang="ts">
  // Check the rules as they are in the editor (before saving): type a path, see whether
  // it's backed up and what decides it; or browse the folders, each item marked.
  import { api } from "../api";
  import type { Explained, Profile } from "../types";
  import Icon from "./Icon.svelte";
  import CoverageDialog from "./CoverageDialog.svelte";

  let { profile }: { profile: Profile } = $props();

  let path = $state("");
  let answer = $state<Explained | null>(null);
  let error = $state<string | null>(null);
  let browsing = $state(false);

  // Asked again when the path or the rules change (a moment after typing stops).
  $effect(() => {
    const p = path.trim();
    const draft = $state.snapshot(profile) as Profile;
    if (!p) {
      answer = null;
      error = null;
      return;
    }
    const timer = setTimeout(() => {
      api.explain(draft, p).then(
        (a) => ((answer = a), (error = null)),
        (e) => ((answer = null), (error = (e as Error).message)),
      );
    }, 250);
    return () => clearTimeout(timer);
  });
</script>

<div class="rows">
  <label class="row gap-3">
    <span class="w-28 shrink-0 font-medium">Test a path</span>
    <input class="input flex-1 font-mono text-[13px]" bind:value={path} placeholder="~/Downloads/film.mkv" aria-label="A path to test" spellcheck="false" />
  </label>
  {#if answer || error}
    <div class="row items-start gap-3" role="status">
      {#if answer}
        <span class="mt-0.5 grid size-5 shrink-0 place-items-center rounded-full {answer.backed_up ? 'bg-success/15 text-success' : 'bg-surface-3 text-fg-2'}">
          <Icon name={answer.backed_up ? "check" : "x"} size={12} />
        </span>
        <span class="min-w-0 flex-1 text-[13px]">{answer.text}</span>
      {:else}
        <span class="text-[13px] text-danger">{error}</span>
      {/if}
    </div>
  {/if}
  <button class="row row-link gap-3 text-left" onclick={() => (browsing = true)}>
    <Icon name="folder" size={17} class="shrink-0 text-fg-2" />
    <span class="min-w-0 flex-1">
      <span class="block font-medium">Browse what's backed up…</span>
      <span class="block text-[12.5px] text-fg-muted">Your folders, each item marked, with how much is in and left out.</span>
    </span>
  </button>
</div>

{#if browsing}
  <CoverageDialog {profile} onclose={() => (browsing = false)} />
{/if}
