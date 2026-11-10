<script lang="ts">
  // Export a profile: as a Flux profile (to import elsewhere), or as the configuration of
  // another backup tool. What a format can't carry over is shown, not hidden.
  import { api, pickFolder } from "../api";
  import { copyText } from "../clipboard";
  import { app } from "../state.svelte";
  import type { ExportFormat, ProfileExport } from "../types";
  import Icon from "./Icon.svelte";
  import Callout from "./Callout.svelte";
  import Modal from "./Modal.svelte";

  let { id, name, onclose }: { id: string; name: string; onclose: () => void } = $props();

  let formats = $state<ExportFormat[] | null>(null);
  let format = $state<ExportFormat["id"]>("flux");
  let result = $state<ProfileExport | null>(null);
  let error = $state<string | null>(null);
  let saving = $state(false);
  let copied = $state(false);

  $effect(() => {
    api.exportFormats().then(
      (f) => (formats = f),
      (e) => (error = (e as Error).message),
    );
  });
  $effect(() => {
    const f = format;
    result = null;
    api.exportProfile(id, f).then(
      (r) => {
        if (f === format) result = r;
      },
      (e) => (error = (e as Error).message),
    );
  });

  async function copy() {
    if (!result) return;
    await copyText(result.contents);
    copied = true;
    setTimeout(() => (copied = false), 1500);
  }

  async function save() {
    const dir = await pickFolder("Save the export into…");
    if (!dir) return;
    saving = true;
    try {
      const path = await api.saveExport(id, format, dir);
      app.toast("ok", `Saved ${path}`);
    } catch (e) {
      app.toast("bad", (e as Error).message);
    } finally {
      saving = false;
    }
  }
</script>

<Modal title="Export “{name}”" {onclose} width="max-w-5xl">
  {#if error}
    <Callout tone="bad">{error}</Callout>
  {:else}
    <div class="flex flex-col gap-4">
      <p class="text-sm text-fg-2">
        Keep a copy of this profile, move it to another computer, or take the same backup to another tool. Exporting doesn't change anything here.
      </p>

      <div class="grid min-h-0 grid-cols-[minmax(0,17rem)_minmax(0,1fr)] gap-4">
        <div class="rows self-start" role="radiogroup" aria-label="Export as">
          {#each formats ?? [] as f (f.id)}
            {@const selected = format === f.id}
            <button role="radio" aria-checked={selected} title="For {f.works_with}" class="row row-link items-start {selected ? '!bg-selected text-on-selected' : ''}" onclick={() => (format = f.id)}>
              <span class="mt-0.5 grid size-5 shrink-0 place-items-center rounded-full {selected ? 'bg-primary text-on-primary' : 'ring-2 ring-fg-muted/60'}">{#if selected}<Icon name="check" size={12} />{/if}</span>
              <span class="min-w-0 flex-1">
                <span class="block text-[14px] font-medium">{f.label}</span>
                <span class="mt-0.5 block text-[12px] opacity-80">{f.description}</span>
                <span class="mt-1 block text-[11.5px] opacity-60">For {f.works_with}</span>
              </span>
            </button>
          {/each}
        </div>

        <div class="flex min-w-0 flex-col gap-3">
          {#if result}
            {#if result.notes.length}
              <Callout title="Not everything carries over">
                <ul class="mt-1 list-disc space-y-0.5 pl-5">
                  {#each result.notes as n (n)}<li>{n}</li>{/each}
                </ul>
              </Callout>
            {/if}
            <span class="label font-mono">{result.filename}</span>
            <pre class="max-h-[max(12rem,calc(100dvh-30rem))] overflow-auto rounded-card bg-surface-3 px-4 py-3 font-mono text-[12px] leading-relaxed text-fg-2">{result.contents}</pre>
          {:else}
            <div class="h-64 rounded-card bg-surface-2 motion-safe:animate-pulse"></div>
          {/if}
        </div>
      </div>

      <div class="flex flex-wrap items-center justify-end gap-2">
        <button class="btn btn-ghost" onclick={copy} disabled={!result}><Icon name={copied ? "check" : "copy"} size={14} /> {copied ? "Copied" : "Copy"}</button>
        <button class="btn btn-primary" onclick={save} disabled={!result || saving}><Icon name="download" size={15} /> {saving ? "Saving…" : "Save…"}</button>
      </div>
    </div>
  {/if}
</Modal>
