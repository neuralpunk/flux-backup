---
name: Flux Backup website (Titanium)
description: The landing page and user guide; a precision instrument's spec page, the real app framed in brushed metal.
colors:
  graphite: "#18191c"
  graphite-raised: "#202125"
  graphite-high: "#2a2b30"
  graphite-deep: "#0f1012"
  ink: "#f3f4f6"
  ink-2: "#b9bcc4"
  muted: "#9195a0"
  line: "rgb(255 255 255 / 0.1)"
  line-2: "rgb(255 255 255 / 0.16)"
  metal-light: "#3a3c42"
  metal-dark: "#26272b"
  metal-mid: "#34363c"
  electric-blue: "#3d8bff"
  blue-fill: "#1c64d6"
  blue-press: "#1757bd"
  blue-text: "#8ab8ff"
  ok: "#57d38c"
  code-comment: "#7c818c"
typography:
  display:
    fontFamily: "Geist, ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(3.4rem, 8.4vw, 7.4rem)"
    fontWeight: 750
    lineHeight: 0.92
    letterSpacing: "-0.055em"
  display-guide:
    fontFamily: "Geist, ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(3rem, 7vw, 5.6rem)"
    fontWeight: 750
    lineHeight: 0.94
    letterSpacing: "-0.055em"
  display-close:
    fontFamily: "Geist, ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(2.8rem, 7vw, 6rem)"
    fontWeight: 700
    lineHeight: 1.04
    letterSpacing: "-0.04em"
  headline:
    fontFamily: "Geist, ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(2.3rem, 5vw, 4.4rem)"
    fontWeight: 700
    lineHeight: 1.04
    letterSpacing: "-0.04em"
  read:
    fontFamily: "Geist, ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(1.9rem, 4.2vw, 3.6rem)"
    fontWeight: 650
    lineHeight: 1.2
    letterSpacing: "-0.035em"
  figure:
    fontFamily: "Geist, ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(2.2rem, 4vw, 3.4rem)"
    fontWeight: 750
    lineHeight: 1
    letterSpacing: "-0.055em"
    fontFeature: "tnum"
  headline-guide:
    fontFamily: "Geist, ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(1.9rem, 3.2vw, 2.6rem)"
    fontWeight: 700
    lineHeight: 1.04
    letterSpacing: "-0.035em"
  title:
    fontFamily: "Geist, ui-sans-serif, system-ui, sans-serif"
    fontSize: "19px"
    fontWeight: 650
    lineHeight: 1.04
    letterSpacing: "-0.02em"
  lede:
    fontFamily: "Geist, ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(17px, 1.5vw, 20px)"
    fontWeight: 400
    lineHeight: 1.6
  body:
    fontFamily: "Geist, ui-sans-serif, system-ui, sans-serif"
    fontSize: "17px"
    fontWeight: 400
    lineHeight: 1.6
  label:
    fontFamily: "Geist, ui-sans-serif, system-ui, sans-serif"
    fontSize: "13px"
    fontWeight: 500
    lineHeight: 1.6
  mono:
    fontFamily: "Geist Mono, ui-monospace, SFMono-Regular, monospace"
    fontSize: "0.86em"
    fontWeight: 400
    fontFeature: "tnum"
  mono-block:
    fontFamily: "Geist Mono, ui-monospace, SFMono-Regular, monospace"
    fontSize: "14px"
    fontWeight: 400
    lineHeight: 1.8
rounded:
  inline: "6px"
  small: "9px"
  control: "12px"
  frame-inner: "15px"
  code: "16px"
  table: "18px"
  panel: "22px"
  frame: "24px"
spacing:
  wrap: "1220px"
  pad: "clamp(20px, 4vw, 40px)"
  nav-h: "54px"
  section: "clamp(110px, 13vw, 180px)"
  block: "48px"
