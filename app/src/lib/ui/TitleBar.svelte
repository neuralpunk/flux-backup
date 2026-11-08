<script lang="ts">
  // Custom titlebar (the window is undecorated). Drag anywhere on it to move the window;
  // double-click to maximize. Window buttons follow GNOME's: round, with symbolic icons.
  import { inTauri } from "../api";
  import { app } from "../state.svelte";
  import Icon from "../components/Icon.svelte";
  import Logo from "./Logo.svelte";

  async function win() {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    return getCurrentWindow();
  }
  const act = (f: "minimize" | "toggleMaximize" | "close") => () => {
    if (inTauri) void win().then((w) => w[f]());
  };
</script>

{#snippet winButton(label: string, icon: "minimize" | "maximize" | "x", f: "minimize" | "toggleMaximize" | "close")}
  <button
    aria-label={label}
    title={label}
    class="grid size-6 place-items-center rounded-full bg-surface-3 text-fg-2 transition-colors hover:bg-track hover:text-fg"
    onclick={act(f)}
  >
    <Icon name={icon} size={icon === "x" ? 13 : 12} />
  </button>
{/snippet}

<header data-tauri-drag-region class="relative flex h-12 shrink-0 items-center gap-2.5 pl-5 text-[13.5px] font-semibold text-fg">
  <Logo class="size-6" />
  <span data-tauri-drag-region>Flux Backup</span>
  {#if !inTauri}
    <span data-tauri-drag-region class="tag ml-3 text-[11.5px] font-normal">Demo: sample data · Vault password “demo”</span>
  {/if}
  <div class="absolute right-3 flex items-center gap-2">
    <button
      aria-label="Keyboard shortcuts"
      title="Keyboard shortcuts (?)"
      class="btn-icon mr-1 size-7"
      onclick={() => (app.showShortcuts = true)}><Icon name="help" size={16} /></button
    >
    {@render winButton("Minimize", "minimize", "minimize")}
    {@render winButton("Maximize", "maximize", "toggleMaximize")}
    {@render winButton("Close", "x", "close")}
  </div>
</header>
