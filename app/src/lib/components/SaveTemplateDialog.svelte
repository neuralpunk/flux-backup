<script lang="ts">
  // Save a backup's settings as a template for new backups: its folders, exclusions,
  // schedule and what to keep, not where it goes.
  import { api } from "../api";
  import { app } from "../state.svelte";
  import type { ProfileSummary } from "../types";
  import Modal from "./Modal.svelte";

  let { profile, onclose }: { profile: ProfileSummary; onclose: () => void } = $props();
  let name = $state("");
  let busy = $state(false);
  let error = $state<string | null>(null);

  async function save(e: Event) {
    e.preventDefault();
    if (!name.trim()) return;
    busy = true;
    error = null;
    try {
      await api.saveTemplate(profile.id, name.trim());
      app.toast("ok", `Saved the template “${name.trim()}”. New backups can start from it.`);
      onclose();
    } catch (err) {
      error = (err as Error).message;
    } finally {
      busy = false;
    }
  }
</script>

<Modal title="Save as a template" {onclose}>
  <form onsubmit={save}>
    <p class="-mt-1 mb-4 text-[13.5px] text-fg-2">
      New backups can start from the settings of “{profile.name}”: its folders, exclusions, schedule, what to keep and how it's stored. Not where it goes, the other places it's copied to, or its webhooks.
    </p>
    <label class="flex flex-col gap-1 text-[13px] font-medium">
      Name
      <!-- svelte-ignore a11y_autofocus -->
      <input class="input" bind:value={name} placeholder="Work laptop" autofocus />
    </label>
    {#if error}<p class="mt-2 text-[13px] text-danger">{error}</p>{/if}
    <div class="mt-5 flex justify-end gap-3">
      <button type="button" class="btn btn-ghost" onclick={onclose}>Cancel</button>
      <button class="btn btn-primary" disabled={!name.trim() || busy}>Save</button>
    </div>
  </form>
</Modal>
