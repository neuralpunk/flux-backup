<script lang="ts">
  // A file manager's *Leave out of backups* and *Back up this folder*: what would change
  // in which backup, said before anything is saved. Leaving out: every backup that takes
  // it, ticked. Backing up: one backup to add it to, or a new backup.
  import { api } from "../api";
  import { bytes } from "../format";
  import { app } from "../state.svelte";
  import type { FolderChange, FolderPlan, FolderWant } from "../types";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";

  let { want, path, onclose }: { want: FolderWant; path: string; onclose: () => void } = $props();

  let plan = $state<FolderPlan | null>(null);
  let error = $state<string | null>(null);
  let busy = $state(false);
  /** Leaving out: the backups ticked. */
  let ticked = $state<string[]>([]);
  /** Backing up: the backup chosen, or "new". */
  let chosen = $state<string | null>(null);

  $effect(() => {
    api.folderPlan(path, want).then(
      (p) => {
        plan = p;
        ticked = p.backups.filter((b) => b.change).map((b) => b.id);
        chosen = p.backups.find((b) => b.change)?.id ?? "new";
      },
      (e) => (error = (e as Error).message),
    );
  });

  const thing = $derived(plan ? (plan.is_dir ? "folder" : "file") : want === "include" ? "folder" : "item");
  const size = $derived(plan?.size != null ? `${plan.partial ? "at least " : ""}${bytes(plan.size)}` : null);
  const takenBy = $derived(plan?.backups.filter((b) => b.backed_up) ?? []);
  const offered = $derived(plan?.backups.filter((b) => !b.backed_up) ?? []);

  /** The home folder, as the service spelled it (`/home/me` when `shown` is `~/x`). */
  const home = $derived(plan?.shown.startsWith("~") ? plan.path.slice(0, plan.path.length - (plan.shown.length - 1)) : null);
  /** `/home/me/Documents` as `~/Documents`, as the file manager's path is shown. */
  function short(p: string): string {
    return home && p.startsWith(home + "/") ? "~" + p.slice(home.length) : p;
  }

  /** The change, in words: before and after the rule (shown as code), if there's one. */
  function says(c: FolderChange): [string, string | null, string] {
    switch (c.kind) {
      case "add_rule":
        return ["Adds the rule ", c.rule, " to what it leaves out."];
      case "remove_folder":
        return ["Stops taking this folder (one of the folders it backs up).", null, ""];
      case "add_folder":
        return [c.replaces.length ? `Adds it to its folders, in place of ${c.replaces.map(short).join(", ")}, which it holds.` : "Adds it to the folders it backs up.", null, ""];
      case "remove_rule":
        return ["Removes the rule ", c.rule, ", which leaves it out."];
    }
  }

  // The dialog opens before its buttons exist: focus the main one when they do.
  let primary = $state<HTMLButtonElement | null>(null);
  $effect(() => {
    if (plan && primary) primary.focus();
  });

  function names(list: [string, string][]): string {
    const n = list.map(([, name]) => name);
    return n.length <= 2 ? n.join(" and ") : `${n.slice(0, -1).join(", ")} and ${n.at(-1)}`;
  }

  async function confirm() {
    if (!plan) return;
    if (want === "include" && chosen === "new") {
      app.newBackupWith(plan.path);
      onclose();
      return;
    }
    const ids = want === "exclude" ? ticked : chosen ? [chosen] : [];
    if (!ids.length) return;
    busy = true;
    try {
      const r = await api.applyFolderChange(plan.path, want, ids);
      if (r.changed.length) {
        app.toast("ok", want === "exclude" ? `${names(r.changed)} will leave out ${plan.shown} from the next backup on. Snapshots already made keep it.` : `${names(r.changed)} will back up ${plan.shown} from the next backup on.`);
      }
      for (const [name, why] of r.failed) app.toast("bad", `${name}: ${why}`);
      void app.refresh();
      onclose();
    } catch (e) {
      error = (e as Error).message;
    } finally {
      busy = false;
    }
  }
</script>

