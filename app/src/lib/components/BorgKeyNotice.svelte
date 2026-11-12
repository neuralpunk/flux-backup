<script lang="ts">
  // An encrypted Borg backup whose key hasn't been exported: losing the repository's copy
  // of the key loses the backups, so say so on the Overview, with a button that does it.
  // Gone once the key is exported (or you say you have), from here or the checklist.
  import { api } from "../api";
  import { app } from "../state.svelte";
  import type { ReadinessCheck } from "../types";
  import BorgKeyDialog from "./BorgKeyDialog.svelte";
  import Callout from "./Callout.svelte";

  let { id, onlocked }: { id: string; onlocked?: (retry: () => void) => void } = $props();

  let check = $state<ReadinessCheck | null>(null);
  $effect(() => {
    const want = id;
    void app.revision; // exported from the checklist, or a backup finished: look again
    api.readiness(want).then(
      (c) => want === id && (check = c.find((x) => x.key === "key_exported") ?? null),
      () => (check = null),
    );
  });
  let open = $state(false);
</script>

{#if check && !check.ok}
  <Callout tone="warn" icon="key" compact title="Export this backup's Borg key">
    If the key inside the repository is ever damaged, the passphrase alone can't open these backups. Keep a copy somewhere safe.
    {#snippet actions()}<button class="btn h-8" onclick={() => (open = true)}>Export the key…</button>{/snippet}
  </Callout>
{/if}

{#if open}
  <BorgKeyDialog {id} detail={check?.detail} {onlocked} onclose={() => (open = false)} ondone={() => app.revision++} />
{/if}
