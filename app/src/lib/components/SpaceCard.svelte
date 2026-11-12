<script lang="ts">
  // Space on the backup drive: used space over the last weeks (from the free space
  // recorded after each backup), and when it's full at that rate. One series, so no
  // legend: the title says what's plotted. Hover or arrow keys for exact values; the
  // same numbers are in the table below the chart.
  import { ago, bytes, dayLabel, dateTime } from "../format";
  import type { DriveHealth, Space } from "../types";
  import type { Snippet } from "svelte";
  import Icon from "./Icon.svelte";
  import StorageMeter from "./StorageMeter.svelte";

  /** `flat`: without its card, title and meter, opened from Compact's one-line space row
   * (which shows those). */
  let { space, target, flat = false, health = null, notes = [], children }: { space: Space; target: string; flat?: boolean; health?: DriveHealth | null; notes?: { text: string; hint: string }[]; children?: Snippet } = $props();
  // The chart is open when the drive is filling up soon; otherwise one click away.
  const soon = $derived(space.days_left != null && space.days_left < 60);

  const size = $derived(space.size ?? 0);
  const pts = $derived(space.points.map((p) => ({ t: new Date(p.t).getTime(), used: Math.max(0, size - p.free), free: p.free })));
  const now = $derived(pts.at(-1)!.t);
  const DAY = 86_400_000;
  // Show the projection when it's within reach of the chart (half a year).
  const fullAt = $derived(space.days_left != null && space.days_left <= 180 ? now + space.days_left * DAY : null);

  function inWords(days: number): string {
    if (days < 2) return "a day";
    if (days < 14) return `${days} days`;
    if (days < 60) return `${Math.round(days / 7)} weeks`;
    if (days < 730) return `${Math.round(days / 30)} months`;
    return `${Math.round(days / 365)} years`;
  }
  const headline = $derived.by(() => {
    if (space.days_left != null) return `At the rate it's been filling, ${target} is full in about ${inWords(space.days_left)}.`;
    if (space.per_day == null) return "A forecast appears after a few more backups.";
    if (space.per_day >= 0) return "Space use is steady: old snapshots are removed about as fast as new ones are added.";
    return "Filling very slowly: more than ten years at this rate.";
  });

  // Geometry.
  let width = $state(600);
  const H = 140;
  const AXIS = 22;
  const PAD = { l: 8, r: 8, t: 18 };
  const t0 = $derived(pts[0]!.t);
  const t1 = $derived(Math.max(fullAt ?? now, now) + (fullAt ? DAY : 0));
  const x = (t: number) => PAD.l + ((t - t0) / Math.max(1, t1 - t0)) * (width - PAD.l - PAD.r);
  const y = (used: number) => PAD.t + (1 - used / Math.max(1, size)) * (H - PAD.t);
  const line = $derived(pts.map((p, i) => `${i ? "L" : "M"}${x(p.t).toFixed(1)},${y(p.used).toFixed(1)}`).join(""));
  const area = $derived(`${line}L${x(now).toFixed(1)},${H}L${x(t0).toFixed(1)},${H}Z`);
  const ticks = $derived.by(() => {
    const n = width < 420 ? 2 : 4;
    return Array.from({ length: n + 1 }, (_, i) => t0 + ((t1 - t0) * i) / n);
  });
  const short = (t: number) => new Date(t).toLocaleDateString(undefined, { month: "short", day: "numeric" });

  // Crosshair: snaps to the nearest measurement.
  let hover = $state<number | null>(null);
  function move(e: PointerEvent) {
    const r = (e.currentTarget as SVGElement).getBoundingClientRect();
    const px = e.clientX - r.left;
    let best = 0;
    for (let i = 1; i < pts.length; i++) if (Math.abs(x(pts[i]!.t) - px) < Math.abs(x(pts[best]!.t) - px)) best = i;
    hover = best;
  }
  function key(e: KeyboardEvent) {
    if (e.key === "ArrowLeft") hover = Math.max(0, (hover ?? pts.length) - 1);
    else if (e.key === "ArrowRight") hover = Math.min(pts.length - 1, (hover ?? -1) + 1);
    else if (e.key === "Escape") hover = null;
    else return;
    e.preventDefault();
  }
  const hp = $derived(hover != null ? pts[hover] : null);
</script>

