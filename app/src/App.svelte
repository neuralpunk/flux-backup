<script lang="ts">
  import { onMount } from "svelte";
  import { api, inTauri } from "./lib/api";
  import { app } from "./lib/state.svelte";
  import Icon from "./lib/components/Icon.svelte";
  import JobPanel from "./lib/components/JobPanel.svelte";
  import Activity from "./lib/components/Activity.svelte";
  import AllBackups from "./lib/components/AllBackups.svelte";
  import Overview from "./lib/components/Overview.svelte";
  import PauseDialog from "./lib/components/PauseDialog.svelte";
  import Preferences from "./lib/components/Preferences.svelte";
  import ProfileEditor from "./lib/components/ProfileEditor.svelte";
  import Shortcuts from "./lib/components/Shortcuts.svelte";
  import PasswordDialog from "./lib/components/PasswordDialog.svelte";
  import ProfileHeader from "./lib/components/ProfileHeader.svelte";
  import Sidebar from "./lib/components/Sidebar.svelte";
  import Snapshots from "./lib/components/Snapshots.svelte";
  import Toasts from "./lib/components/Toasts.svelte";
  import MenuHost from "./lib/components/MenuHost.svelte";
  import DeleteProfileDialog from "./lib/components/DeleteProfileDialog.svelte";
  import RecoveryKitDialog from "./lib/components/RecoveryKitDialog.svelte";
  import SystemUnlockDialog from "./lib/components/SystemUnlockDialog.svelte";
  import ExportDialog from "./lib/components/ExportDialog.svelte";
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
      if (profile) app.open(profile);
      if (tab === "overview" || tab === "snapshots" || tab === "activity" || tab === "settings") app.tab = tab;
      if (q.get("new") === "1") app.newProfile(false);
      if (q.get("new") === "system") app.newProfile(true);
      if (theme === "dark" || theme === "light" || theme === "auto") app.setTheme(theme);
      if (q.get("simulate") === "backup" && app.selectedId) void api.backupNow(app.selectedId);
      if (q.get("open") === "recovery" && app.selectedId) app.dialog = { kind: "recovery", id: app.selectedId };
      if (q.get("open") === "delete" && app.selectedId) app.dialog = { kind: "delete-profile", id: app.selectedId };
      if (q.get("open") === "system") app.dialog = { kind: "system-unlock" };
      if (q.get("open") === "export" && app.selectedId) app.dialog = { kind: "export", id: app.selectedId };
      if (q.get("open") === "pause") app.dialog = { kind: "pause" };
      if (q.get("view") === "preferences") app.openPreferences();
    });
  });

  // A view that hit a locked Vault asks for the password, then retries.
  const onLocked = (retry: () => void) => app.selected && (app.unlock = { id: app.selected.id, retry });
  const dialogProfile = $derived(app.profiles.find((p) => p.id === app.dialog?.id) ?? null);
  const unlockProfile = $derived(app.profiles.find((p) => p.id === app.unlock?.id) ?? null);

  const job = $derived(app.selectedId ? app.jobs[app.selectedId] : undefined);

  function typing(e: KeyboardEvent) {
    const t = e.target as HTMLElement | null;
    return !!t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable);
  }

  /** The webview's own right-click menu (Reload, Inspect…) means nothing here. Text that
   * can be selected keeps it, for Copy. Our own menus stop the event before it gets here. */
  function noBrowserMenu(e: MouseEvent) {
    const t = e.target as HTMLElement | null;
    if (t?.closest("input, textarea, .selectable, pre, code") || window.getSelection()?.toString()) return;
    e.preventDefault();
  }

  function onKey(e: KeyboardEvent) {
    if (document.querySelector("dialog[open]")) return; // dialogs handle their own keys
    const ctrl = e.ctrlKey || e.metaKey;
    if (ctrl && e.key.toLowerCase() === "n") {
      app.newProfile(false);
    } else if (ctrl && e.key.toLowerCase() === "b") {
      document.getElementById("backup-now")?.click();
    } else if (ctrl && e.key === "0" && app.profiles.length) {
      app.openHome();
    } else if (ctrl && ["1", "2", "3", "4"].includes(e.key) && app.selected) {
      app.open(app.selected.id, (["overview", "snapshots", "activity", "settings"] as const)[Number(e.key) - 1]!);
    } else if (e.altKey && (e.key === "ArrowUp" || e.key === "ArrowDown") && app.profiles.length) {
      const i = app.home ? -1 : app.profiles.findIndex((p) => p.id === app.selectedId);
      const next = app.profiles[(i + (e.key === "ArrowDown" ? 1 : -1) + app.profiles.length) % app.profiles.length];
      if (next) app.open(next.id);
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

<svelte:window onkeydown={onKey} oncontextmenu={noBrowserMenu} />

<div class="flex h-full flex-col bg-bg">
  <TitleBar />
  <div class="flex min-h-0 flex-1">
    <Sidebar />

    <main class="flex min-w-0 flex-1 flex-col overflow-hidden rounded-tl-panel bg-surface-1 shadow-[-1px_-1px_0_var(--c-group-edge)]">
      {#if app.loadError}
        <div class="m-8 card flex items-center gap-3 bg-danger/10 p-5 text-danger"><Icon name="alert" /> {app.loadError}</div>
      {:else if app.prefsOpen}
        <Preferences />
      {:else if app.creating}
        <div class="mx-auto w-full max-w-[760px] px-10 pt-6 pb-5">
          <h1 class="page-title flex items-center gap-3">
            {app.creatingSystem ? "New system profile" : "New backup"}
            {#if app.creatingSystem}
              <span class="tag text-warning"><Icon name="shield" size={13} /> Runs as root</span>
            {/if}
          </h1>
          <p class="mt-1 text-sm text-fg-2">
            {app.creatingSystem
              ? "Back up the whole computer's settings and folders only root can read. Hooks of system profiles run as root too."
              : "A few questions, one at a time. You can change everything later."}
          </p>
        </div>
        <div class="min-h-0 flex-1">
          <ProfileEditor
            system={app.creatingSystem}
            onsaved={(id) => app.open(id)}
            oncancel={() => (app.creating = false)}
          />
        </div>
      {:else if app.home && app.profiles.length}
        <AllBackups />
      {:else if app.selected}
        {#key app.selected.id}
          {#if app.tab === "settings"}
            <div class="mx-auto w-full max-w-[760px] px-10 pt-6"><ProfileHeader profile={app.selected} /></div>
            <div class="min-h-0 flex-1 pt-4">
              <ProfileEditor id={app.selected.id} onsaved={() => (app.tab = "overview")} oncancel={() => (app.tab = "overview")} />
            </div>
          {:else}
            <div class="flex min-h-0 flex-1 flex-col overflow-y-auto">
              <div class="mx-auto flex w-full max-w-[1180px] flex-1 flex-col gap-6 px-10 pt-6 pb-10 {app.tab !== 'overview' ? 'min-h-0' : ''}">
                <ProfileHeader profile={app.selected} />
                {#if job}
                  <JobPanel {job} />
                {/if}
                {#if app.selected.errors.length}
                  <div class="card flex gap-3 bg-danger/8 px-5 py-4 text-sm">
                    <span class="glyph glyph-bad"><Icon name="alert" size={13} /></span>
                    <div class="min-w-0 flex-1">
                      <div class="font-medium text-danger">This backup's settings have problems, so it can't run:</div>
                      <ul class="mt-1.5 list-disc pl-5 text-fg-2">
                        {#each app.selected.errors as e (e)}<li>{e}</li>{/each}
                      </ul>
                    </div>
                    <button class="btn shrink-0" onclick={() => (app.tab = "settings")}><Icon name="settings" size={14} /> Open settings</button>
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
            </div>
          {/if}
        {/key}
      {:else if !app.loading}
        <div class="grid flex-1 place-items-center p-8">
          <div class="max-w-md text-center">
            <Logo class="mx-auto size-16" />
            <h1 class="mt-6 text-[26px] font-semibold tracking-[-0.015em]">Welcome to Flux Backup</h1>
            <p class="mt-2 text-sm text-fg-2">Set up your first backup: pick what to protect, where to keep it, and how often. It takes about a minute.</p>
            <div class="mt-7 flex justify-center gap-2">
              <button class="btn btn-primary h-10 px-5" onclick={() => app.newProfile(false)}><Icon name="plus" size={15} /> Set up a backup</button>
              <button class="btn btn-ghost h-10" onclick={() => app.importProfile(false)}>Import a profile…</button>
            </div>
          </div>
        </div>
      {/if}
    </main>
  </div>
</div>

{#if app.unlock && unlockProfile}
  <PasswordDialog
    profile={unlockProfile.id}
    name={unlockProfile.name}
    onunlocked={() => {
      const r = app.unlock?.retry;
      app.unlock = null;
      r?.();
    }}
    onclose={() => (app.unlock = null)}
  />
{/if}

{#if app.dialog?.kind === "pause"}
  <PauseDialog onclose={() => (app.dialog = null)} />
{:else if app.dialog?.kind === "delete-profile" && dialogProfile}
  <DeleteProfileDialog profile={dialogProfile} onclose={() => (app.dialog = null)} />
{:else if app.dialog?.kind === "recovery" && dialogProfile}
  <RecoveryKitDialog id={dialogProfile.id} name={dialogProfile.name} onclose={() => (app.dialog = null)} />
{:else if app.dialog?.kind === "export" && dialogProfile}
  <ExportDialog id={dialogProfile.id} name={dialogProfile.name} onclose={() => (app.dialog = null)} />
{:else if app.dialog?.kind === "system-unlock"}
  <SystemUnlockDialog onclose={() => (app.dialog = null)} />
{/if}

{#if app.showShortcuts}
  <Shortcuts />
{/if}

<Toasts />
<MenuHost />
