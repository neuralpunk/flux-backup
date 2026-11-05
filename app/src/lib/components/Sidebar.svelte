<script lang="ts">
  import { app, type Theme } from "../state.svelte";
  import { ago } from "../format";
  import type { ProfileSummary } from "../types";
  import Icon from "./Icon.svelte";
  import Logo from "../ui/Logo.svelte";

  /** Dot colour and tooltip. Hues only for status: running, failed, had problems. */
  function health(p: ProfileSummary): { dot: string; label: string } {
    if (p.running) return { dot: "bg-brand motion-safe:animate-pulse", label: "Backing up" };
    if (p.errors.length) return { dot: "bg-danger", label: "Profile has errors" };
    const r = p.last_run;
    if (!r) return { dot: "bg-track", label: "Never backed up" };
    if (r.status === "failed") return { dot: "bg-danger", label: `Last backup failed ${ago(r.finished)}` };
    if (r.status === "completed_with_errors") return { dot: "bg-warning", label: `Last backup had problems, ${ago(r.finished)}` };
    return { dot: "bg-track", label: `Backed up ${ago(r.finished)}` };
  }

  /** Percentage of a running backup's transfer, if known. */
  function percent(p: ProfileSummary): number | null {
    const pr = app.jobs[p.id]?.progress;
    if (!p.running || !pr || pr.bytes_total <= 0 || pr.phase !== "transferring") return null;
    return Math.min(100, Math.round((pr.bytes_done / pr.bytes_total) * 100));
  }

  const mine = $derived(app.profiles.filter((p) => p.scope !== "system"));
  const system = $derived(app.profiles.filter((p) => p.scope === "system"));

  const themes: { value: Theme; label: string }[] = [
    { value: "dark", label: "Dark" },
    { value: "light", label: "Light" },
    { value: "auto", label: "Auto" },
  ];

  function open(p: ProfileSummary) {
    app.creating = false;
    app.selectedId = p.id;
    app.tab = "overview";
  }
</script>

{#snippet row(p: ProfileSummary)}
  {@const h = health(p)}
  {@const pct = percent(p)}
  {@const active = app.selectedId === p.id && !app.creating}
  <button
    class="pill flex w-full items-center gap-3 px-4 py-3 text-left text-sm transition-colors
      {active ? 'bg-primary-container font-medium text-on-primary-container' : 'text-fg-2 hover:bg-surface-sunken hover:text-fg'}"
    aria-current={active ? "page" : undefined}
    title="{p.name} · {h.label} · {p.target}"
    onclick={() => open(p)}
  >
    <span class="size-2 shrink-0 rounded-full {h.dot}" aria-hidden="true"></span>
    <span class="min-w-0 flex-1 truncate">{p.name}</span>
    {#if pct != null}<span class="num font-mono text-xs">{pct}%</span>{/if}
    <span class="sr-only">{h.label}</span>
  </button>
{/snippet}

<aside class="flex w-[232px] shrink-0 flex-col">
  <div class="flex items-center gap-3 px-2 pt-2 pb-6">
    <Logo class="size-7" />
    <span class="text-[14px] font-semibold">Flux Backup</span>
  </div>

  <nav class="min-h-0 flex-1 space-y-1 overflow-y-auto" aria-label="Backup profiles">
    <div class="label px-2 pb-1.5">Profiles</div>
    {#each mine as p (p.id)}
      {@render row(p)}
    {/each}
    <button
      class="pill flex w-full items-center gap-3 px-4 py-3 text-left text-sm transition-colors
        {app.creating && !app.creatingSystem ? 'bg-primary-container font-medium text-on-primary-container' : 'text-fg-2 hover:bg-surface-sunken hover:text-fg'}"
      title="New profile (Ctrl+N)"
      onclick={() => app.newProfile(false)}
    >
      <Icon name="plus" size={15} class="-ml-0.5" /> New profile
    </button>

    {#if app.system.available}
      <div class="flex items-center justify-between px-2 pt-5 pb-1.5">
        <span class="label">System</span>
        {#if app.system.unlocked}
          <span class="flex items-center gap-0.5">
            <button class="pill p-1.5 text-fg-muted hover:bg-surface-sunken hover:text-fg" title="New system profile" aria-label="New system profile" onclick={() => app.newProfile(true)}>
              <Icon name="plus" size={14} />
            </button>
            <button class="pill p-1.5 text-fg-muted hover:bg-surface-sunken hover:text-fg" title="Lock system backups" aria-label="Lock system backups" onclick={() => app.lockSystem()}>
              <Icon name="lock" size={14} />
            </button>
          </span>
        {/if}
      </div>
      {#if app.system.unlocked}
        {#each system as p (p.id)}
          {@render row(p)}
        {:else}
          <p class="px-4 py-2 text-[13px] text-fg-muted">No system profiles yet. Back up <span class="font-mono">/etc</span> and folders only root can read.</p>
        {/each}
      {:else}
        <button
          class="w-full rounded-card bg-surface-sunken px-4 py-3.5 text-left transition-colors hover:bg-surface-2 disabled:opacity-60"
          onclick={() => app.unlockSystem()}
          disabled={app.unlockingSystem}
        >
          <span class="block text-sm font-medium">{app.unlockingSystem ? "Waiting for password…" : "Authenticate"}</span>
          <span class="block text-[12.5px] text-fg-muted">to manage system backups</span>
        </button>
      {/if}
    {/if}
  </nav>

  <div class="pt-3">
    <div class="pill flex bg-surface-sunken p-1" role="radiogroup" aria-label="Theme">
      {#each themes as t (t.value)}
        <button
          role="radio"
          aria-checked={app.theme === t.value}
          class="pill flex-1 py-2 text-[13px] transition-colors
            {app.theme === t.value ? 'bg-primary-container font-medium text-on-primary-container' : 'text-fg-2 hover:text-fg'}"
          onclick={() => app.setTheme(t.value)}
        >
          {t.label}
        </button>
      {/each}
    </div>
  </div>
</aside>
