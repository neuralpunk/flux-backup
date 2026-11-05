<script lang="ts">
  import { tick, untrack } from "svelte";
  import { api, pickFile, pickFolder } from "../api";
  import { app } from "../state.svelte";
  import { bytes, dateTime } from "../format";
  import { SYSTEM_PREFIX, isSystem, type Drive, type Issue, type PreviewRow, type Profile, type Suggestion, type TargetTest } from "../types";
  import Field from "./form/Field.svelte";
  import ListEditor from "./form/ListEditor.svelte";
  import NumberInput from "./form/NumberInput.svelte";
  import Toggle from "./form/Toggle.svelte";
  import Icon, { type IconName } from "./Icon.svelte";

  let {
    id = null,
    system = false,
    onsaved,
    oncancel,
  }: { id?: string | null; system?: boolean; onsaved: (id: string) => void; oncancel: () => void } = $props();
  // The editor is re-created for each profile, so these capture `id` once on purpose.
  const creating = untrack(() => id == null);
  /** A system (root) profile, served by the helper. */
  const sys = untrack(() => system || isSystem(id));

  let draft = $state<Profile | null>(null);
  let initial = $state("");
  let profileId = $state(untrack(() => (id ?? "").replace(SYSTEM_PREFIX, "")));
  let idTouched = $state(false);
  let issues = $state<Issue[]>([]);
  let saving = $state(false);
  let loadError = $state<string | null>(null);
  let confirmDelete = $state(false);

  $effect(() => {
    (creating ? api.newProfile(sys) : api.getProfile(id!)).then(
      (e) => {
        draft = e.profile;
        initial = JSON.stringify(e.profile);
        issues = e.issues;
      },
      (err) => (loadError = (err as Error).message),
    );
  });

  // ---- validation (debounced, on every change) ----
  let validateTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    if (!draft) return;
    const snap = $state.snapshot(draft) as Profile;
    clearTimeout(validateTimer);
    validateTimer = setTimeout(() => api.validateProfile(snap, sys).then((i) => (issues = i)), 250);
  });

  const dirty = $derived(draft ? JSON.stringify(draft) !== initial || creating : false);

  // A new profile gets an id from its name until the user edits the id.
  let idTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    if (!creating || idTouched || !draft) return;
    const name = draft.name;
    clearTimeout(idTimer);
    idTimer = setTimeout(() => api.suggestId(name, sys).then((s) => (profileId = s)), 200);
  });

  // ---- simple / advanced ----
  let advanced = $state(untrack(() => app.editorAdvanced));
  $effect(() => {
    app.editorAdvanced = advanced;
  });
  /** Fields that only the advanced view shows. */
  const advancedField = (f: string) =>
    /^(options|hooks|retention|vault|schedule\.(catch_up|skip_on_battery|every)|sources\.(exclude|exclude_larger_than|exclude_caches|exclude_if_present|one_file_system)|target\.(port|identity_file|ssh_options|bandwidth_limit))/.test(f);
  const totalErrors = $derived(issues.filter((i) => i.severity === "error").length);
  const at = (prefix: string) => issues.filter((i) => i.field === prefix || i.field.startsWith(prefix + "[") || i.field.startsWith(prefix + "."));
  const general = $derived(issues.filter((i) => i.field === "" || i.field === "name"));

  let scroller: HTMLElement | undefined = $state();
  type SectionId = "general" | "sources" | "destination" | "schedule" | "retention" | "hooks" | "advanced";
  function sectionOf(field: string): SectionId {
    if (field.startsWith("target") || field.startsWith("vault")) return "destination";
    if (field.startsWith("sources")) return "sources";
    if (field.startsWith("schedule")) return "schedule";
    if (field.startsWith("retention")) return "retention";
    if (field.startsWith("hooks")) return "hooks";
    if (field.startsWith("options")) return "advanced";
    return "general";
  }
  /** Show the first problem: open the advanced options if that's where it is. */
  async function jumpTo(field: string) {
    if (advancedField(field)) advanced = true;
    await tick();
    scroller?.querySelector(`#sec-${sectionOf(field)}`)?.scrollIntoView({ behavior: "smooth", block: "start" });
  }

  // ---- destination ----
  let drives = $state<Drive[]>([]);
  let testing = $state(false);
  let test = $state<TargetTest | null>(null);
  $effect(() => {
    api.drives().then((d) => (drives = d), () => {});
  });

  function setKind(kind: "local" | "ssh") {
    if (!draft || draft.target.kind === kind) return;
    draft.target = kind === "local" ? { kind: "local", path: "", fs_uuid: null } : { kind: "ssh", host: "", user: null, port: null, path: "backups", identity_file: null, ssh_options: [], bandwidth_limit: null };
    test = null;
  }
  function useDrive(d: Drive) {
    if (!draft) return;
    draft.target = { kind: "local", path: d.mount_point, fs_uuid: d.fs_uuid };
  }
  async function chooseFolder() {
    const f = await pickFolder("Back up into…");
    if (f && draft) draft.target = { kind: "local", path: f, fs_uuid: null };
  }
  async function runTest() {
    if (!draft) return;
    testing = true;
    test = null;
    try {
      test = await api.testTarget($state.snapshot(draft) as Profile, sys);
    } catch (e) {
      test = { ok: false, messages: [(e as Error).message] };
    } finally {
      testing = false;
    }
  }
  function setEngine(engine: "mirror" | "vault") {
    if (!draft) return;
    draft.engine = engine;
    // Root has no desktop keyring: system Vaults get their password once, then keep it in
    // a root-only key file (see PasswordDialog).
    if (engine === "vault" && !draft.vault) draft.vault = { compression_level: 3, encryption: true, password_source: { from: sys ? "prompt" : "keyring" } };
  }
  function setPasswordSource(from: string) {
    if (!draft?.vault) return;
    draft.vault.password_source =
      from === "file" ? { from: "file", path: "" } : from === "command" ? { from: "command", command: "" } : from === "prompt" ? { from: "prompt" } : { from: "keyring" };
  }

  // ---- sources ----
  let suggestions = $state<Suggestion[]>([]);
  $effect(() => {
    if (!draft) return;
    const paths = [...draft.sources.paths];
    api.excludeSuggestions(paths).then((s) => (suggestions = s), () => {});
  });

  // ---- schedule ----
  type Preset = "hourly" | "daily" | "weekdays" | "weekly" | "custom";
  const days = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
  let preset = $state<Preset>("daily");
  let timeOfDay = $state("20:00");
  let weekday = $state("Sun");
  let everyN = $state(4);
  let everyUnit = $state<"min" | "h" | "d">("h");
  let scheduleParsed = false;

  $effect(() => {
    if (!draft || scheduleParsed) return;
    scheduleParsed = true;
    const c = (draft.schedule.on_calendar ?? "").trim();
    let m: RegExpMatchArray | null;
    if (c === "hourly") preset = "hourly";
    else if (c === "daily" || c === "") (preset = "daily"), (timeOfDay = "00:00");
    else if ((m = c.match(/^\*-\*-\* (\d\d):(\d\d)(:00)?$/))) (preset = "daily"), (timeOfDay = `${m[1]}:${m[2]}`);
    else if ((m = c.match(/^Mon\.\.Fri \*-\*-\* (\d\d):(\d\d)(:00)?$/))) (preset = "weekdays"), (timeOfDay = `${m[1]}:${m[2]}`);
    else if ((m = c.match(/^(Mon|Tue|Wed|Thu|Fri|Sat|Sun) \*-\*-\* (\d\d):(\d\d)(:00)?$/))) (preset = "weekly"), (weekday = m[1]!), (timeOfDay = `${m[2]}:${m[3]}`);
    else preset = "custom";
    const e = (draft.schedule.every ?? "").match(/^(\d+)\s*(min|m|h|d)/);
    if (e) (everyN = Number(e[1])), (everyUnit = e[2] === "m" ? "min" : (e[2] as "min" | "h" | "d"));
  });

  function applyCalendar() {
    if (!draft || preset === "custom") return;
    const t = `${timeOfDay}:00`;
    draft.schedule.on_calendar =
      preset === "hourly" ? "hourly" : preset === "daily" ? `*-*-* ${t}` : preset === "weekdays" ? `Mon..Fri *-*-* ${t}` : `${weekday} *-*-* ${t}`;
  }
  function applyEvery() {
    if (draft) draft.schedule.every = `${Math.max(1, everyN)}${everyUnit}`;
  }
  // Keep `every` in step with the interval controls.
  $effect(() => {
    void everyN;
    void everyUnit;
    if (draft?.schedule.trigger === "interval" && scheduleParsed) applyEvery();
  });

  function setTrigger(t: "manual" | "calendar" | "interval") {
    if (!draft) return;
    draft.schedule.trigger = t;
    if (t === "calendar" && !draft.schedule.on_calendar) applyCalendar();
    if (t === "interval" && !draft.schedule.every) applyEvery();
  }

  // The simple view's choices.
  type When = "hourly" | "daily" | "weekly" | "manual";
  const when = $derived.by<When | "custom">(() => {
    if (!draft) return "manual";
    if (draft.schedule.trigger === "manual") return "manual";
    if (draft.schedule.trigger === "calendar" && (preset === "hourly" || preset === "daily" || preset === "weekly")) return preset;
    return "custom";
  });
  function setWhen(w: When) {
    if (!draft) return;
    if (w === "manual") return setTrigger("manual");
    preset = w;
    draft.schedule.trigger = "calendar";
    applyCalendar();
  }
  const encrypted = $derived(draft?.engine === "vault" && (draft.vault?.encryption ?? true));
  function setEncrypted(on: boolean) {
    setEngine(on ? "vault" : "mirror");
    if (on && draft?.vault) draft.vault.encryption = true;
  }
  /** "Keeps 7 daily, 4 weekly and 12 monthly snapshots." */
  const keepSummary = $derived.by(() => {
    const r = draft?.retention;
    if (!r) return "";
    if (!r.enabled) return "Keeps every snapshot; nothing is removed automatically.";
    const parts = [
      r.keep_hourly && `${r.keep_hourly} hourly`,
      r.keep_daily && `${r.keep_daily} daily`,
      r.keep_weekly && `${r.keep_weekly} weekly`,
      r.keep_monthly && `${r.keep_monthly} monthly`,
      r.keep_yearly && `${r.keep_yearly} yearly`,
    ].filter(Boolean) as string[];
    if (!parts.length) return `Keeps the newest ${Math.max(1, r.keep_last)} ${r.keep_last === 1 ? "snapshot" : "snapshots"}.`;
    const list = parts.length > 1 ? `${parts.slice(0, -1).join(", ")} and ${parts.at(-1)}` : parts[0];
    return `Keeps ${list} snapshots, and removes older ones as new backups are made.`;
  });

  // ---- retention preview ----
  let preview = $state<PreviewRow[] | null>(null);
  let previewTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    if (creating || !draft) return;
    const r = $state.snapshot(draft.retention) as Profile["retention"];
    clearTimeout(previewTimer);
    previewTimer = setTimeout(() => api.retentionPreview(id!, r).then((p) => (preview = p), () => (preview = null)), 350);
  });
  const previewRemoved = $derived(preview?.filter((p) => !p.keep).length ?? 0);

  // Optional string fields: empty input ↔ null.
  const opt = (v: string) => (v.trim() === "" ? null : v.trim());
  const lines = (v: string) => v.split("\n").map((l) => l.trim()).filter(Boolean);

  async function save() {
    if (!draft) return;
    saving = true;
    try {
      const r = await api.saveProfile(creating ? (sys ? SYSTEM_PREFIX : "") + profileId : id!, $state.snapshot(draft) as Profile, creating);
      issues = r.issues;
      if (r.saved) {
        app.toast("ok", creating ? `Created “${draft.name}”.` : `Saved “${draft.name}”.`);
        if (r.schedule) app.toast("info", r.schedule);
        initial = JSON.stringify(draft);
        await app.refresh();
        onsaved(r.id);
      } else {
        const first = r.issues.find((i) => i.severity === "error");
        if (first) void jumpTo(first.field);
      }
    } catch (e) {
      app.toast("bad", (e as Error).message);
    } finally {
      saving = false;
    }
  }

  async function remove() {
    if (!id) return;
    try {
      await api.deleteProfile(id);
      app.toast("ok", "Profile deleted. Its backups were left in place.");
      await app.refresh();
      oncancel();
    } catch (e) {
      app.toast("bad", (e as Error).message);
    }
  }

  const presetLabel: Record<Preset, string> = { hourly: "Every hour", daily: "Every day", weekdays: "Weekdays", weekly: "Once a week", custom: "Custom" };
