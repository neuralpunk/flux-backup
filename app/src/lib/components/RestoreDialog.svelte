<script lang="ts">
  import { untrack } from "svelte";
  import { api, listen, pickFolder } from "../api";
  import { sizeLabel } from "../size";
  import { app } from "../state.svelte";
  import { bytes, count, dateTime } from "../format";
  import type { ProgressSnapshot, RestoreOutcome, RestorePreview, SnapshotMeta } from "../types";
  import Icon from "./Icon.svelte";
  import Callout from "./Callout.svelte";
  import Modal from "./Modal.svelte";

  let {
    profile,
    snapshot,
    paths,
    missing = false,
    startAt,
    note,
    onrestored,
    onclose,
  }: {
    profile: string;
    snapshot: SnapshotMeta;
    paths: string[];
    /** Everything picked is gone from disk: putting it back can't touch anything. */
    missing?: boolean;
    /** Which choice is made to begin with (default: another folder, or the original place for something gone). */
    startAt?: "original" | "elsewhere";
    /** Said above the choices (why this restore was offered). */
    note?: string;
    /** A restore finished without errors; `where`: back to the original place, or into another folder. */
    onrestored?: (r: RestoreOutcome, where: "original" | "elsewhere") => void;
    onclose: () => void;
  } = $props();

  // Bringing back something deleted: its own place is the natural (and safe) choice.
  // Otherwise another folder is safest.
  let where = $state<"original" | "elsewhere">(untrack(() => startAt ?? (missing ? "original" : "elsewhere")));
  let folder = $state<string | null>(null);
  let overwrite = $state(false);
  /** Also remove what's in the folders now but not in the snapshot. */
  let exact = $state(false);
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

  // What restoring would change at the chosen destination, checked before anything is written.
  let preview = $state<RestorePreview | null>(null);
  let previewError = $state<string | null>(null);
  $effect(() => {
    const to = where === "elsewhere" ? folder : null;
    const ow = where === "original" && overwrite;
    const ex = where === "original" && exact;
    preview = null;
    previewError = null;
    if (where === "elsewhere" && !folder) return;
    let current = true;
    const t = setTimeout(() => {
      api.restorePreview(profile, snapshot.id, paths, to, ow, ex).then(
        (p) => current && (preview = p),
        (e) => current && (previewError = (e as Error).message),
      );
    }, 150);
    return () => {
      current = false;
      clearTimeout(t);
    };
  });
  const removes = $derived(where === "original" && exact ? (preview?.remove ?? 0) : 0);
  const nothingToDo = $derived(preview != null && preview.new === 0 && preview.differ === 0 && preview.new_folders === 0 && removes === 0);
  /** The snapshot holds folders here but not one file: say so before restoring. */
  const onlyFolders = $derived(preview != null && preview.new === 0 && preview.differ === 0 && preview.identical === 0 && preview.new_folders > 0);
  const sizeLimit = $derived(app.profiles.find((p) => p.id === profile)?.size_limit ?? null);
  // Whether current versions are kept aside is the backup's setting (*Keep my files when
  // restoring over them*) as well as the box here: the preview says what will be done.
  /** Files that differ are replaced, not kept aside. */
  const replacing = $derived(where === "original" && (overwrite || preview?.keep_existing === false));
  /** The backup itself is set not to keep current versions (the box here isn't what replaces them). */
  const neverKeeps = $derived(where === "original" && !overwrite && preview?.keep_existing === false);
  const replaces = $derived(replacing && (preview?.differ ?? 0) > 0);
  /** Something on disk is deleted, not kept: replaced, or removed with nothing kept. */
  const destructive = $derived(replaces || (removes > 0 && preview?.keep_existing === false));
  const removeWord = (n: number) => `${count(n)} ${n === 1 ? "item" : "items"}`;

  async function choose() {
    const f = await pickFolder("Restore into…");
    if (f) folder = f;
  }

  // Whether this dialog is still on screen: a restore's outcome is shown here, and if the
  // dialog is gone by the time it ends (the view under it was replaced), as a toast.
  let shown = true;
  $effect(() => () => {
    shown = false;
  });

  async function run() {
    busy = true;
    error = null;
    try {
      const r = await api.restore(profile, snapshot.id, paths, where === "elsewhere" ? folder : null, where === "original" && overwrite, where === "original" && exact);
      result = r;
      if (r.errors.length === 0) {
        app.toast("ok", `Restored ${count(r.restored)} items.`);
        onrestored?.(r, where);
      }
      else if (!shown) app.toast("bad", `Restored ${count(r.restored)} items, but ${count(r.errors.length)} couldn't be restored: ${r.errors[0]!.path} (${r.errors[0]!.message}).`);
    } catch (e) {
      error = (e as Error).message;
      if (!shown) app.toast("bad", `The restore failed: ${error}`);
    } finally {
      busy = false;
      // Even a failed restore may have written some files: whatever shows them, check again.
      app.diskRevision++;
    }
  }
