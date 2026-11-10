<script lang="ts">
  // A time of day that's easy to type: "9", "930", "9:30 pm" and "21:30" all work, the
  // arrow keys step it (15 minutes; Shift: an hour), and the clock button lists times to
  // pick from. Shown in the clock chosen in Preferences; `value` is always "HH:MM".
  import { clockTime } from "../../clock.svelte";
  import { fromHHMM, parseTime, toHHMM } from "../../timeparse";
  import Icon from "../Icon.svelte";

  let {
    value = $bindable("00:00"),
    onchange,
    label = "Time",
    id,
    compact = false,
    placeholder = "",
  }: {
    value: string;
    onchange?: (v: string) => void;
    label?: string;
    id?: string;
    /** Smaller, for rows of several (the upload-limit windows). */
    compact?: boolean;
    placeholder?: string;
  } = $props();

  const minutes = $derived(fromHHMM(value));
  /** What's in the box while it's being typed in; `null`: the value, shown. */
  let typing = $state<string | null>(null);
  const shown = $derived(typing ?? (minutes == null ? value : clockTime(minutes)));
  const invalid = $derived(typing != null && typing.trim() !== "" && parseTime(typing) == null);

  let input: HTMLInputElement;
  let list = $state<HTMLDivElement>();
  let open = $state(false);
  let at = $state({ left: 0, top: 0, width: 0 });

  function set(t: number) {
    const v = toHHMM(t);
    typing = null;
    if (v === value) return;
    value = v;
    onchange?.(v);
  }

  /** Take what was typed; something unreadable goes back to the time before. */
  function commit() {
    if (typing == null) return;
    const t = parseTime(typing);
    if (t == null) typing = null;
    else set(t);
  }

  /** Move by `by` minutes, to whole steps first: from 9:07, up is 9:15 and down 9:00. */
  function step(by: number) {
    const q = Math.abs(by);
    const base = (typing != null ? parseTime(typing) : null) ?? minutes ?? 0;
    set(by > 0 ? Math.floor(base / q) * q + q : Math.ceil(base / q) * q - q);
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Enter") {
      commit();
      open = false;
      return;
    }
    if (e.key === "Escape" && typing != null) {
      typing = null;
      e.stopPropagation(); // don't close the dialog around it as well
      return;
    }
    const quarter = e.shiftKey ? 60 : 15;
    const by = { ArrowUp: quarter, ArrowDown: -quarter, PageUp: 60, PageDown: -60 }[e.key];
    if (!by) return;
    e.preventDefault();
    step(by);
  }

  /** Every half hour, and the current time if it's between them. */
  const choices = $derived.by(() => {
    const out = Array.from({ length: 48 }, (_, i) => i * 30);
    if (minutes != null && minutes % 30 !== 0) out.splice(Math.ceil(minutes / 30), 0, minutes);
    return out;
  });

  /** When the list last closed by itself (a click outside, which may be on the button). */
  let closedAt = 0;
  function toggle() {
    if (open || Date.now() - closedAt < 250) {
      open = false;
      return;
    }
    commit();
    const r = input.getBoundingClientRect();
    const below = window.innerHeight - r.bottom;
    const height = Math.min(264, window.innerHeight - 16);
    at = { left: r.left, top: below >= height + 8 ? r.bottom + 4 : Math.max(8, r.top - height - 4), width: r.width + 32 };
    open = true;
  }

  // Opened: show the chosen time in the middle of the list.
  $effect(() => {
    if (!open || !list) return;
    list.showPopover?.();
    list.querySelector<HTMLElement>("[aria-selected=true]")?.scrollIntoView({ block: "center" });
  });

  function pick(t: number) {
    set(t);
    open = false;
    input.focus();
  }
</script>

<span class="relative inline-flex shrink-0 items-center {compact ? 'w-28' : 'w-36'}">
  <input
    bind:this={input}
    {id}
    class="input num pr-8 {compact ? 'h-8 py-0' : ''} {invalid ? 'outline-2 outline-danger' : ''}"
    type="text"
    inputmode="text"
    autocomplete="off"
    spellcheck="false"
    aria-label={label}
    aria-invalid={invalid}
    title={invalid ? "Not a time Flux can read: try 9:30, 930, 9:30 pm or 21:30" : "Type a time (9:30, 930, 9:30 pm, 21:30); ↑ ↓ change it by 15 minutes"}
    {placeholder}
    value={shown}
    onfocus={(e) => e.currentTarget.select()}
    oninput={(e) => (typing = e.currentTarget.value)}
    onblur={commit}
    {onkeydown}
  />
  <button
    type="button"
    class="absolute right-1 grid size-7 place-items-center rounded-md text-fg-muted hover:bg-surface-2 hover:text-fg"
    aria-label="Choose a time"
    aria-expanded={open}
    tabindex="-1"
    onclick={toggle}
  >
    <Icon name="clock" size={14} />
  </button>
</span>

{#if open}
  <div
    bind:this={list}
    popover="auto"
    ontoggle={(e) => {
      if ((e as ToggleEvent).newState === "closed") (open = false), (closedAt = Date.now());
    }}
    role="listbox"
    aria-label={label}
    class="m-0 max-h-[264px] border-0 overflow-y-auto rounded-lg bg-surface-1 p-1 text-[13.5px] text-fg shadow-[0_12px_32px_-8px_rgb(0_0_0/0.45),0_0_0_1px_var(--c-group-edge)]"
    style="position: fixed; inset: auto; left: {at.left}px; top: {at.top}px; width: {Math.max(at.width, 132)}px"
  >
    {#each choices as t (t)}
      <button
        type="button"
        role="option"
        aria-selected={t === minutes}
        class="num block w-full rounded-md px-3 py-1.5 text-left hover:bg-surface-3 {t === minutes ? 'bg-primary/15 font-medium text-primary' : ''}"
        onclick={() => pick(t)}>{clockTime(t)}</button
      >
    {/each}
  </div>
{/if}