components:
  button-primary:
    backgroundColor: "{colors.blue-fill}"
    textColor: "#ffffff"
    typography: "{typography.body}"
    rounded: "{rounded.control}"
    padding: "0 24px"
    height: "48px"
  button-primary-hover:
    backgroundColor: "{colors.blue-press}"
  button-metal:
    backgroundColor: "{colors.metal-dark}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "0 24px"
    height: "48px"
  button-small:
    rounded: "{rounded.small}"
    padding: "0 14px"
    height: "36px"
  spec-sheet-cell:
    backgroundColor: "{colors.graphite-raised}"
    textColor: "{colors.ink}"
    padding: "28px 26px 30px"
  panel:
    backgroundColor: "{colors.graphite-raised}"
    textColor: "{colors.ink-2}"
    rounded: "{rounded.panel}"
  code-block:
    backgroundColor: "{colors.graphite-deep}"
    textColor: "#d6d9e0"
    typography: "{typography.mono-block}"
    rounded: "{rounded.code}"
    padding: "20px 22px"
  toc-link:
    textColor: "{colors.ink-2}"
    rounded: "10px"
    padding: "7px 12px"
  toc-link-active:
    backgroundColor: "{colors.metal-dark}"
    textColor: "{colors.ink}"
  chip:
    backgroundColor: "{colors.metal-dark}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "12px 18px"
---

# Design System: Flux Backup website (Titanium)

Scope: `site/index.html` and `site/guide.html`, every rule in `site/assets/style.css`. The desktop app's system is `docs/DESIGN.md`; sidecar for this file is `.impeccable/site-design.json`. Tokens are the `:root` custom properties of `style.css`; the frontmatter names them descriptively (`graphite` is `--bg`, `electric-blue` is `--blue`, the three `metal-*` stops make `--metal`).

## Overview

**Creative North Star: "Titanium: the spec page of a precision instrument"**

Flux is shown the way a pro hardware maker shows a device: the real app, framed in brushed metal, on a graphite ground, with every claim a measured number in a ruled sheet. The page tells its story with a cinematic scroll grammar (the app rises out of the page and flattens, a paragraph lights word by word, figures count up, bars race on one time axis), then gives way to plain, dense reference: a timings table, formats beside a terminal, install cards.

Density is generous at the top of each chapter (headlines to 4.4rem, 110 to 180px section gaps) and tight inside the instruments (hairline-ruled sheets, 13px labels, mono figures). The guide shares every token and swaps the motion for a sticky numbered contents list and a 46rem reading column. Dark only.

**Key Characteristics:**
- Graphite ground (`--bg`) with two raised steps and one deeper well; never pure black.
- Brushed metal (`--metal` with `--metal-edge`) on everything that is a device or an object: screenshot frames, the secondary button, chips, install cards, the close band, the guide's active contents item.
- Hairlines (`--line`) do all the dividing: spec rows, sheet gutters, fact tops, table rows, guide h2s.
- One electric blue, split by job: `--blue` marks, `--blue-fill` carries white text, `--blue-text` is blue type.
- Geist for words at heavy weights with tight negative tracking; Geist Mono only for machine text.

## Colors

A cool neutral graphite scale with brushed-metal objects and a single electric blue that is always either a mark, a fill under white text, or text itself.

### Primary
- **Electric Blue** (`--blue`): Flux's own speed bars, the focus ring, text selection tint. A mark, never a background for text.
- **Blue Fill** (`--blue-fill`): the primary button and the skip link; white text on it measures 5.4:1. Pressed and hover state is **Blue Press** (`--blue-press`, 6.7:1).
- **Blue Text** (`--blue-text`): links, Flux's figures in the spec sheet, race times and timings table, the install cards' "Download", the active contents number. 7.9:1 on the raised surface.

### Tertiary
- **Confirm Green** (`--ok`): the copy button's done state only.

### Neutral
- **Graphite** (`--bg`): page ground, the nav at 78% with blur.
- **Graphite Raised** (`--bg-2`): spec sheet cells, the race, tables, formats list, guide blockquotes, contents hover.
- **Graphite High** (`--bg-3`): inline code chips, the version chip, bar tracks, scrollbars.
- **Graphite Deep** (`--bg-deep`): code blocks and the terminal, the only surface darker than the ground.
- **Brushed Metal** (`--metal`, 135deg: light, dark at 45%, mid): device and object surfaces (see Elevation).
- **Ink / Ink 2 / Muted** (`--ink`, `--ink-2`, `--muted`): headings and figures; reading text and ledes; labels, keys, table heads, fine print, the muted second clause of a heading.
- **Hairline / Hairline Strong** (`--line`, `--line-2`): all rules and inset outlines; strong on hover of metal objects.
- **Code Comment** (`--code-comment`): `#` comments in shell and TOML blocks (4.9:1 on the deep well).

