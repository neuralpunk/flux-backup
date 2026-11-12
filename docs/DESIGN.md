---
name: Flux Backup
description: Snapshot backups for the Linux desktop, at home next to GNOME's own apps.
colors:
  shell: "#2e2e32"
  view: "#222226"
  group: "#343438"
  control: "#3d3d42"
  shell-hover: "#38383d"
  track: "#4a4a4f"
  sep: "#29292d"
  selected: "#3b3b40"
  on-selected: "#ffffff"
  fg: "#f4f4f5"
  fg-2: "#c4c4c8"
  fg-muted: "#a8a8ad"
  primary: "#78aeed"
  on-primary: "#0b1626"
  accent: "#3170c8"
  on-accent: "#ffffff"
  brand: "#7abdff"
  chart: "#6aa6ea"
  success: "#72c79a"
  warning: "#e7b65a"
  danger: "#f39488"
  group-edge: "transparent"
  seg-on: "#55555b"
  shell-light: "#ebebed"
  view-light: "#fafafb"
  group-light: "#ffffff"
  control-light: "#e7e7e9"
  shell-hover-light: "#e0e0e2"
  track-light: "#d9d9dc"
  sep-light: "#ebebed"
  selected-light: "#dcdcdf"
  on-selected-light: "#1e1e22"
  fg-light: "#1e1e22"
  fg-2-light: "#45454b"
  fg-muted-light: "#5c5c63"
  primary-light: "#185cb4"
  on-primary-light: "#ffffff"
  accent-light: "#1c6dd0"
  on-accent-light: "#ffffff"
  brand-light: "#1c6dd0"
  chart-light: "#1c6dd0"
  success-light: "#1b6e45"
  warning-light: "#7d5200"
  danger-light: "#b3321f"
  group-edge-light: "rgb(0 0 6 / 0.03)"
  seg-on-light: "#ffffff"
typography:
  display:
    fontFamily: "Inter Variable, Adwaita Sans, Cantarell, ui-sans-serif, system-ui, sans-serif"
    fontSize: "30px"
    fontWeight: 600
    lineHeight: 1.25
    letterSpacing: "-0.015em"
  headline:
    fontFamily: "Inter Variable, Adwaita Sans, Cantarell, ui-sans-serif, system-ui, sans-serif"
    fontSize: "22px"
    fontWeight: 650
    lineHeight: 1.2
    letterSpacing: "-0.012em"
  title:
    fontFamily: "Inter Variable, Adwaita Sans, Cantarell, ui-sans-serif, system-ui, sans-serif"
    fontSize: "15px"
    fontWeight: 650
  dialog-title:
    fontFamily: "Inter Variable, Adwaita Sans, Cantarell, ui-sans-serif, system-ui, sans-serif"
    fontSize: "17px"
    fontWeight: 600
  body:
    fontFamily: "Inter Variable, Adwaita Sans, Cantarell, ui-sans-serif, system-ui, sans-serif"
    fontSize: "14px"
    fontWeight: 400
    fontFeature: "\"cv11\""
  button:
    fontFamily: "Inter Variable, Adwaita Sans, Cantarell, ui-sans-serif, system-ui, sans-serif"
    fontSize: "13.5px"
    fontWeight: 550
  group-title:
    fontFamily: "Inter Variable, Adwaita Sans, Cantarell, ui-sans-serif, system-ui, sans-serif"
    fontSize: "13px"
    fontWeight: 650
  label:
    fontFamily: "Inter Variable, Adwaita Sans, Cantarell, ui-sans-serif, system-ui, sans-serif"
    fontSize: "12.5px"
    fontWeight: 400
  mono:
    fontFamily: "JetBrains Mono, Adwaita Mono, ui-monospace, monospace"
    fontSize: "12px"
    fontWeight: 400
    fontFeature: "\"tnum\""
rounded:
  panel: "14px"
  card: "12px"
  segmented: "10px"
  control: "8px"
  segment: "7px"
  tag: "6px"
  round: "9999px"
spacing:
  view-x: "40px"
  view-top: "24px"
  section: "32px"
  row-x: "16px"
  row-y: "10px"
  row-gap: "14px"
  control-x: "14px"
  group-title: "8px"
