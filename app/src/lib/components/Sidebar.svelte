<script lang="ts">
  import { app, pauseUntil } from "../state.svelte";
  import { contextMenu, openMenuBelow } from "../menu.svelte";
  import { profileMenu } from "../profileMenu";
  import { protection, type Tone } from "../protection";
  import type { ProfileSummary } from "../types";
  import type { Snippet } from "svelte";
  import Icon from "./Icon.svelte";

  /** Percentage of a running backup's transfer, if known. */
  function percent(p: ProfileSummary): number | null {
    const pr = app.jobs[p.id]?.progress;
    if (!p.running || !pr || pr.bytes_total <= 0 || pr.phase !== "transferring") return null;
    return Math.min(100, Math.round((pr.bytes_done / pr.bytes_total) * 100));
  }

  const glyphClass: Record<Tone, string> = { ok: "glyph-ok", warn: "glyph-warn", bad: "glyph-bad", run: "glyph-run", idle: "" };
  const glyphIcon: Record<Tone, "check" | "alert" | "refresh" | "clock"> = { ok: "check", warn: "alert", bad: "alert", run: "refresh", idle: "clock" };
  const toneText: Record<Tone, string> = { ok: "text-fg-muted", warn: "text-warning", bad: "text-danger", run: "text-primary", idle: "text-fg-muted" };

  const mine = $derived(app.profiles.filter((p) => p.scope !== "system"));
  const system = $derived(app.profiles.filter((p) => p.scope === "system"));
  const all = $derived(app.profiles.map((p) => protection(p, app.statuses[p.id])));
  const needing = $derived(all.filter((x) => !x.protected && x.tone !== "run").length);
  // A problem outranks a running backup: it stays visible until it's dealt with.
  const homeTone = $derived<Tone>(all.some((x) => x.tone === "bad") ? "bad" : needing || all.some((x) => x.tone === "warn") ? "warn" : app.profiles.some((p) => p.running) ? "run" : "ok");

  const onBackup = (id: string) => !app.home && app.selectedId === id && !app.creating && !app.prefsOpen;
  const itemBase = "flex w-full items-center gap-3 rounded-lg px-2.5 py-2 text-left transition-colors";
  const itemState = (on: boolean) => (on ? "bg-selected text-on-selected" : "text-fg hover:bg-surface-sunken");
</script>

