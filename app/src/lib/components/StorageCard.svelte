<script lang="ts">
  // What the snapshots hold and what they take: the space all of them would need as full
  // copies, and what sharing (deduplication) and compression save.
  import { api } from "../api";
  import { bytes, count } from "../format";
  import type { Storage } from "../types";
  import Icon from "./Icon.svelte";

  /** `folded`: one line (what all the snapshots take) until opened, for the Compact layout. */
  let { id, embedded = false, folded = false }: { id: string; embedded?: boolean; folded?: boolean } = $props();
  let st = $state<Storage | null>(null);
  $effect(() => {
    const want = id;
    st = null;
    api.storage(want).then(
      (s) => want === id && (st = s),
      () => {},
    );
  });
  const parts = $derived.by(() => {
    if (!st || st.stored_bytes == null || st.logical_bytes <= 0) return null;
    const whole = Math.max(st.logical_bytes, st.stored_bytes + (st.dedup_saved ?? 0) + (st.compression_saved ?? 0));
    return {
      stored: (st.stored_bytes / whole) * 100,
      dedup: ((st.dedup_saved ?? 0) / whole) * 100,
      compression: ((st.compression_saved ?? 0) / whole) * 100,
    };
  });
</script>

{#if st && st.snapshots > 0}
  {@const s = st}
  {#snippet details()}
    <p class="text-[13.5px]" title={s.basis}>
      The newest snapshot holds <span class="font-semibold">{bytes(s.latest_bytes)}</span> ({count(s.latest_files)} files).
      {#if s.stored_bytes != null}
        All {count(s.snapshots)} take <span class="font-semibold">{bytes(s.stored_bytes)}</span>{s.shared_with ? `, in a Vault shared with ${s.shared_with === 1 ? "another backup" : `${s.shared_with} other backups`}` : ""}, instead of {bytes(s.logical_bytes)} as full copies.
      {/if}
      {#if s.repair_bytes}
        {bytes(s.repair_bytes)} of that is repair data, {(s.packs_with_repair ?? 0) >= (s.packs ?? 0) ? "for everything stored" : `for ${count(s.packs_with_repair ?? 0)} of ${count(s.packs ?? 0)} stored files (Verify every byte adds the rest)`}.
      {/if}
    </p>
    {#if parts}
      <div class="mt-2.5 flex h-2 overflow-hidden rounded-full bg-surface-3" role="img" aria-label="Space taken compared with full copies">
        <span class="h-full bg-primary" style="width: {parts.stored}%"></span>
        <span class="h-full bg-success/70" style="width: {parts.dedup}%"></span>
        <span class="h-full bg-warning/70" style="width: {parts.compression}%"></span>
      </div>
      <div class="mt-2 flex flex-wrap gap-x-5 gap-y-1 text-[12.5px] text-fg-2">
        <span class="flex items-center gap-1.5"><span class="size-2 rounded-full bg-primary"></span>Taken {bytes(s.stored_bytes ?? 0)}</span>
        {#if s.dedup_saved}<span class="flex items-center gap-1.5"><span class="size-2 rounded-full bg-success/70"></span>Saved by storing things once {bytes(s.dedup_saved)}</span>{/if}
        {#if s.compression_saved}<span class="flex items-center gap-1.5"><span class="size-2 rounded-full bg-warning/70"></span>Saved by compression {bytes(s.compression_saved)}</span>{/if}
      </div>
    {/if}
  {/snippet}
  {#if embedded}
    {@render details()}
  {:else if folded}
    <details class="group/storage">
      <summary class="flex cursor-pointer list-none items-center gap-2 rounded-md px-0.5 py-1 text-[13px] font-semibold text-fg-2 select-none hover:text-fg [&::-webkit-details-marker]:hidden">
        <Icon name="chevronRight" size={14} class="transition-transform group-open/storage:rotate-90" />
        Storage
        <span class="num font-normal text-fg-muted">
          {count(s.snapshots)} {s.snapshots === 1 ? "snapshot" : "snapshots"}{#if s.stored_bytes != null}{" "}take {bytes(s.stored_bytes)}{:else}, the newest {bytes(s.latest_bytes)}{/if}
        </span>
      </summary>
      <div class="card mt-2 px-5 py-4">{@render details()}</div>
    </details>
  {:else}
    <section>
      <h2 class="group-title">Storage</h2>
      <div class="card px-5 py-4">{@render details()}</div>
    </section>
  {/if}
{/if}
