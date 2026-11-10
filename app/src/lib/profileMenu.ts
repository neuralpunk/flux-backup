// What you can do with a backup profile: one list for its right-click menu (sidebar)
// and its ⋯ button (header). Every item is also on screen somewhere (tabs, buttons).

import { app } from "./state.svelte";
import type { MenuEntry } from "./menu.svelte";
import { isCompat, type ProfileSummary } from "./types";

export function profileMenu(p: ProfileSummary): MenuEntry[] {
  const selected = app.selectedId === p.id && !app.creating;
  return [
    p.running
      ? { label: "Stop backup", icon: "stop", onselect: () => void app.stop(p.id) }
      : {
          label: "Back up now",
          icon: "play",
          shortcut: selected ? "Ctrl+B" : undefined,
          disabled: p.errors.length > 0,
          hint: p.errors.length ? "This profile has problems; fix them in Settings first" : "",
          onselect: () => void app.backupNow(p.id),
        },
    ...(p.running
      ? []
      : [
          {
            label: "Back up now, then shut down",
            icon: "power" as const,
            disabled: p.errors.length > 0,
            hint: "The computer shuts down a minute after the backup ends",
            onselect: () => void app.backupNow(p.id, true),
          },
        ]),
    "separator",
    { label: "Browse snapshots", icon: "history", onselect: () => app.open(p.id, "snapshots") },
    { label: "Activity", icon: "activity", onselect: () => app.open(p.id, "activity") },
    { label: "Settings", icon: "settings", onselect: () => app.open(p.id, "settings") },
    ...(isCompat(p.engine)
      ? [{ label: "Recovery guide", icon: "lifebuoy" as const, onselect: () => (app.dialog = { kind: "recovery", id: p.id }) }]
      : []),
    { label: "Duplicate", icon: "copy", onselect: () => void app.duplicateProfile(p.id) },
    { label: "Save as a template…", icon: "layers", hint: "New backups can start from its settings", onselect: () => (app.dialog = { kind: "save-template", id: p.id }) },
    { label: "Export…", icon: "download", onselect: () => (app.dialog = { kind: "export", id: p.id }) },
    "separator",
    app.pause
      ? { label: "Resume scheduled backups", icon: "play", hint: "All backups' schedules run again", onselect: () => void app.setPause("off") }
      : { label: "Pause all scheduled backups…", icon: "clock", onselect: () => (app.dialog = { kind: "pause" }) },
    "separator",
    {
      label: "Delete…",
      icon: "trash",
      danger: true,
      disabled: p.running,
      hint: p.running ? "Stop the backup first" : "Delete this profile (its backups stay)",
      onselect: () => (app.dialog = { kind: "delete-profile", id: p.id }),
    },
  ];
}
