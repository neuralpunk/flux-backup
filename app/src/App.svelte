<script lang="ts">
  import { onMount } from "svelte";
  import { api, inTauri } from "./lib/api";
  import { app } from "./lib/state.svelte";
  import Icon from "./lib/components/Icon.svelte";
  import JobPanel from "./lib/components/JobPanel.svelte";
  import Activity from "./lib/components/Activity.svelte";
  import Overview from "./lib/components/Overview.svelte";
  import ProfileEditor from "./lib/components/ProfileEditor.svelte";
  import Shortcuts from "./lib/components/Shortcuts.svelte";
  import PasswordDialog from "./lib/components/PasswordDialog.svelte";
  import ProfileHeader from "./lib/components/ProfileHeader.svelte";
  import Sidebar from "./lib/components/Sidebar.svelte";
  import Snapshots from "./lib/components/Snapshots.svelte";
  import Toasts from "./lib/components/Toasts.svelte";
  import Logo from "./lib/ui/Logo.svelte";
  import TitleBar from "./lib/ui/TitleBar.svelte";

  onMount(() => {
    void app.start().then(async () => {
      // Demo mode only: ?profile=…&tab=…&theme=… set the starting view (for screenshots).
      if (inTauri) return;
      const q = new URLSearchParams(location.search);
      const profile = q.get("profile");
      const tab = q.get("tab");
      const theme = q.get("theme");
      if (q.get("system") === "unlocked") await app.unlockSystem();
      if (q.get("advanced") === "1") app.editorAdvanced = true;
      if (profile) app.selectedId = profile;
      if (tab === "overview" || tab === "snapshots" || tab === "activity" || tab === "settings") app.tab = tab;
      if (q.get("new") === "1") app.newProfile(false);
      if (q.get("new") === "system") app.newProfile(true);
      if (theme === "dark" || theme === "light" || theme === "auto") app.setTheme(theme);
      if (q.get("simulate") === "backup" && app.selectedId) void api.backupNow(app.selectedId);
    });
  });

  // A view that hit a locked Vault asks for the password, then retries.
  let unlock = $state<{ retry: () => void } | null>(null);
  const onLocked = (retry: () => void) => (unlock = { retry });

  const job = $derived(app.selectedId ? app.jobs[app.selectedId] : undefined);

  function typing(e: KeyboardEvent) {
    const t = e.target as HTMLElement | null;
    return !!t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable);
  }

  function onKey(e: KeyboardEvent) {
    if (document.querySelector("dialog[open]")) return; // dialogs handle their own keys
    const ctrl = e.ctrlKey || e.metaKey;
    if (ctrl && e.key.toLowerCase() === "n") {
      app.newProfile(false);
    } else if (ctrl && e.key.toLowerCase() === "b") {
      document.getElementById("backup-now")?.click();
    } else if (ctrl && ["1", "2", "3", "4"].includes(e.key) && app.selected) {
      app.creating = false;
      app.tab = (["overview", "snapshots", "activity", "settings"] as const)[Number(e.key) - 1]!;
    } else if (e.altKey && (e.key === "ArrowUp" || e.key === "ArrowDown") && app.profiles.length) {
      const i = app.profiles.findIndex((p) => p.id === app.selectedId);
      const next = app.profiles[(i + (e.key === "ArrowDown" ? 1 : -1) + app.profiles.length) % app.profiles.length];
      if (next) (app.selectedId = next.id), (app.creating = false);
    } else if (!typing(e) && e.key === "?") {
      app.showShortcuts = true;
    } else if (!typing(e) && e.key === "/" && document.getElementById("browser-filter")) {
      document.getElementById("browser-filter")?.focus();
    } else {
      return;
    }
    e.preventDefault();
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="flex h-full flex-col bg-bg">
  <TitleBar />
  <div class="flex min-h-0 flex-1 gap-3 px-3 pb-3">
    <Sidebar />

    <main class="flex min-w-0 flex-1 flex-col overflow-hidden rounded-panel bg-surface-1">
      {#if app.loadError}
        <div class="m-8 card flex items-center gap-3 bg-danger/10 p-5 text-danger"><Icon name="alert" /> {app.loadError}</div>
      {:else if app.creating}
        <div class="px-8 pt-7 pb-5">
          <h1 class="flex items-center gap-3 text-[28px] font-medium tracking-tight">
            {app.creatingSystem ? "New system profile" : "New backup"}
            {#if app.creatingSystem}
              <span class="tag text-warning"><Icon name="shield" size={13} /> Runs as root</span>
            {/if}
          </h1>
          <p class="mt-1 text-sm text-fg-2">
            {app.creatingSystem
              ? "Back up the whole computer's settings and folders only root can read. Hooks of system profiles run as root too."
              : "Three questions: what to back up, where to, and when. You can change everything later."}
          </p>
        </div>
        <div class="min-h-0 flex-1">
          <ProfileEditor
            system={app.creatingSystem}
            onsaved={(id) => {
              app.creating = false;
              app.selectedId = id;
              app.tab = "overview";
            }}
            oncancel={() => (app.creating = false)}
          />
        </div>
      {:else if app.selected}
        {#key app.selected.id}
          {#if app.tab === "settings"}
            <div class="px-8 pt-7"><ProfileHeader profile={app.selected} {onLocked} /></div>
            <div class="min-h-0 flex-1 pt-4">
              <ProfileEditor id={app.selected.id} onsaved={() => (app.tab = "overview")} oncancel={() => (app.tab = "overview")} />
            </div>
          {:else}
            <div class="flex min-h-0 flex-1 flex-col gap-[22px] overflow-y-auto px-8 py-7">
              <ProfileHeader profile={app.selected} {onLocked} />
              {#if job}
                <JobPanel {job} />
              {/if}
              {#if app.selected.errors.length}
                <div class="card bg-danger/10 p-5 text-sm">
                  <div class="font-medium text-danger">This profile has problems and can't run:</div>
                  <ul class="mt-2 list-disc pl-5 text-fg-2">
                    {#each app.selected.errors as e (e)}<li>{e}</li>{/each}
                  </ul>
                  <button class="btn mt-3" onclick={() => (app.tab = "settings")}><Icon name="settings" size={14} /> Fix in Settings</button>
                </div>
              {/if}
              <div class={app.tab !== "overview" ? "flex min-h-0 flex-1 flex-col" : ""}>
                {#if app.tab === "overview"}
                  <Overview id={app.selected.id} {onLocked} />
                {:else if app.tab === "snapshots"}
                  <Snapshots id={app.selected.id} {onLocked} />
                {:else}
                  <Activity id={app.selected.id} />
                {/if}
              </div>
            </div>
          {/if}
        {/key}
      {:else if !app.loading}
        <div class="grid flex-1 place-items-center p-8">
          <div class="max-w-md text-center">
            <Logo class="mx-auto size-16" />
            <h1 class="mt-6 text-[28px] font-medium tracking-tight">Welcome to Flux Backup</h1>
            <p class="mt-2 text-sm text-fg-2">Set up your first backup: pick what to protect, where to keep it, and how often. It takes about a minute.</p>
            <button class="btn btn-primary mt-7 h-11 px-6" onclick={() => app.newProfile(false)}><Icon name="plus" size={15} /> Set up a backup</button>
          </div>
        </div>
      {/if}
    </main>
  </div>
</div>

{#if unlock && app.selected}
  <PasswordDialog
    profile={app.selected.id}
    name={app.selected.name}
    onunlocked={() => {
      const r = unlock?.retry;
      unlock = null;
      r?.();
    }}
    onclose={() => (unlock = null)}
  />
{/if}

{#if app.showShortcuts}
  <Shortcuts />
{/if}

<Toasts />
