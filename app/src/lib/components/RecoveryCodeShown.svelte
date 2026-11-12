<script lang="ts">
  // A recovery code just made, shown this once: copy it, say it's kept somewhere safe, done.
  import { copyText } from "../clipboard";
  import Icon from "./Icon.svelte";

  let { code, ondone }: { code: string; ondone: () => void } = $props();

  const halves = $derived.by(() => {
    const groups = code.split("-");
    const mid = Math.ceil(groups.length / 2);
    return [groups.slice(0, mid).join("-"), groups.slice(mid).join("-")].filter(Boolean);
  });
  let saved = $state(false);
  let copied = $state(false);

  async function copy() {
    await copyText(code);
    copied = true;
  }
</script>

<div class="flex flex-col gap-4 text-[14px] text-fg-2">
  <p>This is the only time it's shown. Keep it somewhere safe:</p>
  <!-- When it doesn't fit on one line, it wraps in the middle: three groups on each. -->
  <div class="selectable rounded-xl bg-surface-3 px-4 py-4 text-center font-mono text-[18px] font-semibold tracking-wide text-fg" aria-label="Recovery code">
    {#each halves as half, i (i)}{#if i > 0}-<wbr />{/if}<span class="whitespace-nowrap">{half}</span>{/each}
  </div>
  <div class="flex flex-wrap items-center gap-2">
    <button class="btn" onclick={copy}><Icon name={copied ? "check" : "copy"} size={14} /> {copied ? "Copied" : "Copy"}</button>
    <span class="text-[12.5px] text-fg-muted">Type it like a password to open the backups; dashes, spaces and capitals don't matter.</span>
  </div>
  <label class="flex items-center gap-2.5 text-fg">
    <input type="checkbox" class="size-4 accent-[var(--c-primary)]" bind:checked={saved} />
    I've written it down or kept it somewhere safe
  </label>
  <div class="flex justify-end">
    <button class="btn btn-primary" onclick={ondone} disabled={!saved}>Done</button>
  </div>
</div>
