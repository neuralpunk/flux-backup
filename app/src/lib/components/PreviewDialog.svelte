<script lang="ts">
  // A look at a file as it was in a snapshot, before restoring it: the picture, the first
  // lines of text, or what's known of it. Read from the backup; nothing is restored.
  import { api } from "../api";
  import { bytes, dateTime } from "../format";
  import { ApiError, type DirEntry, type FilePreview, type SnapshotMeta } from "../types";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";

  let {
    profile,
    snapshot,
    entry,
    onrestore,
    onopen,
    onversions,
    onLocked,
    onclose,
  }: {
    profile: string;
    snapshot: SnapshotMeta;
    entry: DirEntry;
    onrestore: () => void;
    onopen: () => void;
    onversions: () => void;
    onLocked: (retry: () => void) => void;
    onclose: () => void;
  } = $props();

  let preview = $state<FilePreview | null>(null);
  let error = $state<string | null>(null);
  /** The picture couldn't be shown (not really one, or a kind the view can't draw). */
  let broken = $state(false);

  function load() {
    error = null;
    api.previewFile(profile, snapshot.id, entry.path).then(
      (p) => (preview = p),
      (e) => {
        if (e instanceof ApiError && e.code === "password_required") onLocked(load);
        else error = (e as Error).message;
      },
    );
  }
  load();
</script>

<Modal title={entry.name} {onclose} width="max-w-3xl">
  <p class="-mt-1 mb-4 flex min-w-0 flex-wrap items-baseline gap-x-2 text-[13px] text-fg-2">
    <span class="min-w-0 truncate font-mono text-[12.5px]" title={entry.path}>{entry.path}</span>
    <span class="num shrink-0 text-fg-muted">{bytes(entry.size)} · as it was on {dateTime(snapshot.started)}</span>
  </p>

  {#if error}
    <div class="flex items-center gap-2 rounded-lg bg-surface-sunken p-6 text-sm text-danger"><Icon name="alert" size={16} /> {error}</div>
  {:else if !preview}
    <div class="h-64 rounded-lg bg-surface-sunken motion-safe:animate-pulse" aria-label="Reading it from the backup…"></div>
  {:else if preview.kind === "image" && !broken}
    <div class="grid max-h-[60vh] place-items-center overflow-hidden rounded-lg bg-surface-sunken p-3">
      <img src={preview.url} alt={entry.name} class="max-h-[calc(60vh-1.5rem)] max-w-full object-contain" onerror={() => (broken = true)} />
    </div>
  {:else if preview.kind === "text"}
    <pre class="max-h-[60vh] overflow-auto rounded-lg bg-surface-sunken px-4 py-3 font-mono text-[12.5px] leading-relaxed break-words whitespace-pre-wrap text-fg">{preview.text}</pre>
    {#if preview.more}<p class="mt-2 px-0.5 text-[12.5px] text-fg-muted">The start of it. Open a copy to see all of it.</p>{/if}
  {:else}
    <div class="flex flex-col items-center gap-2 rounded-lg bg-surface-sunken px-6 py-10 text-center">
      <span class="glyph size-10"><Icon name="file" size={18} /></span>
      <p class="text-sm text-fg-2">
        {preview.kind === "other" && preview.too_large ? "Too large to preview here (over 32 MB)." : "No preview for this kind of file."}
      </p>
      <p class="text-[12.5px] text-fg-muted">Open a copy to look at it in its usual program.</p>
    </div>
  {/if}

  <div class="mt-5 flex flex-wrap items-center gap-2">
    <button class="btn" onclick={onversions}><Icon name="history" size={14} /> Versions…</button>
    <button class="btn" onclick={onopen} title="A read-only copy, in its usual program"><Icon name="file" size={14} /> Open a copy</button>
    <button class="btn btn-primary ml-auto" onclick={onrestore}><Icon name="restore" size={14} /> Restore…</button>
  </div>
</Modal>
