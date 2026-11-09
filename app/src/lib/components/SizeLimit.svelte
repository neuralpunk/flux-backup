<script lang="ts">
  import { untrack } from "svelte";
  import { parseSize, TINY_SIZE_LIMIT } from "../size";

  /** A file size limit: a number and a unit, saved as e.g. "500MB" (`null`: no limit). */
  let { value = $bindable(), label = "Skip files larger than" }: { value: string | null | undefined; label?: string } = $props();

  const parsed = $derived(parseSize(value ?? ""));
  // Units people think in; a value saved with another unit (by hand, an older version)
  // keeps it, so nothing is rounded.
  const units = $derived(["MB", "GB", "TB", ...(parsed && !["MB", "GB", "TB"].includes(parsed.unit) ? [parsed.unit] : [])]);
  let number = $state("");
  let unit = $state("GB");
  // Follow the bound value when it changes from outside (loading a profile, defaults),
  // not while it's being typed.
  $effect(() => {
    const p = parsed;
    const v = value;
    untrack(() => {
      if (p) {
        if (`${p.number}${p.unit}` !== `${Number(number)}${unit}`) (number = String(p.number)), (unit = p.unit);
      } else if (!v) number = "";
    });
  });

  /** Save what's typed once it's a number (a half-typed "1." waits). */
  function commit() {
    const n = number.trim();
    if (n === "") value = null;
    else if (/^\d+(\.\d+)?$/.test(n) && Number(n) > 0) value = `${n}${unit}`;
  }
  const tiny = $derived(parsed != null && parsed.bytes < TINY_SIZE_LIMIT);
  const unitName = (u: string) => (u === "B" ? "bytes" : u);
</script>

<span class="inline-flex flex-col items-end gap-1">
  <span class="inline-flex items-center gap-1.5">
    <input
      class="input num w-24 text-right"
      inputmode="decimal"
      placeholder="no limit"
      aria-label="{label} (number)"
      bind:value={number}
      oninput={commit}
    />
    <select class="input w-28" aria-label="{label} (unit)" bind:value={unit} onchange={commit} disabled={number.trim() === ""}>
      {#each units as u (u)}<option value={u}>{unitName(u)}</option>{/each}
    </select>
  </span>
  {#if tiny}
    <span class="text-[12px] font-medium text-warning" role="alert">That leaves out almost every file</span>
  {/if}
</span>
