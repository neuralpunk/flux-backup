<script lang="ts">
  // The start page: are you protected? One sentence for every backup at once, then each
  // backup as a row with its last 30 days.
  import { app } from "../state.svelte";
  import { ago } from "../format";
  import { lastGood, nextRun, protection, type Protection, type Tone } from "../protection";
  import { contextMenu } from "../menu.svelte";
  import { profileMenu } from "../profileMenu";
  import type { ProfileSummary } from "../types";
  import HistoryStrip from "./HistoryStrip.svelte";
  import Icon from "./Icon.svelte";

  function percent(p: ProfileSummary): number | null {
    const pr = app.jobs[p.id]?.progress;
    if (!p.running || !pr || pr.bytes_total <= 0 || pr.phase !== "transferring") return null;
    return Math.min(100, Math.round((pr.bytes_done / pr.bytes_total) * 100));
  }

  const rows = $derived(app.profiles.map((p) => ({ p, s: protection(p, app.statuses[p.id], percent(p), app.jobs[p.id]?.progress?.phase === "waiting") })));

  // Filters: which backups missed runs, failed, aren't verified by a restore test, have a
  // copy kept elsewhere, or are short of space.
  type FilterKey = "missed" | "failed" | "unverified" | "offsite" | "low_space";
  const kinds = (p: ProfileSummary) => app.statuses[p.id]?.health.map((h) => h.kind) ?? [];
  const filters: { key: FilterKey; label: string; hint: string; test: (p: ProfileSummary) => boolean }[] = [
    { key: "missed", label: "Missed", hint: "Backups that haven't run when they should have (overdue, or the drive wasn't there)", test: (p) => kinds(p).some((k) => k === "overdue" || k === "unplugged" || k === "source_missing") },
    { key: "failed", label: "Failed", hint: "The last backup failed, or a place it's copied to did", test: (p) => p.last_run?.status === "failed" || kinds(p).some((k) => k === "failing" || k === "destination_failing") },
    { key: "unverified", label: "Unverified", hint: "No restore test has passed recently", test: (p) => !!app.statuses[p.id]?.unverified || kinds(p).some((k) => k === "restore_test_failed" || k === "unverified") },
    { key: "offsite", label: "Offsite", hint: "A copy is kept somewhere else", test: (p) => !!p.offsite || !!p.destinations?.some((d) => d.offsite) },
    { key: "low_space", label: "Low space", hint: "Little space left where the backups go", test: (p) => kinds(p).includes("low_space") },
  ];
  let filter = $state<FilterKey | null>(null);
  const shown = $derived(filter ? rows.filter((r) => filters.find((f) => f.key === filter)!.test(r.p)) : rows);
  const mine = $derived(shown.filter((r) => r.p.scope !== "system"));
  const system = $derived(shown.filter((r) => r.p.scope === "system"));
  const total = $derived(rows.length);
  const safe = $derived(rows.filter((r) => r.s.protected).length);
  // Most serious first: the headline speaks of a failure before a warning.
  const severity: Record<string, number> = { bad: 0, warn: 1, idle: 2 };
  const attention = $derived(
    rows.filter((r) => r.s.tone === "bad" || r.s.tone === "warn" || r.s.tone === "idle").sort((a, b) => severity[a.s.tone]! - severity[b.s.tone]!),
  );
  const running = $derived(rows.filter((r) => r.p.running));
  const tone = $derived<Tone>(attention.some((r) => r.s.tone === "bad") ? "bad" : attention.length ? "warn" : running.length ? "run" : "ok");

  const headline = $derived(
    total === 1
      ? safe
        ? "Your backup is protecting you."
        : "Your backup isn't protecting you yet."
      : safe === total
        ? `All ${total} backups are protected.`
        : safe === 0
          ? "None of your backups is protecting you yet."
          : `${safe} of ${total} backups are protected.`,
  );

  /** The soonest scheduled backup. */
  const next = $derived(
    rows
      .map((r) => ({ name: r.p.name, at: app.statuses[r.p.id]?.next_run }))
      .filter((x): x is { name: string; at: string } => !!x.at && Number.isFinite(Date.parse(x.at)))
      .sort((a, b) => Date.parse(a.at) - Date.parse(b.at))[0],
  );

  const glyph: Record<Tone, { cls: string; icon: "shieldCheck" | "alert" | "refresh" | "clock" }> = {
    ok: { cls: "glyph-ok", icon: "shieldCheck" },
    warn: { cls: "glyph-warn", icon: "alert" },
    bad: { cls: "glyph-bad", icon: "alert" },
    run: { cls: "glyph-run", icon: "refresh" },
    idle: { cls: "", icon: "clock" },
  };
  const rowGlyph: Record<Tone, "check" | "alert" | "refresh" | "clock"> = { ok: "check", warn: "alert", bad: "alert", run: "refresh", idle: "clock" };
  const toneText: Record<Tone, string> = { ok: "text-fg-muted", warn: "text-warning", bad: "text-danger", run: "text-primary", idle: "text-fg-muted" };

  let backingUpAll = $state(false);
  async function backUpAll() {
    backingUpAll = true;
    try {
      for (const { p } of rows) if (!p.running && !p.errors.length) await app.backupNow(p.id);
    } finally {
      backingUpAll = false;
    }
  }

  /** "BackupDisk", "nas" — where a backup goes, short. */
  function where(target: string): string {
    const ssh = target.match(/^ssh:\/\/(?:[^@/]+@)?([^/:]+)/);
    if (ssh) return ssh[1]!;
    return target.split("/").filter(Boolean).at(-1) ?? target;
  }

