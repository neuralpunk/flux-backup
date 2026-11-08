<script lang="ts">
  import { api } from "../api";
  import { coalesce } from "../coalesce";
  import { app, pauseUntil } from "../state.svelte";
  import { ago, bytes, count, dateTime, runStatusLabel, shortDate } from "../format";
  import { ApiError, engineLabel, isCompat, type Overview, type RunRecord } from "../types";
  import Icon from "./Icon.svelte";
  import SpaceCard from "./SpaceCard.svelte";
  import HistoryStrip from "./HistoryStrip.svelte";
  import { nextRun, protection, type Tone } from "../protection";

  let { id, onLocked }: { id: string; onLocked: (retry: () => void) => void } = $props();
  let data = $state<Overview | null>(null);
  let error = $state<string | null>(null);
  let locked = $state(false);
  /** The engine, also while locked (the summary in the sidebar list knows it). */
  const engine = $derived(data?.profile.engine ?? app.profiles.find((p) => p.id === id)?.engine ?? "mirror");
  const storedBytes = $derived(engine === "vault" || engine === "borg");

  // The one-time card after the first backup (remembered on this computer only).
  const seenKey = $derived(`flux.firstBackupSeen.${id}`);
  let firstSeen = $state(false);
  $effect(() => {
    try {
      firstSeen = localStorage.getItem(seenKey) === "1";
    } catch {
      firstSeen = false;
    }
  });
  function dismissFirst() {
    firstSeen = true;
    try {
      localStorage.setItem(seenKey, "1");
    } catch {
      /* storage unavailable: it just shows again next time */
    }
  }

  function load() {
    const current = id;
    return api.overview(current).then(
      (o) => {
        if (current === id) {
          data = o;
          error = null;
          locked = false;
        }
      },
      (e) => {
        if (e instanceof ApiError && e.code === "password_required") locked = true;
        else error = (e as Error).message;
      },
    );
  }

  // Reload when a backup finishes or the profile list refreshes; both happen together
  // after a backup, so they're merged into one reload (opening an encrypted repository
  // is not free). The component is recreated per backup ({#key} in App).
  const reload = coalesce(load);
  $effect(() => {
    void app.revision;
    void app.profiles;
    reload.run();
    return () => reload.cancel();
  });

  const summary = $derived(app.profiles.find((p) => p.id === id));
  const status = $derived(app.statuses[id]);
  const prot = $derived(summary ? protection(summary, status) : null);
  const sentence: Record<Tone, (n: string) => string> = {
    ok: (n) => `${n} is protected.`,
    warn: (n) => `${n} needs a look.`,
    bad: (n) => `${n} isn't protecting you.`,
    run: (n) => `${n} is backing up.`,
    idle: (n) => `${n} hasn't made its first backup yet.`,
  };
  const glyph: Record<Tone, { cls: string; icon: "shieldCheck" | "alert" | "refresh" | "clock" }> = {
    ok: { cls: "glyph-ok", icon: "shieldCheck" },
    warn: { cls: "glyph-warn", icon: "alert" },
    bad: { cls: "glyph-bad", icon: "alert" },
    run: { cls: "glyph-run", icon: "refresh" },
    idle: { cls: "", icon: "clock" },
  };

  // Status dots: hue only for outcomes that need attention.
  const dot: Record<string, string> = {
    completed: "bg-success",
    unchanged: "bg-track",
    completed_with_errors: "bg-warning",
    failed: "bg-danger",
    cancelled: "bg-track",
    skipped: "bg-track",
    dry_run: "bg-track",
  };

  // ---- maintenance ----
  let busy = $state<"check" | "verify" | "prune" | null>(null);
  let result = $state<{ ok: boolean; text: string; details?: string[] } | null>(null);
  let pruneConfirm = $state<string[] | null>(null);

  async function check(readData: boolean) {
    busy = readData ? "verify" : "check";
    result = null;
    try {
      const c = await api.check(id, readData);
      result = c.errors.length
        ? { ok: false, text: `${c.errors.length} problems found in ${count(c.snapshots)} snapshots.`, details: c.errors.slice(0, 50) }
        : { ok: true, text: `All ${count(c.snapshots)} snapshots are intact${readData ? `; ${bytes(c.bytes_read)} read back and verified` : ""}.`, details: c.warnings };
    } catch (e) {
      result = { ok: false, text: (e as Error).message };
    } finally {
      busy = null;
    }
  }

  async function prune(dryRun: boolean) {
    busy = "prune";
    result = null;
    try {
      const r = await api.prune(id, dryRun);
      if (dryRun) {
        if (r.removed.length) pruneConfirm = r.removed;
        else {
          // No snapshot goes, but a Vault may still hold data that none uses (an
          // interrupted backup, or a deletion by an older version). Collecting it removes
          // no snapshots, so it needs no confirmation.
          const done = await api.prune(id, false);
          result = done.bytes_freed
            ? { ok: true, text: `Every snapshot is kept by the retention rules; reclaimed ${bytes(done.bytes_freed)} of data no snapshot used.` }
            : { ok: true, text: "Nothing to remove: every snapshot is kept by the retention rules." };
          if (done.removed.length || done.bytes_freed) app.revision++;
        }
      } else {
        pruneConfirm = null;
        result = { ok: true, text: `Removed ${r.removed.length} snapshots${r.bytes_freed != null ? ` and reclaimed ${bytes(r.bytes_freed)}` : ""}.` };
        app.revision++;
      }
    } catch (e) {
      result = { ok: false, text: (e as Error).message };
    } finally {
      busy = null;
    }
  }

  function failureHint(r: RunRecord): string | null {
    const e = r.error ?? "";
    if (e.includes("does not exist") || e.includes("not on a mounted drive")) return "Connect and mount the backup drive, then back up again.";
    if (e.includes("passphrase")) return "Unlock the backups, or store the passphrase in the keyring (flux key store).";
    if (e.includes("password")) return "Unlock the Vault or store its password in the keyring (flux key store).";
    if (e.includes("borg is not installed") || e.includes("Borg is not installed")) return "Install Borg (the “borgbackup” package), then back up again.";
    if (e.includes("rsync is not installed")) return "Install rsync, then back up again.";
    if (e.includes("ssh")) return "Check the connection with `flux ssh test`.";
    if (e.includes("space")) return "Free up space on the target or tighten the retention rules.";
    return null;
  }
</script>

{#if locked}
  <div class="card grid place-items-center gap-3 p-14 text-center">
    <span class="glyph size-12"><Icon name="lock" size={22} /></span>
    <div class="title">{engine === "borg" ? "These Borg backups are locked" : "This Vault is locked"}</div>
    <p class="max-w-sm text-sm text-fg-2">
      {engine === "borg"
        ? "The repository is encrypted. Enter its passphrase to see snapshots, browse files and restore."
        : "Its contents are encrypted. Enter the password to see snapshots, browse files and restore."}
    </p>
    <div class="mt-2 flex gap-2">
      <button class="btn btn-primary" onclick={() => onLocked(load)}>Unlock</button>
      {#if isCompat(engine)}<button class="btn btn-ghost" onclick={() => (app.dialog = { kind: "recovery", id })}><Icon name="lifebuoy" size={15} /> Recovery guide</button>{/if}
    </div>
  </div>
{:else if error}
  <div class="card flex items-center gap-3 bg-danger/10 p-5 text-danger"><Icon name="alert" /> {error}</div>
{:else if !data}
  <div class="flex flex-col gap-4" aria-busy="true">
    <div class="h-14 w-2/3 rounded-lg bg-surface-2 motion-safe:animate-pulse"></div>
    <div class="h-40 rounded-card bg-surface-2 motion-safe:animate-pulse"></div>
  </div>
{:else}
  {@const last = data.profile.last_run}
  {@const manual = data.profile.schedule.startsWith("manual")}
  {@const pausedNext = !manual && app.pause != null && (!app.pause.until || (data.next_run != null && new Date(data.next_run) < new Date(app.pause.until)))}
  {@const tone = prot?.tone ?? "ok"}
  <div class="flex flex-col gap-7">
    <!-- The answer, in one sentence. -->
    <section class="flex items-start gap-4" aria-live="polite">
      <span class="glyph {glyph[tone].cls} size-12"><Icon name={glyph[tone].icon} size={24} /></span>
      <div class="min-w-0 pt-0.5">
        <h2 class="text-[22px] leading-tight font-semibold tracking-[-0.01em] text-balance">{sentence[tone](data.profile.name)}</h2>
        <p class="num mt-1.5 text-[14px] text-fg-2">
          Last backup <span class="text-fg">{data.latest ? ago(data.latest.finished) : "never"}</span>
          <span class="text-fg-muted">·</span>
          Next <span class="text-fg">{summary ? nextRun(summary, status ?? { next_run: data.next_run, timer_active: data.timer_active, last_good: null, health: [], days: [] }, pausedNext ? pauseUntil(app.pause!) : null) : ""}</span>
          {#if !manual && !data.timer_active}<span class="text-warning"> (its timer isn't installed)</span>{/if}
          <span class="text-fg-muted">·</span>
          <span class="text-fg">{count(data.snapshot_count)}</span> {data.snapshot_count === 1 ? "snapshot" : "snapshots"}{#if data.oldest}<span class="text-fg-muted">{" "}since {shortDate(data.oldest)}</span>{/if}
        </p>
        {#if prot?.detail && tone !== "ok"}
          <p class="mt-2 max-w-[68ch] text-[14px] {tone === 'bad' ? 'text-danger' : tone === 'warn' ? 'text-warning' : 'text-fg-2'}">{prot.detail}</p>
        {/if}
        {#if last && last.status === "failed" && !status?.health.length}
          {@const hint = failureHint(last)}
          {#if hint}<p class="mt-1 max-w-[68ch] text-[14px] text-fg">{hint}</p>{/if}
        {/if}
      </div>
    </section>

    {#if status}
      <section>
        <div class="flex items-baseline justify-between gap-4 px-0.5 pb-2">
          <h2 class="text-[13px] font-semibold text-fg-2">Last 30 days</h2>
          <span class="text-[12.5px] text-fg-muted">One mark a day · bigger marks copied more</span>
        </div>
        <div class="card px-4 pt-3.5 pb-2.5"><HistoryStrip days={status.days} fill /></div>
      </section>
    {/if}

    {#if data.snapshot_count === 1 && !firstSeen}
      <section class="card flex gap-4 bg-success/8 px-5 py-4">
        <span class="glyph glyph-ok size-9"><Icon name="check" size={17} /></span>
        <div class="min-w-0 flex-1">
          <h2 class="title">Your first backup is done</h2>
          <p class="mt-1 max-w-[70ch] text-sm text-fg-2">
            {manual
              ? "From now on, click Back up now whenever you want a new snapshot; each one stores only what changed."
              : `It runs ${data.profile.schedule} from now on, storing only what changed.`}
            To get a file back: open <strong class="font-medium text-fg">Snapshots</strong>, find it, and click
            <strong class="font-medium text-fg">Restore</strong> (or double-click it to open a copy). It's worth trying once with a file you don't need.
          </p>
          <div class="mt-3 flex gap-2">
            <button class="btn" onclick={() => (app.tab = "snapshots")}>Try it: browse files</button>
            <button class="btn btn-ghost" onclick={dismissFirst}>Got it</button>
          </div>
        </div>
      </section>
    {/if}

    {#if data.repo_error || (data.health ?? []).length > 1}
      <div class="rows">
        {#if data.repo_error}
          <div class="row">
            <span class="glyph glyph-warn"><Icon name="alert" size={13} /></span>
            <span class="text-sm"><span class="font-medium">Snapshots can't be read right now.</span> <span class="text-fg-2">{data.repo_error}</span></span>
          </div>
        {/if}
        {#each (data.health ?? []).slice(1) as h (h.text)}
          <div class="row">
            <span class="glyph {h.level === 'danger' ? 'glyph-bad' : 'glyph-warn'}"><Icon name="alert" size={13} /></span>
            <span class="text-sm"><span class="font-medium">{h.text}</span>{#if h.hint} <span class="text-fg-2">{h.hint}</span>{/if}</span>
          </div>
        {/each}
      </div>
    {/if}

    <section>
      <div class="flex items-center justify-between gap-3 px-0.5 pb-2">
        <h2 class="text-[13px] font-semibold text-fg-2">Recent backups</h2>
        <div class="flex gap-1">
          {#if data.latest}<button class="btn btn-ghost h-7 px-2.5 text-[13px]" onclick={() => (app.tab = "snapshots")}>Browse files</button>{/if}
          {#if data.recent_runs.length}<button class="btn btn-ghost h-7 px-2.5 text-[13px]" onclick={() => (app.tab = "activity")}>All activity</button>{/if}
        </div>
      </div>
      <ul class="rows">
        {#each data.recent_runs.slice(0, 5) as r (r.run_id + r.started)}
          <li class="row min-h-11 py-2 text-sm" title={r.error ?? `${dateTime(r.started)} · ${r.trigger}`}>
            <span class="size-2 shrink-0 rounded-full {dot[r.status]}" aria-hidden="true"></span>
            <span class="w-44 shrink-0 font-medium">{runStatusLabel[r.status]}</span>
            <span class="num min-w-0 flex-1 truncate {r.error ? 'text-danger' : 'text-fg-2'}">
              {#if r.error}{r.error}{:else if r.stats}{bytes(storedBytes ? r.stats.stored_bytes : r.stats.copied_bytes)} {storedBytes ? "stored" : "copied"}{/if}
            </span>
            <span class="num shrink-0 text-[13px] text-fg-muted">{ago(r.started)}</span>
          </li>
        {:else}
          <li class="row text-sm text-fg-muted">No backups yet. They'll appear here.</li>
        {/each}
      </ul>
    </section>

    {#if isCompat(data.profile.engine)}
      <button class="row row-link" onclick={() => (app.dialog = { kind: "recovery", id })}>
        <span class="glyph glyph-ok size-8"><Icon name="lifebuoy" size={16} /></span>
        <span class="min-w-0 flex-1">
          <span class="block font-medium">Usable without Flux</span>
          <span class="block text-[13px] text-fg-2">
            {data.profile.engine === "borg"
              ? "A standard Borg repository. The recovery guide next to it says how to restore with borg alone."
              : "Plain folders written by rsync. The recovery guide next to them says how to restore with rsync or a file manager."}
          </span>
        </span>
        <span class="shrink-0 text-[13px] text-fg-2">Recovery guide</span>
        <Icon name="chevronRight" size={16} class="shrink-0 text-fg-muted" />
      </button>
    {/if}

    {#if data.space && data.space.size && data.space.per_day != null}
      <SpaceCard space={data.space} target={data.profile.target.split("/").filter(Boolean).at(-1) ?? data.profile.target} />
    {/if}

    <section>
      <h2 class="group-title">Maintenance</h2>
      <div class="rows">
        <div class="row">
          <span class="min-w-0 flex-1"><span class="block font-medium">Check backups</span><span class="block text-[13px] text-fg-2">Reads the index and structure of every snapshot. Quick.</span></span>
          <button class="btn" onclick={() => check(false)} disabled={busy !== null}>{busy === "check" ? "Checking…" : "Check"}</button>
        </div>
        <div class="row">
          <span class="min-w-0 flex-1"><span class="block font-medium">Verify every byte</span><span class="block text-[13px] text-fg-2">Reads all stored data back and compares it. Can take a while.</span></span>
          <button class="btn" onclick={() => check(true)} disabled={busy !== null}>{busy === "verify" ? "Verifying…" : "Verify"}</button>
        </div>
        <div class="row">
          <span class="min-w-0 flex-1"><span class="block font-medium">Free up space</span><span class="block text-[13px] text-fg-2">Removes the snapshots the retention rules no longer keep. Asks first.</span></span>
          <button class="btn" onclick={() => prune(true)} disabled={busy !== null}>{busy === "prune" ? "Working…" : "Free up space"}</button>
        </div>
      </div>
      {#if pruneConfirm}
        <div class="mt-3 rounded-card bg-warning/10 p-4 text-sm">
          <div class="font-medium">Retention would remove {pruneConfirm.length} snapshots:</div>
          <div class="mt-1 max-h-24 overflow-y-auto font-mono text-xs text-fg-2">{pruneConfirm.join(", ")}</div>
          <div class="mt-3 flex gap-2">
            <button class="btn btn-danger-filled" onclick={() => prune(false)} disabled={busy !== null}>Remove them</button>
            <button class="btn btn-ghost" onclick={() => (pruneConfirm = null)}>Not now</button>
          </div>
        </div>
      {/if}
      {#if result}
        <div class="mt-3 rounded-card p-4 text-sm {result.ok ? 'bg-success/10' : 'bg-danger/10'}" role="status">
          <div class="flex items-center gap-2 font-medium {result.ok ? 'text-success' : 'text-danger'}"><Icon name={result.ok ? "check" : "alert"} size={15} /> {result.text}</div>
          {#if result.details?.length}
            <ul class="mt-2 max-h-40 overflow-y-auto font-mono text-xs text-fg-2">{#each result.details as d (d)}<li class="truncate">{d}</li>{/each}</ul>
          {/if}
        </div>
      {/if}
    </section>
  </div>
{/if}
