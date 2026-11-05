# Design

The app follows the **"Graphite Tonal" (2a)** design below (reference:
[screenshots/overview-dark.png](screenshots/overview-dark.png)). Tokens live in
`app/src/app.css`; the shell is `App.svelte`, `lib/ui/TitleBar.svelte`, `lib/ui/Logo.svelte`
and `lib/components/Sidebar.svelte`.

## How Flux applies it

- **Component classes** in `app.css` carry the recipes: `.card` (surface-2, 20px), `.btn`
  (chip), `.btn-primary` (filled pill), `.btn-tonal`, `.btn-ghost`, `.input`, `.label`,
  `.title`, `.tag`. One filled button per view; in Settings and dialogs the page's own
  action is the filled one.
- **Less on screen:** details live behind a click or a tooltip. Snapshots list only times
  (names and pins when set), with rare actions in a ⋯ menu. Overview shows one *Recent
  backups* card. The profile editor asks three questions plus an encryption switch;
  **Advanced options** reveals the rest in place.
- **Light theme status colours** keep the hues but are darker than the dark theme's
  (`--c-success/-warning/-danger` under `[data-theme="light"]`). The dark values put text
  on white at about 1.9:1; these give at least 4.5:1 on cards, chips and tags (WCAG AA).
- **Logo:** always on its dark tile (the tile is part of the icon), also in the light theme.
- **Progress** comes from each backup's status socket (`flux://job` events, ~5/s), not a
  per-command Channel: backups run as separate processes, so any window (or `flux watch`)
  can follow them.
- **Not implemented:** *Pause* from the reference. A backup can be stopped and continues
  from where it stopped next time.
- **Window:** undecorated with a custom titlebar (drag to move, double-click to maximize,
  three window buttons, plus `?` for shortcuts). The window stays square; the content panel
  carries the 24px radius.
- **Icons:** `app/src-tauri/icons/` (Tauri bundle, tray idle/active),
  `packaging/linux/icons/` (extra hicolor sizes, scalable and symbolic SVGs, written
  from the geometry below without embedded metadata).

---

## Spec: Flux Backup — "Graphite Tonal" theme (2a)

Implementation guide for **Rust + Tauri 2 + Svelte 5 + Tailwind CSS 4**.
Reference: `2a-tonal.png`.

---

### 1. Design principles

- **No borders.** Hierarchy comes from tonal surface steps, not outlines.
- **Three surface levels:** window → content panel → cards. Each step is slightly lighter.
- **One accent hue (blue, ~250°)**, used in two strengths:
  - *Primary* (bright): primary action, progress fill, percentages.
  - *Primary container* (dim, tinted): selected nav item, active tab, secondary button.
- **Pills for anything selectable** (nav items, tabs, theme switch, buttons). Rounded rectangles for content.
- **Mono only for machine data**: paths, sizes, speeds, counts.
- The content panel floats inside the window with a 12px gutter and 24px radius. That's the signature look.

---

### 2. Design tokens

### Colour (dark — default)

| Token | Value | Use |
|---|---|---|
| `bg` | `#0e0f11` | Window, sidebar, titlebar |
| `surface-1` | `#15171a` | Main content panel |
| `surface-2` | `#1b1d21` | Cards, progress card, tab track |
| `surface-3` | `#24272c` | Chip buttons inside cards |
| `surface-sunken` | `#17191c` | Sidebar cards, theme switch track |
| `tag` | `#1f2226` | Metadata chips under title |
| `track` | `#2a2d33` | Progress track |
| `fg` | `#e8e9eb` | Primary text |
| `fg-2` | `#b9bcc2` | Secondary text, inactive nav |
| `fg-muted` | `#8b9099` | Labels, captions |
| `primary` | `oklch(0.78 0.12 250)` | Primary button, progress fill |
| `on-primary` | `#0b1626` | Text on primary |
| `primary-container` | `oklch(0.32 0.06 250)` | Selected pill, tonal button |
| `on-primary-container` | `oklch(0.90 0.06 250)` | Text on container |
| `brand` | `oklch(0.72 0.14 250)` | Logo mark, status dots |

