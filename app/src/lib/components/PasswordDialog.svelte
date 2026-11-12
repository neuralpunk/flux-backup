<script lang="ts">
  import Callout from "./Callout.svelte";
  import { api } from "../api";
  import { app } from "../state.svelte";
  import { ApiError, isSystem, type KeyInfo, type Replaced } from "../types";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";
  import RecoveryCodeShown from "./RecoveryCodeShown.svelte";

  let { profile, name, onunlocked, onclose }: { profile: string; name: string; onunlocked: () => void; onclose: () => void } = $props();
  const system = $derived(isSystem(profile));
  const borg = $derived(app.profileOrDestination(profile)?.engine === "borg");
  /** "password" / "passphrase", "Vault" / "repository": the words of each engine. */
  const pw = $derived(borg ? "passphrase" : "password");
  const what = $derived(borg ? "Borg repository" : "Vault");
  /**
   * "unlock": asking for the password (or the recovery code).
   * "create": the Vault doesn't exist yet, so this password becomes its password.
   * "replace": it was opened with the recovery code, so the password was lost: a new one
   *   is chosen before going on (this step can't be skipped).
   * "code": the new recovery code, shown once.
   */
  let mode = $state<"unlock" | "create" | "replace" | "code">("unlock");
  let password = $state("");
  let confirm = $state("");
  // System backups run without anyone logged in: keep the password where root's timers find it.
  let keep = $state(true);
  let busy = $state(false);
  let error = $state<string | null>(null);
  /** The Vault's passwords, when replacing: the lost one(s) to stop working. */
  let passwords = $state<KeyInfo[]>([]);
  let removing = $state<string[]>([]);
  let replaced = $state<Replaced | null>(null);

  const choosing = $derived(mode === "create" || mode === "replace");
  const mismatch = $derived(choosing && confirm.length > 0 && confirm !== password);

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    if (choosing && password !== confirm) return;
    busy = true;
    error = null;
    try {
      if (mode === "replace") {
        replaced = await api.replacePassword(profile, password, removing);
        password = confirm = "";
        if (replaced.recovery_code) mode = "code";
        else await finish();
        return;
      }
      const opened = await api.unlock(profile, password, mode === "create");
      password = confirm = "";
      if (opened?.recovery_code) {
        await startReplacing();
        return;
      }
      await keepForTimers();
      onunlocked();
    } catch (err) {
      if (err instanceof ApiError && err.code === "new_vault") {
        mode = "create";
        error = null;
      } else {
        error = err instanceof ApiError && err.code === "wrong_password" ? `That ${pw} doesn't open this ${what}.` : (err as Error).message;
      }
    } finally {
      busy = false;
    }
  }

  /** Opened with the recovery code: on to choosing a new password. */
  async function startReplacing() {
    try {
      const s = await api.vaultSecurity(profile);
      passwords = s.keys.filter((k) => k.label !== "recovery code");
    } catch {
      passwords = [];
    }
    // All of them by default: the lost password shouldn't open the backups any more.
    removing = passwords.map((k) => k.id);
    mode = "replace";
  }

  /** A system backup's password, kept in a root-only key file for its timers. */
  async function keepForTimers() {
    if (!system || !keep) return;
    const path = await api.saveKeyFile(profile);
    app.toast("ok", `Password kept in ${path} for scheduled backups.`);
  }

  async function finish() {
    const r = replaced;
    replaced = null;
    try {
      await keepForTimers();
    } catch (err) {
      app.toast("bad", (err as Error).message);
    }
    if (r?.kept === "command") app.toast("info", "New password set. This backup gets its password from a command: change it to give the new one.");
    else app.toast("ok", r?.kept === "keyring" ? "New password set and kept in the keyring." : "New password set.");
    if (r?.unlock_file_dropped) app.toast("info", `The unlock file wasn't at ${r.unlock_file_dropped}, so the new password works without one. Set one up again in Security.`);
    if (r && !r.recovery_code) app.toast("info", `Make a new recovery code with sudo flux --system key recovery ${profile}: the one you used still works until then.`);
    onunlocked();
  }

  function toggle(id: string, on: boolean) {
    removing = on ? [...removing, id] : removing.filter((r) => r !== id);
  }

  const title = $derived(
    mode === "create" ? `Choose a ${pw} for “${name}”` : mode === "replace" ? `Choose a new password for “${name}”` : mode === "code" ? `New recovery code for “${name}”` : `Unlock “${name}”`,
  );
