// One menu for the whole app: right-click (context) menus and "⋯" buttons open it.
//
// Menus are shortcuts, never the only way: every action in one is also reachable on
// screen (see docs/UX_PRINCIPLES.md). Keyboard: the Menu key or Shift+F10 opens a
// focused element's context menu; arrows move, Enter/Space choose, Escape closes and
// returns focus to where it was.

import type { IconName } from "./components/Icon.svelte";

export type MenuItem = {
  label: string;
  icon?: IconName;
  onselect: () => void;
  /** Destructive: shown in the danger colour, kept apart by a separator. */
  danger?: boolean;
  disabled?: boolean;
  /** Why it's disabled, or what it does (tooltip). */
  hint?: string;
  /** Keyboard shortcut, shown at the right. */
  shortcut?: string;
};
export type MenuEntry = MenuItem | "separator";

interface OpenMenu {
  items: MenuEntry[];
  x: number;
  y: number;
  /** For screen readers: what the menu is about. */
  label: string;
  /** Focus goes back here when the menu closes. */
  returnFocus: HTMLElement | null;
  /** The element that opened it (a ⋯ button), for aria-expanded. */
  owner: HTMLElement | null;
  /** "end": `x` is where the menu's right edge goes (menus under ⋯ buttons). */
  align: "start" | "end";
  /** The item a context menu is about: outlined while the menu is open. */
  target: HTMLElement | null;
}

class MenuState {
  open = $state<OpenMenu | null>(null);

  show(items: MenuEntry[], x: number, y: number, label: string, owner: HTMLElement | null = null, align: "start" | "end" = "start", target: HTMLElement | null = null) {
    if (!items.some((i) => i !== "separator")) return;
    this.open?.target?.removeAttribute("data-menu-target");
    target?.setAttribute("data-menu-target", "");
    this.open = { items, x, y, label, owner, align, target, returnFocus: (document.activeElement as HTMLElement | null) ?? owner ?? target };
  }

  close(restoreFocus = true) {
    const o = this.open;
    this.open = null;
    o?.target?.removeAttribute("data-menu-target");
    if (restoreFocus) o?.returnFocus?.focus?.();
  }

  /** Is the menu open, opened by `owner`? */
  isOpenFor(owner: HTMLElement | undefined | null): boolean {
    return !!owner && this.open?.owner === owner;
  }
}

export const menu = new MenuState();

/** Open `items` below `anchor` (a "⋯" button), right-aligned to it. */
export function openMenuBelow(anchor: HTMLElement, items: MenuEntry[], label: string) {
  if (menu.isOpenFor(anchor)) return menu.close();
  const r = anchor.getBoundingClientRect();
  menu.show(items, r.right, r.bottom + 6, label, anchor, "end");
}

/**
 * Svelte action: a context menu for `node`. `items` is called when the menu opens, so
 * it always reflects the current state.
 *
 *   <button use:contextMenu={{ items: () => [...], label: "Home folder" }}>
 */
export function contextMenu(node: HTMLElement, opts: { items: () => MenuEntry[]; label: string }) {
  let current = opts;
  const onContext = (e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    menu.show(current.items(), e.clientX, e.clientY, current.label, null, "start", node);
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
      e.preventDefault();
      const r = node.getBoundingClientRect();
      menu.show(current.items(), r.left + 16, r.bottom - 4, current.label, null, "start", node);
    }
  };
  node.addEventListener("contextmenu", onContext);
  node.addEventListener("keydown", onKey);
  return {
    update(next: { items: () => MenuEntry[]; label: string }) {
      current = next;
    },
    destroy() {
      node.removeEventListener("contextmenu", onContext);
      node.removeEventListener("keydown", onKey);
    },
  };
}
