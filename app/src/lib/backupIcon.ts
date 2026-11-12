// The icon a backup is shown by in the folded sidebar: what it backs up, from its first
// folder. Pure (no Svelte), so it's checked with node.

export type BackupIcon = "home" | "document" | "image" | "music" | "mail" | "code" | "film" | "settings" | "drive" | "server" | "folder";

/** By the last part of a folder's name (lower case). */
const BY_NAME: [RegExp, BackupIcon][] = [
  [/^(documents?|docs|papers|office|dokumente|documentos)$/, "document"],
  [/^(pictures|photos?|images|camera|dcim|bilder|imágenes)$/, "image"],
  [/^(music|audio|sounds?|musik|música)$/, "music"],
  [/^(mail|\.?thunderbird|maildir|evolution|email)$/, "mail"],
  [/^(projects?|code|src|dev|git|repos|workspace|work)$/, "code"],
  [/^(videos?|movies|films?)$/, "film"],
];

export function backupIcon(p: { scope: "user" | "system"; sources?: string[]; remote?: boolean }): BackupIcon {
  const raw = p.sources?.[0] ?? "";
  if (/^\/+$/.test(raw)) return "drive"; // the whole disk
  const first = raw.replace(/\/+$/, "");
  if (first === "~" || /^\/home\/[^/]+$/.test(first)) return "home";
  if (p.scope === "system" || /^\/(etc|var|srv|opt|usr|root)(\/|$)/.test(first)) return "settings";
  const name = first.split("/").pop()?.toLowerCase() ?? "";
  for (const [re, icon] of BY_NAME) if (re.test(name)) return icon;
  // ~/.local/share/mail and the like: any part of the path.
  for (const part of first.toLowerCase().split("/").reverse()) for (const [re, icon] of BY_NAME) if (re.test(part)) return icon;
  return "folder";
}
