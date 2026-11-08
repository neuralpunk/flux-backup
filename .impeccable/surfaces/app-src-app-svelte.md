---
version: 1
slug: "app-src-app-svelte"
primary_target: "app/src/App.svelte"
related_targets: ["app/src/lib/components"]
---

# Flux Backup desktop app

Scope: the whole desktop app (`app/src`): shell, All backups overview, a backup's pages
(overview, snapshots, activity), new backup and settings, dialogs, preferences.
Mode: **Operate**.

Audience and job: a Linux desktop user, often non-technical, who opens Flux to learn
whether they're protected, to start or watch a backup, to get a file back, or to set a
backup up. First answer the window gives: **am I protected right now?**

Constraints: keep the Stack logo and icon, the sidebar list of backups, the dark graphite
+ calm blue family (light theme kept and derived). IBM Plex is not a commitment. All
behaviour, copy facts and features stay; WCAG 2.2 AA in both themes. Runs in WebKitGTK.

Unresolved: none at direction time.

## Direction contract

THESIS: Flux looks like it shipped with the user's Linux desktop and answers "am I
protected?" in one sentence. It refuses the category default it replaced: a dashboard of
equal-weight rounded boxes nested three deep, tabs that mix using and configuring, and one
blue meaning selected, primary and active at once.

OWN-WORLD: The GNOME/Adwaita screen tradition in graphite and calm blue. A blue-graphite
shell (sidebar, titlebar) around a flatter graphite view; content in grouped lists (one
rounded group, rows split by hairline separators) instead of a box per item. Colour has
one job each: blue = act (primary button, progress, focus), green/amber/red = state,
always with words; selection is a neutral lift. Inter (the face Adwaita Sans is built on)
for UI, JetBrains Mono for machine data in tabular columns. Raised: type carries
hierarchy, no stat boxes (Metro); one strict 8px grid (Crouwel); a fixed magnitude ramp
for data marks (star atlas); dense tabular mono data (Ikeda); a problem stays marked until
seen (event catalog).

STORY: The user opens Flux and reads one sentence ("4 of 5 backups are protected"), sees
which one needs them and why in plain words, and acts with the one blue button. Opening a
backup shows the same answer for it, its history as a strip of days, and its snapshots to
browse and restore. Setting up is a preferences page of grouped rows, not a web form.

FIRST VIEWPORT: Titlebar; sidebar 272px: logo, "All backups" (selected, neutral lift,
amber glyph "1 needs attention"), "Your backups" with a state glyph and "Protected · 59
min ago" per backup, New backup, then System backups and Preferences at the foot. View:
small headerbar label left, "Pause schedules" + blue "Back up all now" right; a 64px state
glyph and a 32px sentence with one plain-words line under it and a flat button to the
backup that needs attention; then a "Your backups" grouped list, one row per backup: glyph,
name, mono source → destination, 30-day history strip, last good copy, next run, chevron.
The failed row carries a red inset edge.

FORM: Adwaita desktop tradition, candidate 7 of 7 on my ordered list (the assigned roll).
Signature move: the protection sentence over a magnitude-ramp history strip, the same
grammar on All backups and on each backup. seed:04e0f933ae5ba750

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance
