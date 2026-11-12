<script lang="ts">
  // A Vault's recovery code: made here, shown once, to print or keep somewhere safe. It
  // opens the Vault like a password when every password is lost (also without the unlock
  // file). A new one replaces the old.
  import { api } from "../api";
  import { app } from "../state.svelte";
  import { ApiError } from "../types";
  import Modal from "./Modal.svelte";
  import RecoveryCodeShown from "./RecoveryCodeShown.svelte";

  let {
    id,
    name,
    replacing = false,
    onclose,
    ondone,
  }: {
    id: string;
    name: string;
    /** There's one already: making a new one stops it working. */
    replacing?: boolean;
    onclose: () => void;
    /** A code was made. */
    ondone?: () => void;
  } = $props();

  let code = $state<string | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);

  async function make() {
    busy = true;
    error = null;
    try {
      code = await api.recoveryCode(id);
      ondone?.();
    } catch (e) {
      if (e instanceof ApiError && e.code === "password_required") {
        app.unlock = { id, retry: () => void make() };
      } else error = (e as Error).message;
    } finally {
      busy = false;
    }
  }

  function close() {
    // Forget it here as soon as the dialog goes: it was shown once.
    code = null;
    onclose();
  }
</script>

<Modal title="Recovery code for “{name}”" onclose={close} locked={busy}>
  {#if !code}
    <div class="flex flex-col gap-3 text-[14px] text-fg-2">
      <p>
        If the password is ever lost, these backups can't be opened, by you or anyone. A recovery code is a long code that opens them
        instead: print it or write it down, and keep it somewhere safe, away from the computer and the backups.
      </p>
      <p>It works without the unlock file too, so keep it as safe as the password.</p>
      {#if replacing}
        <p class="text-warning">There's a recovery code already. A new one replaces it: the old one stops working.</p>
      {/if}
      {#if error}<p class="text-danger">{error}</p>{/if}
      <div class="mt-1 flex justify-end gap-2">
        <button class="btn btn-ghost" onclick={close} disabled={busy}>Not now</button>
        <button class="btn btn-primary" onclick={make} disabled={busy}>{busy ? "Making it…" : replacing ? "Make a new one" : "Make a recovery code"}</button>
      </div>
    </div>
  {:else}
    <RecoveryCodeShown {code} ondone={close} />
  {/if}
</Modal>
