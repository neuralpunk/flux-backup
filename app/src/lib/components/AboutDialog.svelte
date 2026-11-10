<script lang="ts">
  import { api, inTauri } from "../api";
  import { app } from "../state.svelte";
  import { copyText } from "../clipboard";
  import type { About, LinkName } from "../types";
  import Icon, { type IconName } from "./Icon.svelte";
  import Modal from "./Modal.svelte";
  import Logo from "../ui/Logo.svelte";

  let { onclose }: { onclose: () => void } = $props();

  // The version is known at build time, so it shows at once; the rest comes from the
  // backend (which also runs the installed `flux` command and helper).
  let about = $state<About | null>(null);
  let failed = $state(false);
  $effect(() => {
    api.about().then(
      (a) => (about = a),
      () => (failed = true),
    );
  });

  const links: { which: LinkName; label: string; icon: IconName }[] = [
    { which: "guide", label: "User guide", icon: "lifebuoy" },
    { which: "changes", label: "What's new", icon: "sparkle" },
    { which: "issues", label: "Report a problem", icon: "alert" },
    { which: "website", label: "Website", icon: "link" },
  ];

  function open(which: LinkName) {
    if (!inTauri) {
      app.toast("info", "Links open in your web browser in the app.");
      return;
    }
    api.openLink(which).catch((e: Error) => app.toast("bad", e.message));
  }

  const shown = (v: string | null | undefined) => v ?? "couldn't be run";

  function versionText(a: About | null): string {
    const lines = [`Flux Backup ${a?.version ?? __APP_VERSION__}`];
    if (a) {
      lines.push(`flux command: ${shown(a.cli.version)} (${a.cli.path})`);
      lines.push(`helper: ${a.helper ? `${shown(a.helper.version)} (${a.helper.path})` : "not installed"}`);
      lines.push(`System: ${a.os}, kernel ${a.kernel}`);
      if (a.webkit) lines.push(`WebKitGTK: ${a.webkit}`);
    }
    return lines.join("\n");
  }

  async function copy() {
    try {
      await copyText(versionText(about));
      app.toast("ok", "Version info copied.");
    } catch (e) {
      app.toast("bad", (e as Error).message);
    }
  }
</script>

<Modal title="About Flux Backup" {onclose} width="max-w-md">
  <div class="flex flex-col items-center text-center">
    <Logo class="size-16" />
    <p class="mt-3 text-[20px] font-semibold">Flux Backup</p>
    <p class="mt-0.5 num text-[14px] text-fg-2">Version {about?.version ?? __APP_VERSION__}</p>
    <p class="mt-2 text-[13px] text-fg-muted">Snapshot backups for Linux.</p>
  </div>

  {#if about?.mismatch}
    <div class="mt-5 flex gap-3 rounded-card bg-warning/12 px-4 py-3 text-[13px]" role="status">
      <Icon name="alert" size={16} class="mt-0.5 shrink-0 text-warning" />
      <p class="text-fg-2">
        <span class="font-medium text-fg">Parts of Flux Backup are different versions.</span>
        Scheduled backups use the <code class="font-mono">flux</code> command
        {#if about.cli.version && about.cli.version !== about.version}(version {about.cli.version}){/if}{#if about.helper?.version && about.helper.version !== about.version}{" "}and system backups the helper (version {about.helper.version}){/if}.
        Installing Flux Backup again from one package brings them back in line.
      </p>
    </div>
  {/if}

  <div class="rows mt-5">
    {#each links as l (l.which)}
      <button class="row row-link gap-3 text-left" onclick={() => open(l.which)}>
        <Icon name={l.icon} size={17} class="shrink-0 text-fg-2" />
        <span class="min-w-0 flex-1 font-medium">{l.label}</span>
        <Icon name="external" size={14} class="shrink-0 text-fg-muted" />
      </button>
    {/each}
  </div>

  <div class="mt-4">
    <details class="group text-[13px]">
      <summary class="flex cursor-pointer list-none items-center gap-2 px-0.5 font-medium text-fg-2 select-none hover:text-fg [&::-webkit-details-marker]:hidden">
        <Icon name="chevronRight" size={14} class="transition-transform group-open:rotate-90" />
        Technical details
      </summary>
      {#if about}
        <dl class="mt-3 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-left">
          <dt class="text-fg-muted">flux command</dt>
          <dd class="min-w-0"><span class="num">{shown(about.cli.version)}</span> <span class="block truncate font-mono text-[12px] text-fg-muted" title={about.cli.path}>{about.cli.path}</span></dd>
          <dt class="text-fg-muted">Helper</dt>
          <dd class="min-w-0">
            {#if about.helper}
              <span class="num">{shown(about.helper.version)}</span> <span class="block truncate font-mono text-[12px] text-fg-muted" title={about.helper.path}>{about.helper.path}</span>
            {:else}
              Not installed (system backups aren't available)
            {/if}
          </dd>
          <dt class="text-fg-muted">System</dt>
          <dd class="min-w-0">{about.os}<span class="block font-mono text-[12px] text-fg-muted">{about.kernel}</span></dd>
          {#if about.webkit}
            <dt class="text-fg-muted">WebKitGTK</dt>
            <dd class="num">{about.webkit}</dd>
          {/if}
        </dl>
      {:else if failed}
        <p class="mt-3 text-fg-muted">The details couldn't be read.</p>
      {:else}
        <div class="mt-3 h-20 rounded-card bg-surface-2 motion-safe:animate-pulse"></div>
      {/if}
    </details>
  </div>

  <div class="mt-5 flex items-center justify-between gap-3">
    <p class="text-left text-[12px] leading-snug text-fg-muted">MIT or Apache-2.0.<br />Made with care for people coming from Back In Time.</p>
    <button class="btn shrink-0" onclick={() => void copy()}><Icon name="copy" size={15} /> Copy version info</button>
  </div>
</Modal>