### Colour (light — derived)

Same structure with lightness inverted. Hue and chroma stay the same.

| Token | Value |
|---|---|
| `bg` | `#eef0f3` |
| `surface-1` | `#f8f9fb` |
| `surface-2` | `#ffffff` |
| `surface-3` | `#eceef2` |
| `surface-sunken` | `#e4e7ec` |
| `tag` | `#e8ebef` |
| `track` | `#dde1e7` |
| `fg` | `#16181c` |
| `fg-2` | `#3d424a` |
| `fg-muted` | `#646a74` |
| `primary` | `oklch(0.55 0.15 250)` |
| `on-primary` | `#ffffff` |
| `primary-container` | `oklch(0.90 0.05 250)` |
| `on-primary-container` | `oklch(0.30 0.08 250)` |

### Status colours (same L/C, different hue)

`success oklch(0.78 0.12 155)` · `warning oklch(0.82 0.12 80)` · `danger oklch(0.72 0.14 25)`

### Type

- **Sans:** IBM Plex Sans (400 / 500 / 600)
- **Mono:** IBM Plex Mono (400 / 500)

| Role | Size / weight |
|---|---|
| Page title | 28 / 500 |
| Card value | 22 / 500 |
| Section title | 18 / 500 (progress), 15 / 500 (cards) |
| Body / nav | 14 / 400–500 |
| Label | 12.5 / 400, `fg-muted` |
| Mono data | 12.5–13 / 400 |

### Shape & spacing

| Token | Value |
|---|---|
| `radius-panel` | 24px |
| `radius-card` | 20px |
| `radius-sm` | 12px (chip buttons), 8px (tags) |
| `radius-pill` | 9999px |
| Window gutter | 12px |
| Panel padding | 28px 32px |
| Card padding | 18–24px |
| Grid gap | 12px |
| Sidebar width | 232px |

---

### 3. Tailwind 4 setup

Tailwind 4 is CSS-first, so the tokens live in `src/app.css`. No JS config needed.

```css
/* src/app.css */
@import "tailwindcss";

/* Fonts are bundled locally, so they work offline */
@import "@fontsource/ibm-plex-sans/400.css";
@import "@fontsource/ibm-plex-sans/500.css";
@import "@fontsource/ibm-plex-sans/600.css";
@import "@fontsource/ibm-plex-mono/400.css";
@import "@fontsource/ibm-plex-mono/500.css";

/* dark: variant follows [data-theme="dark"] on <html> */
@custom-variant dark (&:where([data-theme="dark"], [data-theme="dark"] *));

@theme {
  --font-sans: "IBM Plex Sans", ui-sans-serif, system-ui, sans-serif;
  --font-mono: "IBM Plex Mono", ui-monospace, monospace;

  --radius-panel: 24px;
  --radius-card: 20px;

  /* Semantic colours point at runtime vars, so utilities like bg-surface-2 switch with the theme */
  --color-bg: var(--c-bg);
  --color-surface-1: var(--c-surface-1);
  --color-surface-2: var(--c-surface-2);
  --color-surface-3: var(--c-surface-3);
  --color-surface-sunken: var(--c-surface-sunken);
  --color-tag: var(--c-tag);
  --color-track: var(--c-track);
  --color-fg: var(--c-fg);
  --color-fg-2: var(--c-fg-2);
  --color-fg-muted: var(--c-fg-muted);
  --color-primary: var(--c-primary);
  --color-on-primary: var(--c-on-primary);
  --color-primary-container: var(--c-primary-container);
  --color-on-primary-container: var(--c-on-primary-container);
  --color-brand: var(--c-brand);
  --color-success: oklch(0.78 0.12 155);
  --color-warning: oklch(0.82 0.12 80);
  --color-danger: oklch(0.72 0.14 25);
}

:root, [data-theme="dark"] {
  color-scheme: dark;
  --c-bg: #0e0f11;
  --c-surface-1: #15171a;
  --c-surface-2: #1b1d21;
  --c-surface-3: #24272c;
  --c-surface-sunken: #17191c;
  --c-tag: #1f2226;
  --c-track: #2a2d33;
  --c-fg: #e8e9eb;
  --c-fg-2: #b9bcc2;
  --c-fg-muted: #8b9099;
  --c-primary: oklch(0.78 0.12 250);
  --c-on-primary: #0b1626;
  --c-primary-container: oklch(0.32 0.06 250);
  --c-on-primary-container: oklch(0.90 0.06 250);
  --c-brand: oklch(0.72 0.14 250);
}

[data-theme="light"] {
  color-scheme: light;
  --c-bg: #eef0f3;
  --c-surface-1: #f8f9fb;
  --c-surface-2: #ffffff;
  --c-surface-3: #eceef2;
  --c-surface-sunken: #e4e7ec;
  --c-tag: #e8ebef;
  --c-track: #dde1e7;
  --c-fg: #16181c;
  --c-fg-2: #3d424a;
  --c-fg-muted: #646a74;
  --c-primary: oklch(0.55 0.15 250);
  --c-on-primary: #ffffff;
  --c-primary-container: oklch(0.90 0.05 250);
  --c-on-primary-container: oklch(0.30 0.08 250);
  --c-brand: oklch(0.60 0.15 250);
}

@layer base {
  html, body { @apply h-full bg-bg text-fg font-sans antialiased; }
  body { user-select: none; } /* desktop feel; re-enable on selectable text */
  .selectable { user-select: text; }
}

/* Utilities you'll reuse a lot */
@utility pill { border-radius: 9999px; }
@utility num { font-variant-numeric: tabular-nums; }
```