<section class={flat ? "" : "card px-5 py-4"}>
  {#if !flat}
    <h2 class="title">Space on {target}</h2>
    <div class="mt-3">
      <StorageMeter free={space.free} {size} label="Space used on {target}" />
    </div>
  {/if}
  <p class="{flat ? '' : 'mt-3'} text-sm {space.days_left != null && space.days_left < 30 ? 'text-warning' : 'text-fg-2'}">{headline}</p>
  <!-- What the drive says about itself, when a backup has asked it. -->
  {#if health}
    <p class="mt-1 text-[12.5px] {health.said ? 'text-warning' : 'text-fg-muted'}">
      {#if health.said}{health.said}{:else if health.reports}The drive reports no problems with its health{#if health.checked}{" "}({ago(health.checked)}){/if}.{:else}This drive doesn't report its health (most USB sticks and many enclosures don't), so Flux can't tell how it is.{/if}
    </p>
  {/if}
  <!-- What slows the drive down that can be fixed: advice, not a problem with the backups. -->
  {#each notes as n (n.text)}
    <p class="mt-1 text-[12.5px] text-fg-muted">{n.text} <span class="text-fg-2">{n.hint}</span></p>
  {/each}

  {#if pts.length >= 3}
  <details class="group/chart mt-2" open={soon}>
  <summary class="flex w-fit cursor-pointer list-none items-center gap-1.5 text-[13px] text-fg-muted select-none hover:text-fg [&::-webkit-details-marker]:hidden">
    <Icon name="chevronRight" size={13} class="transition-transform group-open/chart:rotate-90" /> History and forecast
  </summary>
  <div class="relative mt-3" bind:clientWidth={width}>
    <svg
      width={width}
      height={H + AXIS}
      role="slider"
      aria-label="Used space on {target} over time (left and right arrow keys move through the measurements)"
      aria-valuemin={0}
      aria-valuemax={pts.length - 1}
      aria-valuenow={hover ?? pts.length - 1}
      aria-valuetext={`${hp ? (hover === pts.length - 1 ? "Now" : dateTime(hp.t)) : "Now"}: ${bytes((hp ?? pts.at(-1)!).used)} used, ${bytes((hp ?? pts.at(-1)!).free)} free`}
      tabindex="0"
      class="block overflow-visible outline-none focus-visible:ring-2 focus-visible:ring-primary/60 rounded-md"
      onpointermove={move}
      onpointerleave={() => (hover = null)}
      onkeydown={key}
      onblur={() => (hover = null)}
    >
      <!-- Capacity and baseline: solid hairlines. -->
      <line x1={PAD.l} x2={width - PAD.r} y1={y(size)} y2={y(size)} stroke="var(--c-track)" stroke-width="1" />
      <line x1={PAD.l} x2={width - PAD.r} y1={H} y2={H} stroke="var(--c-track)" stroke-width="1" />
      <text x={PAD.l} y={y(size) - 6} class="fill-fg-muted text-[11px]">Full · {bytes(size)}</text>

      <path d={area} fill="var(--c-chart)" fill-opacity="0.1" />
      <path d={line} fill="none" stroke="var(--c-chart)" stroke-width="2" stroke-linejoin="round" stroke-linecap="round" />

      {#if fullAt}
        <!-- Projection: dashed, because it hasn't happened. -->
        <line x1={x(now)} y1={y(pts.at(-1)!.used)} x2={x(fullAt)} y2={y(size)} stroke="var(--c-chart)" stroke-width="2" stroke-dasharray="4 4" stroke-linecap="round" />
        <circle cx={x(fullAt)} cy={y(size)} r="4" fill="var(--c-surface-2)" stroke="var(--c-chart)" stroke-width="2" />
        <text x={x(fullAt)} y={y(size) - 8} text-anchor="end" class="fill-fg-2 text-[11px]">Full around {short(fullAt)}</text>
      {/if}

      <!-- Now: end dot with a surface ring. -->
      <circle cx={x(now)} cy={y(pts.at(-1)!.used)} r="6" fill="var(--c-surface-2)" />
      <circle cx={x(now)} cy={y(pts.at(-1)!.used)} r="4" fill="var(--c-chart)" />

      {#if hp}
        <line x1={x(hp.t)} x2={x(hp.t)} y1={PAD.t - 4} y2={H} stroke="var(--c-fg-muted)" stroke-width="1" />
        <circle cx={x(hp.t)} cy={y(hp.used)} r="6" fill="var(--c-surface-2)" />
        <circle cx={x(hp.t)} cy={y(hp.used)} r="4" fill="var(--c-chart)" />
      {/if}

      {#each ticks as t, i (i)}
        <text x={x(t)} y={H + 16} text-anchor={i === 0 ? "start" : i === ticks.length - 1 ? "end" : "middle"} class="num fill-fg-muted text-[11px]">{short(t)}</text>
      {/each}
    </svg>

    {#if hp}
      <!-- Beside the crosshair, inside the plot, on whichever side has room. -->
      {@const right = x(hp.t) < width / 2}
      <div class="pointer-events-none absolute w-[200px] rounded-lg bg-surface-3 px-3 py-2 text-[12px] shadow-lg" style="top: {PAD.t + 4}px; left: {right ? x(hp.t) + 12 : x(hp.t) - 212}px;">
        <div class="flex items-center gap-2"><span class="h-0.5 w-3 rounded bg-[var(--c-chart)]"></span><span class="font-medium text-fg">{bytes(hp.used)} used</span></div>
        <div class="mt-0.5 text-fg-2">{bytes(hp.free)} free</div>
        <div class="text-fg-muted">{hover === pts.length - 1 ? "Now" : dateTime(hp.t)}</div>
      </div>
    {/if}
  </div>

  <details class="mt-2 text-[13px]">
    <summary class="cursor-pointer text-fg-muted select-none">Show as a table</summary>
    <table class="num mt-2 w-full max-w-md text-left">
      <thead class="text-fg-muted"><tr><th class="py-1 font-medium">When</th><th class="py-1 text-right font-medium">Used</th><th class="py-1 text-right font-medium">Free</th></tr></thead>
      <tbody class="text-fg-2">
        {#each [...pts].reverse() as p (p.t)}
          <tr><td class="py-0.5">{p.t === now ? "Now" : dayLabel(p.t)}</td><td class="py-0.5 text-right">{bytes(p.used)}</td><td class="py-0.5 text-right">{bytes(p.free)}</td></tr>
        {/each}
      </tbody>
    </table>
  </details>
  </details>
  {/if}
  {#if children}
    <div class="mt-4 border-t border-[var(--c-sep)] pt-4">{@render children()}</div>
  {/if}
</section>
