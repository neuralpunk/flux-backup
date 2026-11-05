<script lang="ts">
  // The "Stack" mark: three snapshots, the newest lit. Always on its dark tile (the tile is
  // part of the icon), with the same hex values as the app icon.
  let { class: klass = "size-7" }: { class?: string } = $props();
</script>

<svg viewBox="0 0 256 256" class={klass} aria-hidden="true">
  <rect width="256" height="256" rx="58" fill="#1b1d21" />
  <rect x="56" y="68" width="144" height="32" rx="16" fill="#7abdff" />
  <rect x="56" y="112" width="144" height="32" rx="16" fill="#32618e" />
  <rect x="56" y="156" width="144" height="32" rx="16" fill="#193550" />
</svg>
