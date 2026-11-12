<script lang="ts">
  import { app, pauseUntil } from "../state.svelte";
  import { contextMenu, openMenuBelow } from "../menu.svelte";
  import { profileMenu } from "../profileMenu";
  import { protection, type Tone } from "../protection";
  import { counts, sideLabel, summary } from "../attention";
  import { backupIcon } from "../backupIcon";
  import { tip } from "../tip";
  import StatusGlyph from "./StatusGlyph.svelte";
  import type { ProfileSummary } from "../types";
  import type { Snippet } from "svelte";
  import Icon from "./Icon.svelte";

  /** Percentage of a running backup's transfer, if known. */
  function percent(p: ProfileSummary): number | null {
    const pr = app.jobs[p.id]?.progress;
    if (!p.running || !pr || pr.bytes_total <= 0 || pr.phase !== "transferring") return null;
    return Math.min(100, Math.round((pr.bytes_done / pr.bytes_total) * 100));
  }

  // Red only for what isn't protected; something worth a look stays quiet (an amber dot,
  // ordinary text), so the serious stands out.
  const toneText: Record<Tone, string> = { ok: "text-fg-muted", warn: "text-fg-2", bad: "text-danger", run: "text-primary", idle: "text-fg-muted" };
  /** Folded, the status is a dot on the backup's icon. */
  const toneDot: Record<Tone, string> = { ok: "bg-success", warn: "bg-warning", bad: "bg-danger", run: "bg-primary motion-safe:animate-pulse", idle: "bg-fg-muted" };

  const mine = $derived(app.profiles.filter((p) => p.scope !== "system"));
  const system = $derived(app.profiles.filter((p) => p.scope === "system"));
  const all = $derived(app.profiles.map((p) => protection(p, app.statuses[p.id])));
  const tally = $derived(counts(all));
  // A problem outranks a running backup: it stays visible until it's dealt with.
  const homeTone = $derived<Tone>(tally.notProtected ? "bad" : tally.toLook ? "warn" : app.profiles.some((p) => p.running) ? "run" : "ok");

  const onBackup = (id: string) => !app.home && app.selectedId === id && !app.creating && !app.prefsOpen;
  // Compact: a backup is one line (name and a word or two of status), so more fit.
  const compact = $derived(app.layout === "compact");
  // Folded to a column of icons: each item keeps its place and height, its words fade
  // and the column narrows, so the icons stay where they were.
  const folded = $derived(app.sidebarFolded);
  const itemBase = $derived(`flex w-full items-center overflow-hidden whitespace-nowrap rounded-lg px-2.5 text-left transition-colors ${compact ? "gap-2.5 py-1.5" : "gap-3 py-2"}`);
  const itemState = (on: boolean) => (on ? "bg-selected text-on-selected" : "text-fg hover:bg-surface-sunken");
  const words = $derived(`flex min-w-0 flex-1 items-center gap-2.5 transition-opacity duration-150 motion-reduce:transition-none ${folded ? "opacity-0" : ""}`);
  /** A label beside the icon, only while folded. */
  const folds = (t: string) => () => (folded ? t : null);
</script>