{#snippet row(p: ProfileSummary)}
  {@const s = protection(p, app.statuses[p.id], percent(p), app.jobs[p.id]?.progress?.phase === "waiting")}
  {@const on = onBackup(p.id)}
  {@const fresh = app.unseenFailure(p)}
  <button
    class="{itemBase} {itemState(on)}"
    aria-current={on ? "page" : undefined}
    title="{p.name}: {s.short}. Right-click for more."
    onclick={() => app.open(p.id)}
    use:contextMenu={{ items: () => profileMenu(p), label: p.name }}
  >
    <span class="glyph {glyphClass[s.tone]}">
      <Icon name={glyphIcon[s.tone]} size={13} class={s.tone === "run" ? "motion-safe:animate-spin [animation-duration:2.4s]" : ""} />
    </span>
    <span class="min-w-0 flex-1">
      <span class="block truncate text-[14px] font-medium">{p.name}</span>
      <span class="num block truncate text-[12.5px] {toneText[s.tone]}">{s.short}</span>
    </span>
    {#if fresh}
      <span class="size-2 shrink-0 rounded-full bg-danger" title="New problem: open it to see what happened"></span>
      <span class="sr-only">New problem</span>
    {/if}
  </button>
{/snippet}

{#snippet sectionHead(label: string, actions: Snippet)}
  <div class="flex items-center justify-between pt-5 pr-1 pb-1 pl-2.5">
    <h2 class="text-[12.5px] font-semibold text-fg-muted">{label}</h2>
    <span class="flex items-center">{@render actions()}</span>
  </div>
{/snippet}

<aside class="flex w-[264px] shrink-0 flex-col px-2.5 pb-2.5">
  <nav class="min-h-0 flex-1 overflow-y-auto" aria-label="Backups">
    {#if app.profiles.length}
      <button class="{itemBase} {itemState(app.home)}" aria-current={app.home ? "page" : undefined} onclick={() => app.openHome()}>
        <span class="glyph {glyphClass[homeTone]}"><Icon name={homeTone === "ok" ? "shieldCheck" : homeTone === "run" ? "refresh" : "alert"} size={13} class={homeTone === "run" ? "motion-safe:animate-spin [animation-duration:2.4s]" : ""} /></span>
        <span class="min-w-0 flex-1">
          <span class="block truncate text-[14px] font-medium">All backups</span>
          <span class="block truncate text-[12.5px] {toneText[homeTone === 'run' ? 'ok' : homeTone]}">
            {needing ? `${needing} ${needing === 1 ? "needs" : "need"} attention` : app.profiles.length === 1 ? "Protected" : "All protected"}
          </span>
        </span>
      </button>
    {/if}

    {#snippet mineActions()}
      <button
        class="btn-icon size-7"
        title="New backup or import"
        aria-label="New backup or import"
        aria-haspopup="menu"
        onclick={(e) =>
          openMenuBelow(
            e.currentTarget,
            [
              { label: "New backup", icon: "plus", shortcut: "Ctrl+N", onselect: () => app.newProfile(false) },
              { label: "Import a profile…", icon: "folder", onselect: () => void app.importProfile(false) },
              { label: "Recover a backup…", icon: "drive", onselect: () => (app.dialog = { kind: "recover" }) },
            ],
            "New backup or import",
          )}
      >
        <Icon name="plus" size={15} />
      </button>
    {/snippet}
    {@render sectionHead("Your backups", mineActions)}
    <div class="space-y-0.5">
      {#each mine as p (p.id)}
        {@render row(p)}
      {/each}
      <button class="{itemBase} {app.creating && !app.creatingSystem ? 'bg-selected text-on-selected' : 'text-fg-2 hover:bg-surface-sunken hover:text-fg'}" title="Set up a new backup (Ctrl+N)" onclick={() => app.newProfile(false)}>
        <span class="glyph"><Icon name="plus" size={13} /></span>
        <span class="text-[14px] font-medium">New backup</span>
      </button>
    </div>

    {#if app.system.available}
      {#if app.system.unlocked}
        {#snippet sysActions()}
          <button class="btn-icon size-7" title="New system backup" aria-label="New system backup" onclick={() => app.newProfile(true)}><Icon name="plus" size={15} /></button>
          <button class="btn-icon size-7" title="Lock system backups (stops the administrator helper)" aria-label="Lock system backups" onclick={() => app.lockSystem()}><Icon name="lock" size={14} /></button>
        {/snippet}
        {@render sectionHead("System backups", sysActions)}
        <div class="space-y-0.5">
          {#each system as p (p.id)}
            {@render row(p)}
          {:else}
            <p class="px-2.5 py-1 text-[12.5px] text-fg-muted">None yet. Use + to back up <span class="font-mono">/etc</span> and other files only the administrator can read.</p>
          {/each}
        </div>
      {:else}
        <div class="pt-5">
          <button
            class="{itemBase} text-fg-2 hover:bg-surface-sunken hover:text-fg disabled:opacity-60"
            title="Back up the whole computer (needs an administrator password)"
            onclick={() => (app.dialog = { kind: "system-unlock" })}
            disabled={app.unlockingSystem}
          >
            <span class="glyph"><Icon name="shield" size={13} /></span>
            <span class="min-w-0 flex-1">
              <span class="block text-[14px] font-medium text-fg">System backups</span>
              <span class="block truncate text-[12.5px] text-fg-muted">{app.unlockingSystem ? "Waiting for the password…" : "Locked · needs an admin password"}</span>
            </span>
            <Icon name="chevronRight" size={15} class="shrink-0 text-fg-muted" />
          </button>
        </div>
      {/if}
    {/if}
  </nav>

  <div class="flex flex-col gap-1 pt-3">
    {#if app.pause}
      <div class="mb-1 flex items-center gap-2.5 rounded-lg bg-warning/12 px-2.5 py-2" role="status">
        <span class="glyph glyph-warn"><Icon name="pause" size={12} /></span>
        <span class="min-w-0 flex-1 text-[12.5px] leading-snug">
          <span class="block font-medium text-fg">Schedules paused</span>
          <span class="block text-fg-2">{pauseUntil(app.pause)}</span>
        </span>
        <button class="btn h-7 shrink-0 px-2.5 text-[12.5px]" onclick={() => app.setPause("off")}>Resume</button>
      </div>
    {/if}
    <button class="{itemBase} {app.prefsOpen ? 'bg-selected text-on-selected' : 'text-fg-2 hover:bg-surface-sunken hover:text-fg'}" aria-current={app.prefsOpen ? "page" : undefined} onclick={() => app.openPreferences()}>
      <span class="glyph"><Icon name="settings" size={13} /></span>
      <span class="text-[14px] font-medium">Preferences</span>
    </button>
  </div>
</aside>
