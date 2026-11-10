<script lang="ts">
  // How backups are stored: Flux's own formats, or a standard tool's (compatibility
  // mode). Written for someone who has never heard of rsync or Borg; the exact details
  // are one click away for someone who has.
  import { api } from "../api";
  import type { Engine, ToolStatus } from "../types";
  import Icon, { type IconName } from "./Icon.svelte";
  import InfoDetails from "./InfoDetails.svelte";

  let { engine, onchoose }: { engine: Engine; onchoose: (e: Engine) => void } = $props();

  let tools = $state<ToolStatus[] | null>(null);
  $effect(() => {
    api.tools().then(
      (t) => (tools = t),
      () => (tools = []),
    );
  });
  const tool = (t: Engine) => (t === "rsync" || t === "borg" ? (tools?.find((x) => x.tool === t) ?? null) : null);
  const usable = (e: Engine) => (e !== "rsync" && e !== "borg") || (tool(e)?.supported ?? false);

  const groups: { title: string; text: string; options: { e: Engine; icon: IconName; title: string; text: string; good: string[]; mind: string }[] }[] = [
    {
      title: "Flux's own formats",
      text: "The fastest, with every feature.",
      options: [
        {
          e: "mirror",
          icon: "folder",
          title: "Plain folders",
          text: "Each backup is an ordinary folder (Mirror).",
          good: ["Open your backups in any file manager", "Unchanged files take no extra space"],
          mind: "Not encrypted: use an encrypted drive for private files.",
        },
        {
          e: "vault",
          icon: "lock",
          title: "Encrypted",
          text: "Encrypted, compressed and deduplicated (Vault).",
          good: ["Nobody can read it without the password", "Smallest: data is stored once"],
          mind: "Needs Flux, or the small recovery tool kept on the drive, to read it.",
        },
      ],
    },
    {
      title: "Works without Flux",
      text: "A standard, free tool writes the backups in its own format; Flux is only the app you use. A recovery guide is kept next to them.",
      options: [
        {
          e: "rsync",
          icon: "folder",
          title: "rsync",
          text: "Plain folders written by rsync, the standard Linux copying tool.",
          good: ["Open your backups in any file manager", "Restore with cp or rsync"],
          mind: "Not encrypted or compressed.",
        },
        {
          e: "borg",
          icon: "vault",
          title: "Borg",
          text: "A standard Borg repository, written by Borg itself.",
          good: ["Encrypted with your passphrase", "Compressed; unchanged data stored once"],
          mind: "Getting files back needs borg and your passphrase.",
        },
      ],
    },
  ];
</script>

<div class="flex flex-col gap-6" role="radiogroup" aria-label="How backups are stored">
  {#each groups as g (g.title)}
    <section>
      <div class="px-0.5 pb-2">
        <h3 class="text-[13px] font-semibold text-fg-2">{g.title}</h3>
        <p class="text-[12.5px] text-fg-muted">{g.text}</p>
      </div>
      <div class="rows">
        {#each g.options as o (o.e)}
          {@const selected = engine === o.e}
          {@const t = tool(o.e)}
          {@const ok = usable(o.e)}
          <button
            role="radio"
            aria-checked={selected}
            disabled={!ok && !selected}
            class="row row-link items-start py-3.5 disabled:cursor-not-allowed"
            onclick={() => onchoose(o.e)}
          >
            <Icon name={o.icon} size={17} class="mt-0.5 shrink-0 text-fg-2" />
            <span class="flex min-w-0 flex-1 flex-col gap-1">
              <span class="font-semibold">{o.title}</span>
              <span class="text-[13px] text-fg-2">{o.text}</span>
              <span class="mt-0.5 flex flex-col gap-0.5 text-[12.5px]">
                {#each o.good as gd (gd)}<span class="flex items-start gap-1.5 text-fg-2"><Icon name="check" size={12} class="mt-[3px] shrink-0 text-success" /> {gd}</span>{/each}
                <span class="flex items-start gap-1.5 text-fg-muted"><Icon name="alert" size={12} class="mt-[3px] shrink-0" /> {o.mind}</span>
              </span>
              {#if o.e === "rsync" || o.e === "borg"}
                {#if !tools}
                  <span class="tag mt-1 self-start">checking…</span>
                {:else if t?.supported}
                  <span class="tag tag-accent num mt-1 self-start"><Icon name="check" size={12} /> {t.tool} {t.version} installed</span>
                {:else}
                  <span class="mt-1 text-[12.5px] text-warning">{t?.hint ?? "Not installed."}</span>
                {/if}
              {/if}
            </span>
            <span class="grid size-5 shrink-0 place-items-center">
              {#if selected}<Icon name="check" size={18} class="text-fg" />{/if}
            </span>
          </button>
        {/each}
      </div>
    </section>
  {/each}

  {#if engine === "rsync" || engine === "borg"}
    <InfoDetails summary="What exactly Flux does (for {engine === 'borg' ? 'Borg' : 'rsync'} users)">
      {#if engine === "rsync"}
        <ul class="flex list-disc flex-col gap-2 pl-4 marker:text-primary/60">
          <li>Each backup is one run of <code class="font-mono text-fg">rsync --archive --hard-links --numeric-ids --relative --delete --link-dest=../../&lt;previous&gt;/data</code> into <code class="font-mono text-fg">snapshots/.partial</code>, renamed to the date and time when it finishes.</li>
          <li>Unchanged files are hard links to the previous snapshot; every snapshot is complete. The exact command and filter rules are saved in each snapshot.</li>
          <li>Your exclusions become rsync filter rules with the same meaning; folders with a <code class="font-mono text-fg">CACHEDIR.TAG</code> are found first and listed as rules.</li>
          <li>Next to the snapshots: <code class="font-mono text-fg">FLUX-RECOVERY.txt</code> and <code class="font-mono text-fg">flux-backup.sh</code>, which does the same backup with rsync alone.</li>
        </ul>
      {:else}
        <ul class="flex list-disc flex-col gap-2 pl-4 marker:text-primary/60">
          <li>A Borg 1.x repository: <code class="font-mono text-fg">borg init --encryption=repokey-blake2</code>, one archive per backup named after its date and time.</li>
          <li><code class="font-mono text-fg">borg create --compression=zstd,3 --exclude-caches --patterns-from=flux-patterns.txt</code>; your exclusions become Borg patterns.</li>
          <li>Names and pins you give snapshots are stored in the archive comment, inside the repository. Retention uses <code class="font-mono text-fg">borg delete</code> and <code class="font-mono text-fg">borg compact</code>.</li>
          <li>Uses your normal <code class="font-mono text-fg">~/.config/borg</code> and <code class="font-mono text-fg">~/.cache/borg</code>; archives you create with borg yourself show up in Flux too.</li>
        </ul>
      {/if}
    </InfoDetails>
  {/if}
</div>
