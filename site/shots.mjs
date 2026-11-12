#!/usr/bin/env node
// Retake the app's screenshots from its demo (sample data, no backend).
//
//   cd app && npx vite --port 5199 --strictPort &      # the demo
//   node site/shots.mjs                                 # every screenshot
//   node site/shots.mjs overview-dark unusual           # only these
//
// Each one is taken in headless Chrome (google-chrome on the PATH) at 1200×780, device
// scale 2, with the clock set to 4 PM today. Written:
//   docs/screenshots/<name>.png          1200×780 (README, AppStream)
//   site/assets/screenshots/<name>.png   the same, for those the site keeps
//   site/assets/screens/<name>.webp      2400×1560 or an element's crop (the website)
// Needs ImageMagick (`magick`). Look at what it made before committing it.

import { spawn, execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const BASE = process.env.FLUX_DEMO ?? "http://localhost:5199/";
const only = process.argv.slice(2);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ---- Chrome over its debugging protocol ----
const dir = mkdtempSync(join(tmpdir(), "flux-shots-"));
const port = 9300 + Math.floor(Math.random() * 500);
const chrome = spawn("google-chrome", ["--headless=new", `--remote-debugging-port=${port}`, `--user-data-dir=${dir}`, "--no-first-run", "--hide-scrollbars", "--force-prefers-reduced-motion", "about:blank"], { stdio: "ignore" });
let target;
for (let i = 0; i < 60 && !target; i++) {
  try {
    target = (await (await fetch(`http://127.0.0.1:${port}/json`)).json()).find((t) => t.type === "page");
  } catch {
    await sleep(200);
  }
}
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let seq = 0;
const pending = new Map();
ws.onmessage = (m) => {
  const d = JSON.parse(m.data);
  if (d.id && pending.has(d.id)) pending.get(d.id)(d), pending.delete(d.id);
};
const send = (method, params = {}) =>
  new Promise((res) => {
    const id = ++seq;
    pending.set(id, res);
    ws.send(JSON.stringify({ id, method, params }));
  });

// 4 PM today, and running from there (animations and "x minutes ago" still work).
const CLOCK = `(() => { const R = Date; const at = new R(); at.setHours(16, 0, 0, 0); const off = at.getTime() - R.now();
  class D extends R { constructor(...a) { if (a.length) super(...a); else super(R.now() + off); } static now() { return R.now() + off; } }
  window.Date = D;
  try { if (location.protocol !== "about:") localStorage.clear(); } catch {} })();`;

const page = {
  sleep,
  async eval(expr) {
    const r = await send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true });
    if (r.result?.exceptionDetails) throw new Error(`${r.result.exceptionDetails.exception?.description ?? "error"} in: ${expr.slice(0, 200)}`);
    return r.result?.result?.value;
  },
  /** Click the last element matching `sel` whose text contains (or matches) `text`. */
  async click(text, sel = "button, a, [role=radio], [role=menuitem], summary") {
    const test = text instanceof RegExp ? `(${text}).test(t)` : `t.includes(${JSON.stringify(text)})`;
    const ok = await page.eval(`(() => { const els = [...document.querySelectorAll(${JSON.stringify(sel)})].filter(e => { const t = e.textContent.replace(/\\s+/g, " ").trim(); return ${test}; }); const el = els.sort((a, b) => a.textContent.length - b.textContent.length)[0]; if (!el) return false; el.scrollIntoView({ block: "nearest" }); el.click(); return true; })()`);
    if (!ok) throw new Error(`nothing to click: ${text}`);
    await sleep(450);
  },
  async waitFor(expr, ms = 8000) {
    for (const end = Date.now() + ms; Date.now() < end; await sleep(100)) if (await page.eval(`!!(${expr})`)) return;
    throw new Error(`never: ${expr}`);
  },
  /** Scroll so the element whose own text starts with `text` is `at` px from the top of its scroller. */
  async scrollTo(text, at = 16) {
    await page.eval(`(() => { const el = [...document.querySelectorAll("h1,h2,h3,summary,span,p,div")].filter(e => e.textContent.replace(/\\s+/g, " ").trim().startsWith(${JSON.stringify(text)})).at(-1); if (!el) throw new Error("no " + ${JSON.stringify(text)}); let s = el.parentElement; while (s && !(s.scrollHeight > s.clientHeight + 4 && /auto|scroll/.test(getComputedStyle(s).overflowY))) s = s.parentElement; if (!s) return; s.scrollTop += el.getBoundingClientRect().top - s.getBoundingClientRect().top - ${at}; })()`);
    await sleep(350);
  },
  async type(sel, text, enter = false) {
    await page.eval(`(() => { const el = document.querySelector(${JSON.stringify(sel)}); el.focus(); el.value = ${JSON.stringify(text)}; el.dispatchEvent(new Event("input", { bubbles: true })); ${enter ? `el.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true })); el.form?.requestSubmit?.();` : ""} })()`);
    await sleep(500);
  },
  /** The rectangle of an element (CSS px), for a crop. */
  rect: (expr) => page.eval(`(() => { const r = (${expr}).getBoundingClientRect(); return { x: r.x, y: r.y, width: r.width, height: r.height }; })()`),
};

