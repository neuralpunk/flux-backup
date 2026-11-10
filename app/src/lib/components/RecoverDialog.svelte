<script lang="ts">
  // Recover an existing backup: on a new computer (or after reinstalling), find the
  // backups on a drive or a server and set one up here again, with the settings that were
  // kept with it.
  import { api, inTauri, pickFolder } from "../api";
  import { app } from "../state.svelte";
  import { ago, bytes } from "../format";
  import type { BackupSearch, Drive, FoundBackup, RecoverThen, Target } from "../types";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";

  let { onclose }: { onclose: () => void } = $props();

  let kind = $state<"local" | "ssh">("local");
  let path = $state("");
  let host = $state("");
  let user = $state("");
  let remotePath = $state("");
  let drives = $state<Drive[]>([]);
  $effect(() => {
    api.drives().then((d) => (drives = d), () => (drives = []));
  });

  let search = $state<BackupSearch | null>(null);
  let searchedAt = $state<Target | null>(null);
  let password = $state("");
  let picked = $state<FoundBackup | null>(null);
  let then = $state<RecoverThen>("restore");
  let busy = $state(false);
  let error = $state<string | null>(null);

  const target = $derived<Target | null>(
    kind === "local"
      ? path
        ? { kind: "local", path, fs_uuid: null }
        : null
      : host
        ? { kind: "ssh", host, user: user || null, port: null, path: remotePath || ".", identity_file: null, bandwidth_limit: null }
        : null,
  );

  const engineLabel: Record<string, string> = {
    vault: "Encrypted (Vault)",
    mirror: "Plain folders (Mirror)",
    rsync: "rsync folders",
    borg: "Borg repository",
  };

  async function look() {
    if (!target) return;
    busy = true;
    error = null;
    try {
      search = await api.findBackups(target, password || null);
      searchedAt = target;
      picked = search.found.length === 1 ? (search.found[0] ?? null) : null;
    } catch (e) {
      error = (e as Error).message;
    } finally {
      busy = false;
    }
  }

  async function recover() {
    if (!searchedAt || !picked) return;
    busy = true;
    error = null;
    try {
      const r = await api.recover(searchedAt, picked, then, null, password || null);
      await app.refresh();
      app.open(r.id, then === "restore" ? "snapshots" : "overview");
      app.toast(
        r.note ? "info" : "ok",
        r.note ??
          (then === "restore"
            ? `“${picked.name}” is here again. Pick a snapshot to restore from.`
            : `“${picked.name}” is here again, and carries on from this computer.`),
      );
      onclose();
    } catch (e) {
      error = (e as Error).message;
    } finally {
      busy = false;
    }
  }

  function startOver() {
    search = null;
    picked = null;
    password = "";
    error = null;
  }
</script>

