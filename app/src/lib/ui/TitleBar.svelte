<script lang="ts">
  // Custom titlebar (the window is undecorated so the content panel can float).
  // Drag anywhere on it to move the window; double-click to maximize.
  import { inTauri } from "../api";
  import { app } from "../state.svelte";

  async function win() {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    return getCurrentWindow();
  }
  const act = (f: "minimize" | "toggleMaximize" | "close") => () => {
    if (inTauri) void win().then((w) => w[f]());
  };
</script>

<header data-tauri-drag-region class="relative flex h-10 shrink-0 items-center justify-center text-[13px] font-medium text-fg-2">
  <span data-tauri-drag-region>Flux Backup</span>
  {#if !inTauri}
    <span data-tauri-drag-region class="tag absolute left-4 py-0.5 text-[11.5px]">Demo: sample data · Vault password “demo”</span>
  {/if}
  <div class="absolute right-4 flex items-center gap-2.5">
    <button aria-label="Keyboard shortcuts" title="Keyboard shortcuts (?)" class="mr-2 grid size-6 place-items-center rounded-full font-mono text-[11px] text-fg-muted hover:bg-surface-2 hover:text-fg" onclick={() => (app.showShortcuts = true)}>?</button>
    <button aria-label="Minimize" title="Minimize" class="size-3 rounded-full bg-surface-3 transition-colors hover:bg-fg-muted" onclick={act("minimize")}></button>
    <button aria-label="Maximize" title="Maximize" class="size-3 rounded-full bg-surface-3 transition-colors hover:bg-fg-muted" onclick={act("toggleMaximize")}></button>
    <button aria-label="Close" title="Close" class="size-3 rounded-full bg-surface-3 transition-colors hover:bg-danger" onclick={act("close")}></button>
  </div>
</header>
