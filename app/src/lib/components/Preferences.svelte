<script lang="ts">
  // The app's own preferences: what applies to every backup (or to new ones), not to one.
  // Changes save as they're made.
  import { api } from "../api";
  import { app, pauseUntil, type Theme } from "../state.svelte";
  import { clock, clockTime, setClock, type Clock } from "../clock.svelte";
  import type { Preferences, Scheduler } from "../types";
  import Icon from "./Icon.svelte";
  import Callout from "./Callout.svelte";
  import Toggle from "./form/Toggle.svelte";
  import ListEditor from "./form/ListEditor.svelte";
  import WebhookList from "./WebhookList.svelte";
  import InfoDetails from "./InfoDetails.svelte";

  let prefs = $state<Preferences | null>(null);
  let error = $state<string | null>(null);
  let autostart = $state<boolean | null>(null);
  /** Is the app's icon shown in a system tray (`null`: can't tell)? */
  let tray = $state<boolean | null>(null);
  let excludes = $state<string[]>([]);
  /** Advanced starts open only when something in it was changed, or cron needs attention. */
  let advancedOpen = $state(false);

  $effect(() => {
    api.preferences().then(
      (p) => {
        prefs = p;
        excludes = [...p.default_exclude];
        advancedOpen = p.scheduler === "cron" || !!p.one_at_a_time || p.diff_tool != null || p.default_exclude_custom;
      },
      (e) => (error = (e as Error).message),
    );
    api.autostart().then(
      (a) => (autostart = a),
      () => (autostart = null),
    );
    api.trayAvailable().then(
      (t) => (tray = t),
      () => (tray = null),
    );
  });

  async function setCloseToTray(on: boolean) {
    try {
      prefs = await api.setCloseToTray(on);
    } catch (e) {
      app.toast("bad", `Couldn't change that: ${(e as Error).message}`);
    }
  }

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
      text: "The classic Unix scheduler, as Back In Time uses. Flux leaves the rest of your crontab alone.",
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
  const clocks: { value: Clock; label: string }[] = [
    { value: "12", label: "12-hour" },
    { value: "24", label: "24-hour" },
    { value: "auto", label: "Like the system" },
  ];
</script>

