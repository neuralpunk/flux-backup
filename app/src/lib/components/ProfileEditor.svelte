<script lang="ts">
  import VaultSecurity from "./VaultSecurity.svelte";
  import DestinationDialog from "./DestinationDialog.svelte";
  import WebhookList from "./WebhookList.svelte";
  import RuleTester from "./RuleTester.svelte";
  import { tick, untrack } from "svelte";
  import { api, inTauri, pickFile, pickFolder } from "../api";
  import StorageChoice from "./StorageChoice.svelte";
  import Modal from "./Modal.svelte";
  import { app } from "../state.svelte";
  import { bytes, count, dateTime } from "../format";
  import { sizeLabel } from "../size";
  import { intervalParts, intervalText, type IntervalUnit } from "../interval";
  import { ApiError, SYSTEM_PREFIX, engineLabel, isCompat, isSystem, type Drive, type Engine, type Estimate, type Issue, type PasswordSource, type PreviewRow, type Profile, type WhenMissing, type Suggestion, type BitProfile, type TargetTest } from "../types";
  import Field from "./form/Field.svelte";
  import ListEditor from "./form/ListEditor.svelte";
  import NumberInput from "./form/NumberInput.svelte";
  import Toggle from "./form/Toggle.svelte";
  import Row from "./form/Row.svelte";
  import SizeLimit from "./SizeLimit.svelte";
  import InfoDetails from "./InfoDetails.svelte";
  import StorageMeter from "./StorageMeter.svelte";
  import BackInTimeOffer from "./BackInTimeOffer.svelte";
  import Icon, { type IconName } from "./Icon.svelte";

  let {
    id = null,
    system = false,
    template = null,
    onsaved,
    oncancel,
  }: {
    id?: string | null;
    system?: boolean;
    /** A new backup starts from this template. */
    template?: string | null;
    onsaved: (id: string) => void;
    oncancel: () => void;
  } = $props();
  // The editor is re-created for each profile, so these capture `id` once on purpose.
  const creating = untrack(() => id == null);
  /** A system (root) profile, served by the helper. */
  const sys = untrack(() => system || isSystem(id));

  let draft = $state<Profile | null>(null);
  let initial = $state("");
  let profileId = $state(untrack(() => (id ?? "").replace(SYSTEM_PREFIX, "")));
  let idTouched = $state(false);
  /** The id field is shown (new profiles: after "Change"). */
  // What to keep: presets for most people, the numbers for whoever wants them.
  type KeepNumbers = Pick<Profile["retention"], "keep_last" | "keep_hourly" | "keep_daily" | "keep_weekly" | "keep_monthly" | "keep_yearly">;
  const keepPresets: { id: string; label: string; text: string; v: KeepNumbers }[] = [
    { id: "recommended", label: "Recommended", text: "Everything from the last day, then one a day for a week, a week for a month, a month for a year and a year for five years.", v: { keep_last: 1, keep_hourly: 0, keep_daily: 7, keep_weekly: 4, keep_monthly: 12, keep_yearly: 5 } },
    { id: "less", label: "Save space", text: "Everything from the last day, then one a day for a week, a week for a month and a month for three months.", v: { keep_last: 1, keep_hourly: 0, keep_daily: 7, keep_weekly: 4, keep_monthly: 3, keep_yearly: 0 } },
    { id: "more", label: "Keep longer", text: "One an hour for a day, one a day for two weeks, a week for two months, a month for two years and a year for ten years.", v: { keep_last: 1, keep_hourly: 24, keep_daily: 14, keep_weekly: 8, keep_monthly: 24, keep_yearly: 10 } },
  ];
  let customKeep = $state(false);
  function keepPresetOf(r: Profile["retention"]): string | null {
    if (r.max_age || r.min_free_space || (r.keep_within && r.keep_within !== "1day")) return null;
    const p = keepPresets.find((k) => (Object.keys(k.v) as (keyof KeepNumbers)[]).every((f) => r[f] === k.v[f]));
    return p?.id ?? null;
  }
  function applyKeepPreset(id: string) {
    const p = keepPresets.find((k) => k.id === id);
    if (!draft || !p) return;
    Object.assign(draft.retention, p.v, { keep_within: "1day", max_age: null, min_free_space: null });
    customKeep = false;
  }
  /** Showing every drive to move an existing backup elsewhere. */
  let changingWhere = $state(false);
  let editingId = $state(false);
  let issues = $state<Issue[]>([]);
  let saving = $state(false);
  let loadError = $state<string | null>(null);

  $effect(() => {
    (creating ? (template ? api.newFromTemplate(template, sys) : api.newProfile(sys)) : api.getProfile(id!)).then(
      (e) => {
        normalize(e.profile);
        draft = e.profile;
        initial = JSON.stringify(e.profile);
        issues = e.issues;
      },
      (err) => (loadError = (err as Error).message),
    );
  });

  // ---- starting from a Back In Time profile (new backups of your own only) ----
  let bitProfiles = $state<BitProfile[]>([]);
  let bitUsed = $state<BitProfile | null>(null);
  let bitDismissed = $state(false);
  /** The new profile as it was before importing, for "Start from scratch". */
  let blank: Profile | null = null;
  $effect(() => {
    // Another way to start: offered unless a specific template was chosen.
    if (!creating || sys || (template && template !== "home" && template !== "blank")) return;
    api.backintimeProfiles().then((v) => (bitProfiles = v), () => {});
  });
  function useBackInTime(b: BitProfile) {
    if (!draft) return;
    blank ??= structuredClone($state.snapshot(draft) as Profile);
    const p = structuredClone($state.snapshot(b.profile) as Profile);
    normalize(p);
    draft = p;
    scheduleParsed = false; // read the imported schedule into the When controls
    bitUsed = b;
    test = null;
    // A name of its own is kept; Back In Time's default one says nothing, so the
    // backup is named after its folders as usual.
    nameTouched = b.name !== "Main profile";
  }
  function startFromScratch() {
    if (blank) {
      draft = structuredClone(blank);
      scheduleParsed = false;
    }
    bitUsed = null;
    nameTouched = false;
  }

  /** Lists the form edits in place exist even when the file leaves them out. */
  function normalize(p: Profile) {
    if (p.borg) p.borg.extra_args ??= [];
    p.sources.exclude_if_present ??= [];
    p.maintenance ??= { verify_monthly: false, free_space_weekly: true };
    p.schedule.skip_on_metered ??= false;
    if (p.target.kind === "ssh") p.target.ssh_options ??= [];
    if (p.engine === "rsync") p.rsync = { extra_args: p.rsync?.extra_args ?? [] };
  }
  /** The profile as saved: no empty option lists. */
  function forSaving(p: Profile): Profile {
    const out = structuredClone(p);
    if (out.rsync && !out.rsync.extra_args?.length) out.rsync = null;
    return out;
  }

  // ---- validation (debounced, on every change) ----
  let validateTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    if (!draft) return;
    const snap = forSaving($state.snapshot(draft) as Profile);
    clearTimeout(validateTimer);
    validateTimer = setTimeout(() => api.validateProfile(snap, sys).then((i) => (issues = i)), 250);
  });

  const dirty = $derived(draft ? JSON.stringify(draft) !== initial || creating : false);

  // A new backup is named after what it backs up ("Documents", "Home folder") until the
  // user types a name.
  let nameTouched = $state(false);
  function nameFor(paths: string[]): string | null {
    const first = paths[0]?.replace(/\/+$/, "");
    if (first == null) return null;
    const base = first === "" ? "Whole computer" : first === "~" || /^\/home\/[^/]+$/.test(first) || first === "/root" ? "Home folder" : first.split("/").at(-1)!;
    const name = base[0]!.toUpperCase() + base.slice(1);
    return paths.length > 1 ? `${name} and ${paths.length - 1} more` : name;
  }
  $effect(() => {
    if (!creating || nameTouched || !draft) return;
    const n = nameFor(draft.sources.paths);
    if (n && n !== draft.name) draft.name = n;
  });

  // A new profile gets an id from its name until the user edits the id.
  let idTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    if (!creating || idTouched || !draft) return;
    const name = draft.name;
    clearTimeout(idTimer);
    idTimer = setTimeout(() => api.suggestId(name, sys).then((s) => (profileId = s)), 200);
  });

  // ---- pages ----
  // The main page asks the three questions; every further group of settings is a row
  // that opens just that group (docs/UX_PRINCIPLES.md, rule 7). Nothing stays "expanded":
  // the editor always opens on the main page.
  type Page = "exclusions" | "storage" | "security" | "connection" | "network" | "schedule" | "retention" | "tests" | "webhooks" | "hooks" | "options";
  const demoPage = untrack(() => (!inTauri && typeof location !== "undefined" ? new URLSearchParams(location.search).get("page") : null));
  // Another view may ask for one group (e.g. a notice's "Change the size limit").
  const askedPage = untrack(() => {
    const p = app.settingsPage;
    app.settingsPage = null;
    return p;
  });
  let page = $state<Page | null>(((askedPage ?? demoPage) as Page | null) ?? null);
  const pageTitle: Record<Page, { title: string; text: string }> = {
    exclusions: { title: "Leave some things out", text: "Files and folders that aren't worth backing up: caches, downloads, build output." },
    storage: { title: "How backups are stored", text: "Flux's own formats, or a standard tool's so the backups work without Flux." },
    security: { title: "Security", text: "How well the Vault's password resists guessing, an optional unlock file, and what the app and the drive give away." },
    connection: { title: "Connection details", text: "How Flux reaches the server. Empty fields use your ssh settings." },
    network: { title: "Network", text: "When backups to a server may use the network, and how fast they may upload." },
    webhooks: { title: "Notify elsewhere", text: "Tell your phone, a chat or a monitoring service how this backup goes (besides the webhooks in Preferences, for every backup)." },
    tests: { title: "Restore tests", text: "Now and then, restore some files into a temporary folder and check them against the backup: proof it can be read back." },
    schedule: { title: "More schedule options", text: "Other timings, when to wait, and upkeep after scheduled backups." },
    retention: { title: "Keep and remove snapshots", text: "Which snapshots stay, and for how long." },
    hooks: { title: "Hooks", text: "Shell commands run around each backup, one per line. A failing “before” command cancels the backup." },
    options: { title: "Performance and behaviour", text: "What's copied along with files, how hard Flux works, and what it tells you." },
  };
  /** The page a setting lives on (`null`: the main page). */
  function pageOf(field: string): Page | null {
    if (/^target\.(port|identity_file|ssh_options|bandwidth_limit)/.test(field)) return "connection";
    if (/^vault\.(hashing|hide_sizes|lock_after_minutes|unlock_file)/.test(field)) return "security";
    if (/^(vault|borg|rsync)/.test(field)) return "storage";
    if (/^sources\.(exclude|exclude_larger_than|exclude_caches|exclude_if_present|one_file_system)/.test(field)) return "exclusions";
    if (/^schedule\.(catch_up|skip_on_battery|every)/.test(field)) return "schedule";
    if (field.startsWith("retention")) return "retention";
    if (field.startsWith("hooks")) return "hooks";
    if (field.startsWith("network")) return "network";
    if (field.startsWith("restore_test")) return "tests";
    if (field.startsWith("webhooks")) return "webhooks";
    if (field.startsWith("options")) return "options";
    return null;
  }
  /** The destination drive isn't plugged in (a warning, not an error). */
  const away = $derived(issues.find((i) => i.field === "target.connected") ?? null);
  const totalErrors = $derived(issues.filter((i) => i.severity === "error").length);
  const at = (prefix: string) => issues.filter((i) => i.field === prefix || i.field.startsWith(prefix + "[") || i.field.startsWith(prefix + "."));
  const general = $derived(issues.filter((i) => i.field === "" || i.field === "name"));
  /** "1 problem" for a page's row, if it has any. */
  function problems(p: Page): string {
    const n = issues.filter((i) => i.severity === "error" && pageOf(i.field) === p).length;
    return n ? (n === 1 ? "1 problem" : `${n} problems`) : "";
  }

  let scroller: HTMLElement | undefined = $state();
  function sectionOf(field: string): string {
    if (field.startsWith("target")) return "destination";
    if (field.startsWith("sources")) return "sources";
    if (field.startsWith("schedule")) return "schedule";
    return "general";
  }
  /** Show a problem: open its page, or scroll to its section on the main page. */
  async function jumpTo(field: string) {
    if (creating) {
      const s = stepOf(field);
      dir = -1;
      step = s;
    }
    page = pageOf(field);
    await tick();
    if (!page) scroller?.querySelector(`#sec-${sectionOf(field)}`)?.scrollIntoView({ behavior: "smooth", block: "start" });
  }
  function openPage(p: Page | null) {
    page = p;
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
  function setEngine(engine: Engine) {
    if (!draft) return;
    draft.engine = engine;
    // Root has no desktop keyring: system Vaults and Borg repositories get their password
    // once, then keep it in a root-only key file (see PasswordDialog).
    const source: PasswordSource = { from: sys ? "prompt" : "keyring" };
    if (engine === "vault" && !draft.vault) draft.vault = { compression_level: 3, encryption: true, password_source: source };
    if (engine === "borg" && !draft.borg) draft.borg = { encryption: "repokey-blake2", compression: "zstd,3", password_source: source, remote_path: null, extra_args: [] };
    // Settings of other engines would only produce "ignored" warnings.
    if (engine !== "vault") draft.vault = null;
    if (engine !== "borg") draft.borg = null;
    if (engine !== "rsync") draft.rsync = null;
    else draft.rsync ??= { extra_args: [] };
  }
  /** Set the key file path or the command of the current password source. */
  function setSourceField(key: "path" | "command", value: string) {
    const src = draft?.borg?.password_source ?? draft?.vault?.password_source;
    if (src && key in src) (src as Record<string, string>)[key] = value;
  }
  function setPasswordSource(from: string) {
    const src: PasswordSource =
      from === "file" ? { from: "file", path: "" } : from === "command" ? { from: "command", command: "" } : from === "prompt" ? { from: "prompt" } : { from: "keyring" };
    if (draft?.borg) draft.borg.password_source = src;
    else if (draft?.vault) draft.vault.password_source = src;
  }
  // ---- sources ----
  let suggestions = $state<Suggestion[]>([]);
  $effect(() => {
    if (!draft) return;
    const paths = [...draft.sources.paths];
    api.excludeSuggestions(paths).then((s) => (suggestions = s), () => {});
  });

  // How much that is, for a new backup: counted again shortly after the folders or
  // exclusions change (a newer count stops the older one in the backend).
  let estimate = $state<Estimate | null>(null);
  let estimating = $state(false);
  let estimateError = $state<string | null>(null);
  /** Bumped by the refresh button: count again with nothing changed (files on disk may
   * have, and exclusion pages don't always change the folders). */
  let recount = $state(0);
  let lastRecount = 0;
  /** What the estimate depends on: the folders and exclusions, and the destination only
   * when it lies inside a source (it's left out then). A `$derived` string, so picking a
   * drive elsewhere, or typing a name, doesn't recount the whole tree. */
  const estimateKey = $derived.by(() => {
    if (!creating || !draft) return null;
    const src = draft.sources;
    const dest = draft.target.kind === "local" ? draft.target.path.replace(/\/+$/, "") : "";
    const inside =
      !!dest &&
      src.paths.some((p) => {
        const s = p.replace(/\/+$/, "");
        return s === "" || dest === s || dest.startsWith(`${s}/`);
      });
    return JSON.stringify([src, inside ? dest : null]);
  });
  $effect(() => {
    if (estimateKey == null) return;
    // A click counts at once; edits wait a moment for more typing.
    const now = recount !== lastRecount;
    lastRecount = recount;
    const snap = untrack(() => JSON.parse(JSON.stringify(draft)) as Profile);
    if (!snap.sources.paths.length) {
      estimate = null;
      estimating = false;
      return;
    }
    estimating = true;
    estimateError = null;
    let current = true;
    const timer = setTimeout(() => {
      api.estimate(snap, sys).then(
        (e) => current && ((estimate = e), (estimating = false)),
        (e) => {
          if (!current || (e instanceof ApiError && e.code === "cancelled")) return;
          estimating = false;
          estimateError = (e as Error).message;
        },
      );
    }, now ? 0 : 700);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  });

  // ---- performance and behaviour: back to the recommended settings ----
  /** What a new profile of this scope starts with. */
  let defaultOptions = $state<Profile["options"] | null>(null);
  $effect(() => {
    if (page !== "options" || defaultOptions) return;
    api.newProfile(sys).then((e) => (defaultOptions = e.profile.options), () => {});
  });
  const optionsAreDefault = $derived.by(() => {
    if (!draft || !defaultOptions) return false;
    const d = defaultOptions as Record<string, unknown>;
    const o = draft.options as Record<string, unknown>;
    return Object.keys(d).every((k) => o[k] === d[k]);
  });
  function useRecommendedOptions() {
    if (!draft || !defaultOptions) return;
    draft.options = { ...draft.options, ...structuredClone($state.snapshot(defaultOptions)) };
    app.toast("info", creating ? "Recommended settings restored." : "Recommended settings restored. Click Save to keep them.");
  }

  // ---- schedule ----
  type Preset = "hourly" | "daily" | "weekdays" | "weekly" | "custom";
  const days = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
  let preset = $state<Preset>("daily");
  let timeOfDay = $state("20:00");
  let weekday = $state("Sun");
  let everyN = $state(4);
  let everyUnit = $state<IntervalUnit>("h");
  /** The interval is edited as text: the controls can't show it. */
  let everyAsText = $state(false);
  /** The controls' values as last read from, or written to, the profile. */
  let everyShown = "";
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
    // The whole interval, as stored ("1h 30m" for 90 minutes): never just its first part.
    const every = draft.schedule.every ?? "";
    const parts = intervalParts(every);
    if (parts) (everyN = parts.n), (everyUnit = parts.unit);
    // One the number-and-unit control can't show (seconds, say) is shown as it is.
    everyAsText = every.trim() !== "" && !parts;
    everyShown = `${everyN}|${everyUnit}`;
    // An interval schedule without an interval: what the controls show is what it gets.
    if (draft.schedule.trigger === "interval" && !every.trim()) applyEvery();
  });

  function applyCalendar() {
    if (!draft || preset === "custom") return;
    const t = `${timeOfDay}:00`;
    draft.schedule.on_calendar =
      preset === "hourly" ? "hourly" : preset === "daily" ? `*-*-* ${t}` : preset === "weekdays" ? `Mon..Fri *-*-* ${t}` : `${weekday} *-*-* ${t}`;
  }
  function applyEvery() {
    if (!draft) return;
    draft.schedule.every = intervalText(everyN, everyUnit);
    everyShown = `${everyN}|${everyUnit}`;
    everyAsText = false;
  }
  // Keep `every` in step with the interval controls: only when they were changed. Just
  // opening the settings must never rewrite the interval.
  $effect(() => {
    const now = `${everyN}|${everyUnit}`;
    if (draft?.schedule.trigger === "interval" && scheduleParsed && now !== everyShown) applyEvery();
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
  const encrypted = $derived(
    draft?.engine === "vault" ? (draft.vault?.encryption ?? true) : draft?.engine === "borg" ? (draft.borg?.encryption ?? "repokey-blake2") !== "none" : false,
  );
  // ---- the password, set while setting up (not at the first backup) ----
  let pw = $state("");
  let pw2 = $state("");
  const pwSource = $derived<PasswordSource | null>(draft?.engine === "vault" ? (draft.vault?.password_source ?? { from: "keyring" }) : draft?.engine === "borg" ? (draft.borg?.password_source ?? null) : null);
  const pwWord = $derived(draft?.engine === "borg" ? "passphrase" : "password");
  const wasEncrypted = $derived.by(() => {
    if (creating || !initial) return false;
    const before = JSON.parse(initial) as Profile;
    return before.engine === draft?.engine && (before.engine === "vault" ? before.vault?.encryption !== false : before.engine === "borg" ? before.borg?.encryption !== "none" : false);
  });
  /** It has never made a backup, so its encrypted repository may not exist yet. */
  const neverRan = $derived(!creating && !!id && app.profiles.find((x) => x.id === id)?.last_run == null);
  /** Ask for the password here: a new encrypted backup, encryption just turned on, or one
   * that hasn't run yet. A command supplies its own. */
  const askPassword = $derived(encrypted && pwSource != null && pwSource.from !== "command" && (!wasEncrypted || neverRan));
  /** Must be typed before saving (for a file, it may already hold one). */
  const pwRequired = $derived(askPassword && !wasEncrypted && (pwSource?.from === "keyring" || pwSource?.from === "prompt"));
  const pwIssue = $derived.by(() => {
    if (!askPassword) return null;
    if (!pw && !pw2) return pwRequired ? `choose a ${pwWord}` : null;
    if (pw !== pw2) return `the two ${pwWord}s don't match`;
    return null;
  });

  function setEncrypted(on: boolean) {
    if (draft?.engine === "rsync") return;
    if (draft?.engine === "borg") {
      if (draft.borg) draft.borg.encryption = on ? "repokey-blake2" : "none";
      return;
    }
    setEngine(on ? "vault" : "mirror");
    if (on && draft?.vault) draft.vault.encryption = true;
  }
  const encryptText = $derived.by(() => {
    if (!draft) return "";
    if (draft.engine === "rsync") return "rsync writes plain folders and can't encrypt them. For private files, back up to an encrypted drive (LUKS).";
    if (draft.engine === "borg")
      return encrypted
        ? "Borg encrypts the backups with your passphrase; they're also compressed and deduplicated."
        : "Borg compresses and deduplicates the backups, without encryption.";
    return encrypted ? `Nobody can read the backups without the password; they're also compressed and deduplicated.` : "Backups are plain folders you can open in any file manager.";
  });
  /** The storage format differs from the saved one (a new backup history). */
  const engineChanged = $derived(!creating && !!initial && (JSON.parse(initial) as Profile).engine !== draft?.engine);
  /** Encryption is fixed once a Borg repository exists; rsync can't encrypt. */
  const encryptLocked = $derived(draft?.engine === "rsync" || (!creating && draft?.engine === "borg" && !engineChanged));
  const encryptReason = $derived(
    draft?.engine === "rsync" ? "rsync can't encrypt; back up to an encrypted drive instead" : "Fixed when the Borg repository was created",
  );

  // ---- one-line summaries for the rows ----
  const listSummary = (xs: string[], none: string) => (xs.length === 0 ? none : xs.length <= 2 ? xs.join(", ") : `${xs.slice(0, 2).join(", ")} and ${xs.length - 2} more`);
  const exclusionsSummary = $derived.by(() => {
    if (!draft) return "";
    const s = draft.sources;
    const parts = [s.exclude.length ? listSummary(s.exclude, "") : "", s.exclude_caches ? "caches" : "", s.exclude_larger_than ? `files over ${sizeLabel(s.exclude_larger_than)}` : ""].filter(Boolean);
    return parts.length ? parts.join(" · ") : "Nothing left out";
  });
  const storageSummary = $derived.by(() => {
    if (!draft) return "";
    switch (draft.engine) {
      case "mirror":
        return "Plain folders";
      case "vault":
        return draft.vault?.encryption === false ? "Compressed (Vault)" : "Encrypted (Vault)";
      case "rsync":
        return "rsync · works without Flux";
      case "borg":
        return "Borg · works without Flux";
    }
  });
  const connectionSummary = $derived.by(() => {
    if (draft?.target.kind !== "ssh") return "";
    const t = draft.target;
    const parts = [t.port ? `port ${t.port}` : "", t.identity_file ? "own key file" : "", t.bandwidth_limit ? `up to ${t.bandwidth_limit}/s` : ""].filter(Boolean);
    return parts.length ? parts.join(" · ") : "Your ssh settings";
  });
  const scheduleSummary = $derived.by(() => {
    if (!draft) return "";
    const sc = draft.schedule;
    if (sc.trigger === "manual") return "Only when you click";
    const parts = [sc.trigger === "interval" ? `every ${sc.every}` : preset === "custom" ? "custom times" : preset === "weekdays" ? "weekdays" : "", sc.trigger === "calendar" && sc.catch_up ? "catches up" : "", sc.skip_on_battery ? "waits for power" : "", sc.skip_on_metered && draft.target.kind === "ssh" ? "not on metered" : "", draft.maintenance?.verify_monthly ? "verifies monthly" : ""].filter(Boolean);
    return parts.join(" · ") || "Standard";
  });
  /** The backup, or one of its places, is on a server. */
  const usesServer = $derived(!!draft && (draft.target.kind === "ssh" || (draft.destinations ?? []).some((d) => d.target.kind === "ssh")));
  const networkSummary = $derived.by(() => {
    const n = draft?.network;
    const parts: string[] = [];
    if (n?.wifi_only?.length) parts.push(`only on ${n.wifi_only.join(", ")}`);
    if (n?.wifi_never?.length) parts.push(`never on ${n.wifi_never.join(", ")}`);
    if (n?.bandwidth?.length) parts.push(`${n.bandwidth.length} upload limit${n.bandwidth.length === 1 ? "" : "s"}`);
    return parts.join(" · ") || "Any network, full speed";
  });
  const testsSummary = $derived.by(() => {
    const t = draft?.restore_test;
    const every = t?.every ?? "off";
    if (every === "off") return "Only when you ask";
    const what = t?.mode === "full" ? "everything" : t?.mode === "percent" ? `${t.percent ?? 5}% of it` : t?.mode === "rolling" ? "every file once a month" : `${t?.files ?? 10} files`;
    return `${every === "backup" ? "After every backup" : every === "week" ? "Weekly" : "Monthly"} · ${what}`;
  });
  function setTest(patch: NonNullable<Profile["restore_test"]>) {
    if (draft) draft.restore_test = { ...(draft.restore_test ?? {}), ...patch };
  }
  /** Change `[network]` (created on the first change, so an untouched profile stays as it was). */
  function setNet(patch: NonNullable<Profile["network"]>) {
    if (draft) draft.network = { ...(draft.network ?? {}), ...patch };
  }
  const hooksSummary = $derived.by(() => {
    const n = (draft?.hooks?.pre_backup?.length ?? 0) + (draft?.hooks?.post_backup?.length ?? 0) + (draft?.hooks?.on_error?.length ?? 0);
    return n === 0 ? "None" : n === 1 ? "1 command" : `${n} commands`;
  });
  const optionsSummary = $derived.by(() => {
    if (!draft) return "";
    const o = draft.options;
    const pri = o.io_priority === "idle" ? "idle disk" : o.io_priority === "normal" ? "normal priority" : "low priority";
    const extra = o.change_detection === "checksum" ? "checksums" : o.preserve_acl && o.preserve_xattr ? "keeps all attributes" : "some attributes off";
    return `${pri} · ${extra}`;
  });
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

  // ---- the setup wizard (new backups): one question per step ----
  type Step = "what" | "where" | "protect" | "when" | "review";
  const steps: { id: Step; label: string; title: string; text: string }[] = [
    { id: "what", label: "What", title: "What should Flux back up?", text: "Pick the folders that matter. Everything inside them is included, except what you leave out." },
    { id: "where", label: "Where", title: "Where should the backups go?", text: "A drive you plug in, a folder, or a server on your network. Best on a different disk than the files themselves." },
    { id: "protect", label: "Protection", title: "How should they be stored?", text: "Plain folders are the simplest. Encryption keeps them private if the drive is lost. Or let a standard tool write them, so they never depend on Flux." },
    { id: "when", label: "When", title: "When should it back up?", text: "Flux backs up in the background, even with the app closed, and only copies what changed." },
    { id: "review", label: "Review", title: "", text: "Check the details. Everything can be changed later in Settings." },
  ];
  let step = $state<Step>("what");
  /** The furthest step reached (earlier ones can be revisited from the stepper). */
  let reached = $state(0);
  /** 1 forward, -1 back: which way the next step slides in. */
  let dir = $state(1);
  const stepIndex = $derived(steps.findIndex((s) => s.id === step));
  /** Set once the backup is created: the closing screen. */
  let done = $state<string | null>(null);
  let encReady = $state(false);

  function stepOf(field: string): Step {
    if (field.startsWith("sources")) return "what";
    if (field.startsWith("target")) return "where";
    if (/^(vault|borg|rsync|engine)/.test(field)) return "protect";
    if (field.startsWith("schedule") || field.startsWith("retention")) return "when";
    return "review";
  }
  /** What still stops this step, in words (null: ready). */
  function stepIssue(s: Step): string | null {
    if (!draft) return null;
    if (s === "what" && !draft.sources.paths.length) return "add a folder";
    if (s === "where" && draft.target.kind === "ssh" && !draft.target.host.trim()) return "type the server's name";
    const e = issues.find((i) => i.severity === "error" && stepOf(i.field) === s);
    if (e) return e.message;
    if (s === "protect" && encrypted && pwSource?.from === "file" && !pwSource.path.trim()) return "choose the key file";
    if (s === "protect" && encrypted && pwSource?.from === "command" && !pwSource.command.trim()) return "type the command that prints the " + pwWord;
    if (s === "protect" && pwIssue) return pwIssue;
    if (s === "review") {
      for (const other of steps.slice(0, -1)) {
        const x = stepIssue(other.id);
        if (x) return x;
      }
      if (!profileId) return "give it a name";
    }
    return null;
  }
  async function go(to: Step) {
    const i = steps.findIndex((s) => s.id === to);
    dir = i >= stepIndex ? 1 : -1;
    step = to;
    page = null;
    reached = Math.max(reached, i);
    await tick();
    scroller?.scrollTo({ top: 0 });
    scroller?.querySelector<HTMLElement>("#step-title")?.focus({ preventScroll: true });
  }
  const next = () => stepIndex < steps.length - 1 && !stepIssue(step) && void go(steps[stepIndex + 1]!.id);
  const back = () => stepIndex > 0 && void go(steps[stepIndex - 1]!.id);

  // The review's one-line answers.
  const whatSummary = $derived.by(() => {
    if (!draft) return "";
    const s = listSummary(draft.sources.paths, "Nothing yet");
    return estimate && !estimateError ? `${s} · about ${bytes(estimate.bytes)}` : s;
  });
  const whereSummary = $derived.by(() => {
    if (!draft) return "";
    const t = draft.target;
    if (t.kind === "ssh") return t.host ? `${t.user ? `${t.user}@` : ""}${t.host}:${t.path}` : "Not chosen yet";
    const d = drives.find((x) => x.mount_point === t.path);
    return d ? `${d.label} · ${bytes(d.free)} free` : t.path || "Not chosen yet";
  });
  const securitySummary = $derived.by(() => {
    const v = draft?.vault;
    if (!v) return "";
    const h = v.hashing?.level ?? "standard";
    const parts = [h === "standard" ? "Standard hashing" : h === "custom" ? "Custom hashing" : `${h[0]!.toUpperCase()}${h.slice(1)} hashing`];
    if (v.unlock_file) parts.push("unlock file");
    if (v.hide_sizes) parts.push("sizes hidden");
    if (v.lock_after_minutes) parts.push(`locks after ${v.lock_after_minutes} min`);
    return parts.join(" · ");
  });
  // Destinations are added and removed at once (they may need a password), and the
  // editor takes the saved list as its own.
  let addingDest = $state(false);
  let removingDest = $state<string | null>(null);
  function syncDestinations(saved: Profile) {
    if (!draft) return;
    draft.destinations = saved.destinations ?? [];
    const base = JSON.parse(initial) as Profile;
    base.destinations = saved.destinations ?? [];
    initial = JSON.stringify(base);
    void app.refresh();
  }
  async function removeDest(name: string) {
    if (!id) return;
    removingDest = name;
    try {
      syncDestinations(await api.removeDestination(id, name));
      app.toast("ok", "Removed. The backups already made there stay where they are.");
    } catch (e) {
      app.toast("bad", (e as Error).message);
    } finally {
      removingDest = null;
    }
  }
  /** The Vault changed and the saved profile with it (Security): take the values as saved,
   * so they neither count as unsaved changes nor get overwritten by an older draft. */
  function syncSaved(fields: Partial<NonNullable<Profile["vault"]>>) {
    if (!draft?.vault) return;
    Object.assign(draft.vault, fields);
    const base = JSON.parse(initial) as Profile;
    if (base.vault) Object.assign(base.vault, fields);
    initial = JSON.stringify(base);
  }
  const sourceWords: Record<string, string> = { keyring: "kept in your desktop keyring", prompt: "asked every time", file: "kept in a key file", command: "from your command" };
  const protectSummary = $derived.by(() => {
    if (!draft) return "";
    const how = storageSummary;
    if (!encrypted) return how;
    return `${how} · ${pwWord} ${pw ? "set, " : ""}${sourceWords[pwSource?.from ?? "keyring"]}`;
  });
  const whenSummary = $derived.by(() => {
    if (!draft) return "";
    const base =
      when === "manual" ? "Only when you click Back up now" : when === "hourly" ? "Every hour" : when === "daily" ? `Every day at ${timeOfDay}` : when === "weekly" ? `Every ${weekday} at ${timeOfDay}` : scheduleSummary;
    return draft.schedule.on_connect ? `${base}, and when the drive is plugged in` : base;
  });

  /** The closing screen's "Back up now": open the new backup and start its first run. */
  function startFirst() {
    if (!done) return;
    const created = done;
    onsaved(created);
    void app.backupNow(created);
  }

  async function save() {
    if (!draft) return;
    saving = true;
    try {
      const r = await api.saveProfile(creating ? (sys ? SYSTEM_PREFIX : "") + profileId : id!, forSaving($state.snapshot(draft) as Profile), creating);
      issues = r.issues;
      if (r.saved) {
        if (!creating) app.toast("ok", `Saved “${draft.name}”.`);
        // Set up encryption now, so the first backup (even a scheduled one) asks nobody.
        if (askPassword && (pw || (creating && pwSource?.from === "file"))) {
          try {
            await api.setPassword(r.id, pw);
            if (creating) encReady = true;
            else app.toast("ok", draft.engine === "borg" ? "The Borg repository is ready, protected by your passphrase." : "The encrypted Vault is ready, protected by your password.");
          } catch (e) {
            app.toast("bad", `Saved, but encryption couldn't be set up yet: ${(e as Error).message} Flux will ask for the ${pwWord} at the first backup.`);
          }
          pw = pw2 = "";
        } else if (encrypted && creating && pwSource?.from === "command") {
          try {
            await api.setPassword(r.id, "");
          } catch (e) {
            app.toast("info", `Saved. Encryption will be set up at the first backup: ${(e as Error).message}`);
          }
        }
        if (r.schedule && !creating) app.toast("info", r.schedule);
        initial = JSON.stringify(draft);
        await app.refresh();
        if (creating) {
          // The wizard ends on its own screen: what happens next, and the first backup.
          done = r.id;
          dir = 1;
          await tick();
          scroller?.scrollTo({ top: 0 });
        } else onsaved(r.id);
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


  const presetLabel: Record<Preset, string> = { hourly: "Every hour", daily: "Every day", weekdays: "Weekdays", weekly: "Once a week", custom: "Custom" };
</script>

{#if loadError}
  <div class="card m-8 flex items-center gap-3 p-5 text-danger"><Icon name="alert" /> {loadError}</div>
{:else if !draft}
  <div class="mx-auto mt-4 h-64 w-full max-w-[680px] rounded-card bg-surface-2 motion-safe:animate-pulse"></div>
{:else}
  {#snippet heading(title: string, sub = "")}
    <div>
      <h2 class="text-[18px] font-medium">{title}</h2>
      {#if sub}<p class="mt-0.5 text-[13px] text-fg-muted">{sub}</p>{/if}
    </div>
  {/snippet}
  {#snippet choice(selected: boolean, label: string, onpick: () => void)}
    <button role="radio" aria-checked={selected} onclick={onpick}>{label}</button>
  {/snippet}
  <!-- A row that opens a page: its current value, or its problems in red. -->
  {#snippet pageRow(p: Page, icon: IconName, value: string, subtitle = "")}
    {@const bad = problems(p)}
    <Row title={pageTitle[p].title} {subtitle} {icon} value={bad || value} tone={bad ? "danger" : "normal"} onclick={() => openPage(p)} />
  {/snippet}

  <div class="flex h-full min-h-0 flex-col">
    <div bind:this={scroller} class="min-h-0 flex-1 overflow-y-auto px-10 pb-12">
      <div class="mx-auto flex max-w-[680px] flex-col gap-7 pt-2">
          <!-- ===================== The questions (shared by Settings and the wizard) ===================== -->
          {#each general.filter((i) => i.field === "") as i (i.message)}
            <div class="rounded-card bg-danger/10 px-4 py-3 text-sm text-danger">{i.message}</div>
          {/each}

          {#snippet nameGroup()}
            <section id="sec-general">
              <div class="rows">
                <label class="row gap-3">
                  <span class="w-20 shrink-0 font-medium">Name</span>
                  <input id="f-name" class="input flex-1" bind:value={draft!.name} oninput={() => (nameTouched = true)} placeholder="Home folder" />
                </label>
                {#if editingId}
                  <label class="row gap-3">
                    <span class="w-20 shrink-0 font-medium">Id</span>
                    <input id="f-id" class="input flex-1 font-mono text-[13px]" bind:value={profileId} oninput={() => (idTouched = true)} />
                  </label>
                {/if}
              </div>
              {#each at("name") as i (i.message)}<p class="mt-1.5 px-0.5 text-[12.5px] {i.severity === 'error' ? 'text-danger' : 'text-warning'}">{i.message}</p>{/each}
              <p class="mt-1.5 flex items-center gap-1.5 px-0.5 text-[12.5px] text-fg-muted">
                {#if editingId}
                  The id names its files and timers; it can't be changed later.
                {:else}
                  Saved as <span class="font-mono text-fg-2">{profileId || "…"}</span>
                  {#if creating}<button class="font-medium text-primary hover:underline" onclick={() => (editingId = true)}>Change</button>{/if}
                {/if}
              </p>
            </section>
          {/snippet}


          {#snippet sourcesAfter()}
            {@render pageRow("exclusions", "filter", exclusionsSummary)}
          {/snippet}

          {#snippet whatBody()}
            {#if draft}
            <ListEditor bind:items={draft.sources.paths} placeholder="/home/you" icon="folder" label="Folders to back up" pick={() => pickFolder("Add a folder to back up")} after={sourcesAfter} />
            {#each at("sources.paths") as i (i.message)}<p class="mt-1.5 px-0.5 text-[12.5px] {i.severity === 'error' ? 'text-danger' : 'text-warning'}">{i.message}</p>{/each}
            {@const vols = (draft.sources.volumes ?? []).filter((v) => draft!.sources.paths.includes(v.path))}
            {#if vols.length}
              <div class="rows mt-3">
                {#each vols as v (v.path)}
                  <Row
                    title={`${v.path.split("/").filter(Boolean).pop() ?? v.path} is on a drive of its own`}
                    subtitle={v.when_missing === "skip"
                      ? (draft.engine === "vault" || (draft.engine === "mirror" && draft.target.kind === "local") ? "If it isn't connected, the backup keeps its files as they were last time, and says so." : "If it isn't connected, the backup leaves it out, and says so. Earlier snapshots still have it.")
                      : v.when_missing === "wait"
                        ? "If it isn't connected, the backup waits up to an hour for it, then fails."
                        : "If it isn't connected, the backup fails, so you notice."}
                    icon="drive"
                  >
                    <select
                      class="input w-auto min-w-48"
                      value={v.when_missing}
                      onchange={(e) => {
                        v.when_missing = e.currentTarget.value as WhenMissing;
                        if (v.when_missing !== "wait") v.wait_for = null;
                      }}
                      aria-label={`When ${v.path} isn't connected`}
                    >
                      <option value="skip">Back up the rest</option>
                      <option value="wait">Wait for it</option>
                      <option value="fail">Fail the backup</option>
                    </select>
                  </Row>
                {/each}
              </div>
              <p class="mt-1.5 px-0.5 text-[12.5px] text-fg-muted">Flux recognises these drives, so an unplugged one is never mistaken for deleted files.</p>
            {/if}
            {#if creating && draft.sources.paths.length}
              <div class="mt-2 flex items-start gap-2 px-0.5 text-[13px] text-fg-2" aria-live="polite">
                <Icon name="gauge" size={15} class="mt-0.5 shrink-0 text-fg-muted" />
                <span class="min-w-0 flex-1">
                  {#if estimateError}
                    <span class="text-fg-muted">Couldn't count the files: {estimateError}</span>
                  {:else if !estimate}
                    <span class="text-fg-muted motion-safe:animate-pulse">Counting what that adds up to…</span>
                  {:else}
                    {@const big = estimate.free != null && estimate.bytes > estimate.free}
                    <span class={estimating ? "opacity-60" : ""}>
                      About <span class="num font-semibold text-brand">{bytes(estimate.bytes)}</span> in <span class="num font-semibold text-brand">{count(estimate.files)}</span> files, after the exclusions.
                      {#if draft.engine === "vault" || draft.engine === "borg"}Compression usually makes the first backup smaller.{:else}The first backup copies all of it; later ones only what changed.{/if}
                      {#if big}<span class="block text-warning">That's more than the {bytes(estimate.free)} free where it would go.</span>{/if}
                      {#if estimate.unreadable}<span class="block text-fg-muted">{count(estimate.unreadable)} items can't be read as {sys ? "root" : "you"} and would be skipped.</span>{/if}
                      {#if estimate.missing.length}<span class="block text-warning">Not found: {estimate.missing.join(", ")}</span>{/if}
                      {#if estimating}<span class="text-fg-muted"> Counting again…</span>{/if}
                    </span>
                  {/if}
                </span>
                <button
                  class="btn-icon -mt-1 size-7 shrink-0"
                  onclick={() => recount++}
                  disabled={estimating}
                  aria-label="Count again"
                  title="Count again (after changing what's left out, or files on disk)"
                ><Icon name="refresh" size={14} class={estimating ? "motion-safe:animate-spin" : ""} /></button>
              </div>
            {/if}
            {/if}
          {/snippet}

          {#snippet whereKind()}
            {#if draft}
              <div class="segmented" role="radiogroup" aria-label="Destination type">
                {@render choice(draft.target.kind === "local", "A drive or folder", () => setKind("local"))}
                {@render choice(draft.target.kind === "ssh", "A server (SSH)", () => setKind("ssh"))}
              </div>
            {/if}
          {/snippet}

          {#snippet whereList()}
            {#if draft}
            {#if draft.target.kind === "local"}
              {@const target = draft.target}
              {@const custom = target.path && !drives.some((d) => d.mount_point === target.path)}
              <!-- A backup that's set up shows its place; every drive only when changing it. -->
              {@const all = creating || changingWhere || !target.path}
              <div class="rows" role="radiogroup" aria-label="Where to keep it">
                {#each drives.filter((d) => all || target.path === d.mount_point) as d (d.mount_point)}
                  {@const chosen = target.path === d.mount_point}
                  <button role="radio" aria-checked={chosen} class="row row-link" onclick={() => useDrive(d)}>
                    <Icon name="drive" size={17} class="shrink-0 text-fg-2" />
                    <span class="min-w-0 flex-1">
                      <span class="block truncate font-medium">{d.label}</span>
                      {#if d.total && d.free != null}
                        <span class="block truncate font-mono text-[12px] text-fg-muted">{d.mount_point}</span>
                        <span class="mt-1.5 block max-w-sm"><StorageMeter free={d.free} size={d.total} compact label="Space used on {d.label}" /></span>
                      {:else}
                        <span class="num block truncate text-[12.5px] text-fg-muted">{bytes(d.free)} free · <span class="font-mono">{d.mount_point}</span></span>
                      {/if}
                    </span>
                    {#if chosen}<Icon name="check" size={18} class="shrink-0 text-fg" />{/if}
                  </button>
                {/each}
                {#if custom}
                  <div class="row" role="radio" aria-checked="true">
                    <Icon name="folder" size={17} class="shrink-0 text-fg-2" />
                    <span class="selectable min-w-0 flex-1 truncate font-mono text-[13px]" title={target.path}>{target.path}</span>
                    <Icon name="check" size={18} class="shrink-0 text-fg" />
                  </div>
                {:else if !drives.length && !target.path}
                  <div class="row text-[13.5px] text-fg-muted">No drive connected. Plug one in, or choose a folder.</div>
                {/if}
                {#if all}
                  <button class="row row-link text-fg-2" onclick={chooseFolder}>
                    <Icon name="folder" size={17} class="shrink-0" />
                    <span class="flex-1 font-medium">{drives.length || custom ? "Another folder…" : "Choose a folder…"}</span>
                  </button>
                {:else}
                  <button class="row row-link text-fg-2" onclick={() => (changingWhere = true)}>
                    <Icon name="refresh" size={16} class="shrink-0" />
                    <span class="flex-1 font-medium">Change where it's kept…</span>
                  </button>
                {/if}
              </div>
              {#each [...at("target.path"), ...at("target.connected")] as i (i.message)}
                <!-- A new profile without a target yet isn't an error, just the next step. -->
                <p class="mt-1.5 px-0.5 text-[12.5px] first-letter:uppercase {creating && !target.path ? 'text-fg-muted' : i.severity === 'error' ? 'text-danger' : 'text-warning'}">{i.message}</p>
              {/each}
              {#if target.fs_uuid}
                <p class="mt-1.5 px-0.5 text-[12.5px] text-fg-muted">Tied to this drive: Flux won't write into the folder when another disk is mounted there.</p>
              {/if}
            {:else}
              {@const target = draft.target}
              <div class="rows">
                <label class="row gap-3"><span class="w-36 shrink-0 font-medium">Server</span><input id="f-host" class="input flex-1" bind:value={target.host} placeholder="nas.local" /></label>
                <label class="row gap-3"><span class="w-36 shrink-0 font-medium">User</span><input id="f-user" class="input flex-1" value={target.user ?? ""} oninput={(e) => (target.user = opt(e.currentTarget.value))} placeholder="optional" /></label>
                <label class="row gap-3"><span class="w-36 shrink-0 font-medium">Folder on the server</span><input id="f-rpath" class="input flex-1 font-mono text-[13px]" bind:value={target.path} /></label>
                <div class="row gap-3">
                  <span class="min-w-0 flex-1 text-[13px]">
                    {#if test}
                      <span class="block {test.ok ? 'text-success' : 'text-danger'}">{#each test.messages as m (m)}<span class="block">{m}</span>{/each}</span>
                    {:else}
                      <span class="text-fg-muted">Checks that Flux can log in and write there.</span>
                    {/if}
                  </span>
                  <button class="btn shrink-0" onclick={runTest} disabled={testing || !target.host}>{testing ? "Testing…" : "Test connection"}</button>
                </div>
              </div>
              {#each [...at("target.host"), ...at("target.user"), ...at("target.path")] as i (i.message)}
                <!-- Nothing typed yet on a new backup: the next step, not an error. -->
                <p class="mt-1.5 px-0.5 text-[12.5px] first-letter:uppercase {creating && !target.host ? 'text-fg-muted' : i.severity === 'error' ? 'text-danger' : 'text-warning'}">{i.message}</p>
              {/each}
            {/if}

            {/if}
          {/snippet}

          {#snippet protectRows()}
            {#if draft}
            <div class="rows mt-3">
              {#if draft.target.kind === "ssh"}{@render pageRow("connection", "terminal", connectionSummary)}{/if}
              <Toggle row icon="lock" checked={encrypted} onchange={setEncrypted} label="Encrypt with a password" hint={encryptText} disabled={encryptLocked} reason={encryptReason} />
              {#if askPassword}
                <label class="row gap-3">
                  <span class="w-5 shrink-0"></span>
                  <span class="w-32 shrink-0 font-medium capitalize">{pwWord}</span>
                  <input id="f-pw" type="password" class="input flex-1" bind:value={pw} autocomplete="new-password" placeholder={pwRequired ? "" : "optional"} />
                </label>
                <label class="row gap-3">
                  <span class="w-5 shrink-0"></span>
                  <span class="w-32 shrink-0 font-medium">Repeat it</span>
                  <input type="password" class="input flex-1" bind:value={pw2} autocomplete="new-password" />
                </label>
              {/if}
              {@render pageRow("storage", "layers", storageSummary, "Flux's formats, or rsync and Borg so backups work without Flux")}
              {#if draft.engine === "vault" && encrypted && draft.vault}
                {@render pageRow("security", "shield", securitySummary, "Password hashing, an unlock file, hiding sizes")}
              {/if}
            </div>
            {#if askPassword}
              <p class="mt-1.5 px-0.5 text-[12.5px] {pw && pw2 && pw !== pw2 ? 'text-danger' : 'text-fg-muted'}">
                {#if pw && pw2 && pw !== pw2}
                  The two {pwWord}s don't match.
                {:else if pwSource?.from === "keyring"}
                  Flux keeps it in your desktop keyring, so scheduled backups run on their own. Write it down too: you need it to restore on another computer, and if it's lost, so are the backups.
                {:else if pwSource?.from === "prompt"}
                  Flux asks for it each time a backup starts, so scheduled backups wait for you. Choose the desktop keyring under How backups are stored to let them run on their own.
                {:else if pwSource?.from === "file"}
                  Kept in <span class="font-mono">{pwSource.path || "the key file"}</span>. Leave it empty if that file already holds the {pwWord}.
                {/if}
                {#if wasEncrypted && neverRan}<span class="block">It hasn't made its first backup yet: set the {pwWord} now so that backup doesn't have to ask.</span>{/if}
              </p>
            {/if}
            {#if engineChanged}
              <p class="mt-1.5 px-0.5 text-[12.5px] text-warning">This starts a new backup history. Existing snapshots stay where they are, but won't appear here.</p>
            {/if}
            {/if}
          {/snippet}

          {#snippet whenBody()}
            {#if draft}
            <div class="segmented flex-wrap" role="radiogroup" aria-label="How often">
              {@render choice(when === "hourly", "Every hour", () => setWhen("hourly"))}
              {@render choice(when === "daily", "Every day", () => setWhen("daily"))}
              {@render choice(when === "weekly", "Once a week", () => setWhen("weekly"))}
              {@render choice(when === "manual", "Only when I click", () => setWhen("manual"))}
              {#if when === "custom"}{@render choice(true, presetLabel[preset] === "Custom" ? "Custom" : draft.schedule.trigger === "interval" ? `Every ${draft.schedule.every}` : presetLabel[preset], () => openPage("schedule"))}{/if}
            </div>
            <div class="rows mt-3">
              {#if when === "daily" || when === "weekly"}
                <div class="row">
                  <Icon name="clock" size={17} class="shrink-0 text-fg-2" />
                  <span class="flex-1 font-medium">{when === "weekly" ? "Day and time" : "Time"}</span>
                  {#if when === "weekly"}
                    <select class="input w-28" bind:value={weekday} onchange={applyCalendar} aria-label="Day">{#each days as d (d)}<option>{d}</option>{/each}</select>
                  {/if}
                  <input type="time" class="input w-32" bind:value={timeOfDay} onchange={applyCalendar} aria-label="Time" />
                </div>
              {/if}
              {#if draft.target.kind === "local" && draft.target.fs_uuid}
                <Toggle
                  row
                  icon="drive"
                  bind:checked={draft.schedule.on_connect}
                  label="Also when this drive is plugged in"
                  hint="Back up as soon as the drive is connected."
                  disabled={!sys && app.scheduler === "cron" && !draft.schedule.on_connect}
                  reason="Needs systemd timers; scheduled backups run with cron (Preferences › Advanced)."
                />
              {/if}
              {@render pageRow("schedule", "calendar", scheduleSummary)}
            </div>
            {#each at("schedule.on_connect") as i (i.message)}<p class="mt-1.5 px-0.5 text-[12.5px] {i.severity === 'error' ? 'text-danger' : 'text-warning'}">{i.message}</p>{/each}
            {/if}
          {/snippet}

          {#snippet howItRuns()}
            {#if draft}
              {@const sc = draft.schedule}
              {@const folders = draft.sources.paths.length}
              {@const at12 = new Date(`1970-01-01T${timeOfDay}`).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })}
              {@const place = draft.target.kind === "ssh" ? (draft.target.host ? `the server ${draft.target.host}` : "your server") : draft.target.path ? whereSummary.split(" · ")[0] : "your backup drive"}
              <InfoDetails summary="What happens each time it backs up, in plain words">
                <ol class="flex list-decimal flex-col gap-2.5 pl-4 marker:font-semibold marker:text-primary">
                  <li>
                    <span class="font-medium text-fg">
                      {when === "manual" ? "When you click Back up now" : when === "hourly" ? "Every hour" : when === "daily" ? `Every day at ${at12}` : when === "weekly" ? `Every ${weekday} at ${at12}` : `On its schedule (${scheduleSummary.toLowerCase()})`}{sc.on_connect ? ", and as soon as the drive is plugged in" : ""},
                    </span>
                    Flux looks through {folders === 1 ? "your folder" : `your ${folders} folders`} for anything new or changed since the last backup, skipping what you chose to leave out.
                  </li>
                  <li>
                    {#if draft.engine === "vault"}
                      It stores only what's new on {place}, compressed and encrypted. A file that appears in several places, or in several backups, is stored once.
                    {:else if draft.engine === "borg"}
                      Borg stores only what's new on {place}, compressed{encrypted ? " and encrypted" : ""}, as a new archive.
                    {:else}
                      It copies only the new and changed files to {place}. Files that didn't change are linked to the previous backup instead of copied again, so each snapshot is a complete copy you can browse, yet costs almost no extra space.
                    {/if}
                    The result is a <span class="font-medium text-fg">snapshot</span>: your files as they were at that moment.
                  </li>
                  {#if sc.trigger !== "manual"}
                    <li>
                      It runs in the background, even with the app closed.
                      {sc.catch_up ? "If the computer is off or asleep at that time, it backs up the next time it can." : ""}
                      {sc.skip_on_battery ? "On battery power it waits until you plug in." : ""}
                    </li>
                  {/if}
                  <li>{keepSummary} Snapshots you pin or name are never removed.</li>
                  <li>To get something back, open <span class="font-medium text-fg">Snapshots</span>, pick a day, and click <span class="font-medium text-fg">Restore</span> next to the file or folder. Nothing you restore overwrites your files unless you ask it to.</li>
                </ol>
              </InfoDetails>
            {/if}
          {/snippet}

          {#snippet passwordGroup()}
            {#if draft && pwSource}
            <section>
              <h3 class="group-title">{askPassword ? `Choose a ${pwWord}` : `Where the ${pwWord} comes from`}</h3>
              <div class="rows">
                {#if askPassword}
                  <label class="row gap-3">
                    <Icon name="lock" size={17} class="shrink-0 text-fg-2" />
                    <span class="w-28 shrink-0 font-medium capitalize">{pwWord}</span>
                    <input id="f-pw" type="password" class="input flex-1" bind:value={pw} autocomplete="new-password" placeholder={pwRequired ? "" : "optional"} />
                  </label>
                  <label class="row gap-3">
                    <span class="w-[17px] shrink-0"></span>
                    <span class="w-28 shrink-0 font-medium">Repeat it</span>
                    <input type="password" class="input flex-1" bind:value={pw2} autocomplete="new-password" />
                  </label>
                {/if}
                <!-- Just this choice, in place: not the whole "How backups are stored" dialog. -->
                <label class="row gap-3">
                  <Icon name="layers" size={17} class="shrink-0 text-fg-2" />
                  <span class="flex-1 font-medium">Where it's kept</span>
                  <select class="input w-auto min-w-56" value={pwSource.from} onchange={(e) => setPasswordSource(e.currentTarget.value)} aria-label="Where the {pwWord} is kept">
                    <option value="keyring" disabled={sys}>Desktop keyring{sys ? " (not for system)" : ""}</option>
                    <option value="prompt">Ask every time</option>
                    <option value="file">A file</option>
                    <option value="command">A command (e.g. pass)</option>
                  </select>
                </label>
                {#if pwSource.from === "file"}
                  <label class="row gap-3">
                    <span class="w-[17px] shrink-0"></span>
                    <span class="w-28 shrink-0 font-medium">Key file</span>
                    <input class="input flex-1 font-mono text-[13px]" value={pwSource.path} oninput={(e) => setSourceField("path", e.currentTarget.value)} placeholder="~/.config/flux-backup/{profileId || 'backup'}.key" aria-label="Key file" />
                  </label>
                {:else if pwSource.from === "command"}
                  <label class="row gap-3">
                    <span class="w-[17px] shrink-0"></span>
                    <span class="w-28 shrink-0 font-medium">Command</span>
                    <input class="input flex-1 font-mono text-[13px]" value={pwSource.command} oninput={(e) => setSourceField("command", e.currentTarget.value)} placeholder="pass show backup/flux" aria-label="Command" />
                  </label>
                {/if}
              </div>
              <p class="mt-1.5 px-0.5 text-[12.5px] {pw && pw2 && pw !== pw2 ? 'text-danger' : 'text-fg-muted'}">
                {#if pw && pw2 && pw !== pw2}
                  The two {pwWord}s don't match.
                {:else if pwSource.from === "keyring"}
                  Flux keeps it in your desktop keyring, so scheduled backups run on their own. Write it down too: you need it to restore on another computer, and if it's lost, so are the backups.
                {:else if pwSource.from === "prompt"}
                  Flux asks for it each time a backup starts, so scheduled backups wait for you. Keep it in the desktop keyring to let them run on their own.
                {:else if pwSource.from === "file"}
                  Flux writes it to this file (readable only by you) if it doesn't exist yet; leave the {pwWord} empty if the file already holds one.
                {:else}
                  Flux runs this command each time it needs the {pwWord}, and sets up the encryption with it when you create the backup.
                {/if}
              </p>
            </section>
            {/if}
          {/snippet}

          {#if !creating}
            <!-- ===================== Settings: every question on one page ===================== -->
            {@render nameGroup()}
            <section id="sec-sources">
              <h2 class="group-title">What to back up</h2>
              {@render whatBody()}
            </section>
            <section id="sec-destination">
              <div class="flex items-end justify-between gap-3 pb-2">
                <h2 class="group-title pb-0">Where to keep it</h2>
                {#if creating || changingWhere || draft.target.kind === "ssh"}{@render whereKind()}{/if}
              </div>
              {@render whereList()}
              {@render protectRows()}
            </section>
            <section id="sec-copies">
              <h2 class="group-title">Also keep copies elsewhere</h2>
              <div class="rows">
                {#each draft.destinations ?? [] as d (d.name)}
                  <Row
                    title={d.label || d.name}
                    subtitle={`${d.target.kind === "local" ? d.target.path : `${d.target.user ? `${d.target.user}@` : ""}${d.target.host}:${d.target.path}`} · ${d.mode === "replicate" ? "copy of the first backup" : engineLabel[d.engine]}${d.offsite ? " · offsite" : ""}`}
                    icon={d.target.kind === "local" ? "drive" : "server"}
                  >
                    <button class="btn" disabled={!!removingDest} title="Stops backing up there; the backups already made stay where they are" onclick={() => void removeDest(d.name)}>{removingDest === d.name ? "Removing…" : "Remove"}</button>
                  </Row>
                {/each}
                <Row
                  title="Add another place…"
                  subtitle={draft.destinations?.length ? "Each backup goes to every place, one after the other." : "A second copy on another drive or a server: if one is damaged or lost, the other isn't."}
                  icon="plus"
                  onclick={creating ? undefined : () => (addingDest = true)}
                  disabled={creating}
                  reason="Available once this backup is set up."
                />
              </div>
            </section>
            <section id="sec-schedule">
              <h2 class="group-title">When</h2>
              {@render whenBody()}
            </section>
            <!-- Keep -->
            <section id="sec-retention">
              <h2 class="group-title">What to keep</h2>
              <div class="rows">
                {@render pageRow("retention", "history", "", keepSummary)}
                {@render pageRow("tests", "shieldCheck", testsSummary, "Check now and then that files come back")}
              </div>
            </section>

            <!-- More -->
            <section>
              <h2 class="group-title">More</h2>
              <div class="rows">
                {#if usesServer}{@render pageRow("network", "server", networkSummary)}{/if}
                {@render pageRow("webhooks", "external", draft.webhooks?.length ? `${draft.webhooks.length} webhook${draft.webhooks.length === 1 ? "" : "s"}` : "Only on this computer")}
                {@render pageRow("hooks", "terminal", hooksSummary)}
                {@render pageRow("options", "gauge", optionsSummary)}
                {#if !creating && id}
                  <Row title="Export this profile" subtitle="As a file to import elsewhere, or for borgmatic, restic, rsync or Borg." icon="download" onclick={() => (app.dialog = { kind: "export", id: id! })} />
                {/if}
              </div>
            </section>

            <!-- Last, and apart from everything else (destructive). -->
            {#if !creating && id}
              <div class="rows mt-6">
                <Row
                  title="Delete this backup profile"
                  subtitle="Stops these backups and forgets their settings. The backups already made stay where they are."
                  icon="trash"
                  tone="danger"
                  onclick={() => (app.dialog = { kind: "delete-profile", id: id! })}
                />
              </div>
            {/if}
          {:else if done}
            <!-- ===================== Wizard: done ===================== -->
            <section class="animate-step flex flex-col items-start gap-6 pt-4" style:--step-dir={dir}>
              <span class="glyph glyph-ok size-16"><Icon name="shieldCheck" size={32} /></span>
              <div>
                <h2 id="step-title" tabindex="-1" class="text-[26px] leading-tight font-semibold tracking-[-0.015em] text-balance outline-none">“{draft.name}” is set up.</h2>
                <p class="mt-2 max-w-[60ch] text-[15px] text-fg-2">
                  {when === "manual" ? "It backs up when you click Back up now." : `It backs up ${whenSummary.charAt(0).toLowerCase()}${whenSummary.slice(1)}, in the background, even with the app closed.`}
                  The first backup copies everything once; the ones after it only what changed.
                </p>
              </div>
              <div class="rows w-full">
                <div class="row"><Icon name="folder" size={17} class="shrink-0 text-fg-2" /><span class="min-w-0 flex-1 truncate">{whatSummary}</span></div>
                <div class="row"><Icon name={draft.target.kind === "ssh" ? "server" : "drive"} size={17} class="shrink-0 text-fg-2" /><span class="min-w-0 flex-1 truncate">{whereSummary}</span></div>
                {#if encrypted}
                  <div class="row">
                    <Icon name="lock" size={17} class="shrink-0 text-fg-2" />
                    <span class="min-w-0 flex-1">{encReady ? `Encrypted and ready: the ${pwWord} is ${sourceWords[pwSource?.from ?? "keyring"]}.` : `Encrypted. The first backup sets it up and asks for the ${pwWord}.`}</span>
                  </div>
                {/if}
              </div>
              {#if away}
                <!-- The drive isn't plugged in: a first backup now could only fail. -->
                <div class="flex w-full items-start gap-3 rounded-card bg-warning/10 px-4 py-3 text-[13.5px] ring-1 ring-warning/30 ring-inset" role="status">
                  <Icon name="alert" size={16} class="mt-0.5 shrink-0 text-warning" />
                  <span class="text-fg-2"><span class="font-medium text-fg">{away.message}</span> When it is, click Back up now on its page.</span>
                </div>
                <button class="btn btn-primary h-10 px-5" onclick={() => done && onsaved(done)}>Done</button>
              {:else}
                <div class="flex gap-2">
                  <button class="btn btn-primary h-10 px-5" onclick={startFirst}><Icon name="play" size={14} /> Back up now</button>
                  <button class="btn btn-ghost h-10" onclick={() => done && onsaved(done)}>Later</button>
                </div>
              {/if}
            </section>
          {:else}
            <!-- ===================== Wizard: one question per step ===================== -->
            <ol class="flex items-center gap-1.5" aria-label="Setup steps">
              {#each steps as s, i (s.id)}
                {@const current = i === stepIndex}
                {@const passed = i < stepIndex || (i <= reached && !current)}
                <li class="flex items-center gap-1.5 {i < steps.length - 1 ? 'flex-1' : ''}">
                  <button
                    class="flex shrink-0 items-center gap-2 rounded-lg py-1 pr-1.5 pl-1 text-[13px] font-medium transition-colors {current ? 'text-fg' : i <= reached ? 'text-fg-2 hover:text-fg' : 'text-fg-muted'}"
                    disabled={i > reached}
                    aria-current={current ? "step" : undefined}
                    onclick={() => go(s.id)}
                  >
                    <span class="grid size-6 place-items-center rounded-full text-[12px] font-semibold {current ? 'bg-primary text-on-primary' : passed ? 'bg-selected text-fg' : 'bg-surface-3 text-fg-muted'}">
                      {#if passed && !stepIssue(s.id)}<Icon name="check" size={13} />{:else}{i + 1}{/if}
                    </span>
                    {s.label}
                  </button>
                  {#if i < steps.length - 1}<span class="h-px min-w-3 flex-1 {i < stepIndex ? 'bg-fg-muted/60' : 'bg-sep'}"></span>{/if}
                </li>
              {/each}
            </ol>

            {#key step}
              <section class="animate-step flex flex-col gap-6" style:--step-dir={dir}>
                <header>
                  <h2 id="step-title" tabindex="-1" class="text-[24px] leading-tight font-semibold tracking-[-0.012em] text-balance outline-none">
                    {step === "review" ? `Ready to create “${draft.name}”` : steps[stepIndex]!.title}
                  </h2>
                  <p class="mt-1.5 max-w-[62ch] text-[14.5px] text-fg-2">{steps[stepIndex]!.text}</p>
                </header>

                {#if step === "what"}
                  {#if bitProfiles.length && (bitUsed || !bitDismissed)}
                    <BackInTimeOffer profiles={bitProfiles} used={bitUsed} onuse={useBackInTime} onstartover={startFromScratch} ondismiss={() => (bitDismissed = true)} />
                  {/if}
                  <div id="sec-sources">{@render whatBody()}</div>
                {:else if step === "where"}
                  <div id="sec-destination" class="flex flex-col gap-3">
                    <div class="self-start">{@render whereKind()}</div>
                    <div>{@render whereList()}</div>
                    {#if draft.target.kind === "ssh"}<div class="rows">{@render pageRow("connection", "terminal", connectionSummary)}</div>{/if}
                  </div>
                {:else if step === "protect"}
                  <StorageChoice engine={draft.engine} onchoose={(e) => setEngine(e)} />
                  {#if encrypted}{@render passwordGroup()}{/if}
                {:else if step === "when"}
                  <div id="sec-schedule">{@render whenBody()}</div>
                  <section>
                    <h3 class="group-title">What to keep</h3>
                    <div class="rows">{@render pageRow("retention", "history", "", keepSummary)}</div>
                  </section>
                  {@render howItRuns()}
                {:else}
                  {@render nameGroup()}
                  <section>
                    <h3 class="group-title">Your backup</h3>
                    <div class="rows">
                      <Row title="What" icon="folder" subtitle={whatSummary} onclick={() => go("what")} />
                      <Row title="Where" icon={draft.target.kind === "ssh" ? "server" : "drive"} subtitle={whereSummary} onclick={() => go("where")} />
                      <Row title="Protection" icon="lock" subtitle={protectSummary} onclick={() => go("protect")} />
                      <Row title="When" icon="clock" subtitle={whenSummary} onclick={() => go("when")} />
                      <Row title="What to keep" icon="history" subtitle={keepSummary} onclick={() => openPage("retention")} />
                    </div>
                  </section>
                  <section>
                    <h3 class="group-title">More options</h3>
                    <div class="rows">
                      {@render pageRow("hooks", "terminal", hooksSummary)}
                      {@render pageRow("options", "gauge", optionsSummary)}
                    </div>
                    <p class="mt-1.5 px-0.5 text-[12.5px] text-fg-muted">Optional: the defaults suit most backups.</p>
                  </section>
                {/if}
              </section>
            {/key}
          {/if}
      </div>
    </div>

    <!-- A group of settings opens in a dialog over the editor, so you never lose your place.
         Its changes join the draft; nothing is saved until Save / Create backup. -->
    {#if page}
      <Modal title={pageTitle[page].title} width="max-w-2xl" onclose={() => openPage(null)}>
        <p class="-mt-1 mb-5 max-w-[62ch] text-[13.5px] text-fg-2">{pageTitle[page].text}</p>
        <div class="flex flex-col gap-5">

          {#if page === "exclusions"}
            <section class="flex flex-col gap-4">
              <Field label="" hint="*.iso or node_modules/ match anywhere; /path or ~/path one place; !pattern keeps something back in." issues={at("sources.exclude")}>
                <ListEditor bind:items={draft.sources.exclude} placeholder="~/Videos/  or  *.iso" label="Left out" icon="filter" />
              </Field>
              {#if suggestions.length}
                {@const exclude = draft.sources.exclude}
                <div>
                  <p class="label pb-2">Often left out (click to add or remove)</p>
                  <div class="flex flex-wrap gap-2">
                    {#each suggestions as sg (sg.pattern)}
                      {@const added = exclude.includes(sg.pattern)}
                      <button
                        class="flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-[13px] transition-colors {added ? 'bg-success/15 font-medium text-success shadow-[inset_0_0_0_1px_color-mix(in_oklab,var(--c-success)_35%,transparent)] hover:bg-success/20' : 'bg-surface-3 text-fg-2 hover:bg-track hover:text-fg'}"
                        title="{sg.why}{sg.size ? ` · ${bytes(sg.size)}` : ''}"
                        aria-pressed={added}
                        onclick={() => draft && (draft.sources.exclude = added ? exclude.filter((x) => x !== sg.pattern) : [...exclude, sg.pattern])}
                      >
                        <Icon name={added ? "check" : "plus"} size={13} />
                        {sg.label}{#if sg.size}<span class="num font-mono text-[12px] opacity-70">{bytes(sg.size)}</span>{/if}
                      </button>
                    {/each}
                  </div>
                </div>
              {/if}
            </section>
            <div class="rows">
              <Toggle row bind:checked={draft.sources.exclude_caches} label="Skip cache folders" hint="Folders marked with a CACHEDIR.TAG (browser and app caches)." />
              <Toggle row bind:checked={draft.sources.one_file_system} label="Stay on one disk" hint="Don't follow into other drives mounted inside the folders you back up." />
              <Row title="Skip files larger than" subtitle="Very large files (disk images, videos) can be left out.">
                <SizeLimit bind:value={draft.sources.exclude_larger_than} />
              </Row>
            </div>
            {#each [...at("sources.exclude_larger_than"), ...at("sources.exclude_caches")] as i (i.message)}<p class="text-[13px] {i.severity === 'error' ? 'text-danger' : 'text-warning'}">{i.message}</p>{/each}
            <section>
              <h2 class="group-title">Skip folders that contain a file named</h2>
              <Field label="" hint="e.g. .nobackup: put an empty file with this name in any folder to leave it out.">
                <ListEditor bind:items={draft.sources.exclude_if_present!} placeholder=".nobackup" label="Marker files" />
              </Field>
            </section>
            <section>
              <h2 class="group-title">Check the rules</h2>
              <RuleTester profile={draft} />
            </section>
          {:else if page === "security" && draft.vault}
            <VaultSecurity bind:vault={draft.vault} id={creating ? null : (id ?? null)} name={draft.name} onsynced={syncSaved} />
          {:else if page === "storage"}
            <section>
              <StorageChoice engine={draft.engine} onchoose={(e) => setEngine(e)} />
              {#if engineChanged}
                <p class="mt-4 rounded-lg bg-warning/10 px-4 py-3 text-[13px] text-warning">This starts a new backup history. Existing snapshots stay where they are, but won't appear here.</p>
              {/if}
            </section>
            {#if draft.engine === "vault" && draft.vault}
              {@const vault = draft.vault}
              <div class="rows">
                <Toggle row bind:checked={vault.encryption} label="Encrypt" hint="Without it, the Vault is compressed and deduplicated but readable by anyone with the drive." disabled={!creating && !engineChanged} reason="Fixed when the Vault was created." />
                {#if vault.encryption}
                  <Row title="Password comes from" subtitle={sys ? "System backups run with nobody logged in: unlock once and Flux keeps the password in a root-only key file." : "Scheduled backups need a source that works without you."}>
                    <select class="input w-auto min-w-56" value={vault.password_source.from} onchange={(e) => setPasswordSource(e.currentTarget.value)} aria-label="Password comes from">
                      <option value="keyring" disabled={sys}>Desktop keyring{sys ? " (not for system)" : ""}</option>
                      <option value="prompt">Ask every time</option>
                      <option value="file">A file</option>
                      <option value="command">A command (e.g. pass)</option>
                    </select>
                  </Row>
                {/if}
                <Row title="Compression: level {vault.compression_level}" subtitle="0 off · 3 fast (default) · 19 smallest">
                  <input type="range" min="0" max="19" class="w-40 accent-[var(--c-primary)]" bind:value={vault.compression_level} aria-label="Compression level" />
                </Row>
                {#if draft.target.kind === "local"}
                  <Toggle
                    row
                    bind:checked={() => vault.keep_recovery_tool ?? true, (v) => (vault.keep_recovery_tool = v)}
                    label="Keep the recovery tool on the drive"
                    hint="A small program next to the Vault that lists, restores and checks your backups without Flux, on any Linux, with a note on using it."
                  />
                {/if}
              </div>
              {#if vault.encryption && vault.password_source.from === "file"}
                {@const src = vault.password_source}
                <Field label="Password file" id="f-pwfile" issues={at("vault")}><input id="f-pwfile" class="input font-mono text-[13px]" bind:value={src.path} placeholder="~/.config/flux-backup/vault.pass" /></Field>
              {:else if vault.encryption && vault.password_source.from === "command"}
                {@const src = vault.password_source}
                <Field label="Command" id="f-pwcmd"><input id="f-pwcmd" class="input font-mono text-[13px]" bind:value={src.command} placeholder="pass show backup/vault" /></Field>
              {/if}
            {:else if draft.engine === "borg" && draft.borg}
              {@const borg = draft.borg}
              <div class="rows">
                <Row title="Encryption" subtitle={creating || engineChanged ? "Fixed when the repository is created." : "Fixed when the repository was created."}>
                  <select class="input w-auto min-w-56" bind:value={borg.encryption} disabled={!creating && !engineChanged} aria-label="Encryption">
                    <option value="repokey-blake2">repokey-blake2 (recommended)</option>
                    <option value="repokey">repokey (SHA-256)</option>
                    <option value="none">none</option>
                  </select>
                </Row>
                {#if borg.encryption !== "none"}
                  <Row title="Passphrase comes from" subtitle={sys ? "System backups run with nobody logged in: unlock once and Flux keeps it in a root-only key file." : "Scheduled backups need a source that works without you."}>
                    <select class="input w-auto min-w-56" value={borg.password_source.from} onchange={(e) => setPasswordSource(e.currentTarget.value)} aria-label="Passphrase comes from">
                      <option value="keyring" disabled={sys}>Desktop keyring{sys ? " (not for system)" : ""}</option>
                      <option value="prompt">Ask every time</option>
                      <option value="file">A file</option>
                      <option value="command">A command (e.g. pass)</option>
                    </select>
                  </Row>
                {/if}
                <Row title="Compression" subtitle="Borg's --compression: lz4, zstd,3, auto,zstd,10, none">
                  <input class="input w-40 font-mono text-[13px]" list="borg-compressions" bind:value={borg.compression} aria-label="Compression" />
                  <datalist id="borg-compressions"><option value="lz4"></option><option value="zstd,3"></option><option value="zstd,10"></option><option value="auto,zstd,6"></option><option value="none"></option></datalist>
                </Row>
                {#if draft.target.kind === "ssh"}
                  <Row title="borg on the server" subtitle="Only if it isn't on the server's PATH.">
                    <input class="input w-40 font-mono text-[13px]" value={borg.remote_path ?? ""} oninput={(e) => (borg.remote_path = opt(e.currentTarget.value))} placeholder="borg" aria-label="borg on the server" />
                  </Row>
                {/if}
              </div>
              {#each at("borg") as i (i.message)}<p class="text-[13px] {i.severity === 'error' ? 'text-danger' : 'text-warning'}">{i.message}</p>{/each}
              {#if borg.encryption !== "none" && borg.password_source.from === "file"}
                {@const src = borg.password_source}
                <Field label="Passphrase file"><input class="input font-mono text-[13px]" bind:value={src.path} placeholder="~/.config/flux-backup/borg.pass" /></Field>
              {:else if borg.encryption !== "none" && borg.password_source.from === "command"}
                {@const src = borg.password_source}
                <Field label="Command"><input class="input font-mono text-[13px]" bind:value={src.command} placeholder="pass show backup/borg" /></Field>
              {/if}
              <section class="card px-6 py-5">
                <Field label="Extra borg create options" hint="For Borg users, e.g. --checkpoint-interval=600. Recorded in the recovery script." issues={at("borg.extra_args")}>
                  <ListEditor bind:items={borg.extra_args!} placeholder="--checkpoint-interval=600" />
                </Field>
              </section>
            {:else if draft.engine === "rsync" && draft.rsync}
              {@const rs = draft.rsync}
              <section class="card px-6 py-5">
                <Field label="Extra rsync options" hint="For rsync users, e.g. --compress for slow links. Recorded with every snapshot." issues={at("rsync.extra_args")}>
                  <ListEditor bind:items={rs.extra_args!} placeholder="--compress" />
                </Field>
              </section>
            {/if}
          {:else if page === "connection" && draft.target.kind === "ssh"}
            {@const target = draft.target}
            <section class="card grid grid-cols-5 gap-4 px-6 py-5">
              <div class="col-span-1"><Field label="Port" id="f-port" issues={at("target.port")}><input id="f-port" class="input num" inputmode="numeric" value={target.port ?? ""} oninput={(e) => (target.port = e.currentTarget.value ? Number(e.currentTarget.value) : null)} placeholder="22" /></Field></div>
              <div class="col-span-4">
                <Field label="Key file" id="f-key" hint="Empty: your ssh config and agent decide.">
                  <div class="flex gap-2">
                    <input id="f-key" class="input font-mono text-[13px]" value={target.identity_file ?? ""} oninput={(e) => (target.identity_file = opt(e.currentTarget.value))} placeholder="~/.ssh/flux_backup_ed25519" />
                    <button class="btn" aria-label="Choose key file" onclick={async () => { const f = await pickFile("SSH private key"); if (f) target.identity_file = f; }}><Icon name="folder" size={14} /></button>
                  </div>
                </Field>
              </div>
              <div class="col-span-5"><Field label="Upload limit per second" id="f-bw" hint="e.g. 5 MiB. Empty: as fast as the connection allows."><input id="f-bw" class="input" value={target.bandwidth_limit ?? ""} oninput={(e) => (target.bandwidth_limit = opt(e.currentTarget.value))} placeholder="unlimited" /></Field></div>
              <div class="col-span-5"><Field label="Extra ssh options" hint="As in ssh -o: Compression=no, ProxyJump=gateway…"><ListEditor bind:items={target.ssh_options!} placeholder="Compression=no" /></Field></div>
            </section>
            <div class="flex items-start gap-3">
              <button class="btn" onclick={runTest} disabled={testing || !target.host}>{testing ? "Testing…" : "Test connection"}</button>
              {#if test}<ul class="pt-2 text-[13px] {test.ok ? 'text-success' : 'text-danger'}">{#each test.messages as m (m)}<li>{m}</li>{/each}</ul>{/if}
            </div>
          {:else if page === "schedule"}
            <section class="card flex flex-col gap-4 px-6 py-5">
              <div class="segmented flex-wrap self-start" role="radiogroup" aria-label="Other timings">
                {@render choice(draft.schedule.trigger === "calendar" && preset === "weekdays", "Weekdays", () => { preset = "weekdays"; if (draft) draft.schedule.trigger = "calendar"; applyCalendar(); })}
                {@render choice(draft.schedule.trigger === "interval", "Every few hours", () => setTrigger("interval"))}
                {@render choice(draft.schedule.trigger === "calendar" && preset === "custom", "Custom times", () => { preset = "custom"; if (draft) draft.schedule.trigger = "calendar"; })}
              </div>
              {#if draft.schedule.trigger === "calendar" && preset === "weekdays"}
                <div class="flex items-center gap-2 text-sm text-fg-2"><span>Monday to Friday at</span><input type="time" class="input w-32" bind:value={timeOfDay} onchange={applyCalendar} aria-label="Time" /></div>
              {:else if draft.schedule.trigger === "calendar" && preset === "custom"}
                <Field label="systemd calendar expression" id="f-cal" issues={at("schedule.on_calendar")} hint="e.g. Mon,Wed,Fri 07:30 or *-*-01 03:00 (man systemd.time)">
                  <input id="f-cal" class="input font-mono text-[13px]" value={draft.schedule.on_calendar ?? ""} oninput={(e) => draft && (draft.schedule.on_calendar = opt(e.currentTarget.value))} />
                </Field>
              {:else if draft.schedule.trigger === "interval"}
                <Field label="Every" issues={at("schedule.every")} hint="Counted from the last successful backup.">
                  {#if everyAsText}
                    <div class="flex items-center gap-2">
                      <input class="input w-48 font-mono text-[13px]" value={draft.schedule.every ?? ""} oninput={(e) => draft && (draft.schedule.every = opt(e.currentTarget.value))} aria-label="Interval" />
                      <button type="button" class="btn" onclick={applyEvery}>Use minutes, hours or days</button>
                    </div>
                  {:else}
                    <div class="flex gap-2">
                      <NumberInput bind:value={everyN} min={1} label="Interval" />
                      <select class="input w-32" bind:value={everyUnit} aria-label="Unit">
                        <option value="min">minutes</option>
                        <option value="h">hours</option>
                        <option value="d">days</option>
                      </select>
                    </div>
                  {/if}
                </Field>
              {:else}
                <p class="text-[13px] text-fg-muted">The main page sets hourly, daily and weekly backups. Pick one of these for other timings.</p>
              {/if}
            </section>
            <div class="rows">
              <Toggle row bind:checked={draft.schedule.catch_up} label="Catch up on missed backups" hint="If the computer was off at the time, back up soon after it starts." disabled={draft.schedule.trigger !== "calendar"} reason="Only for backups at set times." />
              <Toggle row bind:checked={draft.schedule.skip_on_battery} label="Wait for mains power" hint="Scheduled backups don't run on battery." />
              <Toggle
                row
                bind:checked={draft.schedule.skip_on_metered!}
                label="Not on metered connections"
                hint="Scheduled backups to a server wait while you're on mobile data or a phone's hotspot (as NetworkManager reports it)."
                disabled={draft.target.kind !== "ssh"}
                reason="Only for backups to a server: a drive doesn't use the network."
              />
            </div>
            <section class="flex flex-col gap-2">
              <h3 class="px-1 text-[14px] font-medium">Upkeep after scheduled backups</h3>
              <div class="rows">
                <Toggle row bind:checked={draft.maintenance!.verify_monthly} label="Verify every byte once a month" hint="Reads all stored data back to make sure it can be restored. Takes a while; you're notified if anything is wrong." />
                <Toggle
                  row
                  bind:checked={draft.maintenance!.free_space_weekly}
                  label="Free up space once a week"
                  hint="Old snapshots are removed at each backup; this deletes the data only they used."
                  disabled={draft.engine !== "vault"}
                  reason={draft.engine === "borg" ? "Borg frees the space when it removes snapshots." : "Plain folders free their space straight away."}
                />
              </div>
            </section>
          {:else if page === "retention"}
            {@const r = draft.retention}
            <div class="rows">
              <Toggle row bind:checked={r.enabled} label="Remove old snapshots automatically" hint="The newest and pinned snapshots always stay." />
              {#if r.enabled}<Toggle row bind:checked={r.keep_named} label="Never remove named snapshots" />{/if}
            </div>
            {#if r.enabled}
              {@const current = keepPresetOf(r)}
              <section>
                <h3 class="group-title">How long to keep them</h3>
                <div class="segmented" role="radiogroup" aria-label="How long to keep snapshots">
                  {#each keepPresets as kp (kp.id)}
                    {@render choice(current === kp.id && !customKeep, kp.label, () => applyKeepPreset(kp.id))}
                  {/each}
                  {@render choice(current === null || customKeep, "Custom", () => (customKeep = true))}
                </div>
                {#if current && !customKeep}
                  <p class="mt-2 px-0.5 text-[13px] text-fg-2">{keepPresets.find((k) => k.id === current)?.text}</p>
                {/if}
              </section>
              {#if current === null || customKeep}
              <section class="card flex flex-col gap-4 px-6 py-5">
                <div class="grid grid-cols-[1fr_auto] items-center gap-x-6 gap-y-2.5 text-sm text-fg-2">
                  <span>The newest snapshots</span><NumberInput bind:value={r.keep_last} label="Keep last" />
                  <span>One per hour, for … hours</span><NumberInput bind:value={r.keep_hourly} label="Hourly" />
                  <span>One per day, for … days</span><NumberInput bind:value={r.keep_daily} label="Daily" />
                  <span>One per week, for … weeks</span><NumberInput bind:value={r.keep_weekly} label="Weekly" />
                  <span>One per month, for … months</span><NumberInput bind:value={r.keep_monthly} label="Monthly" />
                  <span>One per year, for … years</span><NumberInput bind:value={r.keep_yearly} label="Yearly" />
                </div>
                <div class="grid grid-cols-3 gap-4">
                  <Field label="Everything from the last" id="f-within" hint="e.g. 1 day" issues={at("retention.keep_within")}>
                    <input id="f-within" class="input" value={r.keep_within ?? ""} oninput={(e) => (r.keep_within = opt(e.currentTarget.value))} placeholder="—" />
                  </Field>
                  <Field label="Never longer than" id="f-maxage" hint="e.g. 2 years" issues={at("retention.max_age")}>
                    <input id="f-maxage" class="input" value={r.max_age ?? ""} oninput={(e) => (r.max_age = opt(e.currentTarget.value))} placeholder="no limit" />
                  </Field>
                  <Field label="Keep this much free" id="f-minfree" hint="e.g. 50 GiB" issues={at("retention.min_free_space")}>
                    <input id="f-minfree" class="input" value={r.min_free_space ?? ""} oninput={(e) => (r.min_free_space = opt(e.currentTarget.value))} placeholder="—" />
                  </Field>
                </div>
                {#each at("retention").filter((i) => i.field === "retention") as i (i.message)}<p class="text-sm text-danger">{i.message}</p>{/each}
              </section>
              {/if}
              <section>
                <h3 class="group-title">Safety</h3>
                <div class="rows">
                  <Row title="Always keep at least" subtitle="Complete snapshots (with no files left out), whatever the rules above say." icon="shieldCheck">
                    <span class="flex items-center gap-2 text-sm text-fg-2"><NumberInput bind:value={() => r.safety_floor ?? 1, (n) => (r.safety_floor = n)} min={0} max={100} label="Always keep at least" /> snapshots</span>
                  </Row>
                  <Toggle
                    row
                    icon="pause"
                    bind:checked={() => (r.after_problems ?? "hold") === "hold", (on) => (r.after_problems = on ? "hold" : "prune")}
                    label="After a backup with problems, keep everything"
                    hint="If a backup fails or leaves something out, nothing is removed until one runs cleanly."
                  />
                  <Row title="Watch for unusual changes" subtitle="Warns when a backup looks like ransomware or a mistake (far more changes than usual, files suddenly encrypted) and keeps the snapshot before." icon="alert">
                    <select class="input w-auto min-w-36" value={draft.unusual_changes ?? "normal"} onchange={(e) => draft && (draft.unusual_changes = e.currentTarget.value as "normal")} aria-label="Watch for unusual changes">
                      <option value="normal">Normally</option>
                      <option value="high">Closely</option>
                      <option value="off">Not at all</option>
                    </select>
                  </Row>
                </div>
              </section>
            {/if}
            <section>
              <h3 class="group-title">Activity history</h3>
              <div class="rows">
                <Row title="Keep the history of backups" subtitle="What Activity shows. Separate from snapshots: removing history never removes backups." icon="activity">
                  <select
                    class="input w-auto min-w-44"
                    value={String(r.keep_history_days ?? "")}
                    onchange={(e) => (r.keep_history_days = e.currentTarget.value ? Number(e.currentTarget.value) : null)}
                    aria-label="Keep the history of backups"
                  >
                    <option value="">The last 1000 backups</option>
                    <option value="30">For 30 days</option>
                    <option value="90">For 3 months</option>
                    <option value="365">For a year</option>
                  </select>
                </Row>
              </div>
            </section>
            {#if preview}
              <details class="group rounded-lg bg-surface-3/50 p-1.5">
                <summary class="flex cursor-pointer list-none justify-between px-3 py-1.5 text-[13px] select-none [&::-webkit-details-marker]:hidden">
                  <span class="flex items-center gap-1.5 text-fg-2"><Icon name="chevronRight" size={13} class="transition-transform group-open:rotate-90" /> Your snapshots with these rules</span>
                  <span class={previewRemoved ? "text-warning" : "text-fg-muted"}>{previewRemoved ? `${previewRemoved} of ${preview.length} would go` : `all ${preview.length} stay`}</span>
                </summary>
                <ul class="max-h-56 overflow-y-auto text-[13px]">
                  {#each preview as row (row.id)}
                    <li class="flex items-center gap-3 rounded-md px-3 py-1.5 {row.keep ? '' : 'text-danger'}" title={row.keep ? row.reasons.join(", ") : row.expired ? "older than the limit" : "no rule keeps it"}>
                      <Icon name={row.keep ? "check" : "trash"} size={13} class={row.keep ? "text-success" : ""} />
                      <span class="num">{dateTime(row.time)}</span>
                      <span class="min-w-0 flex-1 truncate text-fg-muted">{row.name ?? ""}</span>
                    </li>
                  {/each}
                </ul>
              </details>
            {/if}
          {:else if page === "webhooks"}
            <WebhookList hooks={draft.webhooks ?? []} onchange={(h) => draft && (draft.webhooks = h)} />
            {#each at("webhooks") as i (i.message)}<p class="mt-1.5 text-[13px] text-danger">{i.message}</p>{/each}
          {:else if page === "tests"}
            {@const t = draft.restore_test ?? {}}
            <section>
              <h2 class="group-title">How often</h2>
              <div class="segmented self-start" role="radiogroup" aria-label="How often">
                {#each [["off", "Only when I ask"], ["backup", "After every backup"], ["week", "Weekly"], ["month", "Monthly"]] as [v, label] (v)}
                  <button role="radio" aria-checked={(t.every ?? "off") === v} onclick={() => setTest({ every: v as "off" })}>{label}</button>
                {/each}
              </div>
              <p class="mt-1.5 px-0.5 text-[12.5px] text-fg-muted">A test runs after a backup once it's due. To run one any time, use <em>Test a restore now</em> on the Overview.</p>
            </section>
            <section>
              <h2 class="group-title">Which files</h2>
              <div class="rows" role="radiogroup" aria-label="Which files">
                {#each [["random_files", "Some files at random", "Quick. Pick how many."], ["percent", "A share of the backup", "Random files making up this much of its size."], ["rolling", "Every file once a month", "A different part each time, so every file is tested within 30 days."], ["full", "Everything", "Restores the whole snapshot: thorough, and as slow as a full restore."]] as [v, label, text] (v)}
                  <button role="radio" aria-checked={(t.mode ?? "random_files") === v} class="row row-link items-start gap-3 py-3 text-left" onclick={() => setTest({ mode: v as "full" })}>
                    <span class="min-w-0 flex-1">
                      <span class="block font-medium">{label}</span>
                      <span class="block text-[12.5px] text-fg-muted">{text}</span>
                    </span>
                    {#if v === "random_files" && (t.mode ?? "random_files") === v}
                      <span class="flex items-center gap-2 text-sm text-fg-2"><NumberInput bind:value={() => t.files ?? 10, (n) => setTest({ files: n })} min={1} max={10000} label="Files" /> files</span>
                    {:else if v === "percent" && t.mode === v}
                      <span class="flex items-center gap-2 text-sm text-fg-2"><NumberInput bind:value={() => t.percent ?? 5, (n) => setTest({ percent: n })} min={1} max={100} label="Percent" /> %</span>
                    {:else if (t.mode ?? "random_files") === v}
                      <Icon name="check" size={18} class="mt-0.5" />
                    {/if}
                  </button>
                {/each}
              </div>
            </section>
            <section>
              <div class="rows">
                <Toggle row icon="diff" bind:checked={() => t.compare_with_source ?? false, (on) => setTest({ compare_with_source: on })} label="Also compare with the files now" hint="Notes files that differ from your copy now (they may have changed since): a hint at damage on either side." />
                <Row title="Stop after" subtitle="A time (e.g. 1h) and an amount read (e.g. 5GB), for big tests; it then ends with a note." icon="clock">
                  <span class="flex items-center gap-2">
                    <input class="input h-8 w-24 py-0" value={t.max_runtime ?? ""} oninput={(e) => setTest({ max_runtime: e.currentTarget.value.trim() || null })} placeholder="time" aria-label="Time limit" />
                    <input class="input h-8 w-24 py-0" value={t.max_read ?? ""} oninput={(e) => setTest({ max_read: e.currentTarget.value.trim() || null })} placeholder="data" aria-label="Data limit" />
                  </span>
                </Row>
              </div>
              {#each at("restore_test") as i (i.message)}<p class="mt-1.5 text-[13px] text-danger">{i.message}</p>{/each}
            </section>
          {:else if page === "network"}
            {@const n = draft.network ?? {}}
            <section>
              <h2 class="group-title">Wi-Fi networks</h2>
              <p class="-mt-1 mb-2 px-0.5 text-[12.5px] text-fg-muted">For scheduled backups to a server. On a wired connection these don't apply; backing up by hand always works.</p>
              <div class="flex flex-col gap-3">
                <Field label="Only on these networks (empty: any)" id="f-wifi-only">
                  <ListEditor bind:items={() => n.wifi_only ?? [], (v) => setNet({ wifi_only: v })} placeholder="Home WiFi" label="Only on" icon="server" />
                </Field>
                <Field label="Never on these networks" id="f-wifi-never">
                  <ListEditor bind:items={() => n.wifi_never ?? [], (v) => setNet({ wifi_never: v })} placeholder="Phone hotspot" label="Never on" icon="server" />
                </Field>
              </div>
            </section>
            <section>
              <h2 class="group-title">Upload limits by time of day</h2>
              <p class="-mt-1 mb-2 px-0.5 text-[12.5px] text-fg-muted">
                Outside these times, {draft.target.kind === "ssh" && draft.target.bandwidth_limit ? `the connection's limit (${draft.target.bandwidth_limit}/s)` : "full speed"}. Vault backups follow a change of limit as they run; rsync and Borg use the limit in force when they start.
              </p>
              <div class="rows">
                {#each n.bandwidth ?? [] as w, i (i)}
                  <div class="row flex-wrap gap-2 text-[13px]">
                    <select class="input h-8 w-auto py-0" bind:value={w.days} aria-label="Days">
                      <option value="Mon..Fri">Weekdays</option>
                      <option value="Sat,Sun">Weekends</option>
                      <option value="*">Every day</option>
                      {#if !["Mon..Fri", "Sat,Sun", "*"].includes(w.days)}<option value={w.days}>{w.days}</option>{/if}
                    </select>
                    <span class="text-fg-2">from</span>
                    <input class="input h-8 w-20 py-0 font-mono" bind:value={w.from} aria-label="From" placeholder="09:00" />
                    <span class="text-fg-2">to</span>
                    <input class="input h-8 w-20 py-0 font-mono" bind:value={w.to} aria-label="To" placeholder="18:00" />
                    <span class="text-fg-2">at most</span>
                    <input class="input h-8 w-24 py-0 font-mono" value={w.limit ?? ""} oninput={(e) => (w.limit = e.currentTarget.value.trim() || null)} aria-label="Limit" placeholder="no limit" />
                    <span class="text-fg-2">/s</span>
                    <button class="btn-icon ml-auto size-8" aria-label="Remove this limit" onclick={() => setNet({ bandwidth: (n.bandwidth ?? []).filter((_, j) => j !== i) })}><Icon name="x" size={14} /></button>
                  </div>
                {/each}
                <button class="row row-link gap-3" onclick={() => setNet({ bandwidth: [...(n.bandwidth ?? []), { days: "Mon..Fri", from: "09:00", to: "18:00", limit: "2MB" }] })}>
                  <Icon name="plus" size={16} class="text-fg-2" /> <span class="font-medium">Add a limit</span>
                </button>
              </div>
              {#each at("network") as i (i.message)}<p class="mt-1.5 text-[13px] text-danger">{i.message}</p>{/each}
            </section>
          {:else if page === "hooks"}
            <section class="card flex flex-col gap-4 px-6 py-5">
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
          {:else if page === "options"}
            <div class="flex min-h-[54px] items-center gap-3 rounded-card bg-surface-2 px-4 py-2.5 text-[13px] shadow-[0_0_0_1px_var(--c-group-edge)]">
              {#if optionsAreDefault}
                <Icon name="check" size={15} class="shrink-0 text-success" />
                <span class="flex-1 text-fg-2">Using the recommended settings.</span>
              {:else}
                <Icon name="sparkle" size={15} class="shrink-0 text-fg-muted" />
                <span class="flex-1 text-fg-2">Changed something and want to undo it? Go back to what Flux recommends.</span>
                <button class="btn h-8 shrink-0" onclick={useRecommendedOptions} disabled={!defaultOptions}>Recommended defaults</button>
              {/if}
            </div>
            <h3 class="pt-2 text-[13px] font-medium text-fg-muted">What's kept with your files</h3>
            <div class="rows">
              <Toggle row bind:checked={draft.options.preserve_acl} label="Keep access lists (ACLs)" hint="Extra permissions some files have." />
              <Toggle row bind:checked={draft.options.preserve_xattr} label="Keep extended attributes" hint="Labels and metadata some apps attach to files." />
              <Row title="Symbolic links" subtitle="Shortcuts to other files or folders.">
                <select class="input w-auto min-w-56" bind:value={draft.options.symlinks} aria-label="Symbolic links">
                  <option value="preserve">Store links as links</option>
                  <option value="follow_unsafe">Follow links pointing outside</option>
                  <option value="follow">Always follow links</option>
                </select>
              </Row>
              <Row title="Detect changes by" subtitle="Checksums also catch edits that kept the date, but read every file.">
                <select class="input w-auto min-w-56" bind:value={draft.options.change_detection} aria-label="Detect changes by">
                  <option value="metadata">Size and time (fast)</option>
                  <option value="checksum">Content checksums (slow)</option>
                </select>
              </Row>
            </div>
            <h3 class="pt-2 text-[13px] font-medium text-fg-muted">How backups run</h3>
            <div class="rows">
              <Toggle row bind:checked={draft.options.continue_on_errors} label="Finish despite unreadable files" hint="They're listed in the backup's details." />
              <Toggle row bind:checked={draft.options.snapshot_if_unchanged} label="Snapshot even when nothing changed" hint={draft.engine === "borg" ? "Borg always makes an archive." : "Usually skipped: the newest snapshot is still up to date."} disabled={draft.engine === "borg"} reason="Borg makes an archive every time." />
              <Row title="Disk priority" subtitle="Lower priority keeps the computer responsive during backups.">
                <select class="input w-auto min-w-56" bind:value={draft.options.io_priority} aria-label="Disk priority">
                  <option value="normal">Normal</option>
                  <option value="best_effort_low">Low</option>
                  <option value="idle">Only when the disk is idle</option>
                </select>
              </Row>
              <Row title="CPU priority: nice {draft.options.nice}" subtitle="0 normal · 19 lowest">
                <input type="range" min="0" max="19" class="w-40 accent-[var(--c-primary)]" bind:value={draft.options.nice} aria-label="CPU priority" />
              </Row>
              <Row title="Worker threads" subtitle="0 lets Flux decide.">
                <NumberInput bind:value={draft.options.threads} max={256} label="Worker threads" />
              </Row>
            </div>
            <h3 class="pt-2 text-[13px] font-medium text-fg-muted">Restoring and notifications</h3>
            <div class="rows">
              <Toggle row bind:checked={draft.options.backup_before_restore} label="Keep my files when restoring over them" hint="Differing files are renamed instead of replaced." />
              <Toggle row bind:checked={draft.options.notify} label="Notify me about problems" hint="A desktop notification when a backup fails or skips files." />
            </div>
          {/if}
        </div>
        <!-- Pinned to the dialog's bottom edge, so Done is always in reach. -->
        <div class="sticky bottom-0 z-10 -mx-6 mt-6 -mb-6 flex items-center gap-3 border-t border-sep bg-surface-1 px-6 py-3">
          <span class="text-[12.5px] text-fg-muted">{creating ? "Applies to this new backup." : "Saved when you click Save."}</span>
          <button class="btn btn-primary ml-auto" onclick={() => openPage(null)}>Done</button>
        </div>
      </Modal>
    {/if}

    <!-- Footer -->
    {#if creating && done}
      <!-- The closing screen carries its own buttons. -->
    {:else if creating}
      {@const issue = stepIssue(step)}
      <footer class="flex items-center gap-3 border-t border-sep bg-surface-1 px-10 py-3">
        <button class="btn btn-ghost -ml-3" onclick={oncancel}>Cancel</button>
        {#if issue}<span class="min-w-0 truncate text-sm text-fg-2 first-letter:uppercase">To {step === "review" ? "finish" : "continue"}: {issue}</span>{/if}
        <div class="ml-auto flex shrink-0 gap-2">
          {#if stepIndex > 0}<button class="btn" onclick={back}><Icon name="chevronLeft" size={15} /> Back</button>{/if}
          {#if step !== "review"}
            <button class="btn btn-primary" onclick={next} disabled={!!issue}>Continue <Icon name="chevronRight" size={15} /></button>
          {:else}
            <button class="btn btn-primary" onclick={save} disabled={saving || !!issue}>{saving ? "Creating…" : "Create backup"}</button>
          {/if}
        </div>
      </footer>
    {:else}
      <footer class="flex items-center gap-3 border-t border-sep bg-surface-1 px-10 py-3">
        {#if totalErrors}
          {@const first = issues.find((i) => i.severity === "error")!}
          <button class="flex min-w-0 items-center gap-1.5 text-left text-sm {creating ? 'text-fg-2' : 'text-danger'} hover:underline" onclick={() => void jumpTo(first.field)}>
            {#if creating}
              <span class="truncate">To finish: {first.message}{totalErrors > 1 ? ` (and ${totalErrors - 1} more)` : ""}</span>
            {:else}
              <Icon name="alert" size={15} /> {totalErrors === 1 ? "One thing to fix" : `${totalErrors} things to fix`}
            {/if}
          </button>
        {:else if pwIssue}
          <span class="truncate text-sm text-fg-2">To finish: {pwIssue}</span>
        {:else if dirty && !creating}
          <span class="text-sm text-fg-muted">Unsaved changes</span>
        {/if}
        <div class="ml-auto flex gap-2">
          <button class="btn btn-ghost" onclick={oncancel}>Cancel</button>
          <button class="btn btn-primary" onclick={save} disabled={saving || totalErrors > 0 || !!pwIssue || (!dirty && !pw) || !profileId}>
            {saving ? "Saving…" : creating ? "Create backup" : "Save"}
          </button>
        </div>
      </footer>
    {/if}
  </div>
{/if}

{#if addingDest && id && draft}
  <DestinationDialog
    {id}
    taken={(draft.destinations ?? []).map((d) => d.name)}
    mainEngine={draft.engine}
    mainRemote={draft.target.kind === "ssh"}
    onadded={(saved) => {
      syncDestinations(saved);
      addingDest = false;
    }}
    onclose={() => (addingDest = false)}
  />
{/if}