### Named Rules
**The Fill Carries Text Rule.** White text sits only on `--blue-fill` (or `--blue-press`). `--blue` is for bars, marks and focus; never put text on it.

**The Flux Is Blue Rule.** In comparisons, Flux's own figures and bars are blue (`--blue-text`, `--blue`); every other tool is neutral (grey bar, `--ink-2` figure).

## Typography

**Display Font:** Geist (variable 100 to 900, self-hosted, with ui-sans-serif, system-ui)
**Body Font:** Geist
**Label/Mono Font:** Geist Mono (variable, self-hosted, with ui-monospace)

**Character:** One grotesque at two extremes: 700 to 750 with tracking pulled to -0.055em for headlines and figures, regular weight at 17px / 1.6 for reading. Mono appears only where a machine would print the text.

### Hierarchy
- **Display** (750, clamp 3.4 to 7.4rem, 0.92, -0.055em): the opening headline. Guide h1 is a sibling (clamp 3 to 5.6rem, 0.94); the close band's h2 is clamp 2.8 to 6rem at the base 700 / -0.04em.
- **Headline** (700, clamp 2.3 to 4.4rem, 1.04, -0.04em, max 15em, balanced): section heads. A second clause in `<em>` is set upright in `--muted`. Guide h2 is clamp 1.9 to 2.6rem, -0.035em, with a hairline above.
- **Read** (650, clamp 1.9 to 3.6rem, 1.2, -0.035em): the word-lit paragraph only.
- **Figure** (750, clamp 2.2 to 3.4rem, 1, -0.055em, tabular): spec sheet numbers; the unit in `<small>` at 0.42em, 500, `--muted`.
- **Title** (650, 19px, -0.02em): fact heads, pair captions; install-more h3 21px; guide h3 1.3rem, h4 1.06rem.
- **Lede** (400, clamp 17 to 20px, `--ink-2`, max 35 to 40rem): opening lede, section leads, guide lede.
- **Body** (400, 17px / 1.6; 16px under 560px): reading text; guide paragraphs in `--ink-2`.
- **Label** (500 to 600, 13 to 14.5px, `--muted`): sheet keys, table heads, contents title, nav (14px), fine print (13.5px).
- **Mono** (0.86em, tabular): inline code on a `--bg-3` chip, race times, number cells (14px), tick labels (12px), contents counters (11.5px), the version chip (12px), install package names (600, 30px, -0.03em), code blocks (14px / 1.8; 13.5px in the guide; 13px under 560px).

### Named Rules
**The Machine Text Rule.** Geist Mono is only for commands, paths, package names, measurements and counters. Never for headings, labels or decoration.

**The Muted Second Clause Rule.** A two-part headline sets its answer in upright `--muted` (`<em>`), not in a colour or italic.

## Layout

Single centered column: `--wrap` 1220px with `--pad` (clamp 20 to 40px) gutters; the guide layout is 1240px. Sections open `clamp(110px, 13vw, 180px)` apart; content blocks inside a section sit 44 to 48px under the lead. Grids: spec sheet 4 columns (2 at 1100px), facts 3, pair 2, formats/terminal 1fr / 1.1fr, timings/fine print 1.2fr / 1fr, install 3 cards then 2 columns, mosaic 6 columns with 250px rows (one 3x2 tile, two 3x1, three 2x1). Guide: 260px sticky contents plus a 46rem reading column.

Breakpoints (max-width):
- **1100px:** optional nav links hide (the Guide link stays, `.keep`); spec sheet goes to 2 columns.
- **900px, and any window under 640px tall:** the opening is not pinned; its parts run in plain flow with the app flat, and the demo-data note shows under it. At 900px wide the order becomes headline, lede and buttons, app, note, then the spec column; every grid goes to one column; mosaic tiles get a 240px picture box; race lanes put the name on its own row; the guide drops its sidebar for a `<details>` contents list in 2 columns.
- **560px:** 16px body; version chip hides; buttons share the row; frames thin to 5px / 14px radius; the timings table restacks into one block per measure with each figure labelled from `data-l`; formats and mobile contents go to one column.

## Elevation & Depth

