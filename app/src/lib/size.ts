/** File size limits as profiles write them ("500MB", "2GiB", "1048576"), parsed the way
 * Flux does (flux-core's `ByteSize`): no unit means bytes. */

/** Below this (1 MiB) a limit leaves out nearly everything; same as flux-core. */
export const TINY_SIZE_LIMIT = 1 << 20;

const MULT: Record<string, number> = {
  B: 1,
  KB: 1e3,
  MB: 1e6,
  GB: 1e9,
  TB: 1e12,
  KiB: 2 ** 10,
  MiB: 2 ** 20,
  GiB: 2 ** 30,
  TiB: 2 ** 40,
};
const ALIASES: Record<string, string> = { "": "B", b: "B", kb: "KB", mb: "MB", gb: "GB", tb: "TB", k: "KiB", kib: "KiB", m: "MiB", mib: "MiB", g: "GiB", gib: "GiB", t: "TiB", tib: "TiB" };

export interface ParsedSize {
  number: number;
  /** Canonical unit: B, KB, MB, GB, TB, KiB, MiB, GiB or TiB. */
  unit: string;
  bytes: number;
}

export function parseSize(s: string): ParsedSize | null {
  const m = /^\s*(\d+(?:\.\d+)?)\s*([a-zA-Z]*)\s*$/.exec(s);
  if (!m) return null;
  const unit = ALIASES[m[2]!.toLowerCase()];
  if (!unit) return null;
  const number = Number(m[1]);
  return { number, unit, bytes: Math.floor(number * MULT[unit]!) };
}

/** A limit for people: "500 MB", "2 GiB", "1 byte". */
export function sizeLabel(s: string): string {
  const p = parseSize(s);
  if (!p) return s;
  if (p.unit === "B") return `${p.number} ${p.number === 1 ? "byte" : "bytes"}`;
  return `${p.number} ${p.unit}`;
}