async function open(query) {
  await send("Page.navigate", { url: "about:blank" });
  await send("Page.navigate", { url: BASE + query });
  await page.waitFor(`document.querySelector("aside, nav") && document.body.innerText.includes("Preferences")`);
  await sleep(900);
}

const tmp = join(dir, "shot.png");
async function capture(clip) {
  // Nothing focused or hovered, and no toast, in a screenshot.
  await page.eval(`document.activeElement?.blur?.()`);
  await sleep(150);
  const r = await send("Page.captureScreenshot", { format: "png", ...(clip ? { clip: { ...clip, scale: 1 } } : {}) });
  writeFileSync(tmp, Buffer.from(r.result.data, "base64"));
}
const magick = (...args) => execFileSync("magick", args);

/** A whole-window screenshot: the docs' PNG, and (`web`) the website's WebP at twice the size. */
async function full(name, { web = null, png = true } = {}) {
  await capture();
  if (png) {
    const out = join(ROOT, "docs/screenshots", `${name}.png`);
    magick(tmp, "-filter", "Lanczos", "-resize", "1200x780", "-strip", out);
    const site = join(ROOT, "site/assets/screenshots", `${name}.png`);
    if (existsSync(site)) magick(out, site);
  }
  if (web) magick(tmp, "-quality", "82", join(ROOT, "site/assets/screens", `${web}.webp`));
}
/** One component, for the website, with `pad` px of its surroundings on every side. */
async function crop(name, expr, pad = 10) {
  const r = await page.rect(expr);
  await capture({ x: r.x - pad, y: r.y - pad, width: r.width + 2 * pad, height: r.height + 2 * pad });
  magick(tmp, "-quality", "86", join(ROOT, "site/assets/screens", `${name}.webp`));
}

const snapshotsOf = async (id) => {
  await open(`?profile=${id}&tab=snapshots&theme=dark`);
  await page.waitFor(`document.body.innerText.includes("Restore this whole folder")`);
};
const running = async (theme) => {
  await open(`?profile=home&simulate=backup&theme=${theme}`);
  await page.waitFor(`/Copying\\s*1[6-9]%/.test(document.body.innerText)`, 30000);
};

