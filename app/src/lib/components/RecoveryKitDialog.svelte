<script lang="ts">
  // The recovery kit of a compatibility-mode profile: the guide Flux keeps next to the
  // backups (FLUX-RECOVERY.txt) and the backup script (flux-backup.sh), readable here,
  // copyable, and saveable somewhere else.
  import { api, pickFolder } from "../api";
  import { app } from "../state.svelte";
  import type { RecoveryKit } from "../types";
  import Icon from "./Icon.svelte";
  import Callout from "./Callout.svelte";
  import Modal from "./Modal.svelte";
  import { copyText } from "../clipboard";

  let { id, name, onclose }: { id: string; name: string; onclose: () => void } = $props();

  let kit = $state<RecoveryKit | null>(null);
  let error = $state<string | null>(null);
  let tab = $state<"guide" | "script">("guide");
  let saving = $state(false);
  let copied = $state<string | null>(null);

  $effect(() => {
    api.recoveryKit(id).then(
      (k) => (kit = k),
      (e) => (error = (e as Error).message),
    );
  });

  const tool = $derived(kit?.engine === "borg" ? "Borg" : "rsync");

  interface Section {
    key: string;
    title: string;
    body: string;
  }
  /** "RESTORE ONE FILE" → "Restore one file" (keeping tool names as they're written). */
  function prettify(h: string): string {
    const s = h.toLowerCase().replace(/\bborg\b/g, "Borg").replace(/\bflux\b/g, "Flux");
    return s.charAt(0).toUpperCase() + s.slice(1);
  }
  /** The guide's sections: a heading line underlined with dashes, then its text. */
  const sections = $derived.by<{ intro: string; list: Section[] }>(() => {
    if (!kit) return { intro: "", list: [] };
    const lines = kit.recovery.split("\n");
    const list: Section[] = [];
    const intro: string[] = [];
    let cur: { title: string; lines: string[] } | null = null;
    for (let i = 0; i < lines.length; i++) {
      const line = lines[i]!;
      const next = lines[i + 1] ?? "";
      if (/^[=]{3,}$/.test(line) || (i === 0 && /^FLUX BACKUP/.test(line))) continue;
      if (line.trim() && /^-{3,}$/.test(next) && line === line.toUpperCase()) {
        if (cur) list.push({ key: `s${list.length}`, title: prettify(cur.title), body: cur.lines.join("\n").replace(/\s+$/, "") });
        cur = { title: line.trim(), lines: [] };
        i++;
        continue;
      }
      (cur ? cur.lines : intro).push(line);
    }
    if (cur) list.push({ key: `s${list.length}`, title: prettify(cur.title), body: cur.lines.join("\n").replace(/\s+$/, "") });
    return { intro: intro.join("\n").trim(), list };
  });

  async function copy(text: string, what: string) {
    await copyText(text);
    copied = what;
    setTimeout(() => copied === what && (copied = null), 1500);
  }

  async function save() {
    const dir = await pickFolder("Save the recovery kit into…");
    if (!dir) return;
    saving = true;
    try {
      const files = await api.saveRecoveryKit(id, dir);
      app.toast("ok", `Saved ${files.length} files in ${dir}.`);
    } catch (e) {
      app.toast("bad", (e as Error).message);
    } finally {
      saving = false;
    }
  }

  let scroller: HTMLElement | undefined = $state();
  function jump(key: string) {
    scroller?.querySelector(`#kit-${key}`)?.scrollIntoView({ behavior: "smooth", block: "start" });
  }
</script>