</script>

<style>
  .chip-filter {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    border-radius: 999px;
    padding: 0.25rem 0.75rem;
    font-size: 13px;
    color: var(--c-fg-2);
    background: var(--c-surface-3);
    transition: background-color 120ms, color 120ms;
  }
  .chip-filter:hover:not(:disabled) {
    color: var(--c-fg);
  }
  .chip-filter[aria-pressed="true"] {
    background: color-mix(in oklab, var(--c-primary) 28%, var(--c-surface-3));
    color: var(--c-fg);
    font-weight: 500;
    box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--c-primary) 55%, transparent);
  }
  .chip-filter:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .chip-filter .num {
    font-variant-numeric: tabular-nums;
    opacity: 0.7;
  }
</style>

{#snippet backupRow(p: ProfileSummary, s: Protection)}
  {@const st = app.statuses[p.id]}
  {@const good = lastGood(p, st)}
  {@const path = `${p.sources?.length ? `${p.sources[0]}${p.sources.length > 1 ? ` +${p.sources.length - 1}` : ""} ` : ""}→ ${where(p.target)}`}
  <button
    class="row row-link grid grid-cols-[24px_minmax(0,1fr)_16px] gap-x-4 gap-y-2 @min-[760px]:grid-cols-[24px_minmax(0,1fr)_12.5rem_16px] @min-[1020px]:grid-cols-[24px_minmax(0,1fr)_auto_12.5rem_16px] {s.tone === 'bad' ? 'bg-danger/7' : ''}"
    onclick={() => app.open(p.id)}
    use:contextMenu={{ items: () => profileMenu(p), label: p.name }}
  >
    <span class="glyph col-start-1 row-start-1 {glyph[s.tone].cls}"><Icon name={rowGlyph[s.tone]} size={13} class={s.tone === "run" ? "motion-safe:animate-spin [animation-duration:2.4s]" : ""} /></span>
    <span class="col-start-2 row-start-1 min-w-0">
      <span class="block truncate font-semibold">{p.name}</span>
      <span class="block truncate text-[12.5px]">
        <span class="num {toneText[s.tone]}">{s.short}{#if s.tone === "bad" && good}<span class="@min-[760px]:hidden">{" "}· last good copy {good}</span>{/if}</span>
        <!-- With the strip on its own line, the path shares this one so rows stay three lines. -->
        <span class="font-mono text-[12px] text-fg-muted @min-[1020px]:hidden" title="{(p.sources ?? []).join(', ')} → {p.target}">{" "}· {path}</span>
      </span>
      <span class="hidden truncate font-mono text-[12px] text-fg-muted @min-[1020px]:block" title="{(p.sources ?? []).join(', ')} → {p.target}">{path}</span>
    </span>
    <span class="col-start-2 row-start-2 @min-[1020px]:col-start-3 @min-[1020px]:row-start-1">
      {#if st}<HistoryStrip days={st.days} />{/if}
    </span>
    <span class="hidden min-w-0 text-[12.5px] @min-[760px]:col-start-3 @min-[760px]:row-start-1 @min-[760px]:block @min-[1020px]:col-start-4">
      <!-- The status line already says when, unless the last good copy is older than that. -->
      {#if !good || !s.short.includes(good)}
        <span class="block truncate"><span class="text-fg-muted">Last good copy</span>{" "}<span class="num {s.tone === 'bad' ? 'text-danger' : 'text-fg'}">{good ?? "none yet"}</span></span>
      {/if}
      {#if p.running}
        <span class="block truncate text-fg">Backing up now</span>
      {:else}
        <span class="block truncate"><span class="text-fg-muted">Next</span>{" "}<span class="num text-fg">{nextRun(p, st)}</span></span>
      {/if}
    </span>
    <span class="col-start-3 row-start-1 self-center @min-[760px]:col-start-4 @min-[1020px]:col-start-5"><Icon name="chevronRight" size={16} class="text-fg-muted" /></span>
  </button>
{/snippet}

<div class="@container flex min-h-0 flex-1 flex-col overflow-y-auto">
  <div class="mx-auto flex w-full max-w-[1080px] flex-col gap-8 px-10 pt-6 pb-10">
    <div class="flex items-center justify-between gap-4">
      <h1 class="text-[15px] font-semibold text-fg-2">All backups</h1>
      <div class="flex gap-2">
        {#if app.pause}
          <button class="btn" onclick={() => app.setPause("off")}><Icon name="play" size={14} /> Resume schedules</button>
        {:else}
          <button class="btn" onclick={() => (app.dialog = { kind: "pause" })}><Icon name="pause" size={14} /> Pause schedules</button>
        {/if}
        <button id="backup-now" class="btn btn-primary" onclick={backUpAll} disabled={backingUpAll || rows.every((r) => r.p.running || r.p.errors.length)}>
          <Icon name="play" size={14} /> Back up all now
        </button>
      </div>
    </div>

    <section class="flex items-start gap-5" aria-live="polite">
      <span class="glyph {glyph[tone].cls} size-14"><Icon name={glyph[tone].icon} size={28} /></span>
      <div class="min-w-0 pt-1">
        <h2 class="text-[30px] leading-tight font-semibold tracking-[-0.015em] text-balance">{headline}</h2>
        {#if attention.length}
          {@const first = attention[0]!}
          <p class="mt-2 max-w-[68ch] text-[15px] text-fg-2">
            <span class="font-semibold {toneText[first.s.tone]}">{first.p.name}</span>: {first.s.detail}
            {#if attention.length > 1}<span class="text-fg-muted"> And {attention.length - 1} more {attention.length === 2 ? "needs" : "need"} a look below.</span>{/if}
          </p>
          <div class="mt-4 flex gap-2">
            <button class="btn" onclick={() => app.open(first.p.id)}>Open {first.p.name}</button>
            {#if first.p.errors.length}<button class="btn btn-ghost" onclick={() => app.open(first.p.id, "settings")}>Fix its settings</button>{/if}
          </div>
        {:else if running.length}
          <p class="mt-2 text-[15px] text-fg-2">Backing up {running.map((r) => r.p.name).join(", ")} now.</p>
        {:else if next}
          <p class="mt-2 text-[15px] text-fg-2">Next: <span class="font-medium text-fg">{next.name}</span>, {ago(next.at)}.</p>
        {/if}
      </div>
    </section>

    <section>
      <div class="flex items-baseline justify-between gap-4 px-0.5 pb-2">
        <h2 class="text-[13px] font-semibold text-fg-2">Your backups</h2>
        <span class="text-[12.5px] text-fg-muted">Last 30 days · taller bars copied more</span>
      </div>
      {#if rows.length > 1}
        <div class="mb-2.5 flex flex-wrap gap-1.5" role="group" aria-label="Show only">
          <button class="chip-filter" aria-pressed={filter === null} onclick={() => (filter = null)}>All <span class="num">{rows.length}</span></button>
          {#each filters as f (f.key)}
            {@const n = rows.filter((r) => f.test(r.p)).length}
            <button class="chip-filter" aria-pressed={filter === f.key} disabled={n === 0 && filter !== f.key} title={f.hint} onclick={() => (filter = filter === f.key ? null : f.key)}>
              {f.label} <span class="num">{n}</span>
            </button>
          {/each}
        </div>
      {/if}
      <div class="rows">
        {#each mine as r (r.p.id)}
          {@render backupRow(r.p, r.s)}
        {:else}
          {#if filter}<p class="row text-[13px] text-fg-muted">None of your backups.</p>{/if}
        {/each}
        {#if !filter}
          <button class="row row-link text-fg-2" onclick={() => app.newProfile(false)}>
            <span class="glyph"><Icon name="plus" size={13} /></span>
            <span class="font-medium">Set up another backup</span>
          </button>
        {/if}
      </div>
    </section>

    {#if system.length}
      <section>
        <h2 class="group-title">System backups</h2>
        <div class="rows">
          {#each system as r (r.p.id)}
            {@render backupRow(r.p, r.s)}
          {/each}
        </div>
      </section>
    {/if}
  </div>
</div>