Depth is material, not shadow. Surfaces step by tone (deep well, ground, raised, high); objects are brushed metal with a 1px top highlight and a hairline ring (`--metal-edge`); panels carry an inset hairline instead of a border. Real drop shadows exist in exactly two places, both under a device: the opening frame (large, soft, far) and the close band's app icon.

### Shadow Vocabulary
- **Metal edge** (`box-shadow: inset 0 1px 0 rgb(255 255 255 / 0.13), 0 0 0 1px var(--line)`): every metal object at rest.
- **Metal edge, hover** (`inset 0 1px 0 rgb(255 255 255 / 0.18), 0 0 0 1px var(--line-2)`): metal button and install card hover.
- **Panel ring** (`inset 0 0 0 1px var(--line)`): race, tables, formats, terminal, code, mosaic tiles, blockquotes, mobile contents.
- **Device drop** (`0 60px 120px -40px rgb(0 0 0 / 0.8)`, added to the metal edge): screenshot frames.
- **Icon drop** (`0 20px 40px -10px rgb(0 0 0 / 0.6)`): the logo in the close band.

### Named Rules
**The Metal Is For Objects Rule.** Brushed metal marks a thing you could hold or press (a frame, a button, a chip, a card, the active contents item, the close band). Reading surfaces and data panels are flat graphite.

## Shapes

Soft, machined corners that grow with the object: 6px inline chips and bar tracks, 9px small buttons and copy buttons, 10px contents items, 12px buttons and chips, 16px code blocks, 18px tables, 20px mosaic tiles and pair frames, 22px panels and cards, 24px the opening frame (inner image 15px, so the metal shows an even lip). Terminal dots are the only circles. No borders around containers; hairlines divide.

## Components

### Buttons
- **Shape:** machined corners (12px), 48px tall, 0 24px padding, Geist 16px 600.
- **Primary** (`.btn-p`): `--blue-fill` with white text; hover `--blue-press`.
- **Metal** (`.btn-m`): brushed metal, `--ink` text, metal edge; hover brightens the edge only.
- **Small** (`.btn-sm`): 36px, 0 14px, 9px radius, 14px; the nav's Download.
- **States:** press nudges down 1px; focus is a 2px `--blue` outline offset 3px. Under 560px, buttons in a row grow to share it.

### Navigation
Sticky 54px bar, graphite at 78% with 20px blur and saturate, hairline beneath. Brand: 24px logo, 15.5px 700 name, mono version chip on `--bg-3`. Links 14px `--ink-2`, `--ink` on hover and for `aria-current`. Ends in the small primary Download.

### The opening (signature)
A 240vh section whose 100vh inner pins. Headline left (1.4fr) and a four-row spec column right (1fr; hairline-ruled rows, `--muted` key left, 550-weight value right), then the lede and two buttons. The app, in a metal frame, sits below the fold and rises with scroll progress `--p` (0 to 1): it lifts 44vh to 90vh, untilts from 18deg `rotateX` to flat, scales 0.88 to 1, while the head fades and lifts 50px. A soft white light band sweeps across the metal once as it settles. Plain flow below 900px wide, under 640px tall, and for reduced motion.

### The word-lit paragraph
One large paragraph (Read style) split into words by script; each word goes from dim `#747883` to `--ink` (500ms) as it passes 60% of the window. Fully lit for reduced motion and without script.

### Spec sheet
Eight cells in a 4-column grid with 1px hairline gutters (the grid background is `--line`), 22px clipped corners, cells on `--bg-2`. Each: 13px `--muted` key, a Figure with small unit, one `--ink-2` line of context. Flux's speed figures are `--blue-text` and count up over 1500ms (quartic ease-out) once in view.

### Chapters, pair and facts
A headline with its muted clause, a lead, the app in a metal frame (10px lip, 24px radius) that settles 50px upward as it enters (`--v`), and three facts in columns, each under a hairline. The pair is two thinner frames (8px lip, 20px) with captions: a 19px title line, then `--ink-2` text.

### Mosaic
Whole-component crops from the app, never cropped further: each picture is fitted with `object-fit: contain` inside a clipped box (18px inset) on `#1c1c20` with a panel ring, 20px radius, and a 14.5px 550 caption under a hairline. Hover scales the picture 1.02 over 1200ms.

