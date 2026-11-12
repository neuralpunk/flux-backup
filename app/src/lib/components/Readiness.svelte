<script lang="ts">
  // Could this backup be recovered on another computer? A short checklist, folded away:
  // what Flux can see (settings kept with the backups, a restore test passed, a copy kept
  // elsewhere), and what only you can tell (the password kept somewhere else). Each line
  // that isn't ready has a button that does it.
  import { api, pickFolder } from "../api";
  import { app } from "../state.svelte";
  import { ApiError, type Engine, type ReadinessCheck } from "../types";
  import Icon from "./Icon.svelte";
  import KeepPasswordDialog from "./KeepPasswordDialog.svelte";
  import DestinationDialog from "./DestinationDialog.svelte";
  import RecoveryCodeDialog from "./RecoveryCodeDialog.svelte";

  let { id, onlocked }: { id: string; onlocked?: (retry: () => void) => void } = $props();

  let checks = $state<ReadinessCheck[] | null>(null);
  $effect(() => {
    const want = id;
    void app.revision; // a backup or a restore test finished: look again
    api.readiness(want).then(
      (c) => want === id && (checks = c),
      () => (checks = []),
    );
  });
  const ready = $derived(checks?.filter((c) => c.ok).length ?? 0);
  const profile = $derived(app.profiles.find((p) => p.id === id));
  const backingUp = $derived(!!profile?.running || !!app.starting[id]);

  /** Which action is running (its check's key). */
  let busy = $state<string | null>(null);
  let keepingPassword = $state(false);
  let makingCode = $state(false);

  async function answer(c: ReadinessCheck, yes = !c.ok) {
    try {
      checks = await api.setReadiness(id, c.key, yes);
    } catch (e) {
      app.toast("bad", (e as Error).message);
    }
  }

  async function testNow() {
    busy = "restore_test";
    try {
      const r = await api.restoreTest(id);
      if (r.result === "failed") app.toast("bad", `Restore test failed: ${r.error ?? `${r.problems_total} files have problems`}. See Restore tests on the Overview.`);
      else app.toast("ok", `Restore test passed: ${r.tested} files read back and checked.`);
      app.revision++;
    } catch (e) {
      if (e instanceof ApiError && e.code === "password_required" && onlocked) onlocked(() => void testNow());
      else app.toast("bad", (e as Error).message);
    } finally {
      busy = null;
    }
  }

  async function exportKey() {
    const dir = await pickFolder("Where to keep the Borg key (with the passphrase, not next to the backups)");
    if (!dir) return;
    busy = "key_exported";
    try {
      const path = await api.exportBorgKey(id, dir);
      app.toast("ok", `The key is in ${path}. Keep it with the passphrase.`);
      checks = await api.readiness(id);
    } catch (e) {
      app.toast("bad", (e as Error).message);
    } finally {
      busy = null;
    }
  }

  /** Adding a place, in its dialog over this page: what it needs to know of the backup. */
  let adding = $state<{ taken: string[]; engine: Engine; remote: boolean } | null>(null);
  async function addPlace() {
    try {
      const p = (await api.getProfile(id)).profile;
      adding = { taken: (p.destinations ?? []).map((d) => d.name), engine: p.engine, remote: p.target.kind === "ssh" };
    } catch (e) {
      app.toast("bad", (e as Error).message);
    }
  }

  /** The button that does a check's job: its label and what it does. */
  function action(c: ReadinessCheck): { label: string; run: () => void; primary?: boolean } | null {
    switch (c.key) {
      case "settings":
        return c.ok ? null : { label: backingUp ? "Backing up…" : "Back up now", run: () => void app.backupNow(id), primary: true };
      case "password_elsewhere":
        return c.ok ? null : { label: "Keep it elsewhere…", run: () => (keepingPassword = true), primary: true };
      case "recovery_code":
        return c.ok ? null : { label: "Make one…", run: () => (makingCode = true), primary: true };
      case "key_exported":
        return c.ok ? null : { label: busy === "key_exported" ? "Exporting…" : "Export the key…", run: () => void exportKey(), primary: true };
      case "restore_test":
        return { label: busy === "restore_test" ? "Testing…" : c.ok ? "Test again" : "Test a restore now", run: () => void testNow(), primary: !c.ok };
      case "offsite":
        return c.ok ? null : { label: "Add a place…", run: () => void addPlace(), primary: true };
    }
    return null;
  }
</script>

{#if checks?.length}
  <details class="group">
    <summary class="flex cursor-pointer list-none items-center gap-2 rounded-md px-0.5 py-1 text-[13px] font-semibold text-fg-2 select-none hover:text-fg [&::-webkit-details-marker]:hidden">
      <Icon name="chevronRight" size={14} class="transition-transform group-open:rotate-90" />
      Could you recover it on another computer?
      <!-- Progress at a glance, folded or not: a segment per line, filled when ready. -->
      <span class="flex items-center gap-[3px]" aria-hidden="true">
        {#each checks as c (c.key)}<span class="h-1.5 w-4 rounded-full {c.ok ? 'bg-success' : 'bg-surface-3'}"></span>{/each}
      </span>
      <span class="font-normal {ready === checks.length ? 'text-success' : 'text-fg-muted'}">{ready} of {checks.length} ready</span>
    </summary>
    <div class="rows mt-2">
      {#each checks as c (c.key)}
        {@const a = action(c)}
        <div class="row items-start gap-3 py-3">
          <!-- Ready: a tick. Not yet: an empty ring, a thing to do (not a failure). -->
          {#if c.ok}
            <span class="mt-0.5 grid size-5 shrink-0 place-items-center rounded-full bg-success/15 text-success"><Icon name="check" size={12} /></span>
          {:else}
            <span class="mt-0.5 size-5 shrink-0 rounded-full border-[1.5px] border-dashed border-fg-muted/60" title="To do"></span>
          {/if}
          <span class="min-w-0 flex-1">
            <span class="block font-medium">{c.title}</span>
            <span class="block text-[12.5px] text-fg-muted">{c.detail}</span>
          </span>
          <span class="flex shrink-0 flex-wrap items-center justify-end gap-2 self-center">
            {#if a}
              <button
                class="btn h-8 {a.primary ? '' : 'btn-ghost text-fg-2'}"
                disabled={busy !== null || (c.key === "settings" && backingUp)}
                onclick={a.run}>{a.label}</button
              >
            {/if}
            {#if c.asked}
              <!-- Only you can tell: say so either way. -->
              <button class="btn btn-ghost h-8 {c.ok ? 'text-fg-muted' : 'text-fg-2'}" aria-pressed={c.ok} onclick={() => void answer(c)}>{c.ok ? "Not any more" : "I have"}</button>
            {/if}
          </span>
        </div>
      {/each}
    </div>
  </details>
{/if}

{#if keepingPassword}
  <KeepPasswordDialog
    {id}
    onclose={() => (keepingPassword = false)}
    ondone={() => {
      keepingPassword = false;
      const c = checks?.find((x) => x.key === "password_elsewhere");
      if (c) void answer(c, true);
    }}
  />
{/if}

{#if makingCode}
  <RecoveryCodeDialog {id} name={profile?.name ?? id} onclose={() => (makingCode = false)} ondone={() => app.revision++} />
{/if}

{#if adding}
  <DestinationDialog
    {id}
    taken={adding.taken}
    mainEngine={adding.engine}
    mainRemote={adding.remote}
    onadded={() => {
      adding = null;
      app.revision++;
      void app.refresh();
    }}
    onclose={() => (adding = null)}
  />
{/if}
