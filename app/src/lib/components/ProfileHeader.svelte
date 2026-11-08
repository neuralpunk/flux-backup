<script lang="ts">
  import { app, type Tab } from "../state.svelte";
  import { profileMenu } from "../profileMenu";
  import type { ProfileSummary } from "../types";
  import Icon, { type IconName } from "./Icon.svelte";
  import MenuButton from "./MenuButton.svelte";

  let { profile }: { profile: ProfileSummary } = $props();
  // Shown as a tag: "manual", "daily", "every 4h + when drive connected"… capitalised.
  const schedule = $derived(profile.schedule ? profile.schedule[0]!.toUpperCase() + profile.schedule.slice(1) : "Manual");
  /** The destination, short: a drive's folder name, or user@host for a server. */
  const where = $derived.by(() => {
    const t = profile.target;
    if (profile.remote) return t.replace(/^ssh:\/\//, "").split("/")[0] || t;
    return t.split("/").filter(Boolean).at(-1) ?? t;
  });
  const starting = $derived(!!app.starting[profile.id]);
  const stopping = $derived(!!app.stopping[profile.id]);
  const editing = $derived(app.tab === "settings");

  // Using a backup; its settings are a page of their own (the gear button), not a tab.
  const tabs: { id: Tab; label: string; key: string; icon: IconName }[] = [
    { id: "overview", label: "Overview", key: "1", icon: "shieldCheck" },
    { id: "snapshots", label: "Snapshots", key: "2", icon: "history" },
    { id: "activity", label: "Activity", key: "3", icon: "activity" },
  ];

  const engineTag: Record<ProfileSummary["engine"], { icon: IconName; text: string; title: string; accent?: boolean }> = {
    mirror: { icon: "folder", text: "Plain folders", title: "Stored as plain folders you can open in any file manager (Mirror)" },
    vault: { icon: "lock", text: "Encrypted", title: "Stored encrypted, compressed and deduplicated (Vault)" },
    rsync: { icon: "lifebuoy", text: "rsync · works without Flux", title: "Compatibility mode: plain folders written by rsync", accent: true },
    borg: { icon: "lifebuoy", text: "Borg · works without Flux", title: "Compatibility mode: a standard Borg repository", accent: true },
  };
</script>

<header class="flex flex-col gap-5">
  {#if editing}
    <div class="flex items-center justify-between gap-6">
      <div class="min-w-0">
        <button class="btn btn-ghost -ml-2.5 h-7 px-2 text-[13px]" onclick={() => (app.tab = "overview")}>
          <Icon name="chevronLeft" size={15} /> {profile.name}
        </button>
        <h1 class="page-title mt-1">Settings</h1>
      </div>
      <MenuButton items={() => profileMenu(profile)} label="More for “{profile.name}”" />
    </div>
  {:else}
    <div class="flex items-start justify-between gap-6">
      <div class="min-w-0">
        <h1 class="page-title truncate">{profile.name}</h1>
        <div class="mt-2.5 flex flex-wrap items-center gap-1.5" aria-label="About this backup">
          <span class="tag max-w-72" title="Backs up to {profile.target}"><Icon name={profile.remote ? "server" : "drive"} size={13} /><span class="truncate">{where}</span></span>
          <span class="tag" title="When it runs"><Icon name="clock" size={13} />{schedule}</span>
          {#if engineTag[profile.engine]}
            {@const e = engineTag[profile.engine]}
            <span class="tag {e.accent ? 'tag-accent' : ''}" title={e.title}><Icon name={e.icon} size={13} />{e.text}</span>
          {/if}
          {#if profile.scope === "system"}
            <span class="tag tag-warn" title="Runs as root; managed through the administrator helper"><Icon name="shield" size={13} />System · runs as root</span>
          {/if}
        </div>
      </div>
      <div class="flex shrink-0 items-center gap-2">
        <button class="btn" title="Settings (Ctrl+4)" onclick={() => (app.tab = "settings")}><Icon name="settings" size={15} /> Settings</button>
        <MenuButton items={() => profileMenu(profile)} label="More for “{profile.name}”" />
        {#if profile.running}
          <button class="btn btn-primary" onclick={() => app.stop(profile.id)} disabled={stopping} title="Stop; the next backup continues where this one stopped">
            <Icon name="stop" size={14} /> {stopping ? "Stopping…" : "Stop"}
          </button>
        {:else}
          <button
            id="backup-now"
            class="btn btn-primary"
            title={profile.errors.length ? "Fix this backup's settings first" : "Back up now (Ctrl+B)"}
            onclick={() => app.backupNow(profile.id)}
            disabled={starting || profile.errors.length > 0}
          >
            <Icon name="play" size={14} /> {starting ? "Starting…" : "Back up now"}
          </button>
        {/if}
      </div>
    </div>

    <div class="segmented self-start" role="tablist" aria-label="{profile.name}: views">
      {#each tabs as t (t.id)}
        <button role="tab" aria-selected={app.tab === t.id} title="{t.label} (Ctrl+{t.key})" onclick={() => (app.tab = t.id)}>
          <Icon name={t.icon} size={15} />
          {t.label}
        </button>
      {/each}
    </div>
  {/if}
</header>