components:
  button:
    backgroundColor: "{colors.control}"
    textColor: "{colors.fg}"
    typography: "{typography.button}"
    rounded: "{rounded.control}"
    padding: "0 14px"
    height: "34px"
  button-hover:
    backgroundColor: "{colors.track}"
  button-primary:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.on-accent}"
    rounded: "{rounded.control}"
    padding: "0 14px"
    height: "34px"
  button-ghost:
    backgroundColor: "transparent"
    textColor: "{colors.fg-2}"
    rounded: "{rounded.control}"
    height: "34px"
  button-ghost-hover:
    backgroundColor: "{colors.control}"
    textColor: "{colors.fg}"
  button-icon:
    textColor: "{colors.fg-2}"
    rounded: "{rounded.control}"
    size: "34px"
  button-danger-filled:
    backgroundColor: "{colors.danger}"
    textColor: "{colors.view}"
    rounded: "{rounded.control}"
    height: "34px"
  input:
    backgroundColor: "{colors.control}"
    textColor: "{colors.fg}"
    typography: "{typography.body}"
    rounded: "{rounded.control}"
    padding: "0 12px"
    height: "36px"
  group:
    backgroundColor: "{colors.group}"
    rounded: "{rounded.card}"
  row:
    backgroundColor: "{colors.group}"
    textColor: "{colors.fg}"
    padding: "10px 16px"
    height: "52px"
  tag:
    textColor: "{colors.fg-2}"
    typography: "{typography.label}"
    rounded: "{rounded.tag}"
    padding: "0 8px"
    height: "24px"
  segmented:
    backgroundColor: "{colors.control}"
    rounded: "{rounded.segmented}"
    padding: "3px"
  segmented-on:
    backgroundColor: "{colors.seg-on}"
    textColor: "{colors.fg}"
    rounded: "{rounded.segment}"
    height: "30px"
  switch:
    backgroundColor: "{colors.track}"
    rounded: "{rounded.round}"
    width: "44px"
    height: "24px"
  switch-on:
    backgroundColor: "{colors.accent}"
  sidebar-item-selected:
    backgroundColor: "{colors.selected}"
    textColor: "{colors.on-selected}"
    rounded: "{rounded.control}"
    padding: "8px 10px"
---

# Design System: Flux Backup

Tokens and component classes live in `app/src/app.css`; the shell is `App.svelte`,
`lib/ui/TitleBar.svelte`, `lib/ui/Logo.svelte` and `lib/components/Sidebar.svelte`.

How the interface behaves (signifiers, rows, menus, tags, confirmations) is in
[UX_PRINCIPLES.md](UX_PRINCIPLES.md); this file is how it looks.

The frontmatter is normative. Unsuffixed colour keys are the dark theme (the default,
`:root, [data-theme="dark"]`); `-light` keys are the same token under
`[data-theme="light"]`. In code every token is a runtime variable `--c-<name>` behind a
Tailwind `--color-<name>`; the code names differ from the keys here for the surfaces:
`shell` is `--c-bg`, `view` is `--c-surface-1`, `group` is `--c-surface-2`, `control` is
`--c-surface-3` and `shell-hover` is `--c-surface-sunken`.

## Overview

**Creative North Star: "At home on the desktop"**

Flux looks like it shipped with the user's Linux desktop. It follows the GNOME/Adwaita
screen tradition in Adwaita's own neutral greys and blue: a shell (titlebar and sidebar) a
step lighter than the view, as in GNOME Files and Settings, and content set in grouped lists, one rounded group with
rows split by hairlines, instead of a box per item. The app answers "am I protected?" in
one sentence before anything else, and type carries the hierarchy; there are no stat
boxes.

Colour has one job each. Blue means act: the one primary button of a view, progress,
focus, an on switch. Green, amber and red mean state, and always sit next to words that
say the same thing. Where you are (the sidebar item, the chosen segment) is a neutral
lift, never blue. Machine data (paths, sizes, counts, times, shortcuts) is JetBrains Mono
with tabular figures; everything else is Inter, the face Adwaita Sans is built on.

The signature is the protection sentence over a 30-day history strip of bars on a fixed
magnitude ramp. The same grammar answers for all backups on the start page and for one
backup on its own page, and the same ramp marks each snapshot.

**Key Characteristics:**
- Two-tone frame: a neutral grey shell a step lighter than the view, which has a 14px
  top-left corner.
- Grouped lists with hairline separators; interactive rows end in a chevron or a control.
- One blue primary button per view; state colours never stand alone without words.
- Selection is a neutral lift, in the sidebar and in segmented controls.
- Inter for UI, JetBrains Mono for machine data, tabular numbers on everything that changes.
- A fixed log-scale magnitude ramp (bar heights) for history and snapshots.

## Colors

Adwaita's neutral greys (no blue cast), one blue for action, and three status hues that
are darker in the light theme so text stays AA.

