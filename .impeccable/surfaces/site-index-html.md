---
version: 1
slug: "site-index-html"
primary_target: "site/index.html"
related_targets: ["site/guide.html","site/assets"]
---

# Flux Backup website (landing page and user guide)

Scope: site/index.html (Persuade) and site/guide.html (Read), sharing site/assets/style.css and site.js. Static HTML, no build step; guide sections come from docs/USER_GUIDE.md via site/build-guide.py.

Audience: Linux desktop users (many from Back In Time) deciding whether to trust Flux with their files; technical users checking formats, speed and the CLI. Job: understand what Flux does in seconds, believe it will get their files back, download 1.0. Proof: the real app (2x screenshots from the demo build), the measured benchmarks, the frozen formats, recovery without Flux. Never invent users, quotes, press or numbers.

Owner's choices: from five directions (Gallery, Keynote, Strata, Timeline, Bento) chose Keynote; from five Keynote variations (Aurora, Ember, Spectrum, Titanium, Nebula) chose Titanium. Wants it modern, Apple-quality, dark, rich rather than sparse, good on phones; the guide follows the same style. Prototype: .impeccable/mocks/site-directions/k/titanium/index.html (gitignored).

## Direction contract

THESIS: Flux is a precision instrument for getting files back, so the site reads like a pro product's spec page: the real app framed in brushed metal, every claim a measured number. It refuses both the sparse black launch page and the card-grid feature page.

OWN-WORLD: Graphite #18191c ground, #202125/#2a2b30 surfaces, brushed-metal gradient frames (#3a3c42 → #26272b → #34363c) with a 1px top highlight, hairline rules rgb(255 255 255 / 0.1), one electric blue #3d8bff for action and Flux's own figures. Geist for everything, Geist Mono for commands, keys and measurements. 12px-radius buttons, 20-24px panels.

STORY: The visitor sees the headline "Linux backups, done right." beside a spec column, then the app rises out of the page and flattens. Scrolling: a sentence lights up word by word, an eight-fact spec sheet counts up, chapters (restore, ransomware, new computer, runs on its own) each with the app in a metal frame, a mosaic of details, a measured speed race, formats beside a terminal, what 1.0 adds, install, close.

FIRST VIEWPORT: Desktop: headline left at ~7.6rem, a four-row spec column right, a lede with Download 1.0 and Specs buttons beneath, the app in a metal frame rising from the bottom edge, tilted, flattening as the page scrolls (pinned 240vh). Phone: headline, lede, buttons, then the framed app static and flat below; the spec column moves under the app.

FORM: Keynote's cinematic scroll grammar (rising app, lit words, counting numbers) in the Titanium language; the owner's pick from a hand of five variations of the roll's pick card. seed:966bd048eddedc94

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance
