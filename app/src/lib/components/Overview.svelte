<script lang="ts">
  import { api } from "../api";
  import { coalesce } from "../coalesce";
  import { app, pauseUntil } from "../state.svelte";
  import { ago, bytes, count, dateTime, runStatusLabel, shortDate } from "../format";
  import { ApiError, engineLabel, isCompat, type Overview, type RunRecord } from "../types";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";
  import ConfirmDialog from "./ConfirmDialog.svelte";
  import { copyText } from "../clipboard";
  import SpaceCard from "./SpaceCard.svelte";
  import HistoryStrip from "./HistoryStrip.svelte";
  import SizeLimitNotice from "./SizeLimitNotice.svelte";
  import Readiness from "./Readiness.svelte";
  import StorageCard from "./StorageCard.svelte";
  import { nextRun, protection, type Tone } from "../protection";

  let { id, onLocked }: { id: string; onLocked: (retry: () => void) => void } = $props();
  // A Vault from before 1.0: offered the newest format.
  let vaultFormat = $state<{ format: number; newest: number } | null>(null);
  let confirmingUpgrade = $state(false);
  let upgrading = $state(false);
  $effect(() => {
    const want = id;
    vaultFormat = null;
    api.vaultFormat(want).then(
      (f) => want === id && (vaultFormat = f),
      () => {},
    );
  });
  async function upgradeVault() {
    upgrading = true;
    try {
      vaultFormat = await api.upgradeVault(id);
      app.toast("ok", "The Vault is in the newest format now.");
      confirmingUpgrade = false;
    } catch (e) {
      app.toast("bad", (e as Error).message);
    } finally {
      upgrading = false;
    }
  }
  let testing = $state(false);
  let showingAffected = $state(false);
  async function dismissUnusual(snapshot: string) {
    try {
      await api.dismissUnusual(id, snapshot);
      app.toast("ok", "Noted: those changes were yours. The extra snapshot that was kept is no longer pinned.");
      app.revision++;
      void app.refresh();
    } catch (e) {
      app.toast("bad", (e as Error).message);
    }
  }
  /** Restore a sample now and say how it went. */
  async function testNow() {
    testing = true;
    try {
      const r = await api.restoreTest(id);
      if (r.result === "passed") app.toast("ok", `Restore test passed: ${r.tested} files restored and checked.`);
      else if (r.result === "warning") app.toast("info", `Restore test passed with ${r.problems_total} notes${r.stopped_early ? " (stopped at a limit)" : ""}.`);
      else app.toast("bad", `Restore test failed: ${r.error ?? r.problems.map((p) => `${p.path} (${p.problem.replaceAll("_", " ")})`).slice(0, 2).join(", ")}`);
      app.revision++;
    } catch (e) {
      if (e instanceof ApiError && e.code === "password_required") onLocked(() => void testNow());
      else app.toast("bad", (e as Error).message);
    } finally {
      testing = false;
    }
  }
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
          {#if !summary?.running}
          <span class="text-fg-muted">·</span>
          Next <span class="text-fg">{summary ? nextRun(summary, status ?? { next_run: data.next_run, timer_active: data.timer_active, last_good: null, health: [], days: [] }, pausedNext ? pauseUntil(app.pause!) : null) : ""}</span>
          {#if !manual && !data.timer_active}<span class="text-warning"> (its timer isn't installed)</span>{/if}
          {/if}
          <span class="text-fg-muted">·</span>
          <span class="text-fg">{count(data.snapshot_count)}</span> {data.snapshot_count === 1 ? "snapshot" : "snapshots"}{#if data.oldest}<span class="text-fg-muted">{" "}since {shortDate(data.oldest)}</span>{/if}
        </p>
        {#if prot?.detail && tone !== "ok" && !data.unusual}
          <p class="mt-2 max-w-[68ch] text-[14px] {tone === 'bad' ? 'text-danger' : tone === 'warn' ? 'text-warning' : 'text-fg-2'}">{prot.detail}</p>
        {/if}
        {#if last && last.status === "failed" && !status?.health.length}
          {@const hint = failureHint(last)}
          {#if hint}<p class="mt-1 max-w-[68ch] text-[14px] text-fg">{hint}</p>{/if}
        {/if}
      </div>
    </section>

    {#if data.unusual}
      {@const u = data.unusual}
      <section class="card flex flex-col gap-3 bg-danger/8 px-5 py-4" aria-labelledby="unusual-title">
        <div class="flex items-start gap-3">
          <span class="glyph glyph-bad size-9"><Icon name="alert" size={17} /></span>
          <div class="min-w-0 flex-1">
            <h2 id="unusual-title" class="title">Unusual changes in the last backup</h2>
            <ul class="mt-1 list-disc pl-5 text-[14px] text-fg">
              {#each u.signals as sig (sig)}<li>{sig}</li>{/each}
            </ul>
            <p class="mt-1.5 text-[13px] text-fg-2">
              {count(u.affected_total)} files{#if u.by_folder.length}, mostly in <span class="font-mono text-[12.5px]">{u.by_folder[0]![0]}</span>{#if u.by_folder.length > 1}{" "}and {u.by_folder.length - 1} other {u.by_folder.length === 2 ? "folder" : "folders"}{/if}{/if}.
            </p>
            <p class="mt-1.5 max-w-[70ch] text-[13px] text-fg-2">
              If you didn't make these changes, don't delete or change anything: the snapshot before them is kept (pinned), and you can restore from it. Old snapshots aren't removed until this is settled.
            </p>
          </div>
        </div>
        <div class="flex flex-wrap gap-2 pl-12">
          <button class="btn" onclick={() => (showingAffected = true)}>Show the files</button>
          {#if u.before}<button class="btn" onclick={() => ((app.snapshotsOf = null), (app.showSnapshot = u.before ?? null), (app.tab = "snapshots"))}>Browse the snapshot before</button>{/if}
          <button class="btn btn-ghost" onclick={() => void dismissUnusual(u.snapshot)}>This was me</button>
        </div>
      </section>
    {/if}

    {#if status}
      <section>
        <div class="flex items-baseline justify-between gap-4 px-0.5 pb-2">
          <h2 class="text-[13px] font-semibold text-fg-2">Last 30 days</h2>
          <span class="text-[12.5px] text-fg-muted">One bar a day · taller bars copied more</span>
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

    <SizeLimitNotice {id} onlyTiny />

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

    {#if data.profile.destinations?.length}
      {@const prof = data.profile}
      {@const last = data.recent_runs.find((r) => r.status !== "skipped" && r.status !== "dry_run")}
      <section>
        <h2 class="group-title">Where it's kept</h2>
        <ul class="rows">
          {#snippet place(name: string, target: string, remote: boolean, offsite: boolean | undefined, engine: string, status: RunRecord["status"] | null, err: string | null | undefined, browse: string, copy = false)}
            <li class="row min-h-12 gap-3 py-2 text-sm">
              <Icon name={remote ? "server" : "drive"} size={17} class="shrink-0 text-fg-2" />
              <span class="min-w-0 flex-1">
                <span class="flex items-center gap-2">
                  <span class="font-medium">{name}</span>
                  <span class="tag text-[11.5px]">{engine}</span>
                  {#if copy}<span class="tag text-[11.5px]">Copy</span>{/if}
                  {#if offsite}<span class="tag text-[11.5px]">Offsite</span>{/if}
                </span>
                <span class="block truncate font-mono text-[12px] text-fg-muted" title={target}>{target}</span>
              </span>
              {#if status}
                <span class="shrink-0 text-[13px] {status === 'failed' ? 'cursor-help text-danger underline decoration-dotted underline-offset-4' : 'text-fg-2'}" title={err ?? ""}>
                  {status === "failed" ? "Failed last time" : runStatusLabel[status]}
                </span>
              {/if}
              <button class="btn btn-ghost h-7 shrink-0 px-2.5 text-[13px]" onclick={() => ((app.snapshotsOf = browse === data?.profile.id ? null : browse), (app.tab = "snapshots"))}>Browse</button>
            </li>
          {/snippet}
          {@render place(prof.target.split("/").filter(Boolean).pop() ?? prof.target, prof.target, prof.remote, prof.offsite, engineLabel[prof.engine], last?.status ?? null, last?.error, prof.id)}
          {#each prof.destinations ?? [] as d (d.id)}
            {@const r = last?.destinations?.find((x) => x.name === d.name)}
            {@render place(d.label, d.target, d.remote, d.offsite, engineLabel[d.engine], r?.status ?? null, r?.error, d.id, d.replica)}
          {/each}
        </ul>
      </section>
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
        {#each data.recent_runs.slice(0, 3) as r (r.run_id + r.started)}
          <li class="row min-h-11 py-2 text-sm" title={r.error ?? `${dateTime(r.started)} · ${r.trigger}`}>
            <span class="size-2 shrink-0 rounded-full {dot[r.status]}" aria-hidden="true"></span>
            <span class="shrink-0 font-medium">{runStatusLabel[r.status]}</span>
            <!-- An error takes the middle; otherwise the size sits by the time, on the right. -->
            <span class="num min-w-0 flex-1 truncate text-danger">{r.error ?? ""}</span>
            {#if !r.error && r.stats}
              <span class="num shrink-0 text-fg-2">{bytes(storedBytes ? r.stats.stored_bytes : r.stats.copied_bytes)} {storedBytes ? "stored" : "copied"}</span>
            {/if}
            <span class="num w-24 shrink-0 text-right text-[13px] text-fg-muted">{ago(r.started)}</span>
          </li>
        {:else}
          <li class="row text-sm text-fg-muted">No backups yet. They'll appear here.</li>
        {/each}
      </ul>
    </section>

    {#if data.restore_test}
      {@const rt = data.restore_test}
      {@const last = rt.last}
      <div class="row gap-3">
        <span class="glyph {last?.result === 'failed' ? 'glyph-bad' : last?.result === 'warning' ? 'glyph-warn' : last ? 'glyph-ok' : ''} size-8"><Icon name={last?.result === "failed" ? "alert" : "shieldCheck"} size={16} /></span>
        <span class="min-w-0 flex-1">
          <span class="block font-medium">
            {#if !last}No restore test yet{:else if last.result === "failed"}The last restore test failed{:else if last.result === "warning"}Restore test passed, with notes{:else}Restore test passed{/if}
          </span>
          <span class="block text-[13px] text-fg-2">
            {#if last}
              {ago(last.started)} · {count(last.tested)} files restored and checked{#if last.result !== "passed"} · {last.error ?? `${last.problems_total} ${last.problems_total === 1 ? "problem" : "problems"}`}{/if}
            {:else}
              Restore a few files into a temporary folder and check them, to know the backup can be read back.
            {/if}
            {#if rt.every !== "off"} · {rt.every === "backup" ? "after every backup" : `every ${rt.every}`}{/if}
          </span>
        </span>
        <button class="btn shrink-0" disabled={testing} onclick={() => void testNow()}>
          {#if testing}<Icon name="refresh" size={14} class="motion-safe:animate-spin" /> Testing…{:else}Test a restore now{/if}
        </button>
      </div>
    {/if}

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

    <!-- Space: how full the drive is, and what the snapshots take of it. -->
    {#if data.space && data.space.size}
      <SpaceCard space={data.space} target={data.profile.target.split("/").filter(Boolean).at(-1) ?? data.profile.target}>
        <StorageCard {id} embedded />
      </SpaceCard>
    {:else}
      <StorageCard {id} />
    {/if}

    {#if vaultFormat && vaultFormat.format < vaultFormat.newest}
      <div class="row card gap-3">
        <Icon name="vault" size={17} class="shrink-0 text-fg-2" />
        <span class="min-w-0 flex-1">
          <span class="block font-medium">This Vault can be upgraded</span>
          <span class="block text-[13px] text-fg-2">Format {vaultFormat.newest} keeps sparse files (disk images) small. Its snapshots stay as they are.</span>
        </span>
        <button class="btn" onclick={() => (confirmingUpgrade = true)} disabled={busy !== null}>Upgrade…</button>
      </div>
    {/if}

    <!-- Less often needed: folded, one line each until opened. -->
    <div class="flex flex-col gap-3">
      <Readiness {id} />
      <details class="group" open={busy !== null || pruneConfirm != null || result != null}>
        <summary class="flex cursor-pointer list-none items-center gap-2 rounded-md px-0.5 py-1 text-[13px] font-semibold text-fg-2 select-none hover:text-fg [&::-webkit-details-marker]:hidden">
          <Icon name="chevronRight" size={14} class="transition-transform group-open:rotate-90" />
          Maintenance
          <span class="font-normal text-fg-muted">Check, verify, free up space</span>
        </summary>
      <div class="rows mt-2">
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
      </details>
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
    </div>
  </div>
{/if}

{#if confirmingUpgrade}
  <ConfirmDialog title="Upgrade the Vault?" icon="vault" confirm={upgrading ? "Upgrading…" : "Upgrade"} busy={upgrading} onconfirm={() => void upgradeVault()} onclose={() => (confirmingUpgrade = false)}>
    <p>New snapshots then keep the holes of sparse files, so disk images and similar files take only the space of their data. The snapshots already there stay as they are.</p>
    <p class="mt-2">Flux Backup before 1.0 can't open the Vault afterwards: if another computer uses it with an older Flux, update that one first.</p>
  </ConfirmDialog>
{/if}

{#if showingAffected && data?.unusual}
  {@const u = data.unusual}
  <Modal title="Files involved" onclose={() => (showingAffected = false)} width="max-w-2xl">
    <p class="-mt-1 mb-3 text-[13px] text-fg-2">
      {count(u.affected_total)} files changed, deleted or added in snapshot {u.snapshot}{u.affected_total > u.affected.length ? ` (the first ${count(u.affected.length)} shown)` : ""}.
      {#if u.by_extension.length}By type: {u.by_extension.map(([x, n]) => `.${x} (${n})`).join(", ")}.{/if}
    </p>
    <ul class="max-h-[50vh] overflow-y-auto rounded-card bg-surface-2 px-3 py-2 font-mono text-[12.5px] leading-6">
      {#each u.affected as f (f)}<li class="truncate" title={f}>{f}</li>{/each}
    </ul>
    <div class="mt-4 flex justify-end gap-2">
      <button class="btn" onclick={() => void copyText(u.affected.join("\n")).then(() => app.toast("info", "Copied the list."))}><Icon name="copy" size={14} /> Copy the list</button>
      {#if u.before}<button class="btn btn-primary" onclick={() => ((showingAffected = false), (app.snapshotsOf = null), (app.showSnapshot = u.before ?? null), (app.tab = "snapshots"))}>Browse the snapshot before</button>{/if}
    </div>
  </Modal>
{/if}
