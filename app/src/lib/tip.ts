// A label beside an icon-only control (the folded sidebar's): shown on pointing (after a
// moment) and on keyboard focus, to the element's right. `text` returns `null` while no
// label is wanted (the sidebar open).

export function tip(node: HTMLElement, text: () => string | null) {
  let get = text;
  let el: HTMLDivElement | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;

  function show() {
    const t = get();
    if (!t) return;
    el ??= document.createElement("div");
    el.className = "tip pointer-events-none fixed z-50 max-w-96 rounded-lg bg-surface-3 px-2.5 py-1.5 text-[12.5px] leading-snug text-fg shadow-[0_8px_24px_-10px_rgb(0_0_0/0.5),0_0_0_1px_var(--c-group-edge)]";
    el.setAttribute("role", "tooltip");
    el.textContent = t;
    const r = node.getBoundingClientRect();
    el.style.left = `${r.right + 10}px`;
    el.style.top = `${r.top + r.height / 2}px`;
    el.style.translate = "0 -50%";
    if (!el.isConnected) document.body.append(el);
  }
  function hide() {
    if (timer) clearTimeout(timer);
    timer = null;
    el?.remove();
  }
  const enter = () => {
    if (timer) clearTimeout(timer);
    timer = setTimeout(show, 350);
  };
  const focus = () => node.matches(":focus-visible") && show();

  node.addEventListener("mouseenter", enter);
  node.addEventListener("mouseleave", hide);
  node.addEventListener("focus", focus);
  node.addEventListener("blur", hide);
  node.addEventListener("click", hide);
  return {
    update(t: () => string | null) {
      get = t;
      if (!get()) hide();
    },
    destroy() {
      hide();
      node.removeEventListener("mouseenter", enter);
      node.removeEventListener("mouseleave", hide);
      node.removeEventListener("focus", focus);
      node.removeEventListener("blur", hide);
      node.removeEventListener("click", hide);
    },
  };
}
