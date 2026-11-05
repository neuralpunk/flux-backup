<script lang="ts">
  import { api } from "../api";
  import { app } from "../state.svelte";
  import { ago, bytes, count, dateTime, runStatusLabel } from "../format";
  import { ApiError, type Overview, type RunRecord } from "../types";
  import Icon from "./Icon.svelte";

  let { id, onLocked }: { id: string; onLocked: (retry: () => void) => void } = $props();
  let data = $state<Overview | null>(null);
  let error = $state<string | null>(null);
  let locked = $state(false);

  function load() {
    const current = id;
    api.overview(current).then(
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

  $effect(() => {
    // Reload when the profile changes, a backup finishes, or the profile list refreshes.
    void app.revision;
    void app.profiles;
    load();
  });

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
    if (e.includes("password")) return "Unlock the Vault or store its password in the keyring (flux key store).";
    if (e.includes("ssh")) return "Check the connection with `flux ssh test`.";
    if (e.includes("space")) return "Free up space on the target or tighten the retention rules.";
    return null;
  }
</script>

{#snippet stat(label: string, value: string, hint: string, warning: string | null = null)}
  <div class="card flex min-w-0 flex-col gap-1.5 px-5 py-[18px]" title={hint}>
    <span class="label">{label}</span>
    <span class="num truncate text-[22px] font-medium">{value}</span>
    {#if warning}<span class="truncate text-[12.5px] text-warning">{warning}</span>{/if}
  </div>
{/snippet}

{#if locked}
  <div class="card grid place-items-center gap-3 p-14 text-center">
    <div class="grid size-12 place-items-center rounded-2xl bg-primary-container text-on-primary-container"><Icon name="lock" size={22} /></div>
    <div class="title">This Vault is locked</div>
    <p class="max-w-sm text-sm text-fg-2">Its contents are encrypted. Enter the password to see snapshots, browse files and restore.</p>
    <button class="btn btn-tonal mt-2 h-10" onclick={() => onLocked(load)}>Unlock</button>
  </div>
{:else if error}
  <div class="card flex items-center gap-3 bg-danger/10 p-5 text-danger"><Icon name="alert" /> {error}</div>
{:else if !data}
  <div class="grid grid-cols-2 gap-3 lg:grid-cols-4">
    {#each [0, 1, 2, 3] as i (i)}
      <div class="card h-[86px] motion-safe:animate-pulse"></div>
    {/each}
  </div>
{:else}
  {@const last = data.profile.last_run}
  {@const manual = data.profile.schedule.startsWith("manual")}
  <div class="flex flex-col gap-3">
    {#if last && last.status === "failed"}
      {@const hint = failureHint(last)}
      <div class="card flex gap-4 bg-danger/10 px-6 py-5">
        <Icon name="alert" class="mt-0.5 shrink-0 text-danger" />
        <div class="min-w-0">
          <div class="title">The last backup failed {ago(last.finished)}</div>
          <p class="mt-1 text-sm break-words text-fg-2">{last.error}</p>
          {#if hint}<p class="mt-2 text-sm text-fg">{hint}</p>{/if}
        </div>
      </div>
    {/if}

    {#if data.repo_error}
      <div class="card flex items-center gap-3 bg-warning/10 px-6 py-4 text-sm">
        <Icon name="alert" class="shrink-0 text-warning" />
        <span><span class="font-medium">Snapshots can't be read right now.</span> <span class="text-fg-2">{data.repo_error}</span></span>
      </div>
    {/if}

    <div class="grid grid-cols-2 gap-3 lg:grid-cols-4">
      {@render stat("Last backup", data.latest ? ago(data.latest.finished) : "Never", data.latest ? dateTime(data.latest.finished) : "Start one with “Back up now”")}
      {@render stat(
        "Next backup",
        data.next_run ? ago(data.next_run) : manual ? "Manual" : "Not scheduled",
        manual ? "Only when you start it" : data.profile.schedule,
        manual || data.timer_active ? null : "Timer not installed",
      )}
      {@render stat("Snapshots", count(data.snapshot_count), data.oldest ? `since ${dateTime(data.oldest)}` : "none yet")}
      {@render stat("Free on target", bytes(data.free_bytes), data.free_bytes == null ? "not available right now" : "on the backup drive")}
    </div>

    <section class="card px-6 py-5">
      <div class="flex items-center justify-between gap-3">
        <h2 class="title">Recent backups</h2>
        <div class="flex gap-2">
          {#if data.latest}<button class="btn" onclick={() => (app.tab = "snapshots")}>Browse files</button>{/if}
          {#if data.recent_runs.length}<button class="btn btn-ghost" onclick={() => (app.tab = "activity")}>All activity</button>{/if}
        </div>
      </div>
      <ul class="mt-3">
        {#each data.recent_runs.slice(0, 5) as r (r.run_id + r.started)}
          <li class="flex items-center gap-3 py-2 text-sm" title={r.error ?? `${dateTime(r.started)} · ${r.trigger}`}>
            <span class="size-2 shrink-0 rounded-full {dot[r.status]}" aria-hidden="true"></span>
            <span class="w-40 shrink-0">{runStatusLabel[r.status]}</span>
            <span class="num min-w-0 flex-1 truncate text-fg-muted">
              {#if r.error}{r.error}{:else if r.stats}{bytes(data.profile.engine === "vault" ? r.stats.stored_bytes : r.stats.copied_bytes)} {data.profile.engine === "vault" ? "stored" : "copied"}{/if}
            </span>
            <span class="shrink-0 text-[12.5px] text-fg-muted">{ago(r.started)}</span>
          </li>
        {:else}
          <li class="py-3 text-sm text-fg-muted">No backups yet. They'll appear here.</li>
        {/each}
      </ul>
    </section>

    <section class="card px-6 py-5">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <h2 class="title">Maintenance</h2>
        <div class="flex flex-wrap gap-2">
          <button class="btn" onclick={() => check(false)} disabled={busy !== null}>{busy === "check" ? "Checking…" : "Check backups"}</button>
          <button class="btn" onclick={() => check(true)} disabled={busy !== null} title="Reads all stored data back; can take a while">{busy === "verify" ? "Verifying…" : "Verify every byte"}</button>
          <button class="btn" onclick={() => prune(true)} disabled={busy !== null}>{busy === "prune" ? "Working…" : "Free up space"}</button>
        </div>
      </div>
      {#if pruneConfirm}
        <div class="mt-4 rounded-2xl bg-warning/10 p-4 text-sm">
          <div class="font-medium">Retention would remove {pruneConfirm.length} snapshots:</div>
          <div class="mt-1 max-h-24 overflow-y-auto font-mono text-xs text-fg-2">{pruneConfirm.join(", ")}</div>
          <div class="mt-3 flex gap-2">
            <button class="btn btn-danger" onclick={() => prune(false)} disabled={busy !== null}>Remove them</button>
            <button class="btn btn-ghost" onclick={() => (pruneConfirm = null)}>Not now</button>
          </div>
        </div>
      {/if}
      {#if result}
        <div class="mt-4 rounded-2xl p-4 text-sm {result.ok ? 'bg-success/10' : 'bg-danger/10'}">
          <div class="flex items-center gap-2 font-medium {result.ok ? 'text-success' : 'text-danger'}"><Icon name={result.ok ? "check" : "alert"} size={15} /> {result.text}</div>
          {#if result.details?.length}
            <ul class="mt-2 max-h-40 overflow-y-auto font-mono text-xs text-fg-2">{#each result.details as d (d)}<li class="truncate">{d}</li>{/each}</ul>
          {/if}
        </div>
      {/if}
    </section>
  </div>
{/if}