{#snippet change(c: FolderChange)}
  {@const [before, rule, after] = says(c)}
  {before}{#if rule}<code class="rounded bg-surface-3 px-1 font-mono text-[12px] text-fg-2">{rule}</code>{/if}{after}
{/snippet}

<Modal title={want === "exclude" ? "Leave out of backups" : "Back up this folder"} {onclose} width="max-w-lg">
  {#if error}
    <p class="flex items-start gap-2 text-sm text-danger"><Icon name="alert" size={16} class="mt-0.5 shrink-0" /> {error}</p>
    <div class="mt-5 flex justify-end"><button class="btn btn-ghost" onclick={onclose}>Close</button></div>
  {:else if !plan}
    <p class="text-sm text-fg-muted" role="status">Looking at your backups…</p>
  {:else if want === "exclude"}
    {#if !plan.backups.length}
      <p class="text-sm text-fg-2">None of your backups takes <span class="font-medium break-all text-fg">{plan.shown}</span>, so there's nothing to leave out.</p>
      <div class="mt-5 flex justify-end"><button class="btn btn-primary" onclick={onclose}>Close</button></div>
    {:else}
      <p class="-mt-1 mb-4 text-[13.5px] text-fg-2">
        The next backups won't copy this {thing},{" "}<span class="font-medium break-all text-fg">{plan.shown}</span>{#if size}&nbsp;({size}){/if}. Snapshots already made keep it until they're deleted.
      </p>
      <fieldset class="rows">
        <legend class="sr-only">Backups that take it</legend>
        {#each plan.backups as b (b.id)}
          <label class="row items-start gap-3 py-3 {b.change ? '' : 'opacity-70'}">
            <input
              type="checkbox"
              class="mt-1 size-4 shrink-0 accent-[var(--c-primary)]"
              disabled={!b.change}
              checked={ticked.includes(b.id)}
              onchange={(e) => (ticked = e.currentTarget.checked ? [...ticked, b.id] : ticked.filter((x) => x !== b.id))}
            />
            <span class="min-w-0 flex-1">
              <span class="block font-medium">{b.name}</span>
              <span class="block text-[12.5px] break-words text-fg-muted">{#if b.change}{@render change(b.change)}{:else}{b.blocked}{/if}</span>
            </span>
          </label>
        {/each}
      </fieldset>
      <div class="mt-5 flex justify-end gap-2">
        <button class="btn btn-ghost" onclick={onclose}>Cancel</button>
        <button bind:this={primary} class="btn btn-primary" disabled={busy || !ticked.length} onclick={confirm}>{busy ? "Leaving out…" : "Leave out"}</button>
      </div>
    {/if}
  {:else}
    {#if takenBy.length}
      <p class="-mt-1 mb-3 flex items-start gap-2 text-[13.5px] text-fg-2">
        <Icon name="check" size={16} class="mt-0.5 shrink-0 text-success" />
        <span><span class="font-medium break-all text-fg">{plan.shown}</span> is already backed up by {names(takenBy.map((b) => [b.id, b.name]))}.</span>
      </p>
    {/if}
    {#if !takenBy.length || offered.length}
      <p class="-mt-1 mb-4 text-[13.5px] text-fg-2">
        {#if takenBy.length}It can go into another backup too:{:else}Which backup should take <span class="font-medium break-all text-fg">{plan.shown}</span>{#if size}&nbsp;({size}){/if}?{/if}
      </p>
      <div class="rows" role="radiogroup" aria-label="Backup to add it to">
        {#each offered as b (b.id)}
          <button role="radio" aria-checked={chosen === b.id} class="row row-link items-start gap-3 py-3 text-left {b.change ? '' : 'opacity-70'}" disabled={!b.change} onclick={() => (chosen = b.id)}>
            <span class="min-w-0 flex-1">
              <span class="block font-medium">{b.name}</span>
              <span class="block text-[12.5px] break-words text-fg-muted">{#if b.change}{@render change(b.change)}{:else}{b.why} {b.blocked ?? ""}{/if}</span>
            </span>
            {#if chosen === b.id}<Icon name="check" size={18} class="mt-0.5" />{/if}
          </button>
        {/each}
        <button role="radio" aria-checked={chosen === "new"} class="row row-link items-start gap-3 py-3 text-left" onclick={() => (chosen = "new")}>
          <span class="min-w-0 flex-1">
            <span class="block font-medium">A new backup</span>
            <span class="block text-[12.5px] text-fg-muted">Set one up for this folder: where it goes, when, and whether to encrypt it.</span>
          </span>
          {#if chosen === "new"}<Icon name="check" size={18} class="mt-0.5" />{/if}
        </button>
      </div>
      <div class="mt-5 flex justify-end gap-2">
        <button class="btn btn-ghost" onclick={onclose}>Cancel</button>
        <button bind:this={primary} class="btn btn-primary" disabled={busy || !chosen} onclick={confirm}>{chosen === "new" ? "Set up…" : busy ? "Adding…" : "Back up"}</button>
      </div>
    {:else}
      <div class="mt-5 flex justify-end"><button class="btn btn-primary" onclick={onclose}>Close</button></div>
    {/if}
  {/if}
</Modal>
