/** A bar along the bottom of the window (an editor's Save bar): toasts rise above it
 * while it's there, instead of covering its buttons. Use as `use:liftToasts`. */
export function liftToasts(node: HTMLElement) {
  const root = document.documentElement;
  const set = () => root.style.setProperty("--toast-lift", `${node.offsetHeight}px`);
  set();
  const ro = new ResizeObserver(set);
  ro.observe(node);
  return {
    destroy() {
      ro.disconnect();
      root.style.removeProperty("--toast-lift");
    },
  };
}
