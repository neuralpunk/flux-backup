<script lang="ts">
  import { api } from "../api";
  import { app } from "../state.svelte";
  import { ApiError, isSystem } from "../types";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";

  let { profile, name, onunlocked, onclose }: { profile: string; name: string; onunlocked: () => void; onclose: () => void } = $props();
  const system = $derived(isSystem(profile));
  const borg = $derived(app.profileOrDestination(profile)?.engine === "borg");
  /** "password" / "passphrase", "Vault" / "repository": the words of each engine. */
  const pw = $derived(borg ? "passphrase" : "password");
  const what = $derived(borg ? "Borg repository" : "Vault");
  /** "create": the Vault doesn't exist yet, so this password becomes its password. */
  let mode = $state<"unlock" | "create">("unlock");
  let password = $state("");
  let confirm = $state("");
  // System backups run without anyone logged in: keep the password where root's timers find it.
  let keep = $state(true);
  let busy = $state(false);
  let error = $state<string | null>(null);

  const mismatch = $derived(mode === "create" && confirm.length > 0 && confirm !== password);

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    if (mode === "create" && password !== confirm) return;
    busy = true;
    error = null;
    try {
      await api.unlock(profile, password, mode === "create");
      if (system && keep) {
        const path = await api.saveKeyFile(profile);
        app.toast("ok", `Password kept in ${path} for scheduled backups.`);
      }
      password = confirm = "";
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
</script>

<Modal title={mode === "create" ? `Choose a ${pw} for “${name}”` : `Unlock “${name}”`} {onclose} width="max-w-md">
  <form class="space-y-4" onsubmit={submit}>
    <div class="flex gap-3">
      <div class="grid size-10 shrink-0 place-items-center rounded-lg bg-surface-3 text-fg-2"><Icon name={mode === "create" ? "sparkle" : "lock"} /></div>
      <p class="text-sm text-fg-2">
        {#if mode === "create"}
          This {what} doesn't exist yet: the first backup creates it, encrypted with this {pw}. Without it nobody, including you, can read the backups, so keep it somewhere safe.
        {:else}
          This {what} is encrypted. Enter its {pw} to browse and restore. Flux keeps it in memory only until you quit.
        {/if}
      </p>
    </div>
    <label class="block">
      <span class="label">{mode === "create" ? `New ${pw}` : pw.charAt(0).toUpperCase() + pw.slice(1)}</span>
      <!-- svelte-ignore a11y_autofocus -->
      <input class="input mt-1.5" type="password" autocomplete={mode === "create" ? "new-password" : "current-password"} bind:value={password} autofocus />
    </label>
    {#if mode === "create"}
      <label class="block">
        <span class="label">Repeat it</span>
        <input class="input mt-1.5" type="password" autocomplete="new-password" bind:value={confirm} aria-invalid={mismatch} />
        {#if mismatch}<span class="mt-1 block text-xs text-danger">The {pw}s don't match.</span>{/if}
      </label>
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
    {#if error}<div class="rounded-lg bg-danger/10 p-3 text-sm text-danger">{error}</div>{/if}
    <div class="flex justify-end gap-2">
      <button type="button" class="btn" onclick={onclose}>Cancel</button>
      <button class="btn btn-primary" disabled={busy || !password || (mode === "create" && password !== confirm)}>
        {busy ? (mode === "create" ? "Saving…" : "Unlocking…") : mode === "create" ? `Use this ${pw}` : "Unlock"}
      </button>
    </div>
  </form>
</Modal>
