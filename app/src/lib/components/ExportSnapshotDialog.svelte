<script lang="ts">
  // Save files out of a snapshot as one archive: to keep somewhere else, or to send.
  import { api, inTauri, pickFolder } from "../api";
  import { app } from "../state.svelte";
  import { bytes, count, dateTime } from "../format";
  import type { ArchiveFormat, ChecksumKind, ExportOutcome, SnapshotMeta } from "../types";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";
  import { copyText } from "../clipboard";

  let {
    profile,
    snapshot,
    paths,
    onclose,
  }: {
    profile: string;
    snapshot: SnapshotMeta;
    /** What to export; empty: the whole snapshot. */
    paths: string[];
    onclose: () => void;
  } = $props();

  let format = $state<ArchiveFormat>("tar_zst");
  let checksums = $state<ChecksumKind | "">("");
  let busy = $state(false);
  let error = $state<string | null>(null);
  let result = $state<ExportOutcome | null>(null);

  const what = $derived(
    paths.length === 0 ? "the whole snapshot" : paths.length === 1 ? `“${paths[0]!.split("/").pop()}”` : `${count(paths.length)} items`,
  );

  async function run() {
    const dir = inTauri ? await pickFolder("Where to save the archive") : "/home/me/Downloads";
    if (!dir) return;
    busy = true;
    error = null;
    try {
      result = await api.exportSnapshot(profile, snapshot.id, paths, format, dir, checksums || null);
      app.toast("ok", `Saved ${result.archive.split("/").pop()}.`);
    } catch (e) {
      error = (e as Error).message;
    } finally {
      busy = false;
    }
  }
</script>

<Modal title="Export from {dateTime(snapshot.started)}" {onclose}>
  {#if result}
    {@const r = result}
    <div class="space-y-4">
      <div class="flex items-center gap-3">
        <div class="grid size-10 place-items-center rounded-lg {r.errors.length ? 'bg-warning/15 text-warning' : 'bg-success/15 text-success'}">
          <Icon name={r.errors.length ? "alert" : "check"} />
        </div>
        <div class="min-w-0">
          <div class="font-medium">{r.errors.length ? "Saved, with problems" : "Saved"}</div>
          <div class="tabular text-sm text-fg-2">{count(r.files)} files · {bytes(r.bytes)}, {bytes(r.size)} in the archive</div>
        </div>
      </div>
      <div class="rows">
        <div class="row gap-3">
          <span class="min-w-0 flex-1 truncate font-mono text-[12.5px]" title={r.archive}>{r.archive}</span>
          <button class="btn shrink-0" onclick={() => void copyText(r.archive).then(() => app.toast("info", "Copied the path."))}><Icon name="copy" size={14} /> Copy path</button>
        </div>
        {#if r.manifest}
          <div class="row gap-3">
            <span class="min-w-0 flex-1">
              <span class="block truncate font-mono text-[12.5px]" title={r.manifest}>{r.manifest.split("/").pop()}</span>
              <span class="block text-[12.5px] text-fg-muted">Check the unpacked files with {r.manifest.endsWith(".b3") ? "b3sum -c" : "sha256sum -c"}, in the folder you unpack into.</span>
            </span>
          </div>
        {/if}
      </div>
      {#if r.left_out.length}
        <ul class="list-disc pl-5 text-[13px] text-fg-2">{#each r.left_out as l (l)}<li>{l}</li>{/each}</ul>
      {/if}
      {#if r.errors.length}
        <ul class="max-h-32 overflow-y-auto rounded-card bg-surface-2 px-3 py-2 font-mono text-[12px] text-danger">{#each r.errors as e (e)}<li class="truncate" title={e}>{e}</li>{/each}</ul>
      {/if}
      <div class="flex justify-end"><button class="btn btn-primary" onclick={onclose}>Done</button></div>
    </div>
  {:else}
    <p class="-mt-1 mb-4 text-[13.5px] text-fg-2">Save {what} as one archive file: to keep somewhere else, or to send to someone. Your backup stays as it is.</p>
    <h2 class="group-title">Kind of archive</h2>
    <div class="rows" role="radiogroup" aria-label="Kind of archive">
      <button role="radio" aria-checked={format === "tar_zst"} class="row row-link items-start gap-3 py-3 text-left" onclick={() => (format = "tar_zst")}>
        <span class="min-w-0 flex-1">
          <span class="block font-medium">Everything kept <span class="font-normal text-fg-muted">· .tar.zst</span></span>
          <span class="block text-[12.5px] text-fg-muted">Owners, permissions, times, links, extended attributes and ACLs. Opens with tar and most Linux archive tools.</span>
        </span>
        {#if format === "tar_zst"}<Icon name="check" size={18} class="mt-0.5" />{/if}
      </button>
      <button role="radio" aria-checked={format === "zip"} class="row row-link items-start gap-3 py-3 text-left" onclick={() => (format = "zip")}>
        <span class="min-w-0 flex-1">
          <span class="block font-medium">For sharing <span class="font-normal text-fg-muted">· .zip</span></span>
          <span class="block text-[12.5px] text-fg-muted">Opens anywhere. Keeps files, folders and their times, but not owners, links, extended attributes or ACLs; hard links become copies.</span>
        </span>
        {#if format === "zip"}<Icon name="check" size={18} class="mt-0.5" />{/if}
      </button>
    </div>
    <div class="rows mt-3">
      <label class="row gap-3">
        <span class="min-w-0 flex-1">
          <span class="block font-medium">Checksums file</span>
          <span class="block text-[12.5px] text-fg-muted">Saved next to the archive, to check the files after unpacking.</span>
        </span>
        <select class="input w-auto" bind:value={checksums} aria-label="Checksums file">
          <option value="">None</option>
          <option value="sha256">SHA-256 (sha256sum)</option>
          <option value="blake3">BLAKE3 (b3sum)</option>
        </select>
      </label>
    </div>
    {#if error}<p class="mt-3 text-[13px] text-danger">{error}</p>{/if}
    <div class="mt-5 flex items-center justify-end gap-3">
      <button class="btn btn-ghost" onclick={onclose}>Cancel</button>
      <button class="btn btn-primary" disabled={busy} onclick={() => void run()}>
        {#if busy}<Icon name="refresh" size={14} class="motion-safe:animate-spin" /> Saving…{:else}Choose where to save…{/if}
      </button>
    </div>
  {/if}
</Modal>
