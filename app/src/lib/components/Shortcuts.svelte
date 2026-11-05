<script lang="ts">
  import { app } from "../state.svelte";
  import Modal from "./Modal.svelte";

  const keys: [string, string][] = [
    ["Ctrl + B", "Back up the selected profile now"],
    ["Ctrl + N", "New profile"],
    ["Ctrl + 1 … 4", "Overview · Snapshots · Activity · Settings"],
    ["Alt + ↑ / ↓", "Previous / next profile"],
    ["← / →", "Previous / next snapshot (timeline focused)"],
    ["/", "Filter the current folder"],
    ["Esc", "Close a dialog"],
    ["?", "This list"],
  ];
</script>

<Modal title="Keyboard shortcuts" onclose={() => (app.showShortcuts = false)} width="max-w-md">
  <dl class="grid grid-cols-[auto_1fr] gap-x-6 gap-y-2.5 text-sm">
    {#each keys as [k, v] (k)}
      <dt><kbd class="rounded-lg bg-surface-3 px-2 py-0.5 font-mono text-xs">{k}</kbd></dt>
      <dd class="text-fg-2">{v}</dd>
    {/each}
  </dl>
</Modal>
