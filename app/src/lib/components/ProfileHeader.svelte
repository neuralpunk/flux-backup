<script lang="ts">
  import { api } from "../api";
  import { app, type Tab } from "../state.svelte";
  import type { ProfileSummary } from "../types";
  import { ApiError } from "../types";
  import Icon from "./Icon.svelte";

  let { profile, onLocked }: { profile: ProfileSummary; onLocked: (retry: () => void) => void } = $props();
  let starting = $state(false);
  let stopping = $state(false);
  // Shown as a tag: "manual", "daily", "every 4h + when drive connected"… capitalised.
  const schedule = $derived(profile.schedule ? profile.schedule[0]!.toUpperCase() + profile.schedule.slice(1) : "Manual");

  async function stop() {
    stopping = true;
    try {
      await api.cancel(profile.id);
    } catch (e) {
      app.toast("bad", (e as Error).message);
      stopping = false;
    }
  }
  // "Stopping…" until the backup has actually stopped.
  $effect(() => {
    if (!profile.running) stopping = false;
  });

  const tabs: { id: Tab; label: string; key: string }[] = [
    { id: "overview", label: "Overview", key: "1" },
    { id: "snapshots", label: "Snapshots", key: "2" },
    { id: "activity", label: "Activity", key: "3" },
    { id: "settings", label: "Settings", key: "4" },
  ];

  async function backupNow() {
    starting = true;
    try {
      await api.backupNow(profile.id);
      const p = app.profiles.find((x) => x.id === profile.id);
      if (p) p.running = true;
    } catch (e) {
      if (e instanceof ApiError && e.code === "password_required") onLocked(backupNow);
      else app.toast("bad", (e as Error).message);
    } finally {
      starting = false;
    }
  }
</script>

<header class="flex flex-col gap-[22px]">
  <div class="flex items-start justify-between gap-6">
    <div class="min-w-0">
      <h1 class="truncate text-[28px] leading-tight font-medium">{profile.name}</h1>
      <div class="mt-2.5 flex flex-wrap items-center gap-2">
        <span class="tag max-w-md font-mono" title="Backup target"><span class="truncate">{profile.target}</span></span>
        <span class="tag" title="Schedule">{schedule}</span>
        <span class="tag" title={profile.engine === "vault" ? "Vault: encrypted, deduplicated" : "Mirror: plain folders"}>
          {profile.engine === "vault" ? "Encrypted" : "Plain folders"}
        </span>
        {#if profile.scope === "system"}
          <span class="tag text-warning" title="Runs as root; managed through the system helper"><Icon name="shield" size={12} /> System</span>
        {/if}
      </div>
    </div>
    <div class="flex shrink-0 gap-2">
      {#if profile.running}
        <button class="btn btn-primary h-10" onclick={stop} disabled={stopping} title="Stop; the next backup continues where this one stopped">
          {stopping ? "Stopping…" : "Stop"}
        </button>
      {:else}
        <button id="backup-now" class="btn {app.tab === 'settings' ? 'btn-tonal' : 'btn-primary'} h-10" title="Back up now (Ctrl+B)" onclick={backupNow} disabled={starting || profile.errors.length > 0}>
          {starting ? "Starting…" : "Back up now"}
        </button>
      {/if}
    </div>
  </div>

  <div class="pill flex gap-1 self-start bg-surface-2 p-1 text-[13.5px]" role="tablist">
    {#each tabs as t (t.id)}
      <button
        role="tab"
        aria-selected={app.tab === t.id}
        title="{t.label} (Ctrl+{t.key})"
        class="pill px-[18px] py-2 transition-colors {app.tab === t.id ? 'bg-primary-container font-medium text-on-primary-container' : 'text-fg-2 hover:text-fg'}"
        onclick={() => (app.tab = t.id)}
      >
        {t.label}
      </button>
    {/each}
  </div>
</header>