</script>

<Modal {title} {onclose} width="max-w-md" locked={busy} required={mode === "replace" || mode === "code"}>
  {#if mode === "code" && replaced?.recovery_code}
    <p class="mb-4 text-[14px] text-fg-2">The recovery code you used no longer works. This one replaces it.</p>
    <RecoveryCodeShown code={replaced.recovery_code} ondone={finish} />
  {:else}
    <form class="space-y-4" onsubmit={submit}>
      <div class="flex gap-3">
        <div class="grid size-10 shrink-0 place-items-center rounded-lg bg-surface-3 text-fg-2"><Icon name={mode === "create" ? "sparkle" : mode === "replace" ? "key" : "lock"} /></div>
        <p class="text-sm text-fg-2">
          {#if mode === "create"}
            This {what} doesn't exist yet: the first backup creates it, encrypted with this {pw}. Without it nobody, including you, can read the backups, so keep it somewhere safe.
          {:else if mode === "replace"}
            You opened the backups with the recovery code. Choose a new password to use from now on. You'll get a new recovery code next, as this one has
            now been typed in.
          {:else}
            This {what} is encrypted. Enter its {pw} to browse and restore. Flux keeps it in memory only until you quit.
          {/if}
        </p>
      </div>
      <label class="block">
        <span class="label">{choosing ? `New ${pw}` : pw.charAt(0).toUpperCase() + pw.slice(1)}</span>
        <!-- svelte-ignore a11y_autofocus -->
        <input class="input mt-1.5" type="password" autocomplete={choosing ? "new-password" : "current-password"} bind:value={password} autofocus />
      </label>
      {#if choosing}
        <label class="block">
          <span class="label">Repeat it</span>
          <input class="input mt-1.5" type="password" autocomplete="new-password" bind:value={confirm} aria-invalid={mismatch} />
          {#if mismatch}<span class="mt-1 block text-xs text-danger">The {pw}s don't match.</span>{/if}
        </label>
      {/if}
      {#if mode === "replace" && passwords.length > 1}
        <fieldset class="rounded-lg bg-surface-3 p-3 text-sm">
          <legend class="sr-only">Passwords to stop</legend>
          <p class="mb-2 font-medium">Stop these passwords working</p>
          <div class="space-y-1.5">
            {#each passwords as k (k.id)}
              <label class="flex items-center gap-2.5">
                <input type="checkbox" class="size-4 accent-[var(--c-primary)]" checked={removing.includes(k.id)} onchange={(e) => toggle(k.id, e.currentTarget.checked)} />
                <span>“{k.label}”</span>
              </label>
            {/each}
          </div>
          <p class="mt-2 text-[12.5px] text-fg-muted">Untick one you still know, such as a spare kept in a safe.</p>
        </fieldset>
      {:else if mode === "replace" && passwords.length === 1}
        <p class="text-[13px] text-fg-muted">The old password stops working.</p>
      {/if}
      {#if system}
        <label class="flex items-start gap-2.5 rounded-lg bg-surface-3 p-3 text-sm">
          <input type="checkbox" class="mt-0.5 accent-[var(--c-primary)]" bind:checked={keep} />
          <span>
            <span class="block font-medium">Keep it for scheduled backups</span>
            <span class="block text-fg-muted">Saved in a key file only root can read, so timers can open the {what} without anyone logged in.</span>
          </span>
        </label>
      {/if}
      {#if error}<Callout tone="bad">{error}</Callout>{/if}
      <div class="flex justify-end gap-2">
        {#if mode !== "replace"}<button type="button" class="btn" onclick={onclose}>Cancel</button>{/if}
        <button class="btn btn-primary" disabled={busy || !password || (choosing && password !== confirm)}>
          {#if busy}
            {mode === "create" ? "Saving…" : mode === "replace" ? "Setting it…" : "Unlocking…"}
          {:else}
            {mode === "create" ? `Use this ${pw}` : mode === "replace" ? "Use this password" : "Unlock"}
          {/if}
        </button>
      </div>
      {#if mode === "unlock" && !borg}
        <p class="text-[12.5px] text-fg-muted">Forgot the password? Paste the recovery code in its place to unlock.</p>
      {/if}
    </form>
  {/if}
</Modal>
