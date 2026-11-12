<script lang="ts">
  // "The Borg key is exported": help to do it. Borg keeps the key inside the repository
  // (repokey): if that copy is ever damaged, the passphrase alone can't open the backups.
  // One button picks a folder and exports the key there (`borg key export`, a new file,
  // never over one); the checklist is ticked by the export itself.
  import { api, pickFolder } from "../api";
  import { ApiError, isSystem } from "../types";
  import Callout from "./Callout.svelte";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";

  let {
    id,
    detail,
    onclose,
    ondone,
    onlocked,
  }: {
    id: string;
    /** The checklist line's detail: names the repository and the command, for a system backup. */
    detail?: string;
    onclose: () => void;
    /** Exported (or said to be): the checklist changed. */
    ondone: () => void;
    onlocked?: (retry: () => void) => void;
  } = $props();

  const sys = $derived(isSystem(id));
  let busy = $state(false);
  let savedTo = $state<string | null>(null);
  let error = $state<string | null>(null);

  async function exportKey() {
    const dir = await pickFolder("Where to keep the Borg key (a USB stick or another computer, not next to the backups)");
    if (!dir) return;
    busy = true;
    error = null;
    try {
      savedTo = await api.exportBorgKey(id, dir);
      ondone();
    } catch (e) {
      if (e instanceof ApiError && e.code === "password_required" && onlocked) onlocked(() => void exportKey());
      else error = (e as Error).message;
    } finally {
      busy = false;
    }
  }

  async function already() {
    try {
      await api.setReadiness(id, "key_exported", true);
      ondone();
      onclose();
    } catch (e) {
      error = (e as Error).message;
    }
  }
</script>

<Modal title="Export the Borg key" {onclose} locked={busy}>
  <p class="text-sm text-fg-2">
    Borg keeps this backup's key inside the repository. If that copy is ever damaged, by a failing drive or a broken server, the passphrase alone can't open the backups and they're lost. A copy of the key, kept somewhere safe, means they can still be opened.
  </p>
  <ul class="mt-3 list-disc space-y-1 pl-5 text-[13px] text-fg-2">
    <li>Keep it <span class="font-medium text-fg">away from the backups</span>: a USB stick, a password manager, another computer.</li>
    <li>Keep it <span class="font-medium text-fg">with the passphrase</span>: opening the backups takes both. The key alone opens nothing, so it's safe to store.</li>
  </ul>

  {#if sys}
    <p class="mt-4 rounded-lg bg-surface-2 px-4 py-3 text-[13px] text-fg-2">A system backup's key can only be exported by an administrator. {detail ?? ""}</p>
  {:else if savedTo}
    <Callout class="mt-4" tone="ok" title="The key is exported">
      <span class="font-mono text-[12.5px] break-all">{savedTo}</span><br />Now move or copy it somewhere safe, away from this computer's backups.
    </Callout>
  {:else}
    <div class="mt-4 rows">
      <div class="row gap-3">
        <Icon name="key" size={17} class="shrink-0 text-fg-2" />
        <span class="min-w-0 flex-1">
          <span class="block font-medium">Save the key to a file</span>
          <span class="block text-[12.5px] text-fg-muted">Choose a folder; Flux writes a new file there and never replaces one.</span>
        </span>
        <button class="btn btn-primary shrink-0" disabled={busy} onclick={() => void exportKey()}>{busy ? "Exporting…" : "Choose a folder…"}</button>
      </div>
    </div>
  {/if}
  {#if error}<p class="mt-2 px-0.5 text-[12.5px] text-danger">{error}</p>{/if}

  <div class="mt-6 flex justify-end gap-2">
    {#if savedTo}
      <button class="btn btn-primary" onclick={onclose}>Done</button>
    {:else}
      <button class="btn btn-ghost text-fg-2" disabled={busy} onclick={() => void already()} title="You exported it yourself (borg key export): stop reminding">I already have</button>
      <button class="btn" disabled={busy} onclick={onclose}>Not now</button>
    {/if}
  </div>
</Modal>
