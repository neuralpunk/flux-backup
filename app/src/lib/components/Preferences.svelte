<script lang="ts">
  // The app's own preferences: what applies to every backup (or to new ones), not to one.
  // Changes save as they're made.
  import { api } from "../api";
  import { app, pauseUntil, type Theme } from "../state.svelte";
  import type { Preferences } from "../types";
  import Icon from "./Icon.svelte";
  import Toggle from "./form/Toggle.svelte";
  import ListEditor from "./form/ListEditor.svelte";

  let prefs = $state<Preferences | null>(null);
  let error = $state<string | null>(null);
  let autostart = $state<boolean | null>(null);
  let excludes = $state<string[]>([]);

  $effect(() => {
    api.preferences().then(
      (p) => {
        prefs = p;
        excludes = [...p.default_exclude];
      },
      (e) => (error = (e as Error).message),
    );
    api.autostart().then(
      (a) => (autostart = a),
      () => (autostart = null),
    );
  });

  async function save(notifySuccess: boolean, defaultExclude: string[] | null) {
    try {
      prefs = await api.savePreferences(notifySuccess, defaultExclude);
      excludes = [...prefs.default_exclude];
    } catch (e) {
      app.toast("bad", (e as Error).message);
    }
  }

  // The exclusion list saves when it changes (and differs from what's saved).
  $effect(() => {
    const list = excludes;
    if (!prefs || JSON.stringify(list) === JSON.stringify(prefs.default_exclude)) return;
    const t = setTimeout(() => prefs && void save(prefs.notify_success, [...list]), 400);
    return () => clearTimeout(t);
  });

  async function setAutostart(on: boolean) {
    try {
      await api.setAutostart(on);
      autostart = on;
    } catch (e) {
      app.toast("bad", `Couldn't change that: ${(e as Error).message}`);
    }
  }

  const themes: { value: Theme; label: string; icon: "moon" | "sun" | "monitor" }[] = [
    { value: "dark", label: "Dark", icon: "moon" },
    { value: "light", label: "Light", icon: "sun" },
    { value: "auto", label: "Like the system", icon: "monitor" },
  ];
</script>

<div class="min-h-0 flex-1 overflow-y-auto px-10 pt-6 pb-12">
  <div class="mx-auto max-w-[680px]">
  <h1 class="page-title">Preferences</h1>
  <p class="mt-1 text-sm text-fg-2">For the app and every backup. Each backup's own settings are behind its Settings button.</p>

  {#if error}
    <div class="mt-6 card flex items-center gap-3 bg-danger/10 p-5 text-danger"><Icon name="alert" /> {error}</div>
  {:else if !prefs}
    <div class="mt-6 h-64 card motion-safe:animate-pulse"></div>
  {:else}
    <div class="mt-7 flex flex-col gap-7">
      <section class="flex flex-col">
        <h2 class="group-title">Scheduled backups</h2>
        <div class="card flex flex-col gap-3 px-5 py-4">
          {#if app.pause}
            <div class="flex items-center gap-3">
              <span class="glyph glyph-warn size-9"><Icon name="pause" size={16} /></span>
              <span class="min-w-0 flex-1">
                <span class="block text-[14px] font-medium">Paused {pauseUntil(app.pause)}</span>
                <span class="block text-[12.5px] text-fg-muted">Backing up by hand still works. Backups that come due meanwhile are skipped, not queued.</span>
              </span>
              <button class="btn" onclick={() => app.setPause("off")}>Resume now</button>
            </div>
          {:else}
            <div>
              <span class="block text-[14px] font-medium">Running as scheduled</span>
              <span class="block text-[12.5px] text-fg-muted">Pause them for a presentation, a slow connection or a full disk. Backing up by hand still works.</span>
            </div>
            <div class="flex flex-wrap gap-2">
              <button class="btn" onclick={() => app.setPause("hour")}>Pause for an hour</button>
              <button class="btn" onclick={() => app.setPause("tomorrow")}>Until tomorrow morning</button>
              <button class="btn" onclick={() => app.setPause("resumed")}>Until I resume them</button>
            </div>
          {/if}
        </div>
      </section>

      <section class="flex flex-col">
        <h2 class="group-title">Notifications</h2>
        <div class="rows">
          <Toggle
            row
            checked={prefs.notify_success}
            onchange={(on) => prefs && void save(on, prefs.default_exclude_custom ? prefs.default_exclude : null)}
            label="Also when a scheduled backup succeeds"
            hint="Failures and skipped files are always notified, for backups with notifications on (Settings › Options)."
          />
        </div>
      </section>

      <section class="flex flex-col">
        <h2 class="group-title">Starting</h2>
        <div class="rows">
          <Toggle
            row
            checked={autostart ?? false}
            onchange={(on) => void setAutostart(on)}
            disabled={autostart == null}
            reason="Not available here."
            label="Start in the tray when I log in"
            hint="Shows how your backups are doing in the system tray. Scheduled backups run without the app either way."
          />
        </div>
      </section>

      <section class="flex flex-col">
        <h2 class="group-title">Appearance</h2>
        <div class="segmented self-start" role="radiogroup" aria-label="Theme">
          {#each themes as t (t.value)}
            <button
              role="radio"
              aria-checked={app.theme === t.value}
              onclick={() => app.setTheme(t.value)}
            >
              <Icon name={t.icon} size={14} /> {t.label}
            </button>
          {/each}
        </div>
      </section>

      <section class="flex flex-col">
        <div class="flex items-baseline justify-between px-0.5 pb-2">
          <h2 class="text-[13px] font-semibold text-fg-2">What new backups leave out</h2>
          {#if prefs.default_exclude_custom}
            <button class="text-[13px] text-primary hover:underline" onclick={() => prefs && void save(prefs.notify_success, null)}>Back to Flux's list</button>
          {/if}
        </div>
        <ListEditor bind:items={excludes} placeholder="~/Downloads/  or  *.iso" icon="filter" label="Left out by new backups" />
        <p class="mt-1.5 px-0.5 text-[12.5px] text-fg-muted">New backups start with these exclusions; each backup can change its own. Existing backups aren't changed.</p>
      </section>
    </div>
  {/if}
</div>
</div>
