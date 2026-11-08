<script lang="ts">
  import { app } from "../state.svelte";
  import type { ProfileSummary } from "../types";
  import ConfirmDialog from "./ConfirmDialog.svelte";

  let { profile, onclose }: { profile: ProfileSummary; onclose: () => void } = $props();
  let busy = $state(false);

  async function remove() {
    busy = true;
    try {
      await app.deleteProfile(profile.id);
      onclose();
    } catch (e) {
      app.toast("bad", (e as Error).message);
      busy = false;
    }
  }
</script>

<ConfirmDialog title="Delete “{profile.name}”?" icon="trash" confirm={busy ? "Deleting…" : "Delete profile"} danger {busy} onconfirm={remove} {onclose}>
  <p>
    Flux stops backing up with this profile and forgets its settings{profile.schedule && !profile.schedule.startsWith("manual") ? " and its schedule" : ""}.
  </p>
  <p><span class="font-medium text-fg">The backups themselves stay where they are:</span></p>
  <p class="rounded-lg bg-surface-3 px-3 py-2 font-mono text-[12.5px] break-all text-fg">{profile.target}</p>
  <p>You can delete them there yourself, or make a new profile with the same destination to see them again.</p>
  {#if profile.scope === "system"}<p>This is a system backup; it covers files only the administrator can read.</p>{/if}
</ConfirmDialog>