Use `tabular-nums` (`num`) on every changing number (%, MiB, speed) so the text doesn't jitter while it updates.

---

### 4. Tauri 2 window setup

The floating panel works best with a custom titlebar.

```jsonc
// src-tauri/tauri.conf.json  (excerpt)
{
  "app": {
    "windows": [{
      "title": "Flux Backup",
      "width": 1200, "height": 780,
      "minWidth": 900, "minHeight": 600,
      "decorations": false,
      "transparent": false,
      "backgroundColor": "#0e0f11"   // avoids a white flash before the webview paints
    }]
  }
}
```

```jsonc
// src-tauri/capabilities/default.json  (excerpt)
{
  "permissions": [
    "core:default",
    "core:window:allow-minimize",
    "core:window:allow-toggle-maximize",
    "core:window:allow-close",
    "core:window:allow-start-dragging",
    "core:window:allow-set-theme"
  ]
}
```

**Linux note:** client-side decorations under WebKitGTK can't draw rounded outer corners on every compositor. Keep the *window* square and let the inner panel carry the 24px radius. That's what the design does anyway.

### Titlebar

```svelte
<!-- src/lib/ui/TitleBar.svelte -->
<script lang="ts">
  import { getCurrentWindow } from '@tauri-apps/api/window';
  const win = getCurrentWindow();
</script>

<header data-tauri-drag-region
  class="h-10 relative flex items-center justify-center text-[13px] font-medium text-fg-2">
  Flux Backup
  <div class="absolute right-4 flex gap-2.5">
    <button aria-label="Minimize" class="size-3 rounded-full bg-surface-3 hover:bg-fg-muted" onclick={() => win.minimize()}></button>
    <button aria-label="Maximize" class="size-3 rounded-full bg-surface-3 hover:bg-fg-muted" onclick={() => win.toggleMaximize()}></button>
    <button aria-label="Close" class="size-3 rounded-full bg-surface-3 hover:bg-danger" onclick={() => win.close()}></button>
  </div>
</header>
```

---

### 5. Theme switching (Dark / Light / Auto) with Svelte 5 runes