</script>

<Modal title="Restore from {dateTime(snapshot.started)}" {onclose} locked={busy}>
  {#if result}
    <div class="space-y-4">
      <div class="flex items-center gap-3">
        <div class="grid size-10 place-items-center rounded-lg {result.errors.length ? 'bg-warning/15 text-warning' : 'bg-success/15 text-success'}">
          <Icon name={result.errors.length ? "alert" : "check"} />
        </div>
        <div>
          <div class="font-medium">{result.errors.length ? "Restored with problems" : "Restored"}</div>
          <div class="tabular text-sm text-fg-2">
            {count(result.restored)} items · {bytes(result.bytes)} · {count(result.skipped_identical)} already identical{#if result.removed} · {removeWord(result.removed)} that weren't in the snapshot removed{/if}
          </div>
          {#each result.exact_notes ?? [] as note (note)}
            <div class="mt-1 text-[13px] text-fg-muted">Exact restore: {note}.</div>
          {/each}
        </div>
      </div>
      {#if result.moved_aside.length}
        <div class="rounded-lg bg-surface-3 p-3 text-sm">
          <div class="font-medium">Your existing versions were kept:</div>
          <ul class="mt-1 max-h-28 overflow-y-auto font-mono text-xs text-fg-2">
            {#each result.moved_aside as p (p)}<li class="truncate">{p}</li>{/each}
          </ul>
        </div>
      {/if}
      {#if result.errors.length}
        <ul class="max-h-36 overflow-y-auto rounded-lg bg-danger/8 p-3 text-xs text-danger">
          {#each result.errors as e (e.path)}<li class="truncate">{e.path}: {e.message}</li>{/each}
        </ul>
      {/if}
      <div class="flex justify-end"><button class="btn btn-primary" onclick={onclose}>Done</button></div>
    </div>
  {:else}
    <div class="space-y-5">
      {#if note}<Callout tone="info" compact>{note}</Callout>{/if}
      <div>
        <div class="label mb-2">Restoring</div>
        <ul class="max-h-28 overflow-y-auto rounded-lg bg-surface-3 p-3 font-mono text-xs text-fg-2">
          {#each paths as p (p)}<li class="truncate">{p}</li>{/each}
        </ul>
      </div>

      <fieldset class="space-y-2">
        <legend class="label mb-2">Where to</legend>
        <label class="flex cursor-pointer gap-3 rounded-card p-4 transition-colors {where === 'elsewhere' ? 'bg-selected text-on-selected' : 'bg-surface-3 hover:bg-track'}">
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
        <label class="flex cursor-pointer gap-3 rounded-card p-4 transition-colors {where === 'original' ? 'bg-selected text-on-selected' : 'bg-surface-3 hover:bg-track'}">
          <input type="radio" class="mt-1 accent-[var(--c-primary)]" bind:group={where} value="original" />
          <span class="min-w-0 flex-1">
            <span class="block font-medium">Back to where it was</span>
            <span class="block text-sm text-fg-muted">
              Files that are already identical are skipped.
              {#if neverKeeps}
                <span class="font-medium text-danger">Different files are replaced:</span> this backup is set not to keep your current versions (<em>Keep my files when restoring over them</em> is off in its settings).
              {:else if overwrite}
                Different files are replaced.
              {:else}
                Different files are kept, renamed to “name.flux-…”.
              {/if}
            </span>
            {#if where === "original"}
              <label class="mt-2 flex items-center gap-2 text-sm">
                <input type="checkbox" class="accent-[var(--c-danger)]" bind:checked={overwrite} />
                Replace my current versions instead of keeping them
              </label>
              <label class="mt-1.5 flex items-start gap-2 text-sm">
                <input type="checkbox" class="mt-[3px] accent-[var(--c-danger)]" bind:checked={exact} />
                <span>
                  Make the folders exactly as they were: remove what's been added since
                  <span class="block text-[12.5px] text-fg-muted">What this backup leaves out (excluded files, the size limit, caches) is never removed.</span>
                </span>
              </label>
            {/if}
          </span>
        </label>
      </fieldset>

      {#if (where === "original" || folder) && !busy}
        <div class="rounded-card bg-surface-3 px-4 py-3 text-sm" aria-live="polite">
          {#if previewError}
            <span class="text-fg-muted">Couldn't check what would change: {previewError}</span>
          {:else if !preview}
            <span class="text-fg-muted motion-safe:animate-pulse">Checking what would change…</span>
          {:else if nothingToDo}
            <span class="flex items-center gap-2"><Icon name="check" size={14} class="text-success" /> Everything is already identical: nothing would change.</span>
          {:else}
            <div class="label mb-2">What will happen</div>
            <ul class="num space-y-1 text-fg-2">
              {#if preview.new}<li class="flex items-center gap-2"><Icon name="plus" size={13} class="text-success" /> {count(preview.new)} {preview.new === 1 ? "item" : "items"} added ({bytes(preview.bytes)} in all){preview.new_folders ? `, ${count(preview.new_folders)} new folders` : ""}</li>{/if}
              {#if preview.differ}
                <li class="flex items-start gap-2 {replaces ? 'text-danger' : ''}">
                  <Icon name={replaces ? "alert" : "copy"} size={13} class="mt-1 shrink-0 {replaces ? '' : 'text-warning'}" />
                  <span>
                    {count(preview.differ)} {preview.differ === 1 ? "file differs" : "files differ"} from what's there now:
                    {preview.keep_existing ? "your current versions are kept, renamed to “name.flux-…”." : "your current versions are replaced."}
                  </span>
                </li>
              {/if}
              {#if preview.identical}<li class="flex items-center gap-2"><Icon name="check" size={13} class="text-fg-muted" /> {count(preview.identical)} already identical, left alone</li>{/if}
              {#if removes}
                <li class="flex items-start gap-2 {preview.keep_existing ? '' : 'text-danger'}">
                  <Icon name={preview.keep_existing ? "trash" : "alert"} size={13} class="mt-1 shrink-0 {preview.keep_existing ? 'text-warning' : ''}" />
                  <span>
                    {removeWord(removes)} that {removes === 1 ? "isn't" : "aren't"} in this snapshot {removes === 1 ? "is" : "are"}
                    {preview.keep_existing ? "moved out of the way, renamed to “name.flux-…”." : "deleted."}
                  </span>
                </li>
              {/if}
            </ul>
            {#each preview.exact_notes ?? [] as note (note)}
              <p class="mt-2 text-[13px] text-fg-muted">Exact restore: {note}.</p>
            {/each}
            {#if removes && preview.remove_paths?.length}
              <details class="mt-2" open>
                <summary class="cursor-pointer text-[13px] text-fg-muted select-none">Show what will be removed</summary>
                <ul class="mt-1.5 max-h-32 overflow-y-auto font-mono text-xs text-fg-2">
                  {#each preview.remove_paths as p (p)}<li class="truncate" title={p}>{p}</li>{/each}
                  {#if removes > preview.remove_paths.length}<li class="text-fg-muted">…and {count(removes - preview.remove_paths.length)} more</li>{/if}
                </ul>
              </details>
            {/if}
            {#if preview.new_paths.length}
              <details class="mt-2">
                <summary class="cursor-pointer text-[13px] text-fg-muted select-none">Show what will be added</summary>
                <ul class="mt-1.5 max-h-32 overflow-y-auto font-mono text-xs text-fg-2">
                  {#each preview.new_paths as p (p)}<li class="truncate" title={p}>{p}</li>{/each}
                  {#if preview.new > preview.new_paths.length}<li class="text-fg-muted">…and {count(preview.new - preview.new_paths.length)} more</li>{/if}
                </ul>
              </details>
            {/if}
            {#if preview.differ_paths.length}
              <details class="mt-2">
                <summary class="cursor-pointer text-[13px] text-fg-muted select-none">Show the {preview.differ === 1 ? "file" : `${count(preview.differ)} files`} that differ</summary>
                <ul class="mt-1.5 max-h-32 overflow-y-auto font-mono text-xs text-fg-2">
                  {#each preview.differ_paths as p (p)}<li class="truncate" title={p}>{p}</li>{/each}
                  {#if preview.differ > preview.differ_paths.length}<li class="text-fg-muted">…and {count(preview.differ - preview.differ_paths.length)} more</li>{/if}
                </ul>
              </details>
            {/if}
          {/if}
        </div>
      {/if}

      {#if onlyFolders}
        <Callout title="This snapshot has no files here, only empty folders.">
          {#if sizeLimit}Files larger than {sizeLabel(sizeLimit)} were left out by this backup's size limit.{:else}The files may have been left out by this backup's rules, or deleted before it ran. An older snapshot may still have them.{/if}
        </Callout>
      {/if}

      {#if error}<Callout tone="bad">{error}</Callout>{/if}

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
        <button class="btn {destructive ? 'btn-danger-filled' : 'btn-primary'}" onclick={run} disabled={busy || (where === "elsewhere" && !folder) || nothingToDo || (where === "original" && !preview && (exact || !previewError))}>
          <Icon name={busy ? "refresh" : "restore"} size={15} class={busy ? "animate-spin" : ""} />
          {busy
            ? "Restoring…"
            : removes
              ? `Restore and remove ${removeWord(removes)}`
              : replaces
                ? `Restore and replace ${count(preview!.differ)} ${preview!.differ === 1 ? "file" : "files"}`
                : "Restore"}
        </button>
      </div>
    </div>
  {/if}
</Modal>
