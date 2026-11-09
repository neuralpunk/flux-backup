<script lang="ts">
  // How full a drive is: one bar, used against its size. It turns amber when nearly full
  // and red when there's almost no room left, and the words say the same, so color is
  // never the only signal.
  import { bytes } from "../format";

  let {
    free,
    size,
    compact = false,
    label = "Space used",
  }: {
    free: number;
    size: number;
    /** A thin bar with one short line, for lists. */
    compact?: boolean;
    label?: string;
  } = $props();

  const used = $derived(Math.max(0, size - free));
  const pct = $derived(size > 0 ? Math.min(100, (used / size) * 100) : 0);
  const tone = $derived(pct >= 95 ? "danger" : pct >= 85 ? "warning" : "ok");
  const shown = $derived(pct > 0 && pct < 1 ? "<1" : Math.round(pct).toString());
</script>

<div class="flex min-w-0 flex-col {compact ? 'gap-1' : 'gap-1.5'}">
  {#if !compact}
    <div class="flex flex-wrap items-baseline justify-between gap-x-4 text-sm">
      <span class="text-fg-2"><span class="font-medium text-fg">{bytes(free)}</span> free of {bytes(size)}</span>
      <span class="num {tone === 'danger' ? 'text-danger' : tone === 'warning' ? 'text-warning' : 'text-fg-muted'}">
        {shown}% full{tone === "danger" ? ": almost no room left" : tone === "warning" ? ": getting full" : ""}
      </span>
    </div>
  {/if}
  <div
    class="meter {compact ? 'h-1.5' : 'h-2.5'} {tone}"
    role="meter"
    aria-label={label}
    aria-valuemin={0}
    aria-valuemax={100}
    aria-valuenow={Math.round(pct)}
    aria-valuetext="{shown}% full, {bytes(free)} free of {bytes(size)}"
  >
    <div class="fill" style:width="{pct}%"></div>
  </div>
  {#if compact}
    <span class="num text-[12px] {tone === 'danger' ? 'text-danger' : tone === 'warning' ? 'text-warning' : 'text-fg-muted'}">{bytes(free)} free of {bytes(size)} · {shown}% full</span>
  {/if}
</div>

<style>
  .meter {
    overflow: hidden;
    border-radius: 999px;
    background: var(--c-track);
  }
  .fill {
    height: 100%;
    border-radius: inherit;
    background: var(--c-chart);
    transition: width 300ms ease-out;
  }
  .warning .fill {
    background: var(--c-warning);
  }
  .danger .fill {
    background: var(--c-danger);
  }
  @media (prefers-reduced-motion: reduce) {
    .fill {
      transition: none;
    }
  }
</style>