<Modal title="Recovery kit for “{name}”" {onclose} width="max-w-3xl">
  {#if error}
    <Callout tone="bad">{error}</Callout>
  {:else if !kit}
    <div class="h-72 rounded-card bg-surface-2 motion-safe:animate-pulse"></div>
  {:else}
    <div class="flex flex-col gap-4">
      <div class="flex gap-4">
        <div class="grid size-10 shrink-0 place-items-center rounded-lg bg-surface-3 text-fg-2"><Icon name="lifebuoy" /></div>
        <div class="min-w-0 text-sm text-fg-2">
          <p>
            These backups are written by <span class="font-medium text-fg">{tool}</span> and don't need Flux. This guide is kept next to them as
            <code class="font-mono text-[12.5px] whitespace-nowrap text-fg">FLUX-RECOVERY.txt</code>, with a script that keeps backing up with {tool} alone. Keep a copy somewhere else too:
            print it, or save it to a USB stick.
          </p>
          <div class="mt-2.5 flex flex-wrap gap-2">
            <span class="tag max-w-full font-mono" title="Where the backups and the kit are"><span class="truncate">{kit.location}</span></span>
            {#if kit.tool.supported}
              <span class="tag num text-success"><Icon name="check" size={12} /> {kit.tool.tool} {kit.tool.version} on this computer</span>
            {:else}
              <span class="tag text-warning" title={kit.tool.hint ?? ""}><Icon name="alert" size={12} /> {kit.tool.tool} isn't installed here</span>
            {/if}
          </div>
        </div>
      </div>

      <div class="segmented self-start" role="tablist" aria-label="Recovery kit">
        {#each [{ t: "guide", label: "Guide" }, { t: "script", label: "Backup script" }] as x (x.t)}
          <button
            role="tab"
            aria-selected={tab === x.t}
            onclick={() => (tab = x.t as "guide" | "script")}>{x.label}</button
          >
        {/each}
      </div>

      {#if tab === "guide"}
        <div class="flex flex-wrap gap-1.5" aria-label="Sections">
          {#each sections.list as s (s.key)}
            <button class="rounded-md bg-surface-3 px-2.5 py-1 text-[12.5px] text-fg-2 transition-colors hover:bg-track hover:text-fg" onclick={() => jump(s.key)}>{s.title}</button>
          {/each}
        </div>
        <div bind:this={scroller} class="flex max-h-[max(12rem,calc(100dvh-31rem))] flex-col gap-2 overflow-y-auto pr-1">
          {#if sections.intro}
            <pre class="rounded-card bg-surface-3 px-4 py-3 font-mono text-[12.5px] leading-relaxed whitespace-pre-wrap text-fg-2">{sections.intro}</pre>
          {/if}
          {#each sections.list as s (s.key)}
            <section id="kit-{s.key}" class="rounded-card bg-surface-2 px-4 py-3">
              <div class="flex items-center justify-between gap-3">
                <h3 class="text-[14px] font-medium">{s.title}</h3>
                <button class="btn btn-ghost h-7 px-2.5 text-[12.5px]" onclick={() => copy(s.body.replace(/^ {2}/gm, ""), s.key)} aria-label="Copy “{s.title}”">
                  <Icon name={copied === s.key ? "check" : "copy"} size={13} />
                  {copied === s.key ? "Copied" : "Copy"}
                </button>
              </div>
              <pre class="mt-1.5 overflow-x-auto font-mono text-[12.5px] leading-relaxed text-fg-2">{s.body.replace(/^ {2}/gm, "")}</pre>
            </section>
          {/each}
        </div>
      {:else}
        <p class="text-[13px] text-fg-muted">
          <code class="font-mono">flux-backup.sh</code> runs the same backup as Flux with {tool} alone. Run it with
          <code class="font-mono">sh flux-backup.sh</code>, or add <code class="font-mono">--dry-run</code> to only see what it would do.
        </p>
        <pre class="max-h-[max(12rem,calc(100dvh-28rem))] overflow-auto rounded-card bg-surface-3 px-4 py-3 font-mono text-[12.5px] leading-relaxed text-fg-2">{kit.script}</pre>
      {/if}

      <div class="flex flex-wrap items-center justify-end gap-2">
        <button class="btn btn-ghost" onclick={() => kit && copy(tab === "guide" ? kit.recovery : kit.script, "all")}>
          <Icon name={copied === "all" ? "check" : "copy"} size={14} />
          {copied === "all" ? "Copied" : tab === "guide" ? "Copy the guide" : "Copy the script"}
        </button>
        <button class="btn btn-primary" onclick={save} disabled={saving}><Icon name="download" size={15} /> {saving ? "Saving…" : "Save a copy…"}</button>
      </div>
    </div>
  {/if}
</Modal>
