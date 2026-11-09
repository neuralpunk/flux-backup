<script lang="ts">
  // The app's own preferences: what applies to every backup (or to new ones), not to one.
  // Changes save as they're made.
  import { api } from "../api";
  import { app, pauseUntil, type Theme } from "../state.svelte";
  import type { Preferences, Scheduler } from "../types";
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

  async function save(notifySuccess: boolean, defaultExclude: string[] | null, oneAtATime?: boolean) {
    try {
      prefs = await api.savePreferences(notifySuccess, defaultExclude, oneAtATime);
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

  let switching = $state<Scheduler | null>(null);
  async function setScheduler(to: Scheduler) {
    if (!prefs || (prefs.scheduler ?? "systemd") === to || switching) return;
    switching = to;
    try {
      prefs = await api.setScheduler(to);
      app.scheduler = prefs.scheduler ?? to;
      app.toast("ok", to === "cron" ? "Scheduled backups now run with cron." : "Scheduled backups now run with systemd timers.");
    } catch (e) {
      app.toast("bad", `Couldn't switch: ${(e as Error).message}`);
    } finally {
      switching = null;
    }
  }
  const schedulers: { value: Scheduler; title: string; text: string; good: string[]; mind: string }[] = [
    {
      value: "systemd",
      title: "systemd timers",
      text: "Built into Linux. Recommended.",
      good: ["Catches up on a backup missed while the computer was off", "Can back up as soon as a drive is plugged in"],
      mind: "Runs while you're logged in (unless lingering is turned on).",
    },
    {
      value: "cron",
      title: "cron",
      text: "The classic Unix scheduler, which Back In Time uses. Flux keeps its own lines in your crontab and leaves the rest alone.",
      good: ["Also runs while you're logged out", "Catches up on a missed backup: Flux checks every 15 minutes"],
      mind: "Can't start a backup when a drive is plugged in.",
    },
  ];

  async function setDiffTool(value: string) {
    if (!prefs) return;
    try {
      prefs = await api.setDiffTool(value === "auto" ? null : value);
    } catch (e) {
      app.toast("bad", (e as Error).message);
    }
  }
  const diffChoice = $derived(prefs?.diff_tool == null ? "auto" : prefs.diff_tool);
  /** A command typed into settings.toml that isn't one of the known programs. */
  const customTool = $derived(
    prefs?.diff_tool && !(prefs.diff_tools ?? []).some(([c]) => c === prefs?.diff_tool) ? prefs.diff_tool : null,
  );

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

      <section class="flex flex-col">
        <h2 class="group-title">Advanced</h2>
        <div class="rows mb-5">
          <Toggle
            row
            checked={prefs.one_at_a_time ?? false}
            onchange={(on) => prefs && void save(prefs.notify_success, prefs.default_exclude_custom ? prefs.default_exclude : null, on)}
            label="Run one backup at a time"
            hint="A backup that starts while another is running waits for it to finish, so they don't compete for the disk or the network. Stopping still works while it waits."
          />
        </div>
        <div class="px-0.5 pb-2">
          <h3 class="text-[13px] font-semibold text-fg-2">What runs scheduled backups</h3>
          <p class="text-[12.5px] text-fg-muted">Either way they run with the app closed. Switching moves every scheduled backup over.</p>
        </div>
        <div class="rows" role="radiogroup" aria-label="What runs scheduled backups">
          {#each schedulers as o (o.value)}
            {@const selected = (prefs.scheduler ?? "systemd") === o.value}
            {@const blocked = o.value === "cron" && !!prefs.cron_problem && !selected}
            <button
              role="radio"
              aria-checked={selected}
              disabled={blocked || !!switching}
              class="row row-link items-start py-3.5 disabled:cursor-not-allowed"
              onclick={() => void setScheduler(o.value)}
            >
              <Icon name={o.value === "cron" ? "terminal" : "clock"} size={17} class="mt-0.5 shrink-0 text-fg-2" />
              <span class="flex min-w-0 flex-1 flex-col gap-1">
                <span class="font-semibold">{o.title}</span>
                <span class="text-[13px] text-fg-2">{o.text}</span>
                <span class="mt-0.5 flex flex-col gap-0.5 text-[12.5px]">
                  {#each o.good as g (g)}<span class="flex items-start gap-1.5 text-fg-2"><Icon name="check" size={12} class="mt-[3px] shrink-0 text-success" /> {g}</span>{/each}
                  <span class="flex items-start gap-1.5 text-fg-muted"><Icon name="alert" size={12} class="mt-[3px] shrink-0" /> {o.mind}</span>
                </span>
                {#if blocked}
                  <span class="mt-1 text-[12.5px] text-warning">{prefs.cron_problem}</span>
                {:else if o.value === "cron" && selected && prefs.cron_running === false}
                  <span class="mt-1 text-[12.5px] text-warning">No cron daemon is running, so nothing will start. Start it with <code class="font-mono">sudo systemctl enable --now crond</code>.</span>
                {/if}
              </span>
              <span class="grid size-5 shrink-0 place-items-center">
                {#if switching === o.value}<Icon name="refresh" size={15} class="text-fg-muted motion-safe:animate-spin" />{:else if selected}<Icon name="check" size={18} class="text-fg" />{/if}
              </span>
            </button>
          {/each}
        </div>
        <p class="mt-1.5 px-0.5 text-[12.5px] text-fg-muted">
          This is for your own backups. System backups have their own setting: <code class="font-mono">sudo flux --system schedule use cron</code>.
        </p>

        <div class="rows mt-5">
          <label class="row gap-3">
            <Icon name="diff" size={17} class="shrink-0 text-fg-2" />
            <span class="min-w-0 flex-1">
              <span class="block font-medium">Compare file versions with</span>
              <span class="block text-[12.5px] text-fg-muted">
                Flux shows what changed in text files itself. A comparison program also handles other files, and is offered as “Open in …”.
                {#if !(prefs.diff_tools ?? []).length}None is installed; Meld is a good one (on Fedora, the meld package).{/if}
              </span>
            </span>
            <select class="input h-9 w-48 py-0" value={diffChoice} onchange={(e) => void setDiffTool(e.currentTarget.value)}>
              <option value="auto">{(prefs.diff_tools ?? [])[0] ? `Automatic (${prefs.diff_tools![0]![1]})` : "Automatic"}</option>
              {#each prefs.diff_tools ?? [] as [cmd, name] (cmd)}<option value={cmd}>{name}</option>{/each}
              {#if customTool}<option value={customTool}>{customTool}</option>{/if}
              <option value="">Only Flux's own view</option>
            </select>
          </label>
        </div>
      </section>
    </div>
  {/if}
</div>
</div>
