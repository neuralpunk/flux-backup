<script lang="ts">
  import { api } from "../api";
  import { coalesce } from "../coalesce";
  import { app, pauseUntil } from "../state.svelte";
  import { scheduleText } from "../clock.svelte";
  import { ago, bytes, count, dateTime, runStatusLabel, shortDate } from "../format";
  import { ApiError, engineLabel, isCompat, storageWords, type Overview, type RunRecord, type SnapshotMeta } from "../types";
  import RestoreDialog from "./RestoreDialog.svelte";
  import Icon from "./Icon.svelte";
  import Callout from "./Callout.svelte";
  import Modal from "./Modal.svelte";
  import ConfirmDialog from "./ConfirmDialog.svelte";
  import { copyText } from "../clipboard";
  import { guaranteeWords } from "../confidence";
  import SpaceCard from "./SpaceCard.svelte";
  import HistoryStrip from "./HistoryStrip.svelte";
  import SizeLimitNotice from "./SizeLimitNotice.svelte";
  import Readiness from "./Readiness.svelte";
  import BorgKeyNotice from "./BorgKeyNotice.svelte";
  import SettingsGlance from "./SettingsGlance.svelte";
  import StorageCard from "./StorageCard.svelte";
  import JobPanel from "./JobPanel.svelte";
  import CompareDialog from "./CompareDialog.svelte";
  import { changeMarks, changeWords, snapshotBefore } from "../changes";
  import { nextRun, protection, type Tone } from "../protection";

  let { id, onLocked }: { id: string; onLocked: (retry: () => void) => void } = $props();
  // A Vault from before 1.0: offered the newest format.
  let vaultFormat = $state<{ format: number; newest: number } | null>(null);
  let confirmingUpgrade = $state(false);
  /** A recent backup compared with the one before it (what it changed). */
  let comparing = $state<{ old: string; newer: string; when: string } | null>(null);
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
  const compact = $derived(app.layout === "compact");
  let testing = $state(false);
  let showingAffected = $state(false);
  /** Guided recovery: everything, from the snapshot kept from before unusual changes. */
  let recovering = $state<{ snapshot: SnapshotMeta; paths: string[]; alarm: string } | null>(null);
  async function recover(before: string, alarm: string) {
    try {
      const [snaps, prof] = await Promise.all([api.snapshots(id), api.getProfile(id)]);
      const snapshot = snaps.find((s) => s.id === before);
      if (!snapshot) throw new Error("The snapshot from before those changes isn't there any more.");
      recovering = { snapshot, paths: prof.profile.sources.paths, alarm };
    } catch (e) {
      if (e instanceof ApiError && e.code === "password_required") onLocked(() => void recover(before, alarm));
      else app.toast("bad", (e as Error).message);
    }
  }
  /** Settle unusual changes: they were mine, or (`resolved`) they weren't and are dealt with. */
  async function dismissUnusual(snapshot: string, resolved = false) {
    try {
      const settled = await api.dismissUnusual(id, snapshot, resolved);
      app.toast(
        "ok",
        resolved
          ? "It's dealt with: backups go on. The snapshot from before those changes stays pinned."
          : settled?.marks_left
            ? "Noted: those changes were yours, and Flux Sentry counts them as normal for this backup from now on. Backups go on. The backups couldn't be reached to unpin the snapshot that was kept: unpin it in Snapshots when they can."
            : "Noted: those changes were yours, and Flux Sentry counts them as normal for this backup from now on. Backups go on, and the extra snapshot that was kept is no longer pinned.",
      );
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
      if (r.result === "passed") app.toast("ok", `Restore test passed: ${count(r.tested)} files read back and checked.${r.guarantee ? ` ${guaranteeWords(r.guarantee)}` : ""}`);
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
  /** Compact puts the space on one row of the list (with its details under it). */
  const spaceRow = $derived(compact && !!data?.space?.size);
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
  /** What a prune would remove, to be confirmed: the snapshots the rules no longer keep,
      and whether more would go, oldest first, to reach the free-space minimum. */
  let pruneConfirm = $state<{ removed: string[]; forSpace: boolean } | null>(null);

  async function check(readData: boolean) {
    busy = readData ? "verify" : "check";
    result = null;
    try {
      const c = await api.check(id, readData);
      const fixed = c.repaired ?? [];
      const mended = fixed.length ? ` ${count(fixed.length)} damaged ${fixed.length === 1 ? "file was" : "files were"} rebuilt from repair data: the drive may be failing.` : "";
      const added = c.parity_added ? ` Repair data was added to ${count(c.parity_added)} stored files that had none.` : "";
      result = c.errors.length
        ? { ok: false, text: `${c.errors.length} problems found in ${count(c.snapshots)} snapshots.${c.repairable ? ` ${count(c.repairable)} of them can be rebuilt from the repair data: turn on Repair data in Settings › Where to keep it, then Verify every byte.` : ""}${mended}`, details: [...c.errors, ...fixed].slice(0, 50) }
        : { ok: true, text: `All ${count(c.snapshots)} snapshots are intact${readData ? `; ${bytes(c.bytes_read)} read back and verified` : ""}.${mended}${added}`, details: [...fixed, ...c.warnings] };
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
        // Below the free-space minimum, older snapshots go as well, as many as it takes:
        // the trial run can't name them, so that is asked about too.
        if (r.removed.length || r.space_floor_unmet) pruneConfirm = { removed: r.removed, forSpace: r.space_floor_unmet };
        else {
          // No snapshot goes, but a Vault may still hold data that none uses (an
          // interrupted backup, or a deletion by an older version). Collecting it removes
          // no snapshots, so it needs no confirmation.
          const done = await api.prune(id, false);
          result = done.bytes_freed
            ? { ok: true, text: `Every snapshot is kept by the retention rules; reclaimed ${bytes(done.bytes_freed)} of data no snapshot used.` }
            : { ok: true, text: "Nothing to remove: every snapshot is kept by the retention rules." };
          if (done.removed.length || done.removed_for_space.length || done.bytes_freed) app.revision++;
        }
      } else {
        pruneConfirm = null;
        const gone = r.removed.length + r.removed_for_space.length;
        result = {
          ok: true,
          text: `Removed ${gone} ${gone === 1 ? "snapshot" : "snapshots"}${r.removed_for_space.length ? ` (${r.removed_for_space.length} of them to free space)` : ""}${r.bytes_freed != null ? ` and reclaimed ${bytes(r.bytes_freed)}` : ""}.${r.space_floor_unmet ? " Free space is still below this backup's minimum: no more snapshots may be removed." : ""}`,
        };
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
  <Callout tone="bad">{error}</Callout>
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
  <!-- Compact: the settings and the places beside the rest, once there's room for both. -->
  {#snippet side()}
    <SettingsGlance {id} sentry={data!.sentry} column={compact} />
    {#if data!.profile.destinations?.length}
      {@const prof = data!.profile}
      {@const last = data!.recent_runs.find((r) => r.status !== "skipped" && r.status !== "dry_run")}
      {@const places = 1 + (prof.destinations?.length ?? 0)}
      {@const failedPlaces = (last?.status === "failed" ? 1 : 0) + (prof.destinations ?? []).filter((d) => last?.destinations?.find((x) => x.name === d.name)?.status === "failed").length}
      <!-- Compact: one line until opened (where and how are in the glance above). -->
      <svelte:element this={compact ? "details" : "section"} class="group/kept {compact ? 'mt-auto' : ''}">
        {#if compact}
          <summary class="flex cursor-pointer list-none items-center gap-2 rounded-md px-0.5 py-1 text-[13px] font-semibold text-fg-2 select-none hover:text-fg [&::-webkit-details-marker]:hidden">
            <Icon name="chevronRight" size={14} class="transition-transform group-open/kept:rotate-90" />
            Where it's kept
            <span class="font-normal text-fg-muted">{places} {places === 1 ? "place" : "places"}</span>
            {#if failedPlaces}<span class="font-normal text-danger">· {failedPlaces} failed last time</span>{/if}
          </summary>
        {:else}
          <h2 class="group-title">Where it's kept</h2>
        {/if}
        <ul class="rows-flat {compact ? 'mt-2' : ''}">
          {#snippet place(name: string, target: string, remote: boolean, offsite: boolean | undefined, engine: string, status: RunRecord["status"] | null, err: string | null | undefined, browse: string, copy = false)}
            {@const said = status ? (status === "failed" ? "Failed last time" : runStatusLabel[status]) : null}
            {@const saidClass = status === "failed" ? "cursor-help text-danger underline decoration-dotted underline-offset-4" : "text-fg-2"}
            <li class="row min-h-12 gap-3 py-2 text-sm">
              <Icon name={remote ? "server" : "drive"} size={17} class="shrink-0 text-fg-2" />
              <span class="min-w-0 flex-1">
                <span class="flex items-center gap-2">
                  <span class="truncate font-medium">{name}</span>
                  <span class="tag text-[11.5px]">{engine}</span>
                  {#if copy}<span class="tag text-[11.5px]">Copy</span>{/if}
                  {#if offsite}<span class="tag text-[11.5px]">Offsite</span>{/if}
                </span>
                <span class="block truncate font-mono text-[12px] text-fg-muted" title={target}>{target}</span>
                {#if compact && said}<span class="block text-[12.5px] {saidClass}" title={err ?? ""}>{said}</span>{/if}
              </span>
              {#if !compact && said}
                <span class="shrink-0 text-[13px] {saidClass}" title={err ?? ""}>{said}</span>
              {/if}
              <button class="btn btn-ghost h-7 shrink-0 px-2.5 text-[13px]" onclick={() => ((app.snapshotsOf = browse === data?.profile.id ? null : browse), (app.tab = "snapshots"))}>Browse</button>
            </li>
          {/snippet}
          {@render place(prof.target.split("/").filter(Boolean).pop() ?? prof.target, prof.target, prof.remote, prof.offsite, storageWords[prof.engine], last?.status ?? null, last?.error, prof.id)}
          {#each prof.destinations ?? [] as d (d.id)}
            {@const r = last?.destinations?.find((x) => x.name === d.name)}
            {@render place(d.label, d.target, d.remote, d.offsite, storageWords[d.engine], r?.status ?? null, r?.error, d.id, d.replica)}
          {/each}
        </ul>
      </svelte:element>
    {/if}
  {/snippet}
  <div class={compact ? "@container" : ""}>
  <div class={compact ? "grid items-stretch gap-7 @min-[760px]:grid-cols-[minmax(0,1fr)_18.5rem] @min-[760px]:gap-x-6" : ""}>
  <div class="flex min-w-0 flex-col {compact ? 'gap-4' : 'gap-7'}">
    <!-- The answer, in one sentence. -->
    <!-- Compact: the sentence, a running backup's progress and the last 30 days are one
         card, under a heading level with the side column's, so the two columns start
         together. -->
    <section>
    {#if compact}<h2 class="mb-2 flex h-7 items-center px-0.5 text-[13px] font-semibold text-fg-2">Status</h2>{/if}
    <div class={compact ? "card px-5 pt-3.5 pb-2.5" : ""}>
    <div class="flex items-start {compact ? 'gap-3.5' : 'gap-4'}" aria-live="polite">
      <span class="glyph {glyph[tone].cls} {compact ? 'size-10' : 'size-12'}"><Icon name={glyph[tone].icon} size={compact ? 20 : 24} /></span>
      <div class="min-w-0 pt-0.5">
        <h2 class="{compact ? 'text-[18px]' : 'text-[22px]'} leading-tight font-semibold tracking-[-0.01em] text-balance">{sentence[tone](data.profile.name)}</h2>
        <p class="num {compact ? 'mt-1 text-[12.5px] text-pretty' : 'mt-1.5 text-[14px]'} text-fg-2">
          <span class="whitespace-nowrap">Last backup <span class="text-fg">{data.latest ? ago(data.latest.finished) : "never"}</span></span>
          {#if !summary?.running}
          <span class="text-fg-muted">·</span>
          <span class="whitespace-nowrap">Next <span class="text-fg">{summary ? nextRun(summary, status ?? { next_run: data.next_run, timer_active: data.timer_active, last_good: null, health: [], days: [] }, pausedNext ? pauseUntil(app.pause!) : null) : ""}</span></span>
          {#if !manual && !data.timer_active}<span class="text-warning"> (its timer isn't installed)</span>{/if}
          {/if}
          <span class="text-fg-muted">·</span>
          <span class="whitespace-nowrap"><span class="text-fg">{count(data.snapshot_count)}</span> {data.snapshot_count === 1 ? "snapshot" : "snapshots"}{#if data.oldest}<span class="text-fg-muted">{" "}since {shortDate(data.oldest)}</span>{/if}</span>
        </p>
        {#if prot?.detail && tone !== "ok" && !data.unusual}
          <p class="mt-2 max-w-[68ch] {compact ? 'text-[13px]' : 'text-[14px]'} {tone === 'bad' ? 'text-danger' : tone === 'warn' ? 'text-warning' : 'text-fg-2'}">{prot.detail}</p>
        {/if}
        {#if last && last.status === "failed" && !status?.health.length}
          {@const hint = failureHint(last)}
          {#if hint}<p class="mt-1 max-w-[68ch] text-[14px] text-fg">{hint}</p>{/if}
        {/if}
      </div>
    </div>
    {#if compact && app.jobs[id]}
      <div class="mt-3 border-t border-[var(--c-sep)] pt-2.5"><JobPanel job={app.jobs[id]!} inline /></div>
    {/if}
    {#if compact && status}
      <div class="mt-3 border-t border-[var(--c-sep)] pt-2.5" title="Last 30 days: one bar a day, taller bars copied more">
        <HistoryStrip days={status.days} fill low />
      </div>
    {/if}
    </div>
    </section>

    {#if data.unusual}
      {@const u = data.unusual}
      <Callout tone="bad" title={u.stopped ? "Flux Sentry stopped the last backup: unusual changes in your files" : "Flux Sentry found unusual changes in the last backup"}>
        <ul class="mt-1 list-disc pl-5 text-fg">
          {#each u.signals as sig (sig)}<li>{sig}</li>{/each}
        </ul>
        <p class="mt-1.5">
          {count(u.affected_total)} files{#if u.by_folder.length}, mostly in <span class="font-mono text-[12.5px]">{u.by_folder[0]![0]}</span>{#if u.by_folder.length > 1}{" "}and {u.by_folder.length - 1} other {u.by_folder.length === 2 ? "folder" : "folders"}{/if}{/if}.
        </p>
        <p class="mt-1.5 max-w-[70ch]">
          {#if u.stopped}<span class="font-semibold text-fg">Nothing was backed up:</span> your backups are as they were before these changes.{" "}{/if}<span class="font-semibold text-fg">Backups are on hold</span> until this is settled: none is made, by schedule or by hand, nothing old is removed, and the other places keep their copies from before.
        </p>
        <p class="mt-1.5 max-w-[70ch]">
          {#if u.before}
            If you didn't make these changes, don't delete or change anything: the {u.stopped ? "newest snapshot, from before them," : "snapshot before them"} {u.before_pinned === false ? "is there, but couldn't be pinned: pin it in Snapshots so it isn't removed" : "is kept (pinned)"}. <em>Restore everything</em> puts every file back as it was then and lets backups go on; or restore what you need yourself, then choose <em>It's dealt with</em>. If you did make them, choose <em>This was me</em>.
          {:else}
            If you didn't make these changes, don't delete or change anything. There is no snapshot from before them to go back to here (none had been made, or the backups couldn't be reached). Once your files are as they should be, choose <em>It's dealt with</em>; if you made the changes, choose <em>This was me</em>.
          {/if}
        </p>
        {#snippet actions()}
          {#if u.before}
            {@const before = u.before}
            <button class="btn btn-primary" title="Put every file back as it was in the snapshot from before these changes. You see what it will do first, and what happens to files that differ now. Backups go on when it's done." onclick={() => void recover(before, u.snapshot)}><Icon name="restore" size={14} /> Restore everything…</button>
            <button class="btn" onclick={() => ((app.snapshotsOf = null), (app.showSnapshot = before), (app.tab = "snapshots"))}>Browse the snapshot before</button>
          {/if}
          <button class="btn" onclick={() => (showingAffected = true)}>Show the files</button>
          <button class="btn btn-ghost" title="The changes were yours: backups go on, and Flux Sentry takes them as normal for this backup" onclick={() => void dismissUnusual(u.snapshot)}>This was me</button>
          <button class="btn btn-ghost" title="The changes weren't yours and you've restored what you need: backups go on, and the snapshot from before stays pinned" onclick={() => void dismissUnusual(u.snapshot, true)}>It's dealt with</button>
        {/snippet}
      </Callout>
    {/if}

    <!-- Further problems beside the one in the sentence: next to it, not below the fold. -->
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

    {#if status && !compact}
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
              : `It runs ${scheduleText(data.profile.schedule)} from now on, storing only what changed.`}
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

    <BorgKeyNotice {id} onlocked={onLocked} />

    {#if !compact}{@render side()}{/if}

    <section>
      <div class="flex items-center justify-between gap-3 px-0.5 {compact ? 'pb-1' : 'pb-2'}">
        <h2 class="text-[13px] font-semibold text-fg-2">Recent backups</h2>
        <div class="flex gap-1">
          {#if data.latest}<button class="btn btn-ghost h-7 px-2.5 text-[13px]" onclick={() => (app.tab = "snapshots")}>Browse files</button>{/if}
          {#if data.recent_runs.length}<button class="btn btn-ghost h-7 px-2.5 text-[13px]" onclick={() => (app.tab = "activity")}>All activity</button>{/if}
        </div>
      </div>
      <ul class="rows-flat">
        {#each data.recent_runs.slice(0, 3) as r, i (r.run_id + r.started)}
          <li class="row {compact ? 'min-h-[34px] py-1 text-[13px]' : 'min-h-11 py-2 text-sm'}" title={r.error ?? `${dateTime(r.started)} · ${r.trigger}`}>
            <span class="size-2 shrink-0 rounded-full {dot[r.status]}" aria-hidden="true"></span>
            <span class="shrink-0 font-medium">{runStatusLabel[r.status]}</span>
            <!-- An error takes the middle; otherwise the size sits by the time, on the right. -->
            {#if r.error || !r.stats || !r.snapshot}
              <span class="num min-w-0 flex-1 truncate text-danger">{r.error ?? ""}</span>
            {:else}
              <!-- What it changed; a click compares it with the snapshot before. -->
              {@const words = changeWords(r.stats)}
              {@const before = snapshotBefore(data.recent_runs, i)}
              <span class="min-w-0 flex-1">
                {#if words === "No changes"}
                  <span class="num text-fg-muted">{words}</span>
                {:else}
                  <!-- Compact says it as a diff does (+new ~changed −deleted); the words in
                       its title and for screen readers. -->
                  <button
                    class="num max-w-full truncate rounded px-1 -mx-1 text-left text-fg-2 hover:bg-surface-3 hover:text-fg"
                    title="{words}. {before ? 'Compare with the backup before' : 'See this snapshot'}."
                    aria-label="{words}: {before ? 'compare with the backup before' : 'see this snapshot'}"
                    onclick={() => {
                      if (before) comparing = { old: before, newer: r.snapshot!, when: r.started };
                      else (app.showSnapshot = r.snapshot), (app.tab = "snapshots");
                    }}>{compact ? changeMarks(r.stats) : words}</button
                  >
                {/if}
              </span>
            {/if}
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

    <!-- Can it be restored: the last restore test, and (rsync, Borg) the guide to do it
         without Flux. Part of the page, not raised: only the answer at the top is. -->
    {#if data.restore_test || isCompat(data.profile.engine) || spaceRow}
    <div class="rows-flat">
    {#if data.restore_test}
      {@const rt = data.restore_test}
      {@const last = rt.last}
      <div class="row gap-3" title={compact && last?.guarantee && last.result !== "failed" ? guaranteeWords(last.guarantee) : undefined}>
        <span class="glyph {last?.result === 'failed' ? 'glyph-bad' : last?.result === 'warning' ? 'glyph-warn' : last ? 'glyph-ok' : ''} {compact ? '' : 'size-8'}"><Icon name={last?.result === "failed" ? "alert" : "shieldCheck"} size={compact ? 13 : 16} /></span>
        <span class="min-w-0 flex-1">
          <span class="block font-medium">
            {#if !last}No restore test yet{:else if last.result === "failed"}The last restore test failed{:else if last.result === "warning"}Restore test passed, with notes{:else}Restore test passed{/if}
          </span>
          <span class="block text-[13px] text-fg-2 {compact ? 'truncate text-[12.5px]' : ''}">
            {#if last}
              {ago(last.started)} · {count(last.tested)} files read back and checked{#if last.result !== "passed"} · {last.error ?? `${last.problems_total} ${last.problems_total === 1 ? "problem" : "problems"}`}{/if}
            {:else}
              Read a few files back and check every byte, to know the backup can be restored. Nothing is written anywhere.
            {/if}
            {#if rt.every !== "off"} · {rt.every === "backup" ? "after every backup" : `every ${rt.every}`}{/if}
          </span>
          {#if last?.guarantee && last.result !== "failed" && !compact}
            <span class="block text-[13px] text-fg-muted">{guaranteeWords(last.guarantee)}</span>
          {/if}
        </span>
        <button class="btn shrink-0 {compact ? 'h-7 px-2.5 text-[13px]' : ''}" disabled={testing} onclick={() => void testNow()}>
          {#if testing}<Icon name="refresh" size={14} class="motion-safe:animate-spin" /> Testing…{:else}{compact ? "Test now" : "Test a restore now"}{/if}
        </button>
      </div>
    {/if}

    {#if isCompat(data.profile.engine)}
      <button class="row row-link" onclick={() => (app.dialog = { kind: "recovery", id })}>
        <span class="glyph glyph-ok {compact ? '' : 'size-8'}"><Icon name="lifebuoy" size={compact ? 13 : 16} /></span>
        <span class="min-w-0 flex-1">
          <span class="block font-medium">Usable without Flux</span>
          <span class="block text-[13px] text-fg-2 {compact ? 'truncate text-[12.5px]' : ''}">
            {data.profile.engine === "borg"
              ? "A standard Borg repository. The recovery guide next to it says how to restore with borg alone."
              : "Plain folders written by rsync. The recovery guide next to them says how to restore with rsync or a file manager."}
          </span>
        </span>
        <span class="shrink-0 text-[13px] text-fg-2">Recovery guide</span>
        <Icon name="chevronRight" size={16} class="shrink-0 text-fg-muted" />
      </button>
    {/if}

    {#if spaceRow && data.space && data.space.size}
      <!-- Compact: space on one line (free, a meter, how full); the forecast and what the
           snapshots take open under it. -->
      {@const sp = data.space}
      {@const spSize = data.space.size}
      {@const full = Math.min(100, Math.max(0, Math.round(((spSize - sp.free) / spSize) * 100)))}
      {@const drive = data.profile.target.split("/").filter(Boolean).at(-1) ?? data.profile.target}
      <details class="group/space shadow-[inset_0_1px_0_var(--c-sep)]" open={sp.days_left != null && sp.days_left < 60}>
        <summary class="row row-link cursor-pointer list-none gap-3 px-1.5 [&::-webkit-details-marker]:hidden">
          <span class="glyph"><Icon name={data.profile.remote ? "server" : "drive"} size={13} /></span>
          <span class="min-w-0 flex-1">
            <span class="block font-medium">Space on {drive}</span>
            <span class="num block truncate text-[12.5px] {sp.days_left != null && sp.days_left < 30 ? 'text-warning' : 'text-fg-2'}">{bytes(sp.free)} free of {bytes(spSize)}</span>
          </span>
          <span class="h-1.5 w-24 shrink-0 overflow-hidden rounded-full bg-track" role="img" aria-label="{full}% full">
            <span class="block h-full rounded-full {full >= 90 ? 'bg-warning' : 'bg-primary'}" style:width="{full}%"></span>
          </span>
          <span class="num w-16 shrink-0 text-right text-[13px] text-fg-2">{full}% full</span>
          <Icon name="chevronRight" size={15} class="shrink-0 text-fg-muted transition-transform group-open/space:rotate-90" />
        </summary>
        <div class="px-1.5 pt-1 pb-4">
          <SpaceCard space={sp} target={drive} flat health={data.drive_health} notes={data.drive_notes ?? []}>
            <StorageCard {id} embedded />
          </SpaceCard>
        </div>
      </details>
    {/if}
    </div>
    {/if}

    <!-- Space: how full the drive is, and what the snapshots take of it. -->
    {#if compact && data.space && data.space.size}
      <!-- In the list above. -->
    {:else if data.space && data.space.size}
      <SpaceCard space={data.space} target={data.profile.target.split("/").filter(Boolean).at(-1) ?? data.profile.target} health={data.drive_health} notes={data.drive_notes ?? []}>
        <StorageCard {id} embedded />
      </SpaceCard>
    {:else}
      <StorageCard {id} folded={compact} />
    {/if}

    {#if vaultFormat && vaultFormat.format < vaultFormat.newest}
      <Callout tone="info" icon="vault" compact title="This Vault can be upgraded">
        Format {vaultFormat.newest} keeps sparse files (disk images) small. Its snapshots stay as they are.
        {#snippet actions()}<button class="btn h-8" onclick={() => (confirmingUpgrade = true)} disabled={busy !== null}>Upgrade…</button>{/snippet}
      </Callout>
    {/if}

    <!-- Less often needed: folded, one line each until opened. In Compact they sit at the
         column's foot, level with the side column's last line. -->
    <div class="flex flex-col gap-3 {compact ? 'mt-auto' : ''}">
      <Readiness {id} onlocked={onLocked} />
      <details class="group" open={busy !== null || pruneConfirm != null || result != null}>
        <summary class="flex cursor-pointer list-none items-center gap-2 rounded-md px-0.5 py-1 text-[13px] font-semibold text-fg-2 select-none hover:text-fg [&::-webkit-details-marker]:hidden">
          <Icon name="chevronRight" size={14} class="transition-transform group-open:rotate-90" />
          Checks and upkeep
          <span class="font-normal text-fg-muted">What each check proves</span>
        </summary>
      <div class="rows mt-2">
        <!-- Three checks, each the answer to one question, from quickest to most thorough. -->
        <div class="row">
          <span class="min-w-0 flex-1"><span class="block font-medium">Is every snapshot whole?</span><span class="block text-[13px] text-fg-2"><span class="text-fg">Check</span> reads each snapshot's record and structure, not the data. Quick.</span></span>
          <button class="btn" onclick={() => check(false)} disabled={busy !== null}>{busy === "check" ? "Checking…" : "Check"}</button>
        </div>
        <div class="row">
          <span class="min-w-0 flex-1"><span class="block font-medium">Can files come back?</span><span class="block text-[13px] text-fg-2">A <span class="text-fg">restore test</span> reads some files back the way a restore does and checks every byte. Nothing is written anywhere.</span></span>
          <button class="btn" onclick={() => void testNow()} disabled={busy !== null || testing}>{testing ? "Testing…" : "Test a restore"}</button>
        </div>
        <div class="row">
          <span class="min-w-0 flex-1"><span class="block font-medium">Is all the stored data undamaged?</span><span class="block text-[13px] text-fg-2"><span class="text-fg">Verify every byte</span> reads everything stored back and checks it: it finds a failing drive. As long as reading the whole backup.</span></span>
          <button class="btn" onclick={() => check(true)} disabled={busy !== null}>{busy === "verify" ? "Verifying…" : "Verify every byte"}</button>
        </div>
        <div class="row">
          <span class="min-w-0 flex-1"><span class="block font-medium">Free up space</span><span class="block text-[13px] text-fg-2">Removes the snapshots the retention rules no longer keep. Asks first.</span></span>
          <button class="btn" onclick={() => prune(true)} disabled={busy !== null}>{busy === "prune" ? "Working…" : "Free up space"}</button>
        </div>
      </div>
      </details>
      {#if pruneConfirm}
        <Callout class="mt-3" title={pruneConfirm.removed.length ? `Retention would remove ${pruneConfirm.removed.length} ${pruneConfirm.removed.length === 1 ? "snapshot" : "snapshots"}:` : "Free space is below this backup's minimum"}>
          {#if pruneConfirm.removed.length}
            <div class="mt-1 max-h-24 overflow-y-auto font-mono text-xs">{pruneConfirm.removed.join(", ")}</div>
          {/if}
          {#if pruneConfirm.forSpace}
            <p class="mt-1 text-sm">Free space is below the minimum set for this backup, so older snapshots would be removed {pruneConfirm.removed.length ? "as well" : ""}, oldest first, until there is enough. Pinned and named snapshots and the newest ones stay.</p>
          {/if}
          {#snippet actions()}
            <button class="btn btn-danger-filled" onclick={() => prune(false)} disabled={busy !== null}>{pruneConfirm?.removed.length ? "Remove them" : "Remove old snapshots"}</button>
            <button class="btn btn-ghost" onclick={() => (pruneConfirm = null)}>Not now</button>
          {/snippet}
        </Callout>
      {/if}
      {#if result}
        <Callout class="mt-3" tone={result.ok ? "ok" : "bad"} title={result.text}>
          {#if result.details?.length}
            <ul class="mt-1 max-h-40 overflow-y-auto font-mono text-xs">{#each result.details as d (d)}<li class="truncate">{d}</li>{/each}</ul>
          {/if}
        </Callout>
      {/if}
    </div>
  </div>
  {#if compact}
    <aside class="flex min-w-0 flex-col gap-7" aria-label="About this backup">{@render side()}</aside>
  {/if}
  </div>
  </div>
{/if}

{#if comparing}
  <CompareDialog profile={id} old={comparing.old} newer={comparing.newer} title="What the backup of {dateTime(comparing.when)} changed" onclose={() => (comparing = null)} />
{/if}
{#if confirmingUpgrade}
  <ConfirmDialog title="Upgrade the Vault?" icon="vault" confirm={upgrading ? "Upgrading…" : "Upgrade"} busy={upgrading} onconfirm={() => void upgradeVault()} onclose={() => (confirmingUpgrade = false)}>
    <p>New snapshots then keep the holes of sparse files, so disk images and similar files take only the space of their data. The snapshots already there stay as they are.</p>
    <p class="mt-2">Flux Backup before 1.0 can't open the Vault afterwards: if another computer uses it with an older Flux, update that one first.</p>
  </ConfirmDialog>
{/if}

{#if recovering}
  {@const rec = recovering}
  <RestoreDialog
    profile={id}
    snapshot={rec.snapshot}
    paths={rec.paths}
    startAt="original"
    note="Everything this backup covers, as it was before the unusual changes. What was added since stays: look through it afterwards. When a restore back to where it was is done, backups go on; after a restore into another folder they stay on hold until you choose It's dealt with."
    onrestored={(_r, where) => {
      // Only files put back where they were settle it: a copy in another folder, to look
      // at first, leaves the files as the attack left them.
      if (where === "original") void dismissUnusual(rec.alarm, true);
      else app.toast("ok", "Restored into another folder. Backups stay on hold: choose It's dealt with once your files are as they should be.");
    }}
    onclose={() => (recovering = null)}
  />
{/if}

{#if showingAffected && data?.unusual}
  {@const u = data.unusual}
  <Modal title="Files involved" onclose={() => (showingAffected = false)} width="max-w-2xl">
    <p class="-mt-1 mb-3 text-[13px] text-fg-2">
      {count(u.affected_total)} files changed, deleted or added {u.stopped ? (u.before ? `since snapshot ${u.before}` : "since the last backup") : `in snapshot ${u.snapshot}`}{u.affected_total > u.affected.length ? ` (the first ${count(u.affected.length)} shown)` : ""}.
      {#if u.by_extension.length}By type: {u.by_extension.map(([x, n]) => `${x === "(none)" ? "no extension" : `.${x}`} (${n})`).join(", ")}.{/if}
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
