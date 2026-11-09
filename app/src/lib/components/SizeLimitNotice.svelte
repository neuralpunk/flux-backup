<script lang="ts">
  import { app } from "../state.svelte";
  import { parseSize, sizeLabel, TINY_SIZE_LIMIT } from "../size";
  import Icon from "./Icon.svelte";

  /** Says when a backup leaves files out by size, louder when the limit is so small that
   * it leaves out nearly everything (a bare "1" is one byte). `onlyTiny`: stay quiet
   * about ordinary limits. */
  let { id, onlyTiny = false, class: cls = "" }: { id: string; onlyTiny?: boolean; class?: string } = $props();

  const limit = $derived(app.profiles.find((p) => p.id === id)?.size_limit ?? null);
  const tiny = $derived(limit != null && (parseSize(limit)?.bytes ?? Infinity) < TINY_SIZE_LIMIT);
</script>

{#if limit && (tiny || !onlyTiny)}
  <div class="card flex items-center gap-3 px-4 py-2.5 text-sm {tiny ? 'bg-warning/10' : ''} {cls}" role={tiny ? "alert" : "note"}>
    <span class="glyph {tiny ? 'glyph-warn' : ''} shrink-0"><Icon name={tiny ? "alert" : "info"} size={13} /></span>
    <span class="min-w-0 flex-1">
      {#if tiny}
        <span class="font-medium">Files larger than {sizeLabel(limit)} are left out, so these backups hold almost no files.</span>
        <span class="text-fg-2">Folders are saved, but they're empty. Set a limit like 4 GB, or none, then back up again.</span>
      {:else}
        <span class="text-fg-2">Files larger than <span class="num font-medium text-fg">{sizeLabel(limit)}</span> aren't backed up, so they can't be restored from here.</span>
      {/if}
    </span>
    <button class="btn {tiny ? '' : 'btn-ghost'} h-8 shrink-0" onclick={() => app.openSettings(id, "exclusions")}>Change the limit</button>
  </div>
{/if}