### Primary
- **Adwaita Blue, as a fill** (`accent` / `on-accent`): the single primary button of a
  view ("Back up all now", "Back up now", "Stop", a dialog's confirm) and the on switch. A
  deeper blue than `primary` so white text on it stays AA (4.9:1 dark, 5.1:1 light).
- **Adwaita Blue, as a line or text** (`primary`): progress fills, focus rings (`2px
  solid`, offset 2px), the caret, text selection (38% mix), the running glyph, blue text,
  and the ring on the item whose context menu is open. Text on a `primary` fill (a wizard
  step, a chosen export) is `on-primary`.
- **Brand Blue** (`brand`): the lit pill of the logo; reserved for the mark.
- **Chart Blue** (`chart`): the one data series in the space chart (at least 3:1 on
  groups); area fill at 10% opacity, dashed projection line.
- **The desktop's accent instead** (*Preferences › App › Accent colour*, the default):
  when the XDG settings portal gives an accent colour (GNOME 47+, KDE), `primary`,
  `on-primary`, `accent` and `chart` are made from it (`lib/accent.ts`) and set inline on
  `<html>`, followed live (`flux://accent`). Each keeps the hue and moves only in OKLCH
  lightness, as far as it must to be as strong as Flux's blue: white on `accent` 4.8:1,
  `primary` 5.3:1 on the groups and the shell, `chart` 4.5:1 on the groups; `on-primary`
  is a near-black of the hue in dark, white in light. GNOME's blue comes out within a
  shade of Flux's own. `brand` (the logo) and the status hues never change. With no
  portal, or no accent set, or *Flux blue* chosen, the tokens above stand.

### Secondary (status)
- **Protected Green** (`success`): protected, a day that kept a copy, the ok glyph.
- **Attention Amber** (`warning`): needs attention, paused schedules, "Runs as root".
- **Failure Red** (`danger`): failed, a day with only failures, the unseen-failure dot,
  destructive menu items and the destructive dialog's confirm (`.btn-danger-filled`).

### Neutral
- **Shell Grey** (`shell`): titlebar and sidebar, the window's own colour; a step lighter
  than the view in dark, a step darker in light, as in libadwaita.
- **View Grey** (`view`): the content area and dialogs.
- **Group Grey** (`group`): grouped lists and cards, one step up from the view.
- **Control Grey** (`control`): buttons, inputs, the segmented track, menus, toasts.
- **Shell Hover** (`shell-hover`): hover on sidebar items.
- **Track** (`track`): button hover, progress and switch tracks, scrollbars.
- **Hairline** (`sep`): the 1px line between rows of a group; darker than the group in
  dark, as in Adwaita.
- **Selected Lift** (`selected` / `on-selected`): the current sidebar item.
- **Segment Lift** (`seg-on`): the chosen button of a segmented control.
- **Group Edge** (`group-edge`): a 1px ring around groups, the view's top-left edge and
  dialogs; transparent in dark, 3% near-black in light, under Adwaita's soft card shadow
  (see Elevation).
- **Ink ramp** (`fg`, `fg-2`, `fg-muted`): text, secondary text, labels and captions.

### Named Rules
**The One Job Rule.** Blue acts, green/amber/red report state, a neutral lift says where
you are. No colour does two of these jobs.

**The One Blue Button Rule.** A view has at most one `.btn-primary`. Its siblings are
neutral `.btn` or `.btn-ghost`.

**The Words-With-Colour Rule.** A status colour always accompanies words that say the same
("Failed 2 days ago", "1 needs attention"). A glyph repeats the state; it never replaces
the sentence. The failed row on All backups is tinted (`danger` at 7%) and its status line
is red text.

### Contrast (WCAG AA, measured from the tokens)
Dark: `fg-muted` is 5.2:1 on groups, 4.6:1 on controls and 4.7:1 on the selected lift;
`primary` text 5.4:1 on groups; status hues 4.9 to 6.6:1 on groups and at least 4.3:1 on
controls (red text on a button 4.8:1); white on `accent` 4.9:1; `on-primary` on `primary`
7.8:1; `view` text on `danger` 7.1:1. Light: `fg-muted` 4.9 to 6.6:1 on every surface;
status hues 4.5 to 6.8:1 on every surface; white on `accent` 5.1:1; white on `danger`
6.2:1; `primary` as text 5.5:1 on the shell and 4.8:1 on the selected lift.

## Typography

**Display Font:** Inter Variable (with Adwaita Sans, Cantarell, system-ui)
**Body Font:** Inter Variable, with the `cv11` alternate on
**Label/Mono Font:** JetBrains Mono 400/500 (with Adwaita Mono, ui-monospace)

**Character:** Inter reads as the desktop's own UI face; JetBrains Mono marks what a
machine produced. Fonts are bundled locally with fontsource so they work offline.

- **Text size** (*Preferences › App*): Small 90%, Default, Large 115%. The webview is
  zoomed (Tauri `set_zoom`), so every size, gap and breakpoint scales together and the
  layout at Large in a window 15% larger is the layout at Default; the window's minimum
  (900×600) scales with it. Kept as `flux.textSize`. The demo zooms `<html>` instead.

### Hierarchy
- **Display** (600, 30px, 1.25, -0.015em, balanced): the protection sentence on All
  backups ("4 of 5 backups are protected.").
- **Headline** (650, 22px, 1.2, -0.012em): page titles (`.page-title`: a backup's name,
  Settings, Preferences, New backup). A backup's own protection sentence uses 22px at 600.
- **Dialog title** (600, 17px).
- **Title** (650, 15px): a heading inside a group (`.title`); the small 15px/600 `fg-2`
  view label "All backups" in the headerbar row.
- **Body** (400, 14px): everything else; sentences under the display at 15px `fg-2`,
  measure capped at 68ch.
- **Button** (550, 13.5px; 650 on primary and danger-filled).
- **Group title** (650, 13px, `fg-2`): the heading above a group (`.group-title`).
- **Label** (400, 12.5px, `fg-muted`): captions, row subtitles, tags; 11.5px for strip
  day labels and menu shortcuts.
- **Mono** (400, 12px, tabular): paths, sizes, counts, shortcuts.

### Named Rules
**The Machine Data Rule.** Paths, sizes, counts and times are JetBrains Mono; every number
that changes while you watch carries tabular figures (`.num` / `.tabular`, automatic on
`.font-mono`) so it doesn't jitter.

**The Sentence Case Rule.** Headings are sentence case and plain words; no uppercase
labels, no letter-spaced captions.

## Layout

**Two layouts** (*Preferences › App › Layout*, `<html data-layout>`, kept in local
storage as `flux.layout`). **Compact** is the default, also for installs from before it
existed (no stored choice reads as Compact); **Full** is the layout described below as it
was through 1.2.0. Compact changes arrangement and density only; colours, type, shapes
and components are the same:
- **Sidebar** 272px; a backup is one line: glyph, name (13.5px/500), and a word or two of
  status on the right (12px, `sideLabel()` in `lib/attention.ts`: "21%", "Failed", "Low
  on space", or for a healthy backup how old its copy is). The name takes what it needs;
  the status gets the rest, down to 3rem, and the whole status is in the item's title.
- **All backups** from 800px of container: a column head row (12px `fg-muted`) and one
  44px row a backup in a grid: glyph 24px, backup (name, status word and path), the
  30-day strip (240px, so every strip starts in the same place and the fixed ramp
  compares across backups), last good copy (7.5rem), next (10.75rem, so "when its drive
  is connected" fits), chevron. Sections 24px apart. Narrower, the Full rows.
- **A backup's header:** the Overview/Snapshots/Activity switcher (28px segments) sits on
  the title's line; the tags have a line of their own under it, the header's whole
  width (so four fit on one line). On the Overview the informational tags (where, when, how stored,
  Flux Sentry) are left out, since the side column says them; tags that warn (on hold,
  system) stay.
- **Overview** fits a 1200×780 window without scrolling when nothing extra needs saying,
  idle or while backing up. From 760px of container: two columns 24px apart.
  - **The two columns are symmetrical.** Each starts with a 28px heading row in the same
    style ("Status" on the left, "Settings at a glance" on the right), 8px above its first
    box, so the first boxes start level; for that, the glance's group titles are the first
    line of their boxes (12px/500 `fg-muted` on `fg` at 3% over `group`). The columns
    stretch to the same height and each one's last block sits at its foot (`mt-auto`), so
    they end level too.
  - Left, 16px between sections: one card holding the answer (a 40px glyph, the sentence
    at 18px/600, the facts line at 12.5px, any detail), a running backup's progress
    (`JobPanel inline`: phase and figures, then the bar with *Shut down when done* at its
    end; Full shows it as a card above the page) and, under a hairline, the 30-day chart
    32px tall (`HistoryStrip low`, the ramp scaled); *Recent backups* in 34px rows, each with what it changed as
    a diff says it (`+214 ~38 −2`, the words in its title; Full says "214 new · 38
    changed · 2 deleted"), a button opening the comparison with the backup before; then
    one flat list of the restore test (a 24px glyph, one truncated line, a 28px *Test
    now*), *Usable without Flux*, and the space as one row (free of size, a 96px meter,
    how full, a chevron) that opens to the forecast and what the snapshots take;
    *Storage* is a folded line when there's no space row; the folded sections as in Full,
    at the column's foot.
  - Right, an 18.5rem column: *Settings at a glance*, one cell a line with the label on
    the left and the value on the right (no chevron; the hover lift says it opens). A
    value that doesn't fit its line switches to its short form ("7d · 4w · 12m · 5y",
    "USB stick +1"), measured as the column's width changes; the full wording is in the
    cell's tooltip and read by screen readers. Open
    unless closed in Compact (remembered apart from Full's, as
    `flux.glanceClosedCompact`); then *Where it's kept* folded to one line with how many
    places and how many failed last time, at the column's foot.
  - The page's bottom padding is 24px instead of 40px (also on the New backup page).
  - **Settings and Preferences: their sections at the side** (`SectionNav`), when the page
    is at least 800px wide (a container query on the scroller; narrower, as in Full).
    The page is 880px: the title over both columns, then a 10.5rem list and the 680px
    form 32px apart. The list is sticky, marks the section in view (`aria-current=
    "location"`, the neutral `selected` lift like the sidebar, never blue) and goes to
    a section on a click (smooth unless motion is reduced; focus moves into it).
    *Advanced* in Preferences opens as it's gone to.
- **Activity:** the run list is 19.5rem wide from a 1100px window (18rem in Full), so
  "Completed with problems" fits.
- **Rows** are at least 44px with 8px block padding, everywhere.

The window is a titlebar (48px) over a row of sidebar (264px; 272px in Compact) and view. The view is
the only rounded thing in the frame: its top-left corner carries the panel radius and a
1px `group-edge` line on its top and left. Content is centred in the view with 40px side
gutters and 24px top padding, at a maximum width per kind of page:

- **All backups:** 1080px, sections 32px apart.
- **A backup** (Overview, Snapshots, Activity): 1180px, sections 24px apart.
- **Preference columns** (Preferences, a backup's Settings, the New backup form): 680px;
  the page header above the New backup and Settings forms sits in a 760px column.

The All backups rows respond to their container, not the window (`@container`). Below
760px a row is glyph, name with status and path, chevron, with the history strip on its
own line. From 760px "Last good copy" and "Next" join as a 200px column. From 1020px the
strip moves onto the first line and the path gets its own line, so a row stays three lines.

Spacing steps are multiples of 4px (row padding 10px by 16px, gaps of 8, 14, 24, 28 and
32px); the direction asked for a strict 8px grid, and the build settled on a 4px rhythm.
Rows are at least 52px tall; controls are 34px, inputs 36px, the segmented control 36px
overall. The minimum window is 900 by 600.

Dialogs are native `<dialog>` elements on a 55% black backdrop, `view` coloured with the
panel radius, 24px padding. The close button is last in the DOM (positioned top right) so
opening a dialog focuses its first real control, not Close. A step that has to be done
before going on (a new password after unlocking with the recovery code) has no Close and
ignores Escape.

## Elevation & Depth

Depth is tonal. Surfaces step up from view to group to control, and the light theme gives
white groups Adwaita's card treatment: a near-invisible ring and a soft two-layer shadow.
Other shadows appear only on things that float above the window or on a chosen segment.

### Shadow Vocabulary
- **Group edge** (`box-shadow: 0 0 0 1px var(--c-group-edge), var(--group-shadow)`):
  groups, lone rows, the view's top-left edge (`-1px -1px 0`, edge only), dialogs and
  toasts. `--group-shadow` is none in dark and Adwaita's card shadow in light
  (`0 1px 3px 1px rgb(0 0 6 / 0.07), 0 2px 6px 2px rgb(0 0 6 / 0.03)`).
- **Dialog** (`box-shadow: 0 24px 64px -16px rgb(0 0 0 / 0.55), 0 0 0 1px var(--c-group-edge)`).
- **Menu** (`box-shadow: 0 18px 48px -12px rgb(0 0 0 / 0.6)` plus a ring of `fg` at 8%).
- **Toast** (`box-shadow: 0 12px 32px -12px rgb(0 0 0 / 0.5), 0 0 0 1px var(--c-group-edge)`).
- **Chosen segment** (`box-shadow: 0 1px 2px rgb(0 0 0 / 0.22), 0 0 0 1px rgb(0 0 0 / 0.04)`).
- **Switch knob** (`box-shadow: 0 1px 2px rgb(0 0 0 / 0.3)`).

### Named Rules
**The Float-Only Shadow Rule.** Only what floats above the window (dialogs, menus, toasts)
casts a real shadow. Groups and rows sit on the view with tone; in light they carry only
Adwaita's soft card shadow, never anything deeper.

## Shapes

Corners shrink as things get smaller: the view's top-left corner and dialogs 14px, groups
and cards and menus and toasts 12px, the segmented track 10px, controls and sidebar items
8px, segments 7px, tags 6px. Round shapes are reserved for things that are round in
Adwaita: window buttons, state glyphs, switches, progress bars and status dots; history
and snapshot bars have 2px corners. The
window itself stays square. Hairline separators run between rows inside a group; a group
has no outline in dark.

## Components

### Buttons
Flat, quiet, Adwaita-proportioned.
- **Shape:** gently rounded (8px), 34px tall, 14px side padding, icon gap 8px.
- **Default:** `control` fill, `fg` text, 13.5px at 550. Hover moves to `track`; active
  dims to 94% brightness; disabled at 45% opacity.
- **Primary:** `accent` fill, `on-accent` (white) text, 650. Hover brightens to 108%. One per view.
- **Ghost:** transparent with `fg-2` text; hover fills with `control`. For secondary
  actions next to a default button ("Fix its settings").
- **Icon:** 34px square, 8px radius (28px in the sidebar and toasts), `fg-2` glyph; hover
  fills with `control`.
- **Danger filled:** the confirm button of a destructive dialog only; `danger` fill with
  `view` text in dark and white in light. `.btn-danger` is red text on a default button.
- **Window buttons:** GNOME's: 24px circles in `control` with 12 to 13px symbolic icons,
  hover to `track`, right-aligned with an 8px gap, after a keyboard-shortcuts icon button.
- **Transitions:** background, colour and filter over 120ms.

### Tags
- **Style:** information, never a control. Outlined, not filled, so it can't be read as a
  button: an inset 1px ring of `fg` at 16%, `fg-2` text, 12.5px, 24px tall, 6px radius.
- **Variants:** `.tag-accent` (green ring at 45%, green text) and `.tag-warn` (amber).

### Grouped lists and cards
- **Group** (`.rows`, `.card`): `group` fill, 12px radius, `group-edge` ring, clipped.
- **Row** (`.row`): at least 52px, 10px by 16px padding, 14px gap. Rows after the first
  carry an inset 1px `sep` line on top. A row outside a group is its own rounded group.
- **Interactive row** (`.row-link`): lifts to `fg` at 5% over `group` on hover (120ms) and
  ends in a chevron or a control; informational rows don't react.
- **Settings row** (`form/Row.svelte`): optional 17px icon, title and subtitle on the
  left, current value and chevron or control on the right; a disabled row says why in
  place of its subtitle.
- **Group title** sits above the group, 13px/650 `fg-2`, 8px below.

### Inputs / Fields
- **Style:** `control` fill, no stroke, 8px radius, 36px tall, 12px side padding, 14px
  text, `fg-muted` placeholder, caret in `primary`.
- **Focus:** 2px `primary` outline at -1px offset.
- **Disabled:** 60% opacity. Field messages sit under the field in 12.5px, red for errors
  and amber for warnings.

### Segmented control
Adwaita toggle group, for views of one thing (a backup's Overview, Snapshots, Activity) or
one choice of a few (theme, destination type, how often).
- **Track:** `control`, 10px radius, 3px padding, 2px gap.
- **Segment:** 30px, 14px padding, 7px radius, 13.5px at 550, `fg-2`; hover `fg` on
  `fg` at 6%.
- **Chosen:** `seg-on` lift with `fg` text and the chosen-segment shadow. Never blue.
- Uses `role="tab"` with `aria-selected`, or `role="radio"` with `aria-checked`.

### Switch
- 44 by 24px, round, `track` off and `accent` on; an 18px `#f4f5f7` knob slides 20px over
  150ms (`cubic-bezier(0.22, 1, 0.36, 1)`). `role="switch"`, disabled at 60%.

### State glyph
- A round 24px mark beside a backup that repeats its state in colour: `.glyph-ok`
  (green), `-warn` (amber), `-bad` (red), `-run` (blue), plain (neutral, `fg` at 7%). The
  tint is the state colour at 16 to 17% with the colour as the icon (13px). It grows to
  48px beside a backup's sentence and 56px beside the All backups sentence. Running
  glyphs spin, one turn per 2.4s.

### Navigation
- **Sidebar** (264px, `shell`): "All backups" with its glyph and a one-line summary, then
  "Your backups" (12.5px/600 `fg-muted` section head with a + icon button), one item per
  backup (glyph, 14px/500 name, 12.5px status in the state's text colour), New backup,
  then System backups (locked: a row with a chevron), and Preferences at the foot. Items
  are 8px-radius, 8px by 10px padding; hover `shell-hover`; current is the `selected`
  lift with `aria-current="page"`. A small red dot marks a failure not yet seen.
- **Folded sidebar** (64px): a column of icons, everything in its place. Each backup's
  status glyph cross-fades to an 18px icon of what it backs up (`lib/backupIcon.ts`:
  home, document, image, music, mail, code, film, settings for system ones, drive for
  `/`, else folder) with a 10px status dot at its corner (`success`, `warning`,
  `danger`, `primary` pulsing while it runs; a 2px ring of the item's background); All
  backups is `layers`, New backup and Preferences their plain icons; the section heads
  become a 28px rule. Every icon is centred 32px in. Names and status fade out (150ms)
  while the width eases 264/272 → 64px (260ms, `cubic-bezier(0.2,0,0,1)`); no motion
  under reduced motion. Pointing (after 350ms) or keyboard focus shows a label to the
  right (`tip`): the name and status. Folded or open by the title bar's sidebar button
  or F9 (GNOME's key for a sidebar), kept as `flux.sidebar`.
- **Titlebar** (48px, `shell`): 24px logo and "Flux Backup" at 13.5px/600 on the left,
  then the sidebar button; drag anywhere to move, double-click to maximise.

### Menus and toasts
- **Menu:** `control`, 12px radius, 4px padding, 13.5px items with 15px icons in
  `fg-muted`, hover and keyboard focus on `track`, mono shortcuts on the right, danger
  items red, separators `fg` at 10%.
- **Toast:** bottom right, 384px wide, `control`, 12px radius, icon in the state colour,
  dismiss icon button.
- **Undo** (deleting snapshots): a bar above the toasts, "Deleting N snapshots" and an
  *Undo* button, with a 2px `primary` line along its foot running out over the eight
  seconds (a still line under reduced motion). Ctrl+Z undoes too, except while typing.

### Progress
- A 6px round `track` bar with a `primary` fill (width eases over 300ms); above it the
  phase and a mono, tabular line of bytes, speed, counts and time left.

### Protection sentence and history strip (signature)
- **Sentence:** the state glyph, then the display sentence ("4 of 5 backups are
  protected."), one plain-words line about the backup that needs you (its name in its
  state colour) and a neutral button to open it. On a backup's page the same grammar runs
  at 22px with last backup, next run and snapshot count beneath.
- **History strip:** the last 30 days, one bar per day. In a row each day is an 8px
  column, 16px tall, holding a 5px bar with 2px corners; as a chart on a backup's page the
  bars share the card's width with 4px gaps and stand up to 40px tall. A day that kept a
  copy is a green bar whose height follows a fixed log ramp (`lib/ramp.ts`): 4px in a row
  (8px as a chart) for up to 1 MiB or nothing changed, up to 16px (40px) for 10 GiB or
  more. A day with only failures is a full-height red bar; a day with nothing is a 2px
  stub of `fg` at 16% on the baseline, so the stubs read as the axis. The ramp is fixed,
  so bars compare across backups. On a backup's page the first day and "Today" are named
  under the chart. The strip carries a sentence summary as its accessible name, and each
  day's whole column carries its title, so thin bars are easy to point at.
- **Snapshots** use the same ramp for each snapshot's mark, a 5px bar 4 to 12px tall
  standing on the text's baseline (stored bytes for Vault and Borg, copied bytes for
  Mirror and rsync); amber when the snapshot finished with problems.

### Motion
Colour and background transitions run 120ms (150ms on switches); progress width eases
over 300ms; running glyphs spin; skeletons pulse. Under `prefers-reduced-motion` every
animation and transition is off.

## Do's and Don'ts

### Do:
- **Do** set content as a grouped list: one 12px group, rows split by 1px `sep` hairlines.
- **Do** give each view at most one `primary` button; everything else is neutral.
- **Do** say the state in words next to every green, amber or red mark.
- **Do** show where you are with the neutral `selected` or `seg-on` lift.
- **Do** set machine data in JetBrains Mono with tabular figures.
- **Do** mark history and snapshots as bars on the fixed log ramp (`lib/ramp.ts`).
- **Do** put a dialog's close button last in the DOM.
- **Do** keep the logo on its dark tile, in the light theme too.

### Don't:
- **Don't** use blue for selection, tabs or state; blue is for acting.
- **Don't** wrap each item in its own box, nest groups inside groups, or build stat boxes.
- **Don't** fill a tag; filled means button.
- **Don't** scale the history ramp to each backup; bars must compare across backups.
- **Don't** go back to dots of different sizes for history; a row of them reads as noise.
- **Don't** add shadows to groups or rows; only floating layers cast them.
- **Don't** round the window; only the view's top-left corner carries the panel radius.

## Implementation notes

- **Progress** comes from each backup's status socket (`flux://job` events, ~5/s), not a
  per-command Channel: backups run as separate processes, so any window (or `flux watch`)
  can follow them.
- **Not implemented:** pausing a running backup. A backup can be stopped and continues
  from where it stopped next time. (Schedules can be paused; that is a different thing.)
- **Theme:** `<html data-theme>` is always the resolved theme, dark or light; components
  use semantic tokens only, so the light theme is another set of values.

### Tauri window setup

The window is undecorated with a custom titlebar (drag to move, double-click to maximise,
GNOME round window buttons, plus `?` for keyboard shortcuts).

```jsonc
// app/src-tauri/tauri.conf.json (excerpt, as shipped)
{
  "app": {
    "windows": [{
      "title": "Flux Backup",
      "width": 1200, "height": 780,
      "minWidth": 900, "minHeight": 600,
      "decorations": false,
      "transparent": false,
      "backgroundColor": "#2e2e32"   // the dark shell; avoids a white flash before the webview paints
    }]
  }
}
```

```jsonc
// app/src-tauri/capabilities/default.json (excerpt)
{
  "permissions": [
    "core:default",
    "core:window:allow-minimize",
    "core:window:allow-toggle-maximize",
    "core:window:allow-close",
    "core:window:allow-start-dragging",
    "core:window:allow-set-theme",
    "dialog:allow-open"
  ]
}
```

**Linux note:** client-side decorations under WebKitGTK can't draw rounded outer corners
on every compositor. The *window* stays square and the view's top-left corner carries the
14px panel radius.

## App icon: "Stack"

Three stacked pills represent snapshots. The top one (the newest) is lit blue, and the
older ones step down to a dim blue. It stays readable at 16px. The pills are the icon's
own metaphor; the UI itself does not use pills.

### Geometry (256 grid)

| Part | Spec |
|---|---|
| Tile | 256×256, `rx 58`, fill `#1b1d21` |
| Pills | 144×32, `rx 16`, x = 56, y = 68 / 112 / 156 (12px gap) |
| Pill 1 (newest) | `#7abdff` (the dark theme's `brand`) |
| Pill 2 | `#32618e` |
| Pill 3 (oldest) | `#193550` |

These are fixed icon colours, not theme tokens: the tile is darker than today's `group`
token. The SVGs use hex values, not oklch, because librsvg (GNOME/KDE icon rendering)
doesn't support oklch. Masters: `app/src-tauri/icons/logo.svg` and `logo-1024.png`;
`packaging/linux/icons/flux-backup.svg` is the same geometry without embedded metadata.

### Tauri bundle icons

`app/src-tauri/icons/` holds the bundle set; regenerate it from the master with the Tauri
CLI (`npx tauri icon icons/logo-1024.png` from `app/src-tauri`). `tauri.conf.json`
references:

```jsonc
"bundle": {
  "icon": ["icons/32x32.png", "icons/64x64.png", "icons/128x128.png", "icons/256x256.png", "icons/icon.png"]
}
```

### Linux desktop integration (.deb / .rpm / AppImage / Flatpak)

`packaging/install.sh` installs the hicolor set under the app ID `flux-backup`:

```
/usr/share/icons/hicolor/{16,24,48}x…/apps/flux-backup.png          ← packaging/linux/icons/NxN/
/usr/share/icons/hicolor/{32,64,128,256}x…/apps/flux-backup.png     ← app/src-tauri/icons/NxN.png
/usr/share/icons/hicolor/512x512/apps/flux-backup.png               ← app/src-tauri/icons/icon.png
/usr/share/icons/hicolor/scalable/apps/flux-backup.svg              ← packaging/linux/icons/flux-backup.svg
/usr/share/icons/hicolor/symbolic/apps/flux-backup-symbolic.svg     ← packaging/linux/icons/flux-backup-symbolic.svg
```

The `.desktop` template (`packaging/linux/flux-backup.desktop`) gets `Icon=flux-backup`.
For Flatpak, rename the files to the reverse-DNS ID used by the metainfo
(`io.github.neuralpunk.FluxBackup`).

### Tray icon

- `app/src-tauri/icons/tray/idle.png`: idle, a grey stack (`#bebebe`, as in
  `flux-backup-symbolic.svg`) that GNOME recolours as a symbolic icon.
- `app/src-tauri/icons/tray/active.png`: a backup is running; the top pill lit blue.

`app/src-tauri/src/tray.rs` embeds both and swaps them as backups start and end:

```rust
const IDLE: &[u8] = include_bytes!("../icons/tray/idle.png");
const ACTIVE: &[u8] = include_bytes!("../icons/tray/active.png");
// …
if let Ok(icon) = Image::from_bytes(if busy { ACTIVE } else { IDLE }) {
    let _ = tray.set_icon(Some(icon));
}
```

(`Image::from_bytes` needs the `image-png` feature on the `tauri` crate.) Desktops without
a tray (GNOME without AppIndicator) carry on without one.

### In-app usage

The titlebar shows the mark at 24px. `lib/ui/Logo.svelte` inlines the same SVG with fixed
hex fills, so it is identical in both themes:

```svelte
<svg viewBox="0 0 256 256" class={klass} aria-hidden="true">
  <rect width="256" height="256" rx="58" fill="#1b1d21" />
  <rect x="56" y="68" width="144" height="32" rx="16" fill="#7abdff" />
  <rect x="56" y="112" width="144" height="32" rx="16" fill="#32618e" />
  <rect x="56" y="156" width="144" height="32" rx="16" fill="#193550" />
</svg>
```

**Don'ts:** don't add gradients or shadows inside the tile, don't change the pill order
(the lit pill is always on top), and don't put the mark on a light tile. The dark tile is
the icon, in the light theme too.
