<script lang="ts">
  // Add one more place a backup goes. Asked in the same few questions as the backup
  // itself (where, how it's stored, the password), and saved at once.
  import { api, inTauri, pickFolder } from "../api";
  import { app } from "../state.svelte";
  import { bytes } from "../format";
  import type { Destination, Drive, Engine, Profile } from "../types";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";
  import Toggle from "./form/Toggle.svelte";

  let {
    id,
    taken,
    mainEngine,
    mainRemote,
    onadded,
    onclose,
  }: {
    /** The backup the destination joins. */
    id: string;
    /** Destination names already used. */
    taken: string[];
    /** How the main destination stores backups, and whether it's a server. */
    mainEngine: Engine;
    mainRemote: boolean;
    onadded: (saved: Profile) => void;
    onclose: () => void;
  } = $props();

  let kind = $state<"local" | "ssh">("local");

  /** Copy the main backup over instead of reading the files again. */
  let replicate = $state(false);
  /** Why a copy of the main backup can't go here, if it can't. */
  const noCopy = $derived(
    mainEngine === "borg"
      ? "Borg repositories can't be copied."
      : mainEngine !== "vault" && (mainRemote || kind === "ssh")
        ? "Plain folders are copied between drives on this computer only."
        : null,
  );
  $effect(() => {
    if (noCopy) replicate = false;
  });
  let path = $state("");
  let fsUuid = $state<string | null>(null);
  let host = $state("");
  let user = $state("");
  let remotePath = $state("");
  let engine = $state<Engine>("vault");
  let label = $state("");
  let offsite = $state(false);
  let pw = $state("");
  let pw2 = $state("");
  let busy = $state(false);
  let error = $state<string | null>(null);
  let drives = $state<Drive[]>([]);

  $effect(() => {
    api.drives().then((d) => (drives = d), () => (drives = []));
  });

  const encrypted = $derived(!replicate && (engine === "vault" || engine === "borg"));
  const pwWord = $derived(engine === "borg" ? "passphrase" : "password");
  const where = $derived(kind === "local" ? path : host ? `${user ? `${user}@` : ""}${host}:${remotePath || "~"}` : "");
  const suggestedLabel = $derived(
    kind === "local" ? (drives.find((d) => d.mount_point === path)?.label ?? path.split("/").filter(Boolean).pop() ?? "") : host,
  );
  const name = $derived.by(() => {
    const base = (label || suggestedLabel).toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 40) || "copy";
    let n = base;
    for (let i = 2; taken.includes(n); i++) n = `${base}-${i}`;
    return n;
  });
  const problem = $derived(
    !where
      ? kind === "local"
        ? "Choose a drive or folder."
        : "Type the server's name."
      : encrypted && !pw
        ? `Choose a ${pwWord}.`
        : encrypted && pw !== pw2
          ? `The two ${pwWord}s don't match.`
          : null,
  );

  const engines: { value: Engine; label: string; text: string }[] = [
    { value: "vault", label: "Encrypted (Vault)", text: "Compressed, deduplicated, and unreadable without the password. Good for drives that leave the house." },
    { value: "mirror", label: "Plain folders (Mirror)", text: "Browsable with any file manager." },
    { value: "rsync", label: "Plain folders with rsync", text: "Works without Flux." },
    { value: "borg", label: "Borg repository", text: "Works without Flux; needs borg." },
  ];

  async function choose(d: Drive) {
    kind = "local";
    path = d.mount_point;
    fsUuid = d.fs_uuid;
  }

  async function add() {
    if (problem) return;
    busy = true;
    error = null;
    const target: Destination["target"] =
      kind === "local"
        ? { kind: "local", path, fs_uuid: fsUuid }
        : { kind: "ssh", host, user: user || null, port: null, path: remotePath || ".", identity_file: null, bandwidth_limit: null };
    const dest: Destination = replicate
      ? { name, label: label || suggestedLabel || null, engine: mainEngine, target, offsite, mode: "replicate" }
      : {
          name,
          label: label || suggestedLabel || null,
          engine,
          target,
          offsite,
          vault: engine === "vault" ? { compression_level: 3, encryption: true, password_source: { from: "keyring" } } : null,
          borg: engine === "borg" ? { encryption: "repokey-blake2", compression: "zstd,3", password_source: { from: "keyring" }, remote_path: null, extra_args: [] } : null,
        };
    try {
      const saved = await api.addDestination(id, dest, encrypted ? pw : null);
      app.toast("ok", `Backups also go to ${dest.label ?? name} now.`);
      onadded(saved);
    } catch (e) {
      error = (e as Error).message;
    } finally {
      busy = false;
    }
  }
</script>

