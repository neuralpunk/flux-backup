<script lang="ts">
  // A Vault's security settings (the "Security" group of an encrypted Vault backup).
  //
  // Two kinds of setting live here. How hard the password is hashed and whether it needs
  // an unlock file belong to the Vault's key, which other computers may share: they change
  // by an action on the Vault right away (it needs the password), and the profile follows
  // (`onsynced`). Hiding sizes and locking again are this backup's settings, saved with
  // the rest of the editor.
  import { api, inTauri, pickFolder } from "../api";
  import { app } from "../state.svelte";
  import { ApiError, type PasswordHashing, type VaultOptions, type VaultSecurity } from "../types";
  import Icon from "./Icon.svelte";
  import InfoDetails from "./InfoDetails.svelte";
  import Row from "./form/Row.svelte";
  import Toggle from "./form/Toggle.svelte";
  import NumberInput from "./form/NumberInput.svelte";

  let {
    vault = $bindable(),
    id,
    name,
    onsynced,
  }: {
    vault: VaultOptions;
    /** `null` while the backup is being set up (no Vault yet). */
    id: string | null;
    name: string;
    /** The Vault was changed and the saved profile with it: take these values as saved. */
    onsynced: (fields: Partial<VaultOptions>) => void;
  } = $props();

  type Level = PasswordHashing["level"];
  const presets: { level: Exclude<Level, "custom">; label: string; detail: string; cost: number }[] = [
    { level: "standard", label: "Standard", detail: "64 MiB of memory, 3 passes. Flux's default.", cost: 64 * 3 },
    { level: "strong", label: "Strong", detail: "256 MiB, 4 passes.", cost: 256 * 4 },
    { level: "maximum", label: "Maximum", detail: "1 GiB, 6 passes. Needs that much free memory each time it's unlocked.", cost: 1024 * 6 },
  ];
  const hashing = $derived<PasswordHashing>(vault.hashing ?? { level: "standard" });
  const costOf = (h: PasswordHashing) =>
    h.level === "custom" ? h.memory_mib * h.passes : (presets.find((p) => p.level === h.level)?.cost ?? 192);
  const same = (a: PasswordHashing, b: PasswordHashing) => JSON.stringify(a) === JSON.stringify(b);

  let security = $state<VaultSecurity | null>(null);
  let standardMs = $state<number | null>(null);
  let busy = $state<"strengthen" | "file" | null>(null);
  let fileNote = $state<string | null>(null);

  async function load() {
    try {
      standardMs = await api.timeHashing({ level: "standard" });
      if (id) security = await api.vaultSecurity(id);
    } catch (e) {
      app.toast("bad", (e as Error).message);
    }
  }
  $effect(() => {
    void load();
  });

  /** "about 0.3 s" on this computer, scaled from one measured standard hash. */
  function seconds(h: PasswordHashing): string {
    if (standardMs == null) return "";
    const s = (standardMs * costOf(h)) / (64 * 3) / 1000;
    return s < 0.1 ? "under 0.1 s" : s < 10 ? `about ${s.toFixed(1)} s` : `about ${Math.round(s)} s`;
  }

  function choose(level: Level) {
    vault.hashing = level === "custom" ? (hashing.level === "custom" ? hashing : { level: "custom", memory_mib: 128, passes: 3, threads: 4 }) : { level };
  }

  /** The password in use (or, while locked, the Vault's first). */
  const inUse = $derived(security?.keys.find((k) => k.current) ?? (security?.keys.length === 1 ? security.keys[0] : null));
  const needsApply = $derived(!!id && !!security?.keys.length && (!inUse || !same(inUse.hashing, hashing)));

  /** Run `f`; when the Vault is locked, ask for its password and try again. */
  async function withPassword(f: () => Promise<void>) {
    try {
      await f();
    } catch (e) {
      if (e instanceof ApiError && e.code === "password_required" && id) {
        app.unlock = { id, retry: () => void withPassword(f) };
      } else app.toast("bad", (e as Error).message);
    }
  }

  async function strengthen() {
    if (!id) return;
    busy = "strengthen";
    await withPassword(async () => {
      await api.strengthenKey(id, hashing);
      onsynced({ hashing });
      app.toast("ok", `The password of “${name}” is now hashed with the ${words(hashing)} setting.`);
      security = await api.vaultSecurity(id);
    });
    busy = null;
  }

  async function setUpFile() {
    if (!id) return;
    const folder = inTauri ? await pickFolder("Where to keep the unlock file (a USB stick, for example)") : "/run/media/me/STICK";
    if (!folder) return;
    const slug = name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "backup";
    busy = "file";
    await withPassword(async () => {
      const r = await api.setUnlockFile(id, `${folder.replace(/\/$/, "")}/${slug}.flux-unlock`, true);
      onsynced({ unlock_file: r.unlock_file });
      fileNote = r.without_file.length
        ? `${r.without_file.length === 1 ? "One other password" : `${r.without_file.length} other passwords`} (${r.without_file.map((k) => `“${k.label}”`).join(", ")}) still open the Vault without the file. Remove ${r.without_file.length === 1 ? "it" : "them"} with flux key remove, or keep ${r.without_file.length === 1 ? "it" : "them"} as a spare that doesn't need the stick.`
        : null;
      security = await api.vaultSecurity(id);
      app.toast("ok", "Unlock file made. Keep a copy of it somewhere safe: without it, the password doesn't open the Vault.");
    });
    busy = null;
  }

  async function removeFile() {
    if (!id) return;
    busy = "file";
    await withPassword(async () => {
      await api.setUnlockFile(id, null);
      onsynced({ unlock_file: null });
      fileNote = null;
      security = await api.vaultSecurity(id);
      app.toast("ok", "The password alone opens the Vault again.");
    });
    busy = null;
  }

  function words(h: PasswordHashing): string {
    return h.level === "custom" ? `custom (${h.memory_mib} MiB, ${h.passes} passes)` : h.level;
  }

  const lockChoices: { value: number | null; label: string }[] = [
    { value: null, label: "Only when the app closes" },
    { value: 5, label: "After 5 minutes unused" },
    { value: 15, label: "After 15 minutes unused" },
    { value: 30, label: "After 30 minutes unused" },
    { value: 60, label: "After an hour unused" },
  ];