```ts
// src/lib/theme.svelte.ts
type Mode = 'dark' | 'light' | 'auto';

const mq = window.matchMedia('(prefers-color-scheme: dark)');

class Theme {
  mode = $state<Mode>((localStorage.getItem('theme') as Mode) ?? 'auto');
  systemDark = $state(mq.matches);
  resolved = $derived(this.mode === 'auto' ? (this.systemDark ? 'dark' : 'light') : this.mode);

  constructor() {
    mq.addEventListener('change', (e) => (this.systemDark = e.matches));
    $effect.root(() => {
      $effect(() => {
        document.documentElement.dataset.theme = this.resolved;
        localStorage.setItem('theme', this.mode);
      });
    });
  }
}
export const theme = new Theme();
```

`prefers-color-scheme` in WebKitGTK follows the GTK theme. If it doesn't track the GNOME/KDE setting on some desktops, read it from Rust with the `dark-light` crate and expose it as a command.

---

### 6. Component recipes

Each one is a small Svelte 5 component. These are the Tailwind classes that produce the 2a look.

### App shell

```svelte
<div class="h-screen flex flex-col bg-bg">
  <TitleBar />
  <div class="flex-1 min-h-0 flex gap-3 px-3 pb-3">
    <Sidebar />                       <!-- w-[232px] -->
    <main class="flex-1 min-w-0 bg-surface-1 rounded-panel px-8 py-7 flex flex-col gap-[22px] overflow-auto">
      {@render children()}
    </main>
  </div>
</div>
```

### Nav item (sidebar profile)

```svelte
<script lang="ts">
  let { label, active = false, status, progress }: {
    label: string; active?: boolean; status?: 'running'|'idle'; progress?: number
  } = $props();
</script>

<button class={[
  'w-full flex items-center gap-3 px-4 py-3 pill text-sm transition-colors',
  active ? 'bg-primary-container text-on-primary-container font-medium'
         : 'text-fg-2 hover:bg-surface-sunken'
]}>
  <span class={['size-2 rounded-full', status === 'running' ? 'bg-brand' : 'bg-surface-3']}></span>
  <span class="flex-1 text-left">{label}</span>
  {#if progress != null}<span class="font-mono text-xs num">{progress}%</span>{/if}
</button>
```

### Buttons

```svelte
<!-- variant: 'filled' | 'tonal' | 'chip' -->
<script lang="ts">
  let { variant = 'tonal', children, ...rest } = $props();
  const styles = {
    filled: 'pill px-5 py-2.5 bg-primary text-on-primary font-semibold hover:brightness-110',
    tonal:  'pill px-5 py-2.5 bg-primary-container text-on-primary-container font-medium hover:brightness-125',
    chip:   'rounded-xl px-4 py-2 bg-surface-3 text-fg text-[13.5px] hover:bg-track'
  };
</script>
<button class={[styles[variant], 'text-sm transition active:scale-[.98] disabled:opacity-40']} {...rest}>
  {@render children()}
</button>
```

Rule: one **filled** button per view (Stop / Back up now). Use **tonal** for its sibling. Use **chip** for actions inside cards.

### Segmented tabs

```svelte
<script lang="ts">
  let { tabs, value = $bindable() }: { tabs: string[]; value: string } = $props();
</script>
<div role="tablist" class="self-start flex gap-1 bg-surface-2 pill p-1 text-[13.5px]">
  {#each tabs as t}
    <button role="tab" aria-selected={value === t} onclick={() => (value = t)}
      class={['pill px-[18px] py-2 transition-colors',
        value === t ? 'bg-primary-container text-on-primary-container font-medium' : 'text-fg-2 hover:text-fg']}>
      {t}
    </button>
  {/each}
</div>
```

### Metadata tag

```svelte
<span class="px-2.5 py-1 rounded-lg bg-tag text-fg-2 text-[12.5px]">Manual</span>
<span class="px-2.5 py-1 rounded-lg bg-tag text-fg-2 text-[12.5px] font-mono selectable">/run/media/…/TEST</span>
```

### Stat card

```svelte
<div class="bg-surface-2 rounded-card px-5 py-[18px] flex flex-col gap-1.5">
  <span class="text-[12.5px] text-fg-muted">{label}</span>
  <span class="text-[22px] font-medium num">{value}</span>
</div>
```

