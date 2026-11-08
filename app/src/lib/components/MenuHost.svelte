<script lang="ts">
  // Renders the app's one open menu (see menu.svelte.ts): positioned at the pointer or
  // under its button, kept inside the window, keyboard-driven.
  import { tick } from "svelte";
  import { menu, type MenuItem } from "../menu.svelte";
  import Icon from "./Icon.svelte";

  let el: HTMLDivElement | undefined = $state();
  let pos = $state({ left: 0, top: 0 });

  const items = $derived(menu.open?.items ?? []);
  const enabled = () => [...(el?.querySelectorAll<HTMLButtonElement>("[role=menuitem]:not([disabled])") ?? [])];

  $effect(() => {
    const o = menu.open;
    if (!o) return;
    void tick().then(() => {
      if (!el || menu.open !== o) return;
      const r = el.getBoundingClientRect();
      const m = 8;
      let left = o.align === "end" ? o.x - r.width : o.x;
      let top = o.y;
      left = Math.max(m, Math.min(left, window.innerWidth - r.width - m));
      // Not enough room below: open upwards.
      if (top + r.height > window.innerHeight - m) top = Math.max(m, (o.align === "end" ? o.y - 6 - 40 : o.y) - r.height);
      pos = { left, top };
      enabled()[0]?.focus();
    });
  });

  function choose(i: MenuItem) {
    if (i.disabled) return;
    menu.close(false);
    i.onselect();
  }

  function onkeydown(e: KeyboardEvent) {
    const list = enabled();
    const at = list.indexOf(document.activeElement as HTMLButtonElement);
    let next = -1;
    if (e.key === "ArrowDown") next = (at + 1) % list.length;
    else if (e.key === "ArrowUp") next = (at - 1 + list.length) % list.length;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = list.length - 1;
    else if (e.key === "Escape") return (e.preventDefault(), menu.close());
    else if (e.key === "Tab") return (e.preventDefault(), menu.close());
    else return;
    e.preventDefault();
    list[next]?.focus();
  }

  function outside(e: PointerEvent) {
    if (menu.open && el && !el.contains(e.target as Node) && !menu.open.owner?.contains(e.target as Node)) menu.close(false);
  }
</script>

<svelte:window onpointerdown={outside} onresize={() => menu.close(false)} onblur={() => menu.close(false)} />

{#if menu.open}
  <div
    bind:this={el}
    role="menu"
    tabindex="-1"
    aria-label={menu.open.label}
    class="fixed z-50 min-w-52 max-w-80 rounded-card bg-surface-3 p-1 text-[13.5px] shadow-[0_18px_48px_-12px_rgb(0_0_0/0.6)] ring-1 ring-fg/8"
    style="left: {pos.left}px; top: {pos.top}px"
    {onkeydown}
    oncontextmenu={(e) => e.preventDefault()}
  >
    {#each items as item, i (i)}
      {#if item === "separator"}
        <div class="mx-2 my-1 h-px bg-fg/10" role="separator"></div>
      {:else}
        <button
          role="menuitem"
          class="flex w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 text-left outline-none transition-colors hover:bg-track focus-visible:bg-track disabled:opacity-40 disabled:hover:bg-transparent {item.danger ? 'text-danger' : 'text-fg'}"
          disabled={item.disabled}
          title={item.hint ?? ""}
          onclick={() => choose(item)}
        >
          <span class="grid size-4 shrink-0 place-items-center {item.danger ? '' : 'text-fg-muted'}">{#if item.icon}<Icon name={item.icon} size={15} />{/if}</span>
          <span class="min-w-0 flex-1 truncate">{item.label}</span>
          {#if item.shortcut}<span class="num shrink-0 font-mono text-[11.5px] text-fg-muted">{item.shortcut}</span>{/if}
        </button>
      {/if}
    {/each}
  </div>
{/if}
