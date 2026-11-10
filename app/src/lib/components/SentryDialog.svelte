<script lang="ts">
  // Flux Sentry's settings for one backup, on their own: whether it keeps watch, how
  // closely, and its tripwire files. (They also live in Settings › What to keep, among the
  // retention rules; this is the short way to them, from the Overview and the header.)
  import { untrack } from "svelte";
  import { api } from "../api";
  import { shareWords, type SentryKnows } from "../glance";
  import { app } from "../state.svelte";
  import { isSystem, type Profile } from "../types";
  import Callout from "./Callout.svelte";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";
  import Toggle from "./form/Toggle.svelte";

  let {
    id,
    knows = null,
    onclose,
  }: {
    id: string;
    /** What Flux Sentry has learned of this backup, when the page that opens this has it. */
    knows?: SentryKnows | null;
    onclose: () => void;
  } = $props();

  type Level = "normal" | "high" | "off";
  const levels: { v: Level; label: string; text: string }[] = [
    { v: "normal", label: "Normally", text: "Warns at about 6 times this backup's usual change, or at clear marks of ransomware: files renamed by the dozen, text that turns to noise, ransom notes." },
    { v: "high", label: "Closely", text: "Warns sooner: at about 3 times the usual change, and at fewer of those marks. For files that rarely change; a busy folder may raise a false alarm now and then." },
    { v: "off", label: "Off", text: "Flux Sentry doesn't watch this backup. Backups and snapshots are made just the same; you only aren't warned about unusual changes." },
  ];

  // (Raw: the profile is sent back as it came, with two fields changed. It isn't edited in
  // place, and a reactive proxy of it can't be handed to the backend as a plain object.)
  let profile = $state.raw<Profile | null>(null);
  let level = $state<Level>("normal");
  let tripwires = $state(false);
  let error = $state<string | null>(null);
  let saving = $state(false);

  $effect(() => {
    const want = id;
    api.getProfile(want).then(
      (e) => {
        if (want !== id) return;
        profile = e.profile;
        level = e.profile.unusual_changes ?? "normal";
        tripwires = e.profile.tripwires ?? false;
      },
      (e) => want === id && (error = (e as Error).message),
    );
  });

  // What Sentry has learned: given by the page that opened this, or asked for (from the
  // header, which doesn't have it). The sentence above the choices fills in when it comes.
  let learned = $state.raw<SentryKnows | null>(untrack(() => knows));
  $effect(() => {
    const want = id;
    if (untrack(() => learned)) return;
    api.overview(want).then(
      (o) => want === id && (learned = o.sentry ?? null),
      () => {},
    );
  });

  const sys = $derived(isSystem(id));
  const held = $derived(app.profiles.find((p) => p.id === id)?.held ?? false);
  const changed = $derived(!!profile && (level !== (profile.unusual_changes ?? "normal") || tripwires !== (profile.tripwires ?? false)));

  /** What Sentry is doing for this backup now, in a sentence (as saved, not as chosen here). */
  const status = $derived.by(() => {
    if (!profile) return "";
    const saved = profile.unusual_changes ?? "normal";
    if (saved === "off") return "Flux Sentry isn't watching this backup.";
    if (!learned) return `Flux Sentry is on watch${saved === "high" ? ", closely" : ""}. It learns what's normal for this backup over its first five backups.`;
    const warns = learned.warns_at == null ? "it would warn at the marks of ransomware (no amount of change alone stands out here)" : `it would warn at about ${shareWords(learned.warns_at)}, or at the marks of ransomware`;
    return `Flux Sentry knows this backup from ${learned.runs} backups: ${shareWords(learned.usual)} of its files usually change; ${warns}.`;
  });

  async function save() {
    if (!profile || saving) return;
    saving = true;
    error = null;
    try {
      const r = await api.saveProfile(id, { ...profile, unusual_changes: level, tripwires: level === "off" ? false : tripwires }, false);
      if (!r.saved) {
        error = r.issues.find((i) => i.severity === "error")?.message ?? "These settings couldn't be saved.";
        return;
      }
      app.toast("ok", level === "off" ? "Flux Sentry is off for this backup." : `Flux Sentry watches this backup${level === "high" ? " closely" : ""}${tripwires ? ", with tripwire files from the next backup on" : ""}.`);
      app.revision++;
      void app.refresh();
      onclose();
    } catch (e) {
      error = (e as Error).message;
    } finally {
      saving = false;
    }
  }
