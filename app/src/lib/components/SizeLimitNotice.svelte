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

{#if limit && tiny}
  <div class="card flex items-center gap-3 bg-warning/10 px-4 py-2.5 text-sm {cls}" role="alert">
    <span class="glyph glyph-warn shrink-0"><Icon name="alert" size={13} /></span>
    <span class="min-w-0 flex-1">
      <span class="font-medium">Files larger than {sizeLabel(limit)} are left out, so these backups hold almost no files.</span>
      <span class="text-fg-2">Folders are saved, but they're empty. Set a limit like 4 GB, or none, then back up again.</span>
    </span>
    <button class="btn h-8 shrink-0" onclick={() => app.openSettings(id, "exclusions")}>Change the limit</button>
  </div>
{:else if limit && !onlyTiny}
  <!-- An ordinary limit is worth knowing, not worth a banner. -->
  <p class="flex items-center gap-1.5 px-0.5 text-[12.5px] text-fg-muted {cls}" role="note">
    <Icon name="info" size={13} class="shrink-0" />
    <span>Files larger than <span class="num text-fg-2">{sizeLabel(limit)}</span> aren't backed up.</span>
    <button class="font-medium text-fg-2 underline-offset-2 hover:text-fg hover:underline" onclick={() => app.openSettings(id, "exclusions")}>Change</button>
  </p>
{/if}