<!-- The leading mark: the status glyph when open; folded, what it backs up with its
     status as a dot. They cross-fade in the same place. -->
{#snippet lead(tone: Tone, icon: Parameters<typeof backupIcon>[0] | "all", on: boolean)}
  <span class="relative grid size-6 shrink-0 place-items-center">
    <span class="grid place-items-center transition-opacity duration-200 motion-reduce:transition-none {folded ? 'opacity-0' : ''}"><StatusGlyph {tone} /></span>
    <span class="absolute inset-0 grid place-items-center transition-opacity duration-200 motion-reduce:transition-none {folded ? '' : 'opacity-0'}" aria-hidden="true">
      <Icon name={icon === "all" ? "layers" : backupIcon(icon)} size={18} class={on ? "" : "text-fg-2"} />
      <span class="absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full ring-2 {on ? 'ring-selected' : 'ring-bg'} {toneDot[tone]}"></span>
    </span>
  </span>
{/snippet}

<!-- A place's mark (New backup, Preferences): its round glyph when open, the plain icon
     folded, like the backups' above it. -->
{#snippet placeLead(icon: "plus" | "settings")}
  <span class="relative grid size-6 shrink-0 place-items-center">
    <span class="glyph transition-opacity duration-200 motion-reduce:transition-none {folded ? 'opacity-0' : ''}"><Icon name={icon} size={13} /></span>
    <span class="absolute inset-0 grid place-items-center transition-opacity duration-200 motion-reduce:transition-none {folded ? '' : 'opacity-0'}" aria-hidden="true"><Icon name={icon} size={18} /></span>
  </span>
{/snippet}

{#snippet row(p: ProfileSummary)}
  {@const s = protection(p, app.statuses[p.id], percent(p), app.jobs[p.id]?.progress?.phase === "waiting")}
  {@const on = onBackup(p.id)}
  {@const fresh = app.unseenFailure(p)}
  {@const left = p.running ? app.timeLeftOf(p.id) : null}
  <button
    class="{itemBase} {itemState(on)}"
    aria-current={on ? "page" : undefined}
    title={folded ? undefined : `${p.name}: ${s.short}${left ? `, ${left}` : ""}. Right-click for more.`}
    onclick={() => app.open(p.id)}
    use:contextMenu={{ items: () => profileMenu(p), label: p.name }}
    use:tip={folds(`${p.name} · ${s.short}${left ? ` · ${left}` : ""}`)}
  >
    {@render lead(s.tone, p, on)}
    <span class={words}>
      {#if compact}
        <!-- One line: the name, and the status in a word or two (all of it in the title). -->
        <!-- The name takes what it needs and the status the rest, down to a few letters;
             only then does the name give way. -->
        <span class="min-w-0 flex-initial truncate text-[13.5px] font-medium">{p.name}</span>
        <span class="num min-w-12 flex-1 truncate text-right text-[12px] {toneText[s.tone]}">{sideLabel(s.tone, s.short)}</span>
      {:else}
        <span class="min-w-0 flex-1">
          <span class="block truncate text-[14px] font-medium">{p.name}</span>
          <span class="num block truncate text-[12.5px] {toneText[s.tone]}">{s.short}</span>
        </span>
      {/if}
      {#if fresh}
        <span class="size-2 shrink-0 rounded-full bg-danger" title="New problem: open it to see what happened"></span>
        <span class="sr-only">New problem</span>
      {/if}
    </span>
  </button>
{/snippet}

{#snippet sectionHead(label: string, actions: Snippet)}
  <div class="relative flex items-center justify-between pt-5 pr-1 pb-1 pl-2.5">
    <h2 class="text-[12.5px] font-semibold whitespace-nowrap text-fg-muted transition-opacity duration-150 motion-reduce:transition-none {folded ? 'opacity-0' : ''}">{label}</h2>
    <span class="flex items-center transition-opacity duration-150 motion-reduce:transition-none {folded ? 'pointer-events-none opacity-0' : ''}" inert={folded}>{@render actions()}</span>
    <!-- Folded: a short rule between the groups instead. -->
    <span class="absolute bottom-3.5 left-3.5 h-px w-7 bg-fg/12 transition-opacity duration-200 motion-reduce:transition-none {folded ? '' : 'opacity-0'}" aria-hidden="true"></span>
  </div>
{/snippet}

<aside
  class="flex shrink-0 flex-col overflow-hidden px-2.5 pb-2.5 transition-[width] duration-[260ms] ease-[cubic-bezier(0.2,0,0,1)] motion-reduce:transition-none"
  style="width: {folded ? 64 : compact ? 272 : 264}px"
  aria-label="Backups and places"
>
  <nav class="min-h-0 flex-1 overflow-x-hidden overflow-y-auto [scrollbar-width:none]" aria-label="Backups">
    {#if app.profiles.length}
      <button class="{itemBase} {itemState(app.home)}" aria-current={app.home ? "page" : undefined} onclick={() => app.openHome()} use:tip={folds(`All backups · ${summary(tally)}`)}>
        {@render lead(homeTone, "all", app.home)}
        <span class={words}>
          <span class="min-w-0 flex-1">
            <span class="block truncate text-[14px] font-medium">All backups</span>
            <span class="block truncate text-[12.5px] {toneText[homeTone === 'run' ? 'ok' : homeTone]}">
              {summary(tally)}
            </span>
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
              ...(app.system.available
                ? [
                    "separator" as const,
                    app.system.unlocked
                      ? { label: "Lock system backups", icon: "lock" as const, hint: "Stops the administrator helper", onselect: () => void app.lockSystem() }
                      : { label: "System backups…", icon: "shield" as const, hint: "Show and change the whole computer's backups (needs an administrator password)", onselect: () => (app.dialog = { kind: "system-unlock" }) },
                  ]
                : []),
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
      <button
        class="{itemBase} {app.creating && !app.creatingSystem ? 'bg-selected text-on-selected' : 'text-fg-2 hover:bg-surface-sunken hover:text-fg'}"
        title={folded ? undefined : "Set up a new backup (Ctrl+N)"}
        onclick={() => app.newProfile(false)}
        use:tip={folds("New backup (Ctrl+N)")}
      >
        {@render placeLead("plus")}
        <span class="text-[14px] font-medium transition-opacity duration-150 motion-reduce:transition-none {folded ? 'opacity-0' : ''}">New backup</span>
      </button>
    </div>

    <!-- System backups: listed once unlocked and there are some. Unlocking, and setting
         one up, are in the + menu above and on the New backup page. -->
    {#if app.system.unlocked && system.length}
      {#snippet sysActions()}
        <button class="btn-icon size-7" title="New system backup" aria-label="New system backup" onclick={() => app.newProfile(true)}><Icon name="plus" size={15} /></button>
        <button class="btn-icon size-7" title="Lock system backups (stops the administrator helper)" aria-label="Lock system backups" onclick={() => app.lockSystem()}><Icon name="lock" size={14} /></button>
      {/snippet}
      {@render sectionHead("System backups", sysActions)}
      <div class="space-y-0.5">
        {#each system as p (p.id)}
          {@render row(p)}
        {/each}
      </div>
    {/if}
  </nav>

  <div class="flex flex-col gap-1 pt-3">
    {#if app.pause}
      <div
        class="mb-1 flex items-center gap-2.5 overflow-hidden rounded-lg whitespace-nowrap {folded ? '' : 'bg-warning/12'} px-2.5 py-2 transition-colors"
        role="status"
        use:tip={folds(`Schedules paused ${pauseUntil(app.pause)}. Open the sidebar to resume them.`)}
      >
        <span class="glyph glyph-warn"><Icon name="pause" size={12} /></span>
        <span class="flex min-w-0 flex-1 items-center gap-2.5 transition-opacity duration-150 motion-reduce:transition-none {folded ? 'opacity-0' : ''}" inert={folded}>
          <span class="min-w-0 flex-1 text-[12.5px] leading-snug">
            <span class="block font-medium text-fg">Schedules paused</span>
            <span class="block truncate text-fg-2">{pauseUntil(app.pause)}</span>
          </span>
          <button class="btn h-7 shrink-0 px-2.5 text-[12.5px]" onclick={() => app.setPause("off")}>Resume</button>
        </span>
      </div>
    {/if}
    <button
      class="{itemBase} {app.prefsOpen ? 'bg-selected text-on-selected' : 'text-fg-2 hover:bg-surface-sunken hover:text-fg'}"
      aria-current={app.prefsOpen ? "page" : undefined}
      onclick={() => app.openPreferences()}
      use:tip={folds("Preferences")}
    >
      {@render placeLead("settings")}
      <span class="text-[14px] font-medium transition-opacity duration-150 motion-reduce:transition-none {folded ? 'opacity-0' : ''}">Preferences</span>
    </button>
  </div>
</aside>
