<script lang="ts">
  // Custom titlebar (the window is undecorated). Drag anywhere on it to move the window;
  // double-click to maximize. Window buttons follow GNOME's: round, with symbolic icons.
  import { api, inTauri } from "../api";
  import { openMenuBelow } from "../menu.svelte";
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

  // GNOME's main menu: the app-wide places that aren't a backup.
  function mainMenu(e: MouseEvent & { currentTarget: HTMLElement }) {
    openMenuBelow(
      e.currentTarget,
      [
        { label: "Preferences", icon: "settings", onselect: () => app.openPreferences() },
        { label: "Keyboard shortcuts", icon: "help", shortcut: "?", onselect: () => (app.showShortcuts = true) },
        {
          label: "User guide",
          icon: "lifebuoy",
          onselect: () => {
            if (inTauri) api.openLink("guide").catch((err: Error) => app.toast("bad", err.message));
            else app.toast("info", "The guide opens in your web browser in the app.");
          },
        },
        "separator",
        { label: "About Flux Backup", icon: "info", onselect: () => (app.dialog = { kind: "about" }) },
      ],
      "Main menu",
    );
  }
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
  <button
    class="btn-icon ml-1 size-7 text-fg-2"
    aria-label={app.sidebarFolded ? "Open the sidebar" : "Fold the sidebar"}
    aria-pressed={!app.sidebarFolded}
    title="{app.sidebarFolded ? 'Open the sidebar' : 'Fold the sidebar'} (F9)"
    onclick={() => app.toggleSidebar()}><Icon name="sidebar" size={16} /></button
  >
  {#if !inTauri}
    <span data-tauri-drag-region class="tag ml-3 text-[11.5px] font-normal">Demo: sample data · Vault password “demo”</span>
  {/if}
  <div class="absolute right-3 flex items-center gap-2">
    <button
      aria-label="Main menu"
      title="Main menu"
      aria-haspopup="menu"
      class="btn-icon mr-1 size-7"
      onclick={mainMenu}><Icon name="menu" size={16} /></button
    >
    {@render winButton("Minimize", "minimize", "minimize")}
    {@render winButton("Maximize", "maximize", "toggleMaximize")}
    {@render winButton("Close", "x", "close")}
  </div>
</header>