// ---- The screenshots ----
const shots = {
  "overview-dark": async () => {
    await running("dark");
    await full("overview-dark", { web: "running" });
  },
  "overview-light": async () => {
    await running("light");
    await full("overview-light");
  },
  "glance-dark": async () => {
    await open("?profile=home&theme=dark");
    await page.scrollTo("Settings at a glance", 12);
    await full("glance-dark");
  },
  "space-dark": async () => {
    // Compact: the space is a row of the Overview; opened, its forecast and chart.
    await open("?profile=home&theme=dark");
    await page.click("Space on BackupDisk", "summary");
    await page.click("History and forecast", "summary");
    await page.scrollTo("Restore test passed", 6);
    await full("space-dark");
    // The opened space row down to its chart (not what the snapshots take, below it).
    await crop("d-space", `(() => { const row = [...document.querySelectorAll("details[open]")].find((d) => d.querySelector("summary")?.textContent.includes("Space on")); const end = [...row.querySelectorAll("summary")].find(e => e.textContent.includes("Show as a table")); const a = row.getBoundingClientRect(), b = end.getBoundingClientRect(); return { getBoundingClientRect: () => ({ x: a.x, y: a.y, width: a.width, height: b.bottom - a.y + 14 }) }; })()`);
    // The last 30 days: the chart in the answer card.
    await page.scrollTo("Status", 0);
    await crop("d-days", `document.querySelector('[title^="Last 30 days"]')`);
  },
  "storage-dark": async () => {
    await open("?profile=photos&theme=dark");
    await page.click("Could you recover it on another computer?", "summary");
    await page.scrollTo("No restore test yet", 12);
    await full("storage-dark");
  },
  "unusual": async () => {
    // The demo's unusual changes come only with ?unusual=1 (or ?trouble=1).
    await open("?profile=plain&unusual=1&theme=dark");
    await full("unusual", { web: "unusual", png: false });
  },
  "settings-dark": async () => {
    await open("?profile=home&tab=settings&theme=dark");
    await full("settings-dark");
  },
  "compat-dark": async () => {
    await open("?profile=plain&tab=settings&page=storage&theme=dark");
    await full("compat-dark");
  },
  "rules-dark": async () => {
    await open("?profile=home&tab=settings&page=exclusions&theme=dark");
    await page.click("Browse what's backed up");
    await page.waitFor(`document.body.innerText.includes("What's backed up")`);
    for (const folder of ["Projects", "site"]) {
      await page.click(new RegExp(`^${folder}\\b`), "dialog[open] button");
      await sleep(500);
    }
    await full("rules-dark");
  },
  "snapshots-dark": async () => {
    await snapshotsOf("home");
    await full("snapshots-dark");
  },
  "search-dark": async () => {
    await snapshotsOf("home");
    await page.type(`input[placeholder^="Find in all snapshots"]`, "notes", true);
    await page.waitFor(`document.body.innerText.includes("Search results")`);
    await full("search-dark", { web: "search" });
  },
  "versions-dark": async () => {
    await snapshotsOf("home");
    await page.eval(`(() => { const row = [...document.querySelectorAll("tr, [role=row], li, button, div")].filter(e => e.textContent.trim().startsWith(".bashrc")).at(-1); row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 900, clientY: 400 })); })()`);
    await sleep(400);
    await page.click("Versions of this file");
    await page.waitFor(`document.body.innerText.includes("different versions")`);
    await full("versions-dark");
    await crop("d-versions", `[...document.querySelectorAll("[role=dialog], dialog")].at(-1)`, 0);
  },
  "restore-dark": async () => {
    await snapshotsOf("home");
    await page.click("Restore this whole folder");
    await page.waitFor(`document.body.innerText.includes("Back to where it was")`);
    await page.eval(`[...document.querySelectorAll("label, [role=radio], button")].filter(e => e.textContent.trim().startsWith("Back to where it was")).at(-1).click()`);
    await page.waitFor(`document.body.innerText.includes("What will happen")`);
    await sleep(600);
    await full("restore-dark");
    await crop("d-restore", `[...document.querySelectorAll("div, section")].filter(e => e.textContent.trim().startsWith("What will happen") && e.textContent.includes("already identical")).at(-1)`);
  },
  "archive-dark": async () => {
    await snapshotsOf("home");
    await page.click("More for this snapshot", "button[aria-label], button[title]").catch(async () => {
      await page.eval(`[...document.querySelectorAll("button")].filter(b => /more/i.test(b.getAttribute("aria-label") ?? b.title ?? "")).at(-1).click()`);
      await sleep(400);
    });
    await page.click("Export…");
    await page.waitFor(`document.body.innerText.includes("Kind of archive")`);
    await full("archive-dark");
  },
  "calendar-dark": async () => {
    await snapshotsOf("photos");
    await page.click("Calendar");
    await sleep(400);
    await page.eval(`document.querySelector('button[aria-label="Previous month"]').click()`);
    await sleep(400);
    await page.eval(`document.querySelector('button[aria-label^="Sunday, October 25"]').click()`);
    await sleep(600);
    await full("calendar-dark");
    await crop("d-calendar", `[...document.querySelectorAll("div")].filter(e => e.querySelector('button[aria-label="Previous month"]') && e.querySelectorAll("button").length > 20).at(-1)`, 0);
  },
  "activity-dark": async () => {
    await open("?profile=home&tab=activity&theme=dark");
    await full("activity-dark");
    // A Vault's backup has the most to show: phases, and the steps of storing.
    await open("?profile=docs&tab=activity&theme=dark");
    await page.click("Where the time went", "summary, button");
    await sleep(500);
    await crop("d-timings", `[...document.querySelectorAll("summary")].find(e => e.textContent.includes("Where the time went")).closest("details")`);
  },
  "export-dark": async () => {
    await open("?profile=home&open=export&theme=dark");
    await full("export-dark");
  },
  "recovery-dark": async () => {
    await open("?profile=plain&open=recovery&theme=dark");
    await full("recovery-dark");
  },
  "recover-dark": async () => {
    await open("?open=recover&theme=dark");
    await page.click(/^BackupDisk/);
    await page.click("Look for backups");
    // The drive holds a Vault too: its backups are listed once it's opened.
    await page.waitFor(`document.body.innerText.includes("Open it")`);
    await page.type(`dialog input[type=password], [role=dialog] input[type=password]`, "demo");
    await page.click("Open it");
    await page.waitFor(`document.body.innerText.includes("Which backup?")`);
    await page.click(/^Home folder Encrypted/);
    await sleep(500);
    await full("recover-dark", { web: "recover" });
  },
  "health-dark": async () => {
    await open("?theme=dark");
    await full("health-dark", { web: "health" });
  },
  "table-dark": async () => {
    await open("?theme=dark");
    await page.click(/^Settings$/, "main button, button");
    await sleep(600);
    await full("table-dark");
  },
  "preferences-dark": async () => {
    await open("?view=preferences&paused=1&theme=dark");
    await full("preferences-dark");
  },
  "templates-dark": async () => {
    await open("?new=1&theme=dark");
    await full("templates-dark", { web: "templates" });
  },
  "new-dark": async () => {
    await open("?new=1&theme=dark");
    await page.click("Home folder Everything");
    await page.waitFor(`document.body.innerText.includes("What should Flux back up?")`);
    await full("new-dark");
  },
  "sentry-dark": async () => {
    await open("?new=1&theme=dark");
    await page.click("Home folder Everything");
    await page.click("Continue");
    await page.click("BackupDisk");
    for (let i = 0; i < 3; i++) await page.click("Continue");
    await page.waitFor(`document.body.innerText.includes("Should Flux Sentry keep watch?")`);
    await full("sentry-dark");
  },
  "system-dark": async () => {
    await open("?system=unlocked&profile=system:etc&theme=dark");
    await page.waitFor(`document.querySelector("h1")?.textContent.includes("System settings")`);
    // The toast about unlocking goes by itself.
    await page.waitFor(`!document.body.innerText.includes("System backups unlocked")`, 20000);
    await sleep(500);
    await full("system-dark");
  },
};

let failed = 0;
try {
  await send("Page.enable");
  await send("Runtime.enable");
  await send("Emulation.setDeviceMetricsOverride", { width: 1200, height: 780, deviceScaleFactor: 2, mobile: false });
  await send("Page.addScriptToEvaluateOnNewDocument", { source: CLOCK });
  for (const [name, take] of Object.entries(shots)) {
    if (only.length && !only.includes(name)) continue;
    try {
      await take();
      console.log(`✓ ${name}`);
    } catch (e) {
      failed++;
      console.log(`✗ ${name}: ${e.message}`);
    }
  }
} finally {
  ws.close();
  chrome.kill();
  await sleep(500);
  try {
    rmSync(dir, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
  } catch {
    /* Chrome was still writing its profile: the system clears its temporary folder */
  }
}
process.exit(failed ? 1 : 0);
