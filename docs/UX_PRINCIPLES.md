# UX principles

How Flux's interface should behave so that people understand it without reading about it.
[DESIGN.md](DESIGN.md) is how it looks (the "At home on the desktop" look); this is how it works.
Written 2026-11-08 after a review of the app against the research below.

## What we found

A review of the app (sidebar, profile header, Overview, Snapshots, Settings) found one
root problem behind most complaints: **things didn't look like what they were.**

| Symptom | Why it happens |
|---|---|
| Settings rows ("Usable without Flux", "What to keep") read as information, so people skip them | Weak signifiers: flat rows with a faint button (or a disabled one with no reason) don't look actionable |
| The switch for encryption sits left of its text | Every Linux/macOS/phone settings screen puts it on the right; ours breaks the habit |
| Header tags (path, schedule, engine) look like buttons | Same fill and similar shape as our chip buttons |
| The sidebar reads like a list of settings pages | "Profiles" and bare names; a status dot without a legend |
| "Authenticate to manage system backups" is large and unclear, then a password box appears | It asks for administrator rights without saying what for or what happens next |
| Deleting a profile is hard to find | Only at the bottom of Advanced options |

## Research

* **Signifiers.** In NN/g's eyetracking study, pages with weak signifiers took about 22%
  longer to use: people hunt for what's clickable and doubt what they find. Clear
  signifiers matter most for anything that isn't a conventional link or button.
  [NN/g: Flat UI elements attract less attention and cause uncertainty](https://www.nngroup.com/articles/flat-ui-less-attention-cause-uncertainty/),
  [NN/g: Beyond blue links, making clickable elements recognizable](https://www.nngroup.com/articles/clickable-elements/)
* **Settings rows.** GNOME's boxed lists: title and subtitle on the left, the control on
  the right; a row that opens more shows a go-next arrow; switch rows have their switch at
  the end. [GNOME HIG: Boxed lists](https://developer.gnome.org/hig/patterns/containers/boxed-lists.html)
* **Tags vs. chips vs. badges.** Status and descriptor labels should look static; chip
  styling (filled pills) signals something you can press or remove.
  [Smart Interface Design Patterns: badges, chips, tags, pills](https://smart-interface-design-patterns.com/articles/badges-chips-tags-pills/),
  [Set Product: chip UI design](https://setproduct.com/blog/chip-ui-design)
* **Context menus.** Good as accelerators for secondary actions on an object, grouped by
  that object; poor for anything people need often, and never the only way to an action.
  [NN/g: Designing effective contextual menus](https://www.nngroup.com/articles/contextual-menus-guidelines/),
  [SAP Fiori: contextual menus](https://www.sap.com/design-system/btp/components/extensions/contextual-menus/usage)
* **Administrator prompts.** The system's password dialog can't be restyled (that is a
  security feature) and people are often unsure which password it wants and why. Explain
  first, then ask. [KDE: Authorization agent](https://docs.kde.org/trunk_kf6/en/plasma-workspace/PolicyKit-kde/authorizationagent-dialog.html),
  [Tails: explain the administration password](https://redmine.tails.boum.org/code/issues/15268)
* **Laws of UX** (lawsofux.com): Jakob's law (people expect what other apps do), mental
  models (people think "my backups", not "profiles"), Hick's law and progressive
  disclosure (fewer visible choices, a clear default), Fitts's law (big targets for the
  common action, destructive ones small and apart), Von Restorff (one emphasised thing
  per view).

## Rules

1. **The shape says what it does.** One look tells static from actionable:
   * *Buttons* are filled (chip, tonal or primary).
   * *Rows that open something* are whole-row buttons ending in **›**, and show their
     current value ("Keeps 7 daily, 4 weekly…  ›").
   * *Rows with a switch* have it on the right; the whole row toggles it.
   * *Information* has no fill change on hover and no chevron.
   * *Tags* (where, when, how) are small, squarer and outlined, with an icon: never
     clickable.
2. **Disabled things say why.** A control that can't be used shows the reason in place
   (with a lock icon), e.g. "Fixed once the first backup exists".
3. **Right-click is a shortcut, not a secret.** Every item in a context menu is also on
   screen (a button, or the ⋯ menu next to the object). Menus are grouped by object;
   destructive items come last, in red, after a separator. The Menu key and Shift+F10
   open the focused item's menu; Escape returns focus.
4. **Name things the way people think of them.** "Backups" in the sidebar, not
   "Profiles"; each shows where it goes and how it's doing in words, not just a dot.
5. **Explain before asking for a password**, especially an administrator's: what it
   unlocks, whose password, how long it lasts, what happens next.
6. **Destructive actions confirm in words.** The dialog names the thing, says what goes
   and what stays ("The backups themselves stay on BackupDisk"), and the button names
   the action ("Delete profile"), filled red. Never the default focus.
7. **Progressive disclosure, one group at a time.** No global "advanced" mode: each group
   of settings is a row with its current value and a chevron, opening just that group in a
   dialog over the editor (Done closes it; the editor stays where it was, so you never lose
   your place; Save stays in place). Nothing "stays expanded". (Dialogs replaced sub-pages
   on 2026-11-08: sub-pages that looked like the page they replaced lost people.)
   Rows whose group has a problem show it in red.
8. **One emphasised action per view** (unchanged from DESIGN.md).
9. **Using and configuring are apart.** A backup's views (Overview, Snapshots, Activity)
   are a segmented switcher; its settings are a page of their own behind a Settings
   button, with "‹ Home folder" to go back. (Added 2026-11-08, GUI overhaul.)
10. **One colour, one job.** Blue acts (the one primary button, progress, focus); green,
    amber and red are state and always stand next to words; where you are is a neutral
    lift, never blue. A problem outranks "running": a failure stays visible while another
    backup runs, and stays marked in the sidebar until its backup has been opened.
11. **Answer first.** The start page and each backup's Overview lead with one sentence
    ("4 of 5 backups are protected.") and say what's wrong and what to do in plain words;
    every place that says when a backup runs next uses the same words (`nextRun`).
12. **New things are set up one question at a time.** A new backup is a wizard (What,
    Where, Protection, When, Review, then a closing screen offering the first backup); each
    step says what's still missing before Continue works, and the review links back to every
    answer. Settings of an existing backup stay on one page. (Added 2026-11-08.)

## Building blocks (app/src)

| Need | Use |
|---|---|
| A context menu on anything | `use:contextMenu={{ items: () => [...], label }}` (`lib/menu.svelte.ts`) |
| The same actions as a visible button | `<MenuButton items={...} label="…" />` |
| A settings row (opens something / has a control / informs) | `<Row>` in a `<div class="rows">` (`lib/components/form/Row.svelte`) |
| An on/off setting | `<Toggle row …>` (switch on the right, whole row clickable; `icon` only beside navigation rows) |
| A confirmation | `<ConfirmDialog …>` |
| A descriptor | `<span class="tag"><Icon …/> text</span>` (`tag-accent`, `tag-warn`) |
| Views of one thing, or one choice of a few | `<div class="segmented" role="tablist">` (or `radiogroup`) with `aria-selected` / `aria-checked` buttons |
| A list of paths or patterns | `<ListEditor …>` (a grouped list: one row per item, a last row to add or Browse…) |
| Is a backup protecting you, and when it runs next | `protection()`, `nextRun()`, `lastGood()` (`lib/protection.ts`) |
| A backup's last 30 days | `<HistoryStrip days={status.days} />` (`fill` to spread across a page) |
