<script lang="ts">
  // A backup's status as a small mark, on a ladder so the serious stands out: red ⚠ for
  // not protected, a quiet amber dot for something worth a look, a green tick when fine,
  // a spinner while it runs, a clock before the first backup.
  import type { Tone } from "../attention";
  import Icon from "./Icon.svelte";

  let { tone, size = 13 }: { tone: Tone; size?: number } = $props();
</script>

{#if tone === "warn"}
  <span class="glyph" aria-hidden="true"><span class="size-2 rounded-full bg-warning"></span></span>
{:else}
  <span class="glyph {tone === 'ok' ? 'glyph-ok' : tone === 'bad' ? 'glyph-bad' : tone === 'run' ? 'glyph-run' : ''}" aria-hidden="true">
    <Icon name={tone === "ok" ? "check" : tone === "bad" ? "alert" : tone === "run" ? "refresh" : "clock"} {size} class={tone === "run" ? "motion-safe:animate-spin [animation-duration:2.4s]" : ""} />
  </span>
{/if}