<Modal title="Recover an existing backup" {onclose} width="max-w-xl">
  {#if !search}
    <p class="-mt-1 mb-5 text-[13.5px] text-fg-2">
      On a new computer, or after reinstalling: Flux finds your backups on a drive or a server and sets them up here again, with the settings that were kept with them.
    </p>
    <section>
      <div class="flex items-end justify-between gap-3 pb-2">
        <h2 class="group-title pb-0">Where are they?</h2>
        <div class="segmented" role="radiogroup" aria-label="Kind of place">
          <button role="radio" aria-checked={kind === "local"} onclick={() => (kind = "local")}>A drive or folder</button>
          <button role="radio" aria-checked={kind === "ssh"} onclick={() => (kind = "ssh")}>A server (SSH)</button>
        </div>
      </div>
      {#if kind === "local"}
        <div class="rows" role="radiogroup" aria-label="Drive">
          {#each drives as d (d.mount_point)}
            <button role="radio" aria-checked={path === d.mount_point} class="row row-link gap-3 text-left" onclick={() => (path = d.mount_point)}>
              <Icon name="drive" size={17} class="shrink-0 text-fg-2" />
              <span class="min-w-0 flex-1">
                <span class="block font-medium">{d.label}</span>
                <span class="block font-mono text-[12px] text-fg-muted">{d.mount_point}{#if d.free != null}{" · "}{bytes(d.free)} free{/if}</span>
              </span>
              {#if path === d.mount_point}<Icon name="check" size={18} />{/if}
            </button>
          {/each}
          <button
            class="row row-link gap-3 text-left"
            onclick={async () => {
              const f = inTauri ? await pickFolder("Where are the backups?") : "/run/media/me/Backup";
              if (f) path = f;
            }}
          >
            <Icon name="folder" size={17} class="shrink-0 text-fg-2" />
            <span class="min-w-0 flex-1 font-medium">{path && !drives.some((d) => d.mount_point === path) ? path : "Another folder…"}</span>
          </button>
        </div>
      {:else}
        <div class="rows">
          <label class="row gap-3"><span class="w-24 shrink-0 font-medium">Server</span><input class="input flex-1" bind:value={host} placeholder="nas.local" /></label>
          <label class="row gap-3"><span class="w-24 shrink-0 font-medium">User</span><input class="input flex-1" bind:value={user} placeholder="your user name there" /></label>
          <label class="row gap-3"><span class="w-24 shrink-0 font-medium">Folder</span><input class="input flex-1 font-mono text-[13px]" bind:value={remotePath} placeholder="/srv/backups" /></label>
        </div>
      {/if}
    </section>
    {#if error}<p class="mt-3 text-[13px] text-danger">{error}</p>{/if}
    <div class="mt-5 flex items-center justify-end gap-3">
      <button class="btn btn-ghost" onclick={onclose}>Cancel</button>
      <button class="btn btn-primary" disabled={!target || busy} onclick={() => void look()}>
        {#if busy}<Icon name="refresh" size={14} class="motion-safe:animate-spin" />{/if} Look for backups
      </button>
    </div>
  {:else}
    <p class="-mt-1 mb-4 text-[13.5px] text-fg-2">
      In <span class="font-mono text-[12.5px]">{searchedAt?.kind === "local" ? searchedAt.path : `${searchedAt?.host}:${searchedAt?.path}`}</span>
      <button class="ml-1 text-primary hover:underline" onclick={startOver}>Somewhere else…</button>
    </p>
    {#if search.vault_locked}
      <div class="card mb-4 flex flex-col gap-2 px-5 py-4">
        <span class="flex items-center gap-2 font-medium"><Icon name="lock" size={16} /> There's an encrypted Vault here</span>
        <span class="text-[13px] text-fg-2">Its backups are listed once it's open. Type the password it was made with.</span>
        <form class="flex gap-2" onsubmit={(e) => (e.preventDefault(), void look())}>
          <input type="password" class="input flex-1" bind:value={password} placeholder="Password" autocomplete="current-password" aria-label="The Vault's password" />
          <button class="btn" disabled={!password || busy}>Open it</button>
        </form>
        {#if search.wrong_password}<span class="text-[13px] text-danger">That password doesn't open it.</span>{/if}
      </div>
    {/if}
    {#if search.found.length}
      <h2 class="group-title">{search.found.length === 1 ? "The backup found" : "Which backup?"}</h2>
      <div class="rows" role="radiogroup" aria-label="Backup">
        {#each search.found as f (`${f.engine}/${f.kept.host}/${f.kept.user}/${f.kept.profile}`)}
          <button role="radio" aria-checked={picked === f} class="row row-link items-start gap-3 py-3 text-left" onclick={() => (picked = f)}>
            <Icon name={f.engine === "vault" || f.engine === "borg" ? "lock" : "folder"} size={17} class="mt-0.5 shrink-0 text-fg-2" />
            <span class="min-w-0 flex-1">
              <span class="block font-medium">{f.name}</span>
              <span class="block text-[12.5px] text-fg-2">
                {engineLabel[f.engine]} · {f.here ? "made on this computer" : `made on ${f.kept.host} by ${f.kept.user}`}
              </span>
              {#if f.sources.length}
                <span class="block truncate font-mono text-[12px] text-fg-muted" title={f.sources.join("\n")}>{f.sources.join(", ")}</span>
              {/if}
              <span class="block text-[12.5px] text-fg-muted">
                {#if f.snapshots != null}{f.snapshots} snapshot{f.snapshots === 1 ? "" : "s"}{/if}{#if f.newest}, the newest {ago(f.newest)}{/if}{f.snapshots != null ? " · " : ""}{f.saved ? "settings kept with it" : "no settings kept with it: you'll choose the folders again"}
              </span>
            </span>
            {#if picked === f}<Icon name="check" size={18} class="mt-0.5" />{/if}
          </button>
        {/each}
      </div>

      {#if picked}
        <h2 class="group-title mt-5">Then</h2>
        <div class="rows" role="radiogroup" aria-label="Then">
          <button role="radio" aria-checked={then === "restore"} class="row row-link items-start gap-3 py-3 text-left" onclick={() => (then = "restore")}>
            <span class="min-w-0 flex-1">
              <span class="block font-medium">Get files back</span>
              <span class="block text-[12.5px] text-fg-muted">Restore everything or only what you need. Nothing is scheduled until you choose.</span>
            </span>
            {#if then === "restore"}<Icon name="check" size={18} class="mt-0.5" />{/if}
          </button>
          <button role="radio" aria-checked={then === "carry_on"} class="row row-link items-start gap-3 py-3 text-left" onclick={() => (then = "carry_on")}>
            <span class="min-w-0 flex-1">
              <span class="block font-medium">Carry on backing up from this computer</span>
              <span class="block text-[12.5px] text-fg-muted">New snapshots join the same backups, on {picked.saved ? "the saved schedule" : "a schedule you set"}. Restore anything you need first.</span>
            </span>
            {#if then === "carry_on"}<Icon name="check" size={18} class="mt-0.5" />{/if}
          </button>
        </div>
      {/if}
    {:else if !search.vault_locked}
      <div class="card flex flex-col items-center gap-1 px-5 py-6 text-center">
        <Icon name="search" size={22} class="text-fg-muted" />
        <span class="font-medium">No Flux backups here</span>
        <span class="text-[13px] text-fg-2">Flux keeps them in a folder called flux-vault, flux-backup, flux-rsync or flux-borg. Pick the drive or folder that has it.</span>
      </div>
    {/if}
    {#if error}<p class="mt-3 text-[13px] text-danger">{error}</p>{/if}
    <div class="mt-5 flex items-center justify-end gap-3">
      <button class="btn btn-ghost" onclick={onclose}>Cancel</button>
      <button class="btn btn-primary" disabled={!picked || busy} onclick={() => void recover()}>
        {#if busy}<Icon name="refresh" size={14} class="motion-safe:animate-spin" />{/if} Recover
      </button>
    </div>
  {/if}
</Modal>
