<script lang="ts">
  // A "⋯" button that opens a menu below itself. The same actions are usually also on
  // the item's right-click menu.
  import { menu, openMenuBelow, type MenuEntry } from "../menu.svelte";
  import Icon from "./Icon.svelte";

  let { items, label, class: cls = "" }: { items: () => MenuEntry[]; label: string; class?: string } = $props();
  let btn: HTMLButtonElement | undefined = $state();
</script>

<button
  bind:this={btn}
  class="btn w-9 justify-center px-0 {cls}"
  aria-label={label}
  title={label}
  aria-haspopup="menu"
  aria-expanded={menu.isOpenFor(btn)}
  onclick={() => btn && openMenuBelow(btn, items(), label)}
>
  <Icon name="more" size={18} class="[stroke-width:3]" />
</button>
