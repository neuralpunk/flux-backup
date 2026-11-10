<script lang="ts">
  // Webhooks: where to tell how backups go, off this computer. Used in Preferences (for
  // every backup) and in a backup's settings (its own). `onchange` gets the new list.
  import { api, inTauri } from "../api";
  import { app } from "../state.svelte";
  import type { Webhook, WebhookEvent, WebhookPreset } from "../types";
  import Icon from "./Icon.svelte";

  let { hooks, onchange }: { hooks: Webhook[]; onchange: (hooks: Webhook[]) => void } = $props();

  const presets: { value: WebhookPreset; label: string; hint: string; placeholder: string }[] = [
    { value: "ntfy", label: "ntfy", hint: "Notifications on your phone: the address of your ntfy topic.", placeholder: "https://ntfy.sh/my-backups-x7k2" },
    { value: "healthchecks", label: "Health checks (pings)", hint: "healthchecks.io and similar: pings when a backup starts, succeeds or fails, so a backup that stops happening raises an alarm there.", placeholder: "https://hc-ping.com/your-check-uuid" },
    { value: "gotify", label: "Gotify", hint: "Your Gotify server's message address, with the app token.", placeholder: "https://gotify.example.org/message?token=…" },
    { value: "slack", label: "Slack", hint: "An incoming webhook of a Slack channel.", placeholder: "https://hooks.slack.com/services/…" },
    { value: "discord", label: "Discord", hint: "A Discord channel's webhook.", placeholder: "https://discord.com/api/webhooks/…" },
    { value: "json", label: "Anything else (JSON)", hint: "Receives a JSON message for each event: event, profile, name, host, message, time, status, snapshot, error.", placeholder: "https://example.org/hooks/backup" },
  ];
  const events: { value: WebhookEvent; label: string }[] = [
    { value: "failed", label: "Failed" },
    { value: "warning", label: "Finished with problems" },
    { value: "unusual_changes", label: "Unusual changes" },
    { value: "restore_test_failed", label: "Restore test failed" },
    { value: "low_space", label: "Little space left" },
    { value: "skipped", label: "Skipped" },
    { value: "succeeded", label: "Succeeded" },
    { value: "started", label: "Started" },
  ];
  const defaults: WebhookEvent[] = ["failed", "warning", "unusual_changes", "restore_test_failed"];
  const labelOf = (p: WebhookPreset | undefined) => presets.find((x) => x.value === (p ?? "json"))?.label ?? "JSON";

  let adding = $state(false);
  let draft = $state<Webhook>({ url: "", preset: "ntfy", events: [...defaults], headers: [] });
  let headersText = $state("");
  let testing = $state<number | null>(null);

  function startAdding() {
    draft = { url: "", preset: "ntfy", events: [...defaults], headers: [] };
    headersText = "";
    adding = true;
  }
  function current(): Webhook {
    return { ...draft, headers: headersText.split("\n").map((h) => h.trim()).filter(Boolean) };
  }
  async function test(w: Webhook, i: number) {
    testing = i;
    try {
      if (!inTauri) {
        app.toast("info", "In the app, this sends a test message.");
        return;
      }
      const code = await api.testWebhook(w);
      app.toast("ok", `Sent: the server answered ${code}.`);
    } catch (e) {
      app.toast("bad", `The test didn't arrive: ${(e as Error).message}`);
    } finally {
      testing = null;
    }
  }
  function toggleEvent(e: WebhookEvent) {
    const have = draft.events ?? [];
    draft.events = have.includes(e) ? have.filter((x) => x !== e) : [...have, e];
  }
</script>

{#if hooks.length || !adding}
<div class="rows">
  {#each hooks as w, i (i)}
    <div class="row gap-3">
      <Icon name="external" size={17} class="shrink-0 text-fg-2" />
      <span class="min-w-0 flex-1">
        <span class="block font-medium">{labelOf(w.preset)}{#if w.enabled === false} <span class="text-fg-muted">(off)</span>{/if}</span>
        <span class="block truncate font-mono text-[12px] text-fg-muted" title={w.url}>{w.url}</span>
        <span class="block text-[12.5px] text-fg-2">{(w.events?.length ? w.events : defaults).map((e) => events.find((x) => x.value === e)?.label ?? e).join(", ")}</span>
      </span>
      <button class="btn shrink-0" disabled={testing === i} onclick={() => void test(w, i)}>{testing === i ? "Sending…" : "Send a test"}</button>
      <button class="btn-icon size-8 shrink-0" aria-label="Remove this webhook" title="Remove" onclick={() => onchange(hooks.filter((_, j) => j !== i))}><Icon name="trash" size={15} /></button>
    </div>
  {/each}
  {#if !adding}
    <button class="row row-link gap-3" onclick={startAdding}>
      <Icon name="plus" size={16} class="text-fg-2" /> <span class="font-medium">Add a webhook…</span>
    </button>
  {/if}
</div>
{/if}

{#if adding}
  {@const preset = presets.find((p) => p.value === draft.preset)!}
  <div class="card {hooks.length ? 'mt-3' : ''} flex flex-col gap-3 px-5 py-4">
    <label class="flex flex-col gap-1 text-[13px] font-medium">
      Kind
      <select class="input h-9 py-0" bind:value={draft.preset}>
        {#each presets as p (p.value)}<option value={p.value}>{p.label}</option>{/each}
      </select>
      <span class="font-normal text-fg-muted">{preset.hint}</span>
    </label>
    <label class="flex flex-col gap-1 text-[13px] font-medium">
      Address
      <input class="input font-mono text-[13px]" bind:value={draft.url} placeholder={preset.placeholder} />
    </label>
    <fieldset class="flex flex-col gap-1.5 text-[13px]">
      <legend class="mb-1 font-medium">Tell it when a backup…</legend>
      <div class="flex flex-wrap gap-1.5">
        {#each events as e (e.value)}
          {@const on = (draft.events ?? []).includes(e.value)}
          <button class="chip-toggle" aria-pressed={on} onclick={() => toggleEvent(e.value)}>{#if on}<Icon name="check" size={13} />{/if}{e.label}</button>
        {/each}
      </div>
      {#if draft.preset === "healthchecks"}<span class="text-fg-muted">Pings always include started, succeeded and failed.</span>{/if}
    </fieldset>
    <label class="flex flex-col gap-1 text-[13px] font-medium">
      Extra headers (optional, one per line)
      <textarea class="input h-14 py-1.5 font-mono text-[12.5px]" bind:value={headersText} placeholder="Authorization: Bearer …"></textarea>
      <span class="font-normal text-fg-muted">Kept in this computer's settings only: never exported or copied into diagnostics.</span>
    </label>
    <div class="flex items-center justify-end gap-2">
      <button class="btn btn-ghost" onclick={() => (adding = false)}>Cancel</button>
      <button class="btn" disabled={!draft.url.trim() || testing === -1} onclick={() => void test(current(), -1)}>{testing === -1 ? "Sending…" : "Send a test"}</button>
      <button
        class="btn btn-primary"
        disabled={!/^https?:\/\//.test(draft.url.trim())}
        onclick={() => {
          onchange([...hooks, current()]);
          adding = false;
        }}>Add</button
      >
    </div>
  </div>
{/if}