### Speed race
A raised panel (22px, panel ring, 30px padding): lanes of name, 12px track, mono time. Bars share one 0 to 8.5 s scale (`--t / 8.5`), grow from the left over 1500ms when in view; Flux lanes are `--blue` with a 600 `--blue-text` time, others grey. A tick row (0 s, 2, 4, 6, 8) in 12px mono labels the axis.

### Tables
In a raised wrap (18px, panel ring, horizontal scroll). 13px 600 `--muted` heads over a hairline; rows divided by hairlines; numbers right-aligned in 14px mono, Flux's in `--blue-text`. Below 560px the landing timings become one block per measure, each figure labelled.

### Formats and terminal
The formats list is a raised panel of ruled rows: an 18px 650 name spanning two lines, an `--ink-2` description, a `--muted` "opens with" line. The terminal is a deep-well panel with a 42px title strip (three dots, a mono `~`) over a borderless code block.

### Code blocks
Deep well, 16px radius, panel ring, `#d6d9e0` mono at 14px / 1.8, comments in `--code-comment`. A 34px copy button (stroke SVG) appears on hover or focus (always on touch), turning `--ok` with a check for 1.6s after copying.

### Chips (New in 1.0)
Metal pills of 12px radius, 12px 18px padding, 15.5px 550, with an optional regular `--muted` note after the name. Not interactive.

### Install cards
Three metal cards (22px, 28px 26px padding): mono package name at 30px, a 17px 600 "for" line, an `--ink-2` note, and a `--blue-text` Download. Hover lifts 3px over 400ms and brightens the edge.

### Close band and footer
The close band is a full-width brushed-metal region with a hairline above, centered: 84px logo with the icon drop, Display-close heading, one line, the two buttons. The footer sits on `#1f2023` with a hairline: brand, link row in `--ink-2`, and the fine-print licence line that also says screens show demo data.

### Guide
Sticky contents (260px, under the nav): a 13px `--muted` title, then links with `decimal-leading-zero` mono counters (01, 02, ...); hover on `--bg-2`; the section being read gets the metal surface, 600 weight and a `--blue-text` counter, and the list scrolls to keep it visible. The intro holds the Display h1, the lede, and a 14.5px `--muted` meta line (version, scope, GitHub link). Sections open 56px apart; h2s sit under a hairline with a hover anchor; lists use `--muted` markers; code blocks and tables sit 20 to 24px from text; blockquotes (from the builder) are raised panels with a ring and 16px radius.

### Motion
One easing, `--ease` (`cubic-bezier(0.16, 1, 0.3, 1)`). Reveals: `.r` fades and lifts 24px over 900ms at 12% visibility. Scroll-linked: `--p` (opening), `--v` (chapter frames), word lighting, all on one rAF-throttled handler. Once-only: `data-go` starts bars and counts. Controls: 200ms buttons, 160ms copy button. **Reduced motion:** the opening is unpinned and flat, frames don't move, reveals show at once, words are lit, bars are full with no transition, counts don't run, and smooth scrolling is off.

## Do's and Don'ts

### Do:
- **Do** keep the page dark only: graphite `--bg` ground, `color-scheme: dark`.
- **Do** put white text only on `--blue-fill`; use `--blue` for bars, marks and focus, and `--blue-text` for blue words and figures.
- **Do** frame every app screenshot in brushed metal, and show mosaic crops whole with `object-fit: contain`.
- **Do** divide with hairlines (`--line`) and inset rings; tone steps carry depth.
- **Do** set Flux's figures in blue and every other tool in neutral grey, on a shared scale.
- **Do** take every number from PRODUCT.md's benchmarks and say how it was measured next to it.
- **Do** label demo data where the app is shown (the plain-flow opening note, the footer) and keep each screenshot's provenance `.json`.
- **Do** give every scroll effect an end state that reduced motion shows immediately.

### Don't:
- **Don't** put eyebrow or kicker labels above headings; a heading's second voice is its muted `<em>` clause.
- **Don't** use Geist Mono for headings, labels or decoration.
- **Don't** put text on `--blue`, or blue fills behind anything but white button text.
- **Don't** add a light theme or pure black grounds.
- **Don't** wrap containers in borders; use the panel ring.
- **Don't** add drop shadows to panels or cards; shadows belong only under the framed device and the close-band icon.
- **Don't** use glyph or emoji icons; icons are inline stroke SVG.