</script>

<Modal title="Flux Sentry" {onclose} locked={saving}>
  {#if !profile && !error}
    <div class="space-y-2" aria-hidden="true">
      {#each [0, 1, 2] as i (i)}<div class="h-14 animate-pulse rounded-card bg-surface-3"></div>{/each}
    </div>
  {:else}
    {#if profile}
      <p class="flex items-start gap-2 text-[13.5px] text-fg-2">
        <Icon name="sentry" size={16} class="mt-0.5 shrink-0 {(profile.unusual_changes ?? 'normal') === 'off' ? 'text-fg-muted' : 'text-success'}" />
        <span>{status}</span>
      </p>
      {#if held}
        <Callout class="mt-3" tone="warn" title="Backups are on hold">
          Flux Sentry found unusual changes in the last backup. Settle them on the Overview (<em>This was me</em>, or <em>It's dealt with</em>). Turning Flux Sentry off here also lets backups go on, without settling anything.
        </Callout>
      {/if}

      <h3 class="mt-4 mb-1.5 px-0.5 text-[12px] font-medium text-fg-muted" id="sentry-watch">How closely it watches</h3>
      <div class="rows" role="radiogroup" aria-labelledby="sentry-watch">
        {#each levels as l (l.v)}
          <button role="radio" aria-checked={level === l.v} class="row row-link items-start gap-3 py-3 text-left" onclick={() => (level = l.v)}>
            <span class="min-w-0 flex-1">
              <span class="flex items-center gap-2 font-medium">{l.label}{#if l.v === "normal"}<span class="tag tag-accent h-5 px-1.5 text-[11.5px]">Recommended</span>{/if}</span>
              <span class="block text-[12.5px] text-fg-muted">{l.text}</span>
            </span>
            {#if level === l.v}<Icon name="check" size={18} class="mt-0.5 shrink-0" />{/if}
          </button>
        {/each}
      </div>

      {#if level !== "off"}
        <div class="rows mt-3">
          {#if sys}
            <div class="row items-start gap-3 py-3">
              <span class="grid w-5 shrink-0 place-items-center text-fg-2"><Icon name="sentry" size={17} /></span>
              <span class="min-w-0 flex-1">
                <span class="block text-[14px] font-medium">Tripwire files</span>
                <span class="mt-0.5 block text-[12.5px] text-fg-muted">Not planted for system backups: Flux doesn't write into the system's folders.</span>
              </span>
            </div>
          {:else}
            <Toggle
              row
              icon="sentry"
              bind:checked={tripwires}
              label="Plant tripwire files"
              hint="A small hidden file (.flux-sentry-tripwire.txt) in each folder this backs up and its first subfolders. If one is changed or removed, Flux Sentry stops the backup at once. Planted at the next backup; removed again when this is turned off."
            />
          {/if}
        </div>
      {/if}
    {/if}

    {#if error}
      <Callout class="mt-3" tone="bad" title="Couldn't {profile ? 'save' : 'read'} this backup's settings">{error}</Callout>
    {/if}

    <div class="mt-5 flex justify-end gap-2">
      <button class="btn" onclick={onclose} disabled={saving}>{profile ? "Cancel" : "Close"}</button>
      {#if profile}
        <button class="btn btn-primary" onclick={save} disabled={!changed || saving}>{saving ? "Saving…" : "Save"}</button>
      {/if}
    </div>
  {/if}
</Modal>
