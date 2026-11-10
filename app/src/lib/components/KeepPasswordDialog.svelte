<script lang="ts">
  // "The password is kept somewhere other than this computer": help to do it. Shows the
  // password from where Flux keeps it (masked until asked), copies it for a password
  // manager (the clipboard is cleared again after 30 seconds), saves a copy of the unlock
  // file, and ticks the checklist once it's done.
  import { onDestroy } from "svelte";
  import { api, pickFolder } from "../api";
  import { copyText } from "../clipboard";
  import { app } from "../state.svelte";
  import { isSystem, type Profile } from "../types";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";

  let { id, onclose, ondone }: { id: string; onclose: () => void; ondone: () => void } = $props();

  let profile = $state<Profile | null>(null);
  $effect(() => {
    api.getProfile(id).then(
      (e) => (profile = e.profile),
      () => (profile = null),
    );
  });
  const word = $derived(profile?.engine === "borg" ? "passphrase" : "password");
  const source = $derived((profile?.engine === "borg" ? profile.borg?.password_source : profile?.vault?.password_source)?.from ?? "keyring");
  const unlockFile = $derived(profile?.vault?.unlock_file ?? null);
  const sys = $derived(isSystem(id));

  /** Fetched once, when first shown or copied; forgotten when the dialog closes. */
  let secret = $state<string | null>(null);
  let shown = $state(false);
  let fetchError = $state<string | null>(null);
  async function fetchSecret(): Promise<string | null> {
    if (secret != null) return secret;
    try {
      secret = await api.revealPassword(id);
      return secret;
    } catch (e) {
      fetchError = (e as Error).message;
      return null;
    }
  }
  async function toggleShown() {
    if (shown) return void (shown = false);
    if ((await fetchSecret()) != null) shown = true;
  }

  let clearTimer: ReturnType<typeof setTimeout> | undefined;
  async function copy() {
    const s = await fetchSecret();
    if (s == null) return;
    await copyText(s);
    app.toast("ok", `Copied. Paste it into your password manager; Flux clears the clipboard in 30 seconds.`);
    clearTimeout(clearTimer);
    clearTimer = setTimeout(() => void copyText(""), 30_000);
  }

  let savedTo = $state<string | null>(null);
  async function saveUnlockFile() {
    const dir = await pickFolder("Where to keep a copy of the unlock file (a USB stick, say)");
    if (!dir) return;
    try {
      savedTo = await api.copyUnlockFile(id, dir);
    } catch (e) {
      app.toast("bad", (e as Error).message);
    }
  }

  onDestroy(() => {
    // Leave nothing on screen or in this component; a pending clipboard clear still runs.
    secret = null;
  });
</script>

<Modal title="Keep the {word} somewhere else" {onclose}>
  <p class="text-sm text-fg-2">
    Without it, nobody can open these backups, you included. If this computer is lost, its keyring and files go with it, so keep the {word} in a password manager, or written down somewhere safe away from the computer.
  </p>

  {#if sys}
    <p class="mt-4 rounded-lg bg-surface-2 px-4 py-3 text-[13px] text-fg-2">A system backup's {word} is in a key file only root can read; copy it from there as an administrator.</p>
  {:else if source === "prompt"}
    <p class="mt-4 rounded-lg bg-surface-2 px-4 py-3 text-[13px] text-fg-2">You type it each time and Flux doesn't keep it, so it's only where you keep it. Make sure that's not only in your head.</p>
  {:else if profile}
    <div class="mt-4 rows">
      <div class="row gap-3">
        <Icon name="lock" size={17} class="shrink-0 text-fg-2" />
        <span class="min-w-0 flex-1 truncate font-mono text-[14px] {shown ? 'select-all' : 'tracking-[0.2em] text-fg-muted'}" aria-live="polite">{shown && secret != null ? secret : "••••••••••••"}</span>
        <button class="btn shrink-0" onclick={() => void toggleShown()}><Icon name={shown ? "eyeOff" : "eye"} size={14} /> {shown ? "Hide" : "Show"}</button>
        <button class="btn shrink-0" onclick={() => void copy()}><Icon name="copy" size={14} /> Copy</button>
      </div>
    </div>
    {#if fetchError}<p class="mt-1.5 px-0.5 text-[12.5px] text-danger">{fetchError}</p>{/if}
  {/if}

  {#if unlockFile && !sys}
    <div class="mt-4 rows">
      <div class="row gap-3">
        <Icon name="key" size={17} class="shrink-0 text-fg-2" />
        <span class="min-w-0 flex-1">
          <span class="block font-medium">And the unlock file</span>
          <span class="block truncate text-[12.5px] text-fg-muted">{savedTo ? `Copied to ${savedTo}` : `The password opens the Vault only with ${unlockFile}.`}</span>
        </span>
        <button class="btn shrink-0" onclick={() => void saveUnlockFile()}>Save a copy…</button>
      </div>
    </div>
  {/if}

  <div class="mt-6 flex justify-end gap-2">
    <button class="btn" onclick={onclose}>Not now</button>
    <button class="btn btn-primary" onclick={ondone}>I've kept it elsewhere</button>
  </div>
</Modal>