<Modal title="Keep a copy somewhere else too" {onclose} width="max-w-xl">
  <p class="-mt-1 mb-5 text-[13.5px] text-fg-2">
    Every backup then goes here as well, after the first place. If one copy is damaged or lost, you still have the other.
  </p>

  <div class="flex flex-col gap-5">
    <section>
      <div class="flex items-end justify-between gap-3 pb-2">
        <h2 class="group-title pb-0">Where</h2>
        <div class="segmented" role="radiogroup" aria-label="Kind of place">
          <button role="radio" aria-checked={kind === "local"} onclick={() => (kind = "local")}>A drive or folder</button>
          <button role="radio" aria-checked={kind === "ssh"} onclick={() => (kind = "ssh")}>A server (SSH)</button>
        </div>
      </div>
      {#if kind === "local"}
        <div class="rows" role="radiogroup" aria-label="Drive">
          {#each drives as d (d.mount_point)}
            <button role="radio" aria-checked={path === d.mount_point} class="row row-link gap-3 text-left" onclick={() => void choose(d)}>
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
              const f = inTauri ? await pickFolder("Where to keep the copy") : "/srv/backup";
              if (f) (path = f), (fsUuid = null);
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

    <section>
      <h2 class="group-title">How the copy is made</h2>
      <div class="rows" role="radiogroup" aria-label="How the copy is made">
        <button role="radio" aria-checked={!replicate} class="row row-link items-start gap-3 py-3 text-left" onclick={() => (replicate = false)}>
          <span class="min-w-0 flex-1">
            <span class="block font-medium">From your files</span>
            <span class="block text-[12.5px] text-fg-muted">Read again for this place: the copies are independent, so a fault in one never reaches the other. Any way of storing.</span>
          </span>
          {#if !replicate}<Icon name="check" size={18} class="mt-0.5" />{/if}
        </button>
        <button role="radio" aria-checked={replicate} disabled={!!noCopy} class="row row-link items-start gap-3 py-3 text-left disabled:cursor-not-allowed disabled:opacity-60" onclick={() => (replicate = true)}>
          <span class="min-w-0 flex-1">
            <span class="block font-medium">Copy the first backup</span>
            <span class="block text-[12.5px] text-fg-muted">{noCopy ?? "Faster: your files are read once, and the new snapshots are copied here. The same snapshots, stored the same way, with the same password."}</span>
          </span>
          {#if replicate}<Icon name="check" size={18} class="mt-0.5" />{/if}
        </button>
      </div>
    </section>

    {#if !replicate}
    <section>
      <h2 class="group-title">How it's stored</h2>
      <div class="rows" role="radiogroup" aria-label="How it's stored">
        {#each engines as e (e.value)}
          <button role="radio" aria-checked={engine === e.value} class="row row-link items-start gap-3 py-3 text-left" onclick={() => (engine = e.value)}>
            <span class="min-w-0 flex-1">
              <span class="block font-medium">{e.label}</span>
              <span class="block text-[12.5px] text-fg-muted">{e.text}</span>
            </span>
            {#if engine === e.value}<Icon name="check" size={18} class="mt-0.5" />{/if}
          </button>
        {/each}
      </div>
      {#if encrypted}
        <div class="rows mt-3">
          <label class="row gap-3"><span class="w-32 shrink-0 font-medium capitalize">{pwWord}</span><input type="password" class="input flex-1" bind:value={pw} autocomplete="new-password" /></label>
          <label class="row gap-3"><span class="w-32 shrink-0 font-medium">Repeat it</span><input type="password" class="input flex-1" bind:value={pw2} autocomplete="new-password" /></label>
        </div>
        <p class="mt-1.5 px-0.5 text-[12.5px] text-fg-muted">Kept in your desktop keyring, so scheduled backups run on their own. Write it down too: you need it to restore elsewhere.</p>
      {/if}
    </section>
    {/if}

    <section>
      <div class="rows">
        <label class="row gap-3">
          <span class="w-24 shrink-0 font-medium">Called</span>
          <input class="input flex-1" bind:value={label} placeholder={suggestedLabel || "USB stick, Office NAS…"} />
        </label>
        <Toggle row icon="external" bind:checked={offsite} label="It's kept somewhere else" hint="Away from this computer's building (a server, a drive at the office): safe from a fire or a burglary here." />
      </div>
    </section>

    {#if error}<p class="text-[13px] text-danger">{error}</p>{/if}
    <div class="flex items-center justify-end gap-3">
      {#if problem && where}<span class="mr-auto text-[12.5px] text-fg-muted">{problem}</span>{/if}
      <button class="btn btn-ghost" onclick={onclose}>Cancel</button>
      <button class="btn btn-primary" disabled={!!problem || busy} onclick={() => void add()}>
        {#if busy}<Icon name="refresh" size={14} class="motion-safe:animate-spin" />{/if} Add this place
      </button>
    </div>
  </div>
</Modal>