Lay out four of these with `grid grid-cols-4 gap-3`. Below ~1000px, switch to `grid-cols-2`.

### Progress card

```svelte
<script lang="ts">
  let { p }: { p: Progress } = $props();
  const pct = $derived(Math.round((p.bytesDone / p.bytesTotal) * 100));
</script>

<section class="bg-surface-2 rounded-card px-6 py-[22px] flex flex-col gap-4">
  <div class="flex justify-between items-baseline">
    <div class="flex gap-3 items-baseline">
      <span class="text-lg font-medium">{p.phase}</span>
      <span class="font-mono text-lg font-medium text-primary num">{pct}%</span>
    </div>
    <span class="font-mono text-[13px] text-fg-muted num">
      {fmtBytes(p.bytesDone)} / {fmtBytes(p.bytesTotal)} · {fmtBytes(p.rate)}/s · {p.itemsDone}/{p.itemsTotal} · {fmtEta(p.eta)} left
    </span>
  </div>
  <div class="h-2 rounded bg-track overflow-hidden"
       role="progressbar" aria-valuenow={pct} aria-valuemin="0" aria-valuemax="100">
    <div class="h-full rounded bg-primary transition-[width] duration-300 ease-out" style:width="{pct}%"></div>
  </div>
</section>
```

---

### 7. Streaming progress from Rust

Use a Tauri 2 **Channel** for high-frequency updates. Throttle to about 10 Hz in Rust so the UI stays smooth.

```rust
// src-tauri/src/backup.rs
use serde::Serialize;
use tauri::ipc::Channel;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    phase: &'static str,        // "Scanning" | "Copying" | "Finalizing"
    bytes_done: u64,
    bytes_total: u64,
    rate: u64,                  // bytes/sec, smoothed (EMA)
    items_done: u32,
    items_total: u32,
    eta: u32,                   // seconds
}

#[tauri::command]
pub async fn run_backup(profile_id: String, on_progress: Channel<Progress>) -> Result<(), String> {
    // ... in your copy loop, at most every 100 ms:
    // on_progress.send(Progress { .. }).ok();
    Ok(())
}
```

```ts
// src/lib/backup.svelte.ts
import { invoke, Channel } from '@tauri-apps/api/core';

export const run = $state<{ progress: Progress | null }>({ progress: null });

export async function startBackup(profileId: string) {
  const ch = new Channel<Progress>();
  ch.onmessage = (p) => (run.progress = p);
  await invoke('run_backup', { profileId, onProgress: ch });
  run.progress = null;
}
```

---

### 8. Motion & states

- Transitions: 150ms colour/background, 300ms ease-out for the progress width. Nothing bouncy.
- Hover on pills: background moves one surface step up. Containers get `brightness-125`.
- Focus: `focus-visible:outline-2 outline-offset-2 outline-primary` on every interactive element.
- Running state: the status dot in the sidebar can pulse (`animate-pulse`). Avoid spinners elsewhere.
- Respect `prefers-reduced-motion` by wrapping the pulse and width transitions in `motion-safe:`.

---

### 9. Do / Don't

**Do**
- Separate things by surface step and spacing.
- Keep accent use to one filled button, the progress bar, and the selected states.
- Use mono + tabular numbers for anything a machine produced.

**Don't**
- Add 1px borders to cards. That turns it back into 1a.
- Use gradients on the progress bar or buttons.
- Introduce a second accent hue for decoration. Hues are for status only (success, warning, danger).

---

### 10. App icon: "Stack"

Three stacked pills represent snapshots. The top one (the newest) is lit in `primary`, and the older ones step down through the tonal ramp. It reuses the pill shape from the UI, and it stays readable at 16px.

### Geometry (256 grid)

