<script lang="ts">
  let {
    value = $bindable(),
    min = 0,
    max,
    id,
    label,
  }: { value: number; min?: number; max?: number; id?: string; label?: string } = $props();

  function set(raw: string) {
    let n = Math.floor(Number(raw));
    if (!Number.isFinite(n)) n = min;
    n = Math.max(min, n);
    if (max != null) n = Math.min(max, n);
    value = n;
  }
</script>

<input
  {id}
  class="input tabular w-24"
  type="number"
  inputmode="numeric"
  {min}
  {max}
  aria-label={label}
  {value}
  oninput={(e) => set(e.currentTarget.value)}
/>
