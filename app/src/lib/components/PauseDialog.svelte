<script lang="ts">
  // Pause every scheduled backup for a while. Backing up by hand still works.
  import { app } from "../state.svelte";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";

  let { onclose }: { onclose: () => void } = $props();
  const choices = [
    { kind: "hour", title: "For an hour", text: "For a call, a presentation, a slow connection." },
    { kind: "tomorrow", title: "Until tomorrow morning", text: "Tonight's backups are skipped; they run again from 7:00." },
    { kind: "resumed", title: "Until I resume them", text: "The sidebar shows they're paused, with Resume." },
  ] as const;
</script>

<Modal title="Pause scheduled backups" {onclose}>
  <p class="-mt-1 mb-4 text-sm text-fg-2">All your backups' schedules wait. Backups that come due meanwhile are skipped, not queued; backing up by hand still works.</p>
  <div class="rows">
    {#each choices as c (c.kind)}
      <button class="row row-link text-left" onclick={() => (void app.setPause(c.kind), onclose())}>
        <span class="grid size-8 shrink-0 place-items-center rounded-lg bg-surface-2 text-fg-2"><Icon name="clock" size={16} /></span>
        <span class="min-w-0 flex-1">
          <span class="block text-[14px] font-medium">{c.title}</span>
          <span class="mt-0.5 block text-[12.5px] text-fg-muted">{c.text}</span>
        </span>
        <Icon name="chevronRight" size={15} class="shrink-0 text-fg-muted" />
      </button>
    {/each}
  </div>
</Modal>