</script>

{#if loadError}
  <div class="card m-8 flex items-center gap-3 p-5 text-danger"><Icon name="alert" /> {loadError}</div>
{:else if !draft}
  <div class="card m-8 h-64 animate-pulse"></div>
{:else}
  {#snippet heading(title: string, sub = "")}
    <div>
      <h2 class="text-[18px] font-medium">{title}</h2>
      {#if sub}<p class="mt-0.5 text-[13px] text-fg-muted">{sub}</p>{/if}
    </div>
  {/snippet}
  {#snippet choice(selected: boolean, label: string, onpick: () => void)}
    <button
      role="radio"
      aria-checked={selected}
      class="pill px-[18px] py-2 text-[13.5px] transition-colors {selected ? 'bg-primary-container font-medium text-on-primary-container' : 'bg-surface-3 text-fg-2 hover:bg-track hover:text-fg'}"
      onclick={onpick}>{label}</button
    >
  {/snippet}

  <div class="flex h-full min-h-0 flex-col">
    <div bind:this={scroller} class="min-h-0 flex-1 overflow-y-auto px-8 pb-10">
      <div class="mx-auto flex max-w-2xl flex-col gap-3 pt-2">
        {#each general.filter((i) => i.field === "") as i (i.message)}
          <div class="card bg-danger/10 px-5 py-3 text-sm text-danger">{i.message}</div>
        {/each}

        <!-- Name -->
        <section id="sec-general" class="card flex flex-col gap-4 px-6 py-5">
          <Field label="Name" id="f-name" issues={at("name")}>
            <input id="f-name" class="input" bind:value={draft.name} placeholder="Home folder" />
          </Field>
          {#if advanced}
            <Field label="Profile id" id="f-id" hint={creating ? "Used for files and timers; can't be changed later." : "Can't be changed."}>
              <input id="f-id" class="input font-mono text-[13px]" bind:value={profileId} disabled={!creating} oninput={() => (idTouched = true)} />
            </Field>
          {/if}
        </section>

        <!-- What -->
        <section id="sec-sources" class="card flex flex-col gap-4 px-6 py-5">
          {@render heading("What to back up")}
          <Field label="" issues={at("sources.paths")}>
            <ListEditor bind:items={draft.sources.paths} placeholder="/home/you" pick={() => pickFolder("Add a folder to back up")} />
          </Field>
          {#if advanced}
            <Field label="Leave out" hint="*.iso or node_modules/ match anywhere; /path or ~/path one place; ! keeps something back in." issues={at("sources.exclude")}>
              <ListEditor bind:items={draft.sources.exclude} placeholder="~/Videos/  or  *.iso" />
            </Field>
            {#if suggestions.length}
              {@const exclude = draft.sources.exclude}
              <div class="flex flex-wrap gap-2" aria-label="Commonly left out">
                {#each suggestions as sg (sg.pattern)}
                  {@const added = exclude.includes(sg.pattern)}
                  <button
                    class="pill flex items-center gap-1.5 px-3.5 py-1.5 text-[13px] transition-colors {added ? 'bg-primary-container text-on-primary-container' : 'bg-surface-3 text-fg-2 hover:bg-track hover:text-fg'}"
                    title="{sg.why}{sg.size ? ` · ${bytes(sg.size)}` : ''}"
                    aria-pressed={added}
                    onclick={() => draft && (draft.sources.exclude = added ? exclude.filter((x) => x !== sg.pattern) : [...exclude, sg.pattern])}
                  >
                    <Icon name={added ? "check" : "plus"} size={13} />
                    {sg.label}{#if sg.size}<span class="num font-mono text-[12px] opacity-70">{bytes(sg.size)}</span>{/if}
                  </button>
                {/each}
              </div>
            {/if}
            <div class="grid grid-cols-2 gap-5">
              <Field label="Skip files larger than" id="f-big" hint="e.g. 4 GiB" issues={at("sources.exclude_larger_than")}>
                <input id="f-big" class="input" value={draft.sources.exclude_larger_than ?? ""} oninput={(e) => draft && (draft.sources.exclude_larger_than = opt(e.currentTarget.value))} placeholder="no limit" />
              </Field>
              <div class="space-y-3 pt-6">
                <Toggle bind:checked={draft.sources.exclude_caches} label="Skip cache folders" />
                <Toggle bind:checked={draft.sources.one_file_system} label="Stay on one disk" />
              </div>
            </div>
          {/if}
        </section>

        <!-- Where -->
        <section id="sec-destination" class="card flex flex-col gap-4 px-6 py-5">
          {@render heading("Where to keep it")}
          <div class="pill flex gap-1 self-start bg-surface-sunken p-1" role="radiogroup" aria-label="Destination type">
            {@render choice(draft.target.kind === "local", "A drive or folder", () => setKind("local"))}
            {@render choice(draft.target.kind === "ssh", "A server (SSH)", () => setKind("ssh"))}
          </div>

          {#if draft.target.kind === "local"}
            {@const target = draft.target}
            {#if drives.length}
              <div class="grid grid-cols-2 gap-2">
                {#each drives as d (d.mount_point)}
                  {@const chosen = target.path === d.mount_point}
                  <button class="flex items-center gap-3 rounded-2xl px-4 py-3 text-left transition-colors {chosen ? 'bg-primary-container text-on-primary-container' : 'bg-surface-3 hover:bg-track'}" onclick={() => useDrive(d)}>
                    <Icon name="drive" />
                    <span class="min-w-0 flex-1">
                      <span class="block truncate text-sm font-medium">{d.label}</span>
                      <span class="num block truncate text-[12.5px] opacity-75">{bytes(d.free)} free</span>
                    </span>
                    {#if chosen}<Icon name="check" />{/if}
                  </button>
                {/each}
              </div>
            {/if}
            <div class="flex items-center gap-2">
              {#if target.path && !drives.some((d) => d.mount_point === target.path)}
                <span class="tag max-w-md font-mono"><span class="truncate">{target.path}</span></span>
              {:else if !drives.length && !target.path}
                <span class="text-sm text-fg-muted">No drive connected. Plug one in, or choose a folder.</span>
              {/if}
              <button class="btn" onclick={chooseFolder}>{drives.length ? "Another folder…" : "Choose a folder…"}</button>
            </div>
            {#each at("target.path") as i (i.message)}
              <!-- A new profile without a target yet isn't an error, just the next step. -->
              <p class="text-[13px] {creating && !target.path ? 'text-fg-muted' : i.severity === 'error' ? 'text-danger' : 'text-warning'}">{i.message}</p>
            {/each}
            {#if advanced}
              <Field label="Folder" id="f-target" hint={target.fs_uuid ? "Tied to this drive: Flux won't write into the folder when another disk is there." : ""}>
                <input id="f-target" class="input font-mono text-[13px]" bind:value={target.path} placeholder="/run/media/you/Backup" />
              </Field>
            {/if}
          {:else}
            {@const target = draft.target}
            <div class="grid grid-cols-5 gap-4">
              <div class="col-span-3"><Field label="Server" id="f-host" issues={at("target.host")}><input id="f-host" class="input" bind:value={target.host} placeholder="nas.local" /></Field></div>
              <div class="col-span-2"><Field label="User" id="f-user" issues={at("target.user")}><input id="f-user" class="input" value={target.user ?? ""} oninput={(e) => (target.user = opt(e.currentTarget.value))} placeholder="optional" /></Field></div>
              <div class="col-span-5"><Field label="Folder on the server" id="f-rpath" issues={at("target.path")}><input id="f-rpath" class="input font-mono text-[13px]" bind:value={target.path} /></Field></div>
              {#if advanced}
                <div><Field label="Port" id="f-port" issues={at("target.port")}><input id="f-port" class="input num" inputmode="numeric" value={target.port ?? ""} oninput={(e) => (target.port = e.currentTarget.value ? Number(e.currentTarget.value) : null)} placeholder="22" /></Field></div>
                <div class="col-span-4">
                  <Field label="Key file" id="f-key" hint="Empty: your ssh config and agent decide.">
                    <div class="flex gap-2">
                      <input id="f-key" class="input font-mono text-[13px]" value={target.identity_file ?? ""} oninput={(e) => (target.identity_file = opt(e.currentTarget.value))} placeholder="~/.ssh/flux_backup_ed25519" />
                      <button class="btn" aria-label="Choose key file" onclick={async () => { const f = await pickFile("SSH private key"); if (f) target.identity_file = f; }}><Icon name="folder" size={14} /></button>
                    </div>
                  </Field>
                </div>
                <div class="col-span-5"><Field label="Upload limit per second" id="f-bw" hint="e.g. 5 MiB"><input id="f-bw" class="input" value={target.bandwidth_limit ?? ""} oninput={(e) => (target.bandwidth_limit = opt(e.currentTarget.value))} placeholder="unlimited" /></Field></div>
              {/if}
            </div>
            <div class="flex items-start gap-3">
              <button class="btn" onclick={runTest} disabled={testing || !target.host}>{testing ? "Testing…" : "Test connection"}</button>
              {#if test}
                <ul class="pt-2 text-[13px] {test.ok ? 'text-success' : 'text-danger'}">
                  {#each test.messages as m (m)}<li>{m}</li>{/each}
                </ul>
              {/if}
            </div>
          {/if}

          <!-- Protection -->
          <div class="flex items-start gap-3 rounded-2xl bg-surface-3 px-4 py-3.5">
            <button
              type="button"
              role="switch"
              aria-checked={encrypted}
              aria-label="Encrypt with a password"
              disabled={!creating && !advanced}
              class="relative mt-0.5 h-5 w-9 shrink-0 rounded-full transition-colors disabled:opacity-50 {encrypted ? 'bg-primary' : 'bg-track'}"
              onclick={() => setEncrypted(!encrypted)}
            >
              <span class="absolute top-0.5 left-0.5 size-4 rounded-full transition-transform {encrypted ? 'translate-x-4 bg-on-primary' : 'bg-fg-muted'}"></span>
            </button>
            <span class="min-w-0">
              <span class="block text-sm font-medium">Encrypt with a password</span>
              <span class="block text-[12.5px] text-fg-muted">
                {encrypted
                  ? `Nobody can read the backups without the password; they're also compressed and deduplicated.${creating ? " You'll choose the password at the first backup." : ""}`
                  : "Backups are plain folders you can open in any file manager."}
              </span>
            </span>
          </div>
          {#if !creating && JSON.parse(initial).engine !== draft.engine}
            <p class="text-[13px] text-warning">This starts a new backup history. Existing snapshots stay where they are, but won't appear here.</p>
          {/if}

          {#if advanced}
            <div class="grid grid-cols-2 gap-2" role="radiogroup" aria-label="Storage engine">
              {#each [{ e: "mirror", title: "Mirror", text: "Plain hard-linked folders" }, { e: "vault", title: "Vault", text: "Deduplicated, compressed, optionally encrypted" }] as eng (eng.e)}
                <button role="radio" aria-checked={draft.engine === eng.e} class="rounded-2xl px-4 py-3 text-left transition-colors {draft.engine === eng.e ? 'bg-primary-container text-on-primary-container' : 'bg-surface-3 hover:bg-track'}" onclick={() => setEngine(eng.e as "mirror" | "vault")}>
                  <span class="block text-sm font-medium">{eng.title}</span>
                  <span class="block text-[12.5px] opacity-75">{eng.text}</span>
                </button>
              {/each}
            </div>
            {#if draft.engine === "vault" && draft.vault}
              {@const vault = draft.vault}
              <Toggle bind:checked={vault.encryption} label="Encrypt" hint={creating ? "Can't be changed once the Vault exists." : "Fixed when the Vault was created."} disabled={!creating} />
              {#if vault.encryption}
                <Field label="Password comes from" id="f-pwsrc" issues={at("vault.password_source")} hint={sys ? "System backups run with nobody logged in: unlock once and Flux keeps the password in a root-only key file." : "Scheduled backups need a source that works without you."}>
                  <select id="f-pwsrc" class="input" value={vault.password_source.from} onchange={(e) => setPasswordSource(e.currentTarget.value)}>
                    <option value="keyring" disabled={sys}>Desktop keyring (asked once){sys ? " — not for system backups" : ""}</option>
                    <option value="prompt">Ask every time</option>
                    <option value="file">A file</option>
                    <option value="command">A command (e.g. pass)</option>
                  </select>
                </Field>
                {#if vault.password_source.from === "file"}
                  {@const src = vault.password_source}
                  <Field label="Password file" id="f-pwfile" issues={at("vault")}><input id="f-pwfile" class="input font-mono text-[13px]" bind:value={src.path} placeholder="~/.config/flux-backup/vault.pass" /></Field>
                {:else if vault.password_source.from === "command"}
                  {@const src = vault.password_source}
                  <Field label="Command" id="f-pwcmd"><input id="f-pwcmd" class="input font-mono text-[13px]" bind:value={src.command} placeholder="pass show backup/vault" /></Field>
                {/if}
              {/if}
              <Field label="Compression: level {vault.compression_level}" id="f-zstd" hint="0 off · 3 fast (default) · 19 smallest" issues={at("vault.compression_level")}>
                <input id="f-zstd" type="range" min="0" max="19" class="w-full accent-[var(--c-primary)]" bind:value={vault.compression_level} />
              </Field>
            {/if}
          {/if}
        </section>

        <!-- When -->
        <section id="sec-schedule" class="card flex flex-col gap-4 px-6 py-5">
          {@render heading("When")}
          <div class="flex flex-wrap gap-2" role="radiogroup" aria-label="How often">
            {@render choice(when === "hourly", "Every hour", () => setWhen("hourly"))}
            {@render choice(when === "daily", "Every day", () => setWhen("daily"))}
            {@render choice(when === "weekly", "Once a week", () => setWhen("weekly"))}
            {@render choice(when === "manual", "Only when I click", () => setWhen("manual"))}
            {#if when === "custom" && !advanced}{@render choice(true, "Custom", () => (advanced = true))}{/if}
          </div>
          {#if when === "daily" || when === "weekly"}
            <div class="flex items-center gap-2 text-sm text-fg-2">
              {#if when === "weekly"}
                <span>on</span>
                <select class="input w-28" bind:value={weekday} onchange={applyCalendar} aria-label="Day">{#each days as d (d)}<option>{d}</option>{/each}</select>
              {/if}
              <span>at</span>
              <input type="time" class="input w-32" bind:value={timeOfDay} onchange={applyCalendar} aria-label="Time" />
            </div>
          {/if}
          {#if draft.target.kind === "local" && draft.target.fs_uuid}
            <Toggle bind:checked={draft.schedule.on_connect} label="Also when this drive is plugged in" />
          {/if}
          {#each at("schedule.on_connect") as i (i.message)}<p class="text-[13px] text-danger">{i.message}</p>{/each}
          {#if advanced}
            <div class="flex flex-wrap gap-2" role="radiogroup" aria-label="More schedules">
              {@render choice(draft.schedule.trigger === "calendar" && preset === "weekdays", "Weekdays", () => { preset = "weekdays"; if (draft) draft.schedule.trigger = "calendar"; applyCalendar(); })}
              {@render choice(draft.schedule.trigger === "interval", "Every few hours", () => setTrigger("interval"))}
              {@render choice(draft.schedule.trigger === "calendar" && preset === "custom", "Custom times", () => { preset = "custom"; if (draft) draft.schedule.trigger = "calendar"; })}
            </div>
            {#if draft.schedule.trigger === "calendar" && preset === "weekdays"}
              <div class="flex items-center gap-2 text-sm text-fg-2"><span>at</span><input type="time" class="input w-32" bind:value={timeOfDay} onchange={applyCalendar} aria-label="Time" /></div>
            {:else if draft.schedule.trigger === "calendar" && preset === "custom"}
              <Field label="systemd calendar expression" id="f-cal" issues={at("schedule.on_calendar")} hint="e.g. Mon,Wed,Fri 07:30 or *-*-01 03:00 (man systemd.time)">
                <input id="f-cal" class="input font-mono text-[13px]" value={draft.schedule.on_calendar ?? ""} oninput={(e) => draft && (draft.schedule.on_calendar = opt(e.currentTarget.value))} />
              </Field>
            {:else if draft.schedule.trigger === "interval"}
              <Field label="Every" issues={at("schedule.every")} hint="Counted from the last successful backup.">
                <div class="flex gap-2">
                  <NumberInput bind:value={everyN} min={1} label="Interval" />
                  <select class="input w-32" bind:value={everyUnit} aria-label="Unit">
                    <option value="min">minutes</option>
                    <option value="h">hours</option>
                    <option value="d">days</option>
                  </select>
                </div>
              </Field>
            {/if}
            {#if draft.schedule.trigger === "calendar"}
              <Toggle bind:checked={draft.schedule.catch_up} label="Catch up on missed backups" hint="If the computer was off at the time, back up soon after it starts." />
            {/if}
            <Toggle bind:checked={draft.schedule.skip_on_battery} label="Wait for mains power" hint="Scheduled backups don't run on battery." />
          {/if}
        </section>

        <!-- Keep -->
        <section id="sec-retention" class="card flex flex-col gap-4 px-6 py-5">
          {@render heading("What to keep", keepSummary)}
          {#if advanced}
            <Toggle bind:checked={draft.retention.enabled} label="Remove old snapshots automatically" hint="The newest, pinned and named snapshots always stay." />
            {#if draft.retention.enabled}
              {@const r = draft.retention}
              <div class="grid grid-cols-[1fr_auto] items-center gap-x-6 gap-y-2.5 text-sm text-fg-2">
                <span>Always keep the newest</span><NumberInput bind:value={r.keep_last} label="Keep last" />
                <span>One per hour, for … hours</span><NumberInput bind:value={r.keep_hourly} label="Hourly" />
                <span>One per day, for … days</span><NumberInput bind:value={r.keep_daily} label="Daily" />
                <span>One per week, for … weeks</span><NumberInput bind:value={r.keep_weekly} label="Weekly" />
                <span>One per month, for … months</span><NumberInput bind:value={r.keep_monthly} label="Monthly" />
                <span>One per year, for … years</span><NumberInput bind:value={r.keep_yearly} label="Yearly" />
              </div>
              <div class="grid grid-cols-3 gap-4">
                <Field label="Keep everything from the last" id="f-within" hint="e.g. 1 day" issues={at("retention.keep_within")}>
                  <input id="f-within" class="input" value={r.keep_within ?? ""} oninput={(e) => (r.keep_within = opt(e.currentTarget.value))} placeholder="—" />
                </Field>
                <Field label="Never keep longer than" id="f-maxage" hint="e.g. 2 years" issues={at("retention.max_age")}>
                  <input id="f-maxage" class="input" value={r.max_age ?? ""} oninput={(e) => (r.max_age = opt(e.currentTarget.value))} placeholder="no limit" />
                </Field>
                <Field label="Keep this much free" id="f-minfree" hint="e.g. 50 GiB" issues={at("retention.min_free_space")}>
                  <input id="f-minfree" class="input" value={r.min_free_space ?? ""} oninput={(e) => (r.min_free_space = opt(e.currentTarget.value))} placeholder="—" />
                </Field>
              </div>
              <Toggle bind:checked={r.keep_named} label="Never remove named snapshots" />
              {#each at("retention").filter((i) => i.field === "retention") as i (i.message)}<p class="text-sm text-danger">{i.message}</p>{/each}
            {/if}
            {#if preview}
              <div class="rounded-2xl bg-surface-sunken p-1.5">
                <div class="flex justify-between px-3 pt-2 pb-1 text-[13px]">
                  <span class="text-fg-2">Your snapshots with these rules</span>
                  <span class={previewRemoved ? "text-warning" : "text-fg-muted"}>{previewRemoved ? `${previewRemoved} of ${preview.length} would go` : `all ${preview.length} stay`}</span>
                </div>
                <ul class="max-h-56 overflow-y-auto text-[13px]">
                  {#each preview as row (row.id)}
                    <li class="flex items-center gap-3 rounded-xl px-3 py-1.5 {row.keep ? '' : 'text-danger'}" title={row.keep ? row.reasons.join(", ") : row.expired ? "older than the limit" : "no rule keeps it"}>
                      <Icon name={row.keep ? "check" : "trash"} size={13} class={row.keep ? "text-success" : ""} />
                      <span class="num">{dateTime(row.time)}</span>
                      <span class="min-w-0 flex-1 truncate text-fg-muted">{row.name ?? ""}</span>
                    </li>
                  {/each}
                </ul>
              </div>
            {/if}
          {/if}
        </section>

        {#if advanced}
          <!-- Hooks -->
          <section id="sec-hooks" class="card flex flex-col gap-4 px-6 py-5">
            {@render heading("Hooks", "Shell commands run around each backup, one per line. A failing “before” command cancels it.")}
            {#each [{ k: "pre_backup", label: "Before the backup", ph: "mount /mnt/nas" }, { k: "post_backup", label: "After a successful backup", ph: "notify-send 'Backup done'" }, { k: "on_error", label: "When a backup fails", ph: "logger -t flux \"$FLUX_ERROR\"" }] as h (h.k)}
              {@const key = h.k as "pre_backup" | "post_backup" | "on_error"}
              <Field label={h.label} id="f-{h.k}">
                <textarea
                  id="f-{h.k}"
                  class="input h-16 py-2 font-mono text-[13px]"
                  placeholder={h.ph}
                  value={(draft.hooks?.[key] ?? []).join("\n")}
                  oninput={(e) => draft && (draft.hooks = { ...(draft.hooks ?? {}), [key]: lines(e.currentTarget.value) })}
                ></textarea>
              </Field>
            {/each}
            <Field label="Stop a hook after" id="f-hooktimeout" issues={at("hooks.timeout")}>
              <input id="f-hooktimeout" class="input w-40" value={draft.hooks?.timeout ?? ""} oninput={(e) => draft && (draft.hooks = { ...(draft.hooks ?? {}), timeout: opt(e.currentTarget.value) })} placeholder="10m" />
            </Field>
          </section>

          <!-- Options -->
          <section id="sec-advanced" class="card flex flex-col gap-4 px-6 py-5">
            {@render heading("Options")}
            <div class="grid grid-cols-2 gap-x-8 gap-y-4">
              <Toggle bind:checked={draft.options.preserve_acl} label="Keep ACLs" />
              <Toggle bind:checked={draft.options.preserve_xattr} label="Keep extended attributes" />
              <Toggle bind:checked={draft.options.continue_on_errors} label="Finish despite unreadable files" />
              <Toggle bind:checked={draft.options.snapshot_if_unchanged} label="Snapshot even when nothing changed" />
              <Toggle bind:checked={draft.options.notify} label="Notify me about problems" />
              <Toggle bind:checked={draft.options.backup_before_restore} label="Keep my files when restoring over them" />
            </div>
            <div class="grid grid-cols-2 gap-5">
              <Field label="Symbolic links" id="f-links">
                <select id="f-links" class="input" bind:value={draft.options.symlinks}>
                  <option value="preserve">Store links as links</option>
                  <option value="follow_unsafe">Follow links pointing outside</option>
                  <option value="follow">Always follow links</option>
                </select>
              </Field>
              <Field label="Detect changes by" id="f-detect">
                <select id="f-detect" class="input" bind:value={draft.options.change_detection}>
                  <option value="metadata">Size and time (fast)</option>
                  <option value="checksum">Content checksums (slow)</option>
                </select>
              </Field>
              <Field label="CPU priority: nice {draft.options.nice}" id="f-nice">
                <input id="f-nice" type="range" min="0" max="19" class="w-full accent-[var(--c-primary)]" bind:value={draft.options.nice} />
              </Field>
              <Field label="Disk priority" id="f-io">
                <select id="f-io" class="input" bind:value={draft.options.io_priority}>
                  <option value="normal">Normal</option>
                  <option value="best_effort_low">Low</option>
                  <option value="idle">Only when the disk is idle</option>
                </select>
              </Field>
              <Field label="Worker threads (0 = automatic)">
                <NumberInput bind:value={draft.options.threads} max={256} label="Worker threads" />
              </Field>
            </div>
          </section>

          {#if !creating}
            <section class="card flex items-center gap-3 px-6 py-4">
              <span class="min-w-0 flex-1 text-sm text-fg-2">Delete this profile. Its backups stay where they are.</span>
              {#if confirmDelete}
                <button class="btn btn-danger" onclick={remove}>Delete profile</button>
                <button class="btn btn-ghost" onclick={() => (confirmDelete = false)}>Keep</button>
              {:else}
                <button class="btn btn-danger" onclick={() => (confirmDelete = true)}>Delete…</button>
              {/if}
            </section>
          {/if}
        {/if}

        <button class="btn btn-ghost mt-1 self-center" aria-expanded={advanced} onclick={() => (advanced = !advanced)}>
          <Icon name={advanced ? "chevronDown" : "settings"} size={15} />
          {advanced ? "Hide advanced options" : "Advanced options"}
        </button>
      </div>
    </div>

    <!-- Footer -->
    <footer class="flex items-center gap-3 px-8 py-4">
      {#if totalErrors}
        {@const first = issues.find((i) => i.severity === "error")!}
        <button class="flex min-w-0 items-center gap-1.5 text-left text-sm {creating ? 'text-fg-2' : 'text-danger'}" onclick={() => void jumpTo(first.field)}>
          {#if creating}
            <span class="truncate">To finish: {first.message}{totalErrors > 1 ? ` (and ${totalErrors - 1} more)` : ""}</span>
          {:else}
            <Icon name="alert" size={15} /> {totalErrors === 1 ? "One thing to fix" : `${totalErrors} things to fix`}
          {/if}
        </button>
      {:else if dirty && !creating}
        <span class="text-sm text-fg-muted">Unsaved changes</span>
      {/if}
      <div class="ml-auto flex gap-2">
        <button class="btn btn-ghost" onclick={oncancel}>Cancel</button>
        <button class="btn btn-primary h-10" onclick={save} disabled={saving || totalErrors > 0 || !dirty || !profileId}>
          {saving ? "Saving…" : creating ? "Create backup" : "Save"}
        </button>
      </div>
    </footer>
  </div>
{/if}