</script>

<section class="flex flex-col">
  <h2 class="group-title">Password hashing</h2>
  <p class="-mt-1 mb-2.5 px-0.5 text-[12.5px] text-fg-muted">
    Harder hashing makes guessing the password of a stolen drive slower, and unlocking slower by the same amount. A long password matters more.
  </p>
  <div class="rows" role="radiogroup" aria-label="Password hashing">
    {#each [...presets, { level: "custom" as const, label: "Custom", detail: "Choose the memory, passes and threads.", cost: 0 }] as o (o.level)}
      {@const selected = hashing.level === o.level}
      <button role="radio" aria-checked={selected} class="row row-link items-start py-3" onclick={() => choose(o.level)}>
        <span class="flex min-w-0 flex-1 flex-col">
          <span class="font-medium">{o.label}</span>
          <span class="text-[12.5px] text-fg-muted">
            {o.detail}
            {#if o.level !== "custom" && standardMs != null}Unlocks in {seconds({ level: o.level })} here.{/if}
          </span>
        </span>
        <span class="grid size-5 shrink-0 place-items-center">{#if selected}<Icon name="check" size={18} />{/if}</span>
      </button>
    {/each}
  </div>
  {#if hashing.level === "custom"}
    {@const c = hashing}
    <div class="mt-3 flex flex-wrap items-center gap-x-5 gap-y-2 px-0.5 text-[13px]">
      <label class="flex items-center gap-2">Memory <NumberInput bind:value={c.memory_mib} min={8} max={4096} label="Memory in MiB" /> MiB</label>
      <label class="flex items-center gap-2">Passes <NumberInput bind:value={c.passes} min={1} max={20} label="Passes" /></label>
      <label class="flex items-center gap-2">Threads <NumberInput bind:value={c.threads} min={1} max={16} label="Threads" /></label>
      {#if standardMs != null}<span class="text-fg-muted">Unlocks in {seconds(c)} here.</span>{/if}
    </div>
  {/if}
  {#if !id}
    <p class="mt-2 px-0.5 text-[12.5px] text-fg-muted">The Vault's password is hashed this way when it's created.</p>
  {:else if security && !security.keys.length}
    <p class="mt-2 px-0.5 text-[12.5px] text-fg-muted">The Vault doesn't exist yet: its password is hashed this way when it's created.</p>
  {:else if security}
    <div class="mt-3 flex items-center gap-3 px-0.5">
      <p class="min-w-0 flex-1 text-[12.5px] text-fg-2">
        {#if inUse}The password {security.unlocked ? "in use" : ""} is hashed with the <span class="font-medium text-fg">{words(inUse.hashing)}</span> setting.{:else}Unlock the backup to see the setting its password uses.{/if}
        {#if needsApply}Applying rewrites only its key, at once; no backup data changes. New passwords use the setting chosen here.{/if}
      </p>
      {#if needsApply}
        <button class="btn btn-primary shrink-0" disabled={busy === "strengthen"} onclick={() => void strengthen()}>
          {#if busy === "strengthen"}<Icon name="refresh" size={14} class="motion-safe:animate-spin" />{/if} Apply to the password
        </button>
      {/if}
    </div>
  {/if}
</section>

<section class="flex flex-col">
  <h2 class="group-title">Unlock file</h2>
  <div class="rows">
    {#if !id}
      <Row title="Also require an unlock file" subtitle="" icon="key" disabled reason="Available once the backup is set up." />
    {:else if vault.unlock_file}
      <Row title="The password needs an unlock file" icon="key" subtitle={security && !security.unlock_file_present ? "It isn't there now: backups and unlocking wait for it." : "Backups and unlocking need it to be there."}>
        <button class="btn" disabled={busy === "file"} onclick={() => void removeFile()}>Stop requiring it</button>
      </Row>
      <div class="row gap-3 text-[13px]">
        <span class="w-5 shrink-0"></span>
        <span class="min-w-0 flex-1 truncate font-mono text-[12.5px] text-fg-2" title={vault.unlock_file}>{vault.unlock_file}</span>
        {#if security}
          <span class="tag shrink-0 {security.unlock_file_present ? 'text-success' : 'text-warning'}"><Icon name={security.unlock_file_present ? "check" : "alert"} size={12} /> {security.unlock_file_present ? "Connected" : "Not connected"}</span>
        {/if}
      </div>
    {:else}
      <Row title="Also require an unlock file" icon="key" subtitle="A small file, kept on a USB stick for example. Then the password alone can't open the Vault: someone also needs the file.">
        <button class="btn" disabled={busy === "file"} onclick={() => void setUpFile()}>Set up…</button>
      </Row>
    {/if}
  </div>
  {#if fileNote}
    <p class="mt-2 px-0.5 text-[12.5px] text-warning">{fileNote}</p>
  {/if}
  {#if vault.unlock_file}
    <p class="mt-2 px-0.5 text-[12.5px] text-fg-muted">Keep a copy of the file somewhere safe. If both the file and its copies are lost, the backups can't be opened with this password.</p>
  {/if}
</section>

<section class="flex flex-col">
  <h2 class="group-title">Privacy and the app</h2>
  <div class="rows">
    <Toggle
      row
      icon="shield"
      bind:checked={() => vault.hide_sizes ?? false, (on) => (vault.hide_sizes = on)}
      label="Hide file sizes"
      hint="Pads what the Vault stores, so someone with the drive can't tell the size of a file you changed. Takes up to a few percent more space; applies to backups from now on."
    />
    <Row title="Lock it again in the app" icon="lock" subtitle="After unlocking, the app keeps the password in memory until then. Scheduled backups aren't affected.">
      <select
        class="input w-auto min-w-52"
        value={String(vault.lock_after_minutes ?? "")}
        onchange={(e) => (vault.lock_after_minutes = e.currentTarget.value ? Number(e.currentTarget.value) : null)}
        aria-label="Lock it again in the app"
      >
        {#each lockChoices as c (c.label)}<option value={String(c.value ?? "")}>{c.label}</option>{/each}
      </select>
    </Row>
  </div>
</section>

<InfoDetails summary="The encryption itself">
  <p>
    Everything in the Vault is encrypted with XChaCha20-Poly1305 using 256-bit keys, and identified with keyed BLAKE3. These are fixed, not settings: neither has a stronger alternative worth choosing. The password protects the key, so its length and the hashing above are what you can strengthen.
  </p>
  {#if security?.keys.length}
    <p class="mt-2">
      This Vault has {security.keys.length === 1 ? "one password" : `${security.keys.length} passwords`}:
      {#each security.keys as k, i (k.id)}{i ? ", " : ""}“{k.label}” ({words(k.hashing)}{k.unlock_file ? ", with the unlock file" : ""}{k.current ? ", in use" : ""}){/each}.
      Add or remove passwords with <span class="font-mono">flux key</span>.
    </p>
  {/if}
</InfoDetails>
