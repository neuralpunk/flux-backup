<script lang="ts" module>
  // Small built-in icon set: 24×24, 1.75px strokes, currentColor.
  export const paths = {
    lifebuoy: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18ZM12 16a4 4 0 1 0 0-8 4 4 0 0 0 0 8ZM5.6 5.6l3.6 3.6M14.8 14.8l3.6 3.6M18.4 5.6l-3.6 3.6M9.2 14.8l-3.6 3.6",
    download: "M12 4v11M7 10l5 5 5-5M5 20h14",
    drive: "M3 13h18M5 13l2-7h10l2 7v5a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1v-5ZM8 16h.01M12 16h4",
    server: "M4 4h16v6H4zM4 14h16v6H4zM8 7h.01M8 17h.01",
    vault: "M4 4h16v15H4zM14 14.5a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5ZM14 9.5V8M7 9v6M7 19v2M17 19v2",
    folder: "M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7Z",
    file: "M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8l-5-5ZM14 3v5h5",
    link: "M10 14a4 4 0 0 0 5.66 0l3-3a4 4 0 0 0-5.66-5.66l-1 1M14 10a4 4 0 0 0-5.66 0l-3 3a4 4 0 0 0 5.66 5.66l1-1",
    play: "M7 4v16l13-8L7 4Z",
    x: "M6 6l12 12M18 6 6 18",
    check: "M5 12.5l4.5 4.5L19 7",
    shieldCheck: "M12 3l8 3v6c0 4.5-3.4 8.3-8 9-4.6-.7-8-4.5-8-9V6l8-3ZM8.5 12l2.5 2.5 4.5-5",
    // Flux Sentry: a shield with an eye in it (it keeps watch).
    sentry: "M12 3l8 3v6c0 4.5-3.4 8.3-8 9-4.6-.7-8-4.5-8-9V6l8-3ZM7.6 11.6c1.2-1.7 2.7-2.6 4.4-2.6s3.2.9 4.4 2.6c-1.2 1.7-2.7 2.6-4.4 2.6s-3.2-.9-4.4-2.6ZM12 11.6h.01",
    minimize: "M6 12h12",
    help: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18ZM9.6 9.4a2.5 2.5 0 1 1 3.3 2.4c-.5.2-.9.7-.9 1.3v.6M12 16.8h.01",
    maximize: "M6 6h12v12H6z",
    pause: "M9 5v14M15 5v14",
    alert: "M12 3 2 20h20L12 3ZM12 10v4M12 17h.01",
    info: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18ZM12 16v-5M12 8h.01",
    clock: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18ZM12 7v5l3 2",
    calendar: "M4 6h16v14H4zM4 10h16M8 3v4M16 3v4",
    refresh: "M20 11a8 8 0 0 0-14.6-4.5L4 8M4 4v4h4M4 13a8 8 0 0 0 14.6 4.5L20 16M20 20v-4h-4",
    chevronRight: "M9 6l6 6-6 6",
    chevronLeft: "M15 6l-6 6 6 6",
    chevronDown: "M6 9l6 6 6-6",
    search: "M11 19a8 8 0 1 0 0-16 8 8 0 0 0 0 16ZM21 21l-4.35-4.35",
    restore: "M3 12a9 9 0 1 0 3-6.7L3 8M3 3v5h5M12 8v4l3 2",
    lock: "M6 11h12v10H6zM8 11V8a4 4 0 0 1 8 0v3",
    sun: "M12 17a5 5 0 1 0 0-10 5 5 0 0 0 0 10ZM12 1v2M12 21v2M4.2 4.2l1.4 1.4M18.4 18.4l1.4 1.4M1 12h2M21 12h2M4.2 19.8l1.4-1.4M18.4 5.6l1.4-1.4",
    moon: "M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8Z",
    monitor: "M3 4h18v12H3zM8 20h8M12 16v4",
    history: "M3 12a9 9 0 1 0 9-9 9 9 0 0 0-6.4 2.6L3 8M3 3v5h5M12 7v5l4 2",
    shield: "M12 3 4 6v6c0 5 3.5 8 8 9 4.5-1 8-4 8-9V6l-8-3Z",
    layers: "M12 3 2 8l10 5 10-5-10-5ZM2 13l10 5 10-5M2 17.5l10 5 10-5",
    pin: "M12 17v5M9 3h6l-1 6 4 4H6l4-4-1-6Z",
    tag: "M3 12V4h8l10 10-8 8L3 12ZM7.5 7.5h.01",
    settings: "M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6ZM19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1Z",
    plus: "M12 5v14M5 12h14",
    more: "M5 12h.01M12 12h.01M19 12h.01",
    menu: "M4 7h16M4 12h16M4 17h16",
    key: "M7.5 19a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7ZM10 13.5l9-9M16 7.5l2.5 2.5M13.5 10l2 2",
    eye: "M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12ZM12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6Z",
    eyeOff: "M3 3l18 18M10.6 5.1Q11.3 5 12 5c6.5 0 10 7 10 7a17 17 0 0 1-3.2 4.2M6.6 6.6C3.9 8.4 2 12 2 12s3.5 7 10 7c1.8 0 3.4-.5 4.8-1.3M9.9 9.9a3 3 0 0 0 4.2 4.2",
    activity: "M3 12h4l3-8 4 16 3-8h4",
    sparkle: "M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8L12 3Z",
    stop: "M6 6h12v12H6z",
    power: "M12 2v10M18.4 6.6a9 9 0 1 1-12.77.04",
    diff: "M12 3v14M5 10h14M5 21h14",
    external: "M14 4h6v6M20 4l-9 9M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5",
    home: "M3 11l9-8 9 8M5 9.5V20h14V9.5",
    edit: "M4 20h4L19 9l-4-4L4 16v4ZM13.5 6.5l4 4",
    trash: "M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13",
    copy: "M9 9h11v11H9zM5 15H4V4h11v1",
    filter: "M3 5h18l-7 8v6l-4 2v-8L3 5Z",
    terminal: "M4 5h16v14H4zM8 10l3 2-3 2M13 15h3",
    gauge: "M12 21a9 9 0 1 1 9-9M12 12l4-4",
  } as const;

  export type IconName = keyof typeof paths;
</script>

<script lang="ts">
  let { name, size = 18, class: klass = "", label }: { name: IconName; size?: number; class?: string; label?: string } = $props();
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="1.75"
  stroke-linecap="round"
  stroke-linejoin="round"
  class={klass}
  role={label ? "img" : undefined}
  aria-label={label}
  aria-hidden={label ? undefined : "true"}
>
  <path d={paths[name]} />
</svg>
