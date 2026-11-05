<script lang="ts">
  import { api, listen, pickFolder } from "../api";
  import { app } from "../state.svelte";
  import { bytes, count, dateTime } from "../format";
  import type { ProgressSnapshot, RestoreOutcome, SnapshotMeta } from "../types";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";

  let { profile, snapshot, paths, onclose }: { profile: string; snapshot: SnapshotMeta; paths: string[]; onclose: () => void } = $props();

  let where = $state<"original" | "elsewhere">("elsewhere");
  let folder = $state<string | null>(null);
  let overwrite = $state(false);
  let busy = $state(false);
  let result = $state<RestoreOutcome | null>(null);
  let error = $state<string | null>(null);
  let progress = $state<ProgressSnapshot | null>(null);

  $effect(() => {
    let off: (() => void) | undefined;
    void listen<{ profile: string; progress: ProgressSnapshot }>("flux://restore", (e) => {
      if (e.profile === profile && busy) progress = e.progress;
    }).then((f) => (off = f));
    return () => off?.();
  });

  async function choose() {
    const f = await pickFolder("Restore into…");
    if (f) folder = f;
  }

  async function run() {
    busy = true;
    error = null;
    try {
      result = await api.restore(profile, snapshot.id, paths, where === "elsewhere" ? folder : null, where === "original" && overwrite);
      if (result.errors.length === 0) app.toast("ok", `Restored ${count(result.restored)} items.`);
    } catch (e) {
      error = (e as Error).message;
    } finally {
      busy = false;
    }
  }
</script>

<Modal title="Restore from {dateTime(snapshot.started)}" {onclose}>
  {#if result}
    <div class="space-y-4">
      <div class="flex items-center gap-3">
        <div class="grid size-10 place-items-center rounded-xl {result.errors.length ? 'bg-warning/15 text-warning' : 'bg-success/15 text-success'}">
          <Icon name={result.errors.length ? "alert" : "check"} />
        </div>
        <div>
          <div class="font-medium">{result.errors.length ? "Restored with problems" : "Restored"}</div>
          <div class="tabular text-sm text-fg-2">{count(result.restored)} items · {bytes(result.bytes)} · {count(result.skipped_identical)} already identical</div>
        </div>
      </div>
      {#if result.moved_aside.length}
        <div class="rounded-xl bg-surface-3 p-3 text-sm">
          <div class="font-medium">Your existing versions were kept:</div>
          <ul class="mt-1 max-h-28 overflow-y-auto font-mono text-xs text-fg-2">
            {#each result.moved_aside as p (p)}<li class="truncate">{p}</li>{/each}
          </ul>
        </div>
      {/if}
      {#if result.errors.length}
        <ul class="max-h-36 overflow-y-auto rounded-xl bg-danger/8 p-3 text-xs text-danger">
          {#each result.errors as e (e.path)}<li class="truncate">{e.path}: {e.message}</li>{/each}
        </ul>
      {/if}
      <div class="flex justify-end"><button class="btn btn-primary" onclick={onclose}>Done</button></div>
    </div>
  {:else}
    <div class="space-y-5">
      <div>
        <div class="label mb-2">Restoring</div>
        <ul class="max-h-28 overflow-y-auto rounded-xl bg-surface-3 p-3 font-mono text-xs text-fg-2">
          {#each paths as p (p)}<li class="truncate">{p}</li>{/each}
        </ul>
      </div>

      <fieldset class="space-y-2">
        <legend class="label mb-2">Where to</legend>
        <label class="flex cursor-pointer gap-3 rounded-2xl p-4 transition-colors {where === 'elsewhere' ? 'bg-primary-container text-on-primary-container' : 'bg-surface-3 hover:bg-track'}">
          <input type="radio" class="mt-1 accent-[var(--c-primary)]" bind:group={where} value="elsewhere" />
          <span class="min-w-0 flex-1">
            <span class="block font-medium">Into another folder</span>
            <span class="block text-sm text-fg-muted">Safest: nothing on your computer is touched.</span>
            {#if where === "elsewhere"}
              <span class="mt-2 flex items-center gap-2">
                <button type="button" class="btn h-8" onclick={choose}><Icon name="folder" size={14} /> Choose folder…</button>
                <span class="truncate font-mono text-xs text-fg-2">{folder ?? "none chosen"}</span>
              </span>
            {/if}
          </span>
        </label>
        <label class="flex cursor-pointer gap-3 rounded-2xl p-4 transition-colors {where === 'original' ? 'bg-primary-container text-on-primary-container' : 'bg-surface-3 hover:bg-track'}">
          <input type="radio" class="mt-1 accent-[var(--c-primary)]" bind:group={where} value="original" />
          <span class="min-w-0 flex-1">
            <span class="block font-medium">Back to where it was</span>
            <span class="block text-sm text-fg-muted">
              Files that are already identical are skipped.
              {overwrite ? "Different files are replaced." : "Different files are kept, renamed to “name.flux-…”."}
            </span>
            {#if where === "original"}
              <label class="mt-2 flex items-center gap-2 text-sm">
                <input type="checkbox" class="accent-[var(--c-danger)]" bind:checked={overwrite} />
                Replace my current versions instead of keeping them
              </label>
            {/if}
          </span>
        </label>
      </fieldset>

      {#if error}<div class="rounded-xl bg-danger/10 p-3 text-sm text-danger">{error}</div>{/if}

      {#if busy && progress && progress.bytes_total > 0}
        <div class="space-y-1.5">
          <div class="h-1.5 overflow-hidden rounded-full bg-track" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round((progress.bytes_done / progress.bytes_total) * 100)}>
            <div class="h-full rounded bg-primary motion-safe:transition-[width] motion-safe:duration-300" style="width: {Math.min(100, (progress.bytes_done / progress.bytes_total) * 100)}%"></div>
          </div>
          <div class="tabular text-xs text-fg-muted">{bytes(progress.bytes_done)} of {bytes(progress.bytes_total)} · {count(progress.entries_done)} of {count(progress.entries_total)} items</div>
        </div>
      {/if}

      <div class="flex justify-end gap-2">
        <button class="btn" onclick={onclose} disabled={busy}>Cancel</button>
        <button class="btn btn-primary" onclick={run} disabled={busy || (where === "elsewhere" && !folder)}>
          <Icon name={busy ? "refresh" : "restore"} size={15} class={busy ? "animate-spin" : ""} />
          {busy ? "Restoring…" : "Restore"}
        </button>
      </div>
    </div>
  {/if}
</Modal>