<div class="min-h-0 flex-1 overflow-y-auto px-10 pt-6 pb-12">
  <div class="mx-auto max-w-[680px]">
  <h1 class="page-title">Preferences</h1>
  <p class="mt-1 text-sm text-fg-2">For the app and every backup. Each backup has its own Settings too.</p>

  {#if error}
    <Callout tone="bad">{error}</Callout>
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
              <span class="block text-[12.5px] text-fg-muted">Pause them for a presentation or a slow connection. Backing up by hand still works.</span>
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
            hint="Failures and skipped files are always notified."
          />
        </div>
        <h3 class="mt-5 px-0.5 text-[13px] font-semibold text-fg-2">Webhooks</h3>
        <p class="mb-2 px-0.5 text-[12.5px] text-fg-muted">Tell your phone, a chat or a monitoring service how backups go.</p>
        <WebhookList
          hooks={prefs.webhooks ?? []}
          onchange={async (h) => {
            try {
              prefs = await api.setWebhooks(h);
            } catch (e) {
              app.toast("bad", (e as Error).message);
            }
          }}
        />
      </section>

      <section class="flex flex-col">
        <h2 class="group-title">App</h2>
        <div class="rows">
          <div class="row gap-3">
            <span class="min-w-0 flex-1 font-medium">Appearance</span>
            <div class="segmented" role="radiogroup" aria-label="Theme">
              {#each themes as t (t.value)}
                <button role="radio" aria-checked={app.theme === t.value} onclick={() => app.setTheme(t.value)}>
                  <Icon name={t.icon} size={14} /> {t.label}
                </button>
              {/each}
            </div>
          </div>
          <div class="row gap-3">
            <span class="min-w-0 flex-1">
              <span class="block font-medium">Clock</span>
              <span class="block text-[12.5px] text-fg-muted">How times are shown and typed: {clockTime(20 * 60 + 30)}.</span>
            </span>
            <div class="segmented" role="radiogroup" aria-label="Clock">
              {#each clocks as c (c.value)}
                <button role="radio" aria-checked={clock.pref === c.value} onclick={() => setClock(c.value)}>{c.label}</button>
              {/each}
            </div>
          </div>
          <Toggle
            row
            checked={(prefs.close_to_tray ?? true) && tray !== false}
            onchange={(on) => void setCloseToTray(on)}
            disabled={tray !== true}
            reason={tray === false ? "No system tray on this desktop, so closing the window quits." : "Not available here."}
            label="Keep running in the tray when the window is closed"
            hint="Its icon shows backups as they run; quit from the tray's menu."
          />
          <Toggle
            row
            checked={autostart ?? false}
            onchange={(on) => void setAutostart(on)}
            disabled={autostart == null}
            reason="Not available here."
            label="Start in the tray when I log in"
            hint="Scheduled backups run without the app either way."
          />
        </div>
      </section>

      <details class="group/adv flex flex-col" open={advancedOpen}>
        <summary class="flex cursor-pointer list-none items-center gap-1.5 px-0.5 pb-2 select-none [&::-webkit-details-marker]:hidden">
          <Icon name="chevronRight" size={14} class="text-fg-muted transition-transform group-open/adv:rotate-90" />
          <span class="text-[13px] font-semibold text-fg-2">Advanced</span>
          <span class="text-[12.5px] text-fg-muted">Default exclusions, the scheduler, comparing files</span>
        </summary>
        <div class="flex flex-col gap-7 pt-2">
          <section class="flex flex-col">
            <div class="flex items-baseline justify-between px-0.5 pb-2">
              <h3 class="text-[13px] font-semibold text-fg-2">Excluded from new backups</h3>
              {#if prefs.default_exclude_custom}
                <button class="text-[13px] text-primary hover:underline" onclick={() => prefs && void save(prefs.notify_success, null)}>Back to Flux's list</button>
              {/if}
            </div>
            <ListEditor bind:items={excludes} placeholder="~/Downloads/  or  *.iso" icon="filter" label="Left out by new backups" />
            <p class="mt-1.5 px-0.5 text-[12.5px] text-fg-muted">Existing backups keep their own list.</p>
          </section>

          <div class="rows">
            <Toggle
              row
              checked={prefs.one_at_a_time ?? false}
              onchange={(on) => prefs && void save(prefs.notify_success, prefs.default_exclude_custom ? prefs.default_exclude : null, on)}
              label="Run one backup at a time"
              hint="A backup that comes due while another runs waits for it, so they don't compete for the disk or network."
            />
          </div>

          <section class="flex flex-col">
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
                  class="row row-link items-start disabled:cursor-not-allowed"
                  onclick={() => void setScheduler(o.value)}
                >
                  <Icon name={o.value === "cron" ? "terminal" : "clock"} size={17} class="mt-0.5 shrink-0 text-fg-2" />
                  <span class="flex min-w-0 flex-1 flex-col">
                    <span class="font-medium">{o.title}</span>
                    <span class="text-[12.5px] text-fg-muted">{o.text}</span>
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
            <div class="mt-2">
              <InfoDetails summary="What's the difference?">
                <div class="flex flex-col gap-3">
                  {#each schedulers as o (o.value)}
                    <div>
                      <span class="font-medium text-fg">{o.title}</span>
                      <span class="mt-1 flex flex-col gap-0.5 text-[12.5px]">
                        {#each o.good as g (g)}<span class="flex items-start gap-1.5"><Icon name="check" size={12} class="mt-[3px] shrink-0 text-success" /> {g}</span>{/each}
                        <span class="flex items-start gap-1.5 text-fg-muted"><Icon name="alert" size={12} class="mt-[3px] shrink-0" /> {o.mind}</span>
                      </span>
                    </div>
                  {/each}
                  <p class="text-[12.5px] text-fg-muted">This is for your own backups. System backups have their own setting: <code class="font-mono">sudo flux --system schedule use cron</code>.</p>
                </div>
              </InfoDetails>
            </div>
          </section>

          <div class="rows">
            <label class="row gap-3">
              <Icon name="diff" size={17} class="shrink-0 text-fg-2" />
              <span class="min-w-0 flex-1">
                <span class="block font-medium">Compare file versions with</span>
                <span class="block text-[12.5px] text-fg-muted">
                  For files Flux can't compare itself, and for “Open in …”.
                  {#if !(prefs.diff_tools ?? []).length}None is installed; Meld is a good one.{/if}
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
        </div>
      </details>
    </div>
  {/if}

  <!-- Shown even while the preferences load or fail: the version is known at build time. -->
  <div class="mt-10 flex items-center justify-center gap-2 text-[12.5px] text-fg-muted">
    <span class="num">Flux Backup {__APP_VERSION__}</span>
    <span aria-hidden="true">·</span>
    <button class="font-medium text-fg-2 underline-offset-2 hover:text-fg hover:underline" onclick={() => (app.dialog = { kind: "about" })}>About</button>
  </div>
</div>
</div>
