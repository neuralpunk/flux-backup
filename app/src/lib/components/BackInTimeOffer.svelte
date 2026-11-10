<script lang="ts">
  import { count } from "../format";
  import type { BitProfile } from "../types";
  import Icon from "./Icon.svelte";
  import InfoDetails from "./InfoDetails.svelte";

  /** Back In Time is set up here: offer to start from one of its profiles, then say what
   * came over (and what to do about the old backups). */
  let {
    profiles,
    used,
    onuse,
    onstartover,
    ondismiss,
  }: {
    profiles: BitProfile[];
    used: BitProfile | null;
    onuse: (b: BitProfile) => void;
    onstartover: () => void;
    ondismiss: () => void;
  } = $props();

  const where = (b: BitProfile) => {
    const t = b.profile.target;
    return t.kind === "ssh" ? `${t.host}:${t.path}` : t.path;
  };
  const folders = (b: BitProfile) => `${b.profile.sources.paths.length} ${b.profile.sources.paths.length === 1 ? "folder" : "folders"}`;
  /** "41 snapshots, the newest Nov 8" or why there are none to show. */
  function state(b: BitProfile): string {
    if (b.mode.startsWith("ssh")) return "on a server";
    if (!b.connected) return "drive not connected";
    if (!b.snapshot_count) return "no snapshots yet";
    const l = b.latest?.match(/^(\d{4})(\d\d)(\d\d)-/);
    const newest = l ? new Date(+l[1]!, +l[2]! - 1, +l[3]!).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" }) : null;
    return `${count(b.snapshot_count)} ${b.snapshot_count === 1 ? "snapshot" : "snapshots"}${newest ? `, newest ${newest}` : ""}`;
  }
</script>

{#if used}
  <section class="bit-used flex flex-col gap-3 rounded-card px-5 py-4" aria-live="polite">
    <div class="flex items-start gap-3">
      <span class="glyph glyph-ok mt-0.5"><Icon name="check" size={13} /></span>
      <div class="min-w-0 flex-1">
        <h3 class="font-semibold">Started from Back In Time's “{used.name}”</h3>
        <p class="mt-0.5 text-[13px] text-fg-2">Its settings are filled in; check each step.</p>
      </div>
      <button class="btn btn-ghost h-8 shrink-0" onclick={onstartover}>Start from scratch</button>
    </div>
    <InfoDetails summary="What happens to the Back In Time backups">
      <ul class="flex list-disc flex-col gap-2 pl-4 marker:text-primary/60">
        <li>
          They stay where they are{#if used.snapshots_dir}{" "}(<code class="font-mono text-[12px] text-fg">{used.snapshots_dir}</code>){/if}. Flux can't continue them, so it starts its own, fresh set of
          backups next to them. Keep Back In Time installed to restore from the old ones, and delete them once you no longer need them.
        </li>
        <li>Turn off Back In Time's schedule (in its settings), so the two don't both back up the same files.</li>
        {#each used.notes as n (n)}<li>{n}</li>{/each}
      </ul>
    </InfoDetails>
  </section>
{:else}
  <section class="bit-offer flex flex-col gap-3 rounded-card px-5 py-4" aria-labelledby="bit-title">
    <div class="flex items-start gap-3">
      <span class="glyph glyph-run mt-0.5"><Icon name="info" size={13} /></span>
      <div class="min-w-0 flex-1">
        <h3 id="bit-title" class="font-semibold">Start from your Back In Time settings?</h3>
        <p class="mt-0.5 max-w-[66ch] text-[13px] text-fg-2">Its folders, exclusions, schedule and what to keep come over. Your old snapshots stay as they are.</p>
      </div>
      <button class="btn-icon size-8 shrink-0" onclick={ondismiss} aria-label="Not now" title="Not now"><Icon name="x" size={15} /></button>
    </div>
    <div class="rows">
      {#each profiles as b (b.number)}
        <div class="row gap-3">
          <Icon name={b.mode.startsWith("ssh") ? "server" : "drive"} size={17} class="shrink-0 text-fg-2" />
          <span class="min-w-0 flex-1">
            <span class="block font-medium">{b.name}</span>
            <span class="flex gap-1 text-[12.5px] text-fg-2">
              <span class="min-w-0 truncate" title={where(b)}>{folders(b)} → <span class="font-mono">{where(b)}</span></span>
              <span class="shrink-0 text-fg-muted">· {state(b)}{b.mode.endsWith("encfs") ? " · EncFS" : ""}</span>
            </span>
          </span>
          <button class="btn h-8 shrink-0" onclick={() => onuse(b)}>Use these settings</button>
        </div>
      {/each}
    </div>
  </section>
{/if}

<style>
  /* Set apart from the form around it: an offer, tinted with the action blue… */
  .bit-offer {
    background: linear-gradient(135deg, color-mix(in oklab, var(--c-primary) 16%, var(--c-surface-2)), color-mix(in oklab, var(--c-primary) 6%, var(--c-surface-2)));
    box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--c-primary) 35%, transparent);
  }
  /* …and, once used, the green of things that are done. */
  .bit-used {
    background: color-mix(in oklab, var(--c-success) 9%, var(--c-surface-2));
    box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--c-success) 28%, transparent);
  }
</style>
