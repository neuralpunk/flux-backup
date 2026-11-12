<script lang="ts">
  import { tick } from "svelte";
  // A long page's sections, listed at its side (Compact's Settings and Preferences): the
  // one in view is marked as you scroll, and a click goes to it.
  let {
    scroller,
    sections,
    label = "Sections",
    onopen,
  }: {
    /** The element that scrolls the page. */
    scroller: HTMLElement | undefined;
    /** Each section's element id and name, in page order. */
    sections: { id: string; label: string }[];
    label?: string;
    /** Before going to a section (to open a folded one, say). */
    onopen?: (id: string) => void;
  } = $props();

  let current = $state<string | null>(null);

  // The section in view: the last one whose top has reached the upper part of the view
  // (at the very bottom, the last one, which may be too short to reach it).
  function track() {
    if (!scroller) return;
    const top = scroller.getBoundingClientRect().top;
    let on = sections[0]?.id ?? null;
    for (const s of sections) {
      const el = scroller.querySelector<HTMLElement>(`#${CSS.escape(s.id)}`);
      if (el && el.getBoundingClientRect().top - top <= 96) on = s.id;
    }
    if (scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 2) on = sections.at(-1)?.id ?? on;
    current = on;
  }

  $effect(() => {
    const el = scroller;
    void sections.length;
    if (!el) return;
    let frame = 0;
    const onScroll = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(track);
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    const ro = new ResizeObserver(onScroll);
    ro.observe(el);
    track();
    return () => {
      el.removeEventListener("scroll", onScroll);
      ro.disconnect();
      cancelAnimationFrame(frame);
    };
  });

  async function go(id: string) {
    onopen?.(id);
    await tick(); // a section opened just now has its height before it's scrolled to
    const el = scroller?.querySelector<HTMLElement>(`#${CSS.escape(id)}`);
    if (!el) return;
    const still = matchMedia("(prefers-reduced-motion: reduce)").matches;
    el.scrollIntoView({ behavior: still ? "auto" : "smooth", block: "start" });
    current = id;
    // Keyboard users land in the section, not back at the list.
    el.focus({ preventScroll: true });
  }
</script>

<nav class="sticky top-0 flex flex-col gap-0.5 pt-1" aria-label={label}>
  {#each sections as s (s.id)}
    <a
      href="#{s.id}"
      class="rounded-md px-2.5 py-1.5 text-[13px] leading-snug transition-colors {current === s.id ? 'bg-selected font-medium text-on-selected' : 'text-fg-2 hover:bg-surface-sunken hover:text-fg'}"
      aria-current={current === s.id ? "location" : undefined}
      onclick={(e) => {
        e.preventDefault();
        void go(s.id);
      }}>{s.label}</a
    >
  {/each}
</nav>