| Part | Spec |
|---|---|
| Tile | 256×256, `rx 58`, fill `#1b1d21` (`surface-2`) |
| Pills | 144×32, `rx 16`, x = 56, y = 68 / 112 / 156 (12px gap) |
| Pill 1 (newest) | `#7abdff` ≈ `oklch(0.78 0.12 250)` (`primary`) |
| Pill 2 | `#32618e` ≈ `oklch(0.48 0.09 250)` |
| Pill 3 (oldest) | `#193550` ≈ `oklch(0.32 0.06 250)` (`primary-container`) |

The SVGs use hex values, not oklch, because librsvg (GNOME/KDE icon rendering) doesn't support oklch.

### Tauri bundle icons

The best route is to let the Tauri CLI generate every platform format from the 1024px master:

```bash
npx tauri icon icons/app-icon-1024.png   # writes src-tauri/icons/*
```

If you'd rather place them by hand, copy `icons/tauri/*` into `src-tauri/icons/` and rename `128x128-2x.png` to `128x128@2x.png`. Then reference them:

```jsonc
// tauri.conf.json
"bundle": {
  "icon": ["icons/32x32.png", "icons/128x128.png", "icons/128x128@2x.png", "icons/icon.png"]
}
```

### Linux desktop integration (.deb / .rpm / AppImage / Flatpak)

Install the hicolor set and the scalable SVG under your app ID:

```
/usr/share/icons/hicolor/{16..512}x{16..512}/apps/flux-backup.png   ← icons/hicolor/*
/usr/share/icons/hicolor/scalable/apps/flux-backup.svg              ← icons/flux-backup.svg
/usr/share/icons/hicolor/symbolic/apps/flux-backup-symbolic.svg     ← icons/flux-backup-symbolic.svg
```

In the `.desktop` file, set `Icon=flux-backup`. For Flatpak, rename the files to your reverse-DNS ID (e.g. `io.flux.Backup.svg`).

### Tray icon

- `flux-backup-symbolic.svg` / `tray-64.png`: idle. The grey `#bebebe` lets GNOME recolour it as a symbolic icon.
- `flux-backup-tray-active.svg` / `tray-active-64.png`: backup running, with the top pill lit blue.

```rust
// Tauri 2: swap the tray icon when a run starts/stops
use tauri::{image::Image, tray::TrayIconBuilder};
let idle   = Image::from_bytes(include_bytes!("../icons/tray-64.png"))?;
let active = Image::from_bytes(include_bytes!("../icons/tray-active-64.png"))?;
let tray = TrayIconBuilder::with_id("main").icon(idle.clone()).build(app)?;
// later: tray.set_icon(Some(active.clone()))?;
```

(`Image::from_bytes` needs the `image-png` feature on the `tauri` crate.)

### In-app usage

The sidebar logo is the same SVG at 28px. Inline it as a Svelte component so it follows the theme:

```svelte
<!-- src/lib/ui/Logo.svelte -->
<svg viewBox="0 0 256 256" class="size-7" aria-hidden="true">
  <rect width="256" height="256" rx="58" class="fill-surface-2" />
  <rect x="56" y="68"  width="144" height="32" rx="16" class="fill-primary" />
  <rect x="56" y="112" width="144" height="32" rx="16" fill="oklch(0.48 0.09 250)" />
  <rect x="56" y="156" width="144" height="32" rx="16" class="fill-primary-container" />
</svg>
```

**Don'ts:** don't add gradients or shadows inside the tile, don't change the pill order (the lit pill is always on top), and don't put the mark on a light tile. The dark tile is the icon.

---

### 11. Files in this bundle

```
THEME.md                         this guide
app.css                          drop-in Tailwind 4 stylesheet (§3)
2a-tonal.png                     UI reference screenshot
icons/
  preview.png                    icon at all sizes
  app-icon-1024.png              master for `tauri icon`
  flux-backup.svg                scalable app icon
  flux-backup-symbolic.svg       monochrome symbolic/tray (idle)
  flux-backup-tray-active.svg    tray while backing up
  tray-64.png, tray-active-64.png
  tauri/                         32, 128, 128@2x, icon.png (512)
  hicolor/NxN/apps/              16–512 PNGs for /usr/share/icons
```
