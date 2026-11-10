<script lang="ts">
  import { app } from "../state.svelte";
  import { parseSize, sizeLabel, TINY_SIZE_LIMIT } from "../size";
  import Icon from "./Icon.svelte";
  import Callout from "./Callout.svelte";
  import ProfileEditor from "./ProfileEditor.svelte";

  /** Says when a backup leaves files out by size, louder when the limit is so small that
   * it leaves out nearly everything (a bare "1" is one byte). `onlyTiny`: stay quiet
   * about ordinary limits. */
  let { id, onlyTiny = false, class: cls = "" }: { id: string; onlyTiny?: boolean; class?: string } = $props();

  const limit = $derived(app.profiles.find((p) => p.id === id)?.size_limit ?? null);
  const tiny = $derived(limit != null && (parseSize(limit)?.bytes ?? Infinity) < TINY_SIZE_LIMIT);
  /** The size limit's dialog, open over this page (not a trip to Settings). */
  let changing = $state(false);
</script>

{#if limit && tiny}
  <Callout compact class={cls} title="Files larger than {sizeLabel(limit)} are left out, so these backups hold almost no files.">
    Folders are saved, but they're empty. Set a limit like 4 GB, or none, then back up again.
    {#snippet actions()}<button class="btn h-8" onclick={() => (changing = true)}>Change the limit</button>{/snippet}
  </Callout>
{:else if limit && !onlyTiny}
  <!-- An ordinary limit is worth knowing, not worth a banner. -->
  <p class="flex items-center gap-1.5 px-0.5 text-[12.5px] text-fg-muted {cls}" role="note">
    <Icon name="info" size={13} class="shrink-0" />
    <span>Files larger than <span class="num text-fg-2">{sizeLabel(limit)}</span> aren't backed up.</span>
    <button class="font-medium text-fg-2 underline-offset-2 hover:text-fg hover:underline" onclick={() => (changing = true)}>Change</button>
  </p>
{/if}

{#if changing}
  <ProfileEditor {id} dialogOnly="exclusions" onsaved={() => ((changing = false), void app.refresh())} oncancel={() => (changing = false)} />
{/if}
