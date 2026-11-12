// The desktop's accent colour in place of Flux's own blue. The desktop gives one colour;
// Flux needs four from it per theme, each readable where it's used: a fill under white
// text (the primary button, an on switch), text and focus rings on the groups, text on
// that colour, and a chart series. Each keeps the hue and is moved in lightness (OKLCH)
// only as far as it must to be readable. Pure (no Svelte), so it's checked with node.

/** Red, green and blue, each 0–1 (as the XDG settings portal gives them). */
export type Rgb = [number, number, number];

export interface AccentTokens {
  /** Text, focus and progress (`--c-primary`). */
  primary: string;
  /** Text on `primary` (`--c-on-primary`). */
  onPrimary: string;
  /** A fill under white text (`--c-accent`). */
  accent: string;
  /** A chart series on the groups (`--c-chart`). */
  chart: string;
}

/** The surfaces the colours must read on, per theme (app.css). */
const SURFACES = {
  dark: { group: [0x34, 0x34, 0x38], shell: [0x2e, 0x2e, 0x32] },
  light: { group: [0xff, 0xff, 0xff], shell: [0xeb, 0xeb, 0xed] },
} as const;

const toLinear = (c: number) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
const fromLinear = (c: number) => (c <= 0.0031308 ? 12.92 * c : 1.055 * c ** (1 / 2.4) - 0.055);

/** WCAG relative luminance. */
function luminance([r, g, b]: Rgb): number {
  return 0.2126 * toLinear(r) + 0.7152 * toLinear(g) + 0.0722 * toLinear(b);
}

/** WCAG contrast ratio of two colours (1–21). */
export function contrast(a: Rgb, b: Rgb): number {
  const [x, y] = [luminance(a), luminance(b)].sort((m, n) => n - m) as [number, number];
  return (x + 0.05) / (y + 0.05);
}

type Lch = [number, number, number];

function toOklch([r, g, b]: Rgb): Lch {
  const [lr, lg, lb] = [toLinear(r), toLinear(g), toLinear(b)];
  const l = Math.cbrt(0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb);
  const m = Math.cbrt(0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb);
  const s = Math.cbrt(0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb);
  const L = 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s;
  const A = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
  const B = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
  return [L, Math.hypot(A, B), Math.atan2(B, A)];
}

/** OKLCH to sRGB, unclipped (a channel outside 0–1 is out of gamut). */
function fromOklchRaw([L, C, h]: Lch): Rgb {
  const A = C * Math.cos(h);
  const B = C * Math.sin(h);
  const l = (L + 0.3963377774 * A + 0.2158037573 * B) ** 3;
  const m = (L - 0.1055613458 * A - 0.0638541728 * B) ** 3;
  const s = (L - 0.0894841775 * A - 1.291485548 * B) ** 3;
  return [
    fromLinear(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s),
    fromLinear(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s),
    fromLinear(-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s),
  ];
}

const inGamut = (c: Rgb) => c.every((v) => v >= -1e-4 && v <= 1 + 1e-4);
const clamp = (c: Rgb): Rgb => c.map((v) => Math.min(1, Math.max(0, v))) as Rgb;

/** OKLCH to sRGB, with the chroma lowered until it fits (the hue and lightness kept). */
function fromOklch([L, C, h]: Lch): Rgb {
  if (inGamut(fromOklchRaw([L, C, h]))) return clamp(fromOklchRaw([L, C, h]));
  let lo = 0;
  let hi = C;
  for (let i = 0; i < 24; i++) {
    const mid = (lo + hi) / 2;
    if (inGamut(fromOklchRaw([L, mid, h]))) lo = mid;
    else hi = mid;
  }
  return clamp(fromOklchRaw([L, lo, h]));
}

/** The colour moved in lightness (`up`: lighter) just far enough that `ok` holds. */
function shift(c: Rgb, up: boolean, test: (x: Rgb) => boolean): Rgb {
  // Checked as it will be written (#rrggbb), so rounding can't tip it under.
  const ok = (x: Rgb) => test(parseHex(hex(x))!);
  if (ok(c)) return c;
  const [L, C, h] = toOklch(c);
  let lo = L;
  let hi = up ? 1 : 0;
  if (!ok(fromOklch([hi, C, h]))) return fromOklch([hi, C, h]);
  for (let i = 0; i < 30; i++) {
    const mid = (lo + hi) / 2;
    if (ok(fromOklch([mid, C, h]))) hi = mid;
    else lo = mid;
  }
  return fromOklch([hi, C, h]);
}

export function hex(c: Rgb): string {
  return `#${c.map((v) => Math.round(Math.min(1, Math.max(0, v)) * 255).toString(16).padStart(2, "0")).join("")}`;
}

const WHITE: Rgb = [1, 1, 1];
const norm = (c: readonly number[]): Rgb => c.map((v) => v / 255) as Rgb;

/** Flux's tokens for the desktop's accent colour `c`, in a theme. */
export function accentTokens(c: Rgb, theme: "dark" | "light"): AccentTokens {
  const group = norm(SURFACES[theme].group);
  const shell = norm(SURFACES[theme].shell);
  const dark = theme === "dark";
  // As strong as Flux's own blue is (white on its fill 4.9:1, its text 5.4:1 on the
  // groups, its chart 4.9:1): so another hue sits at the same weight. Never under AA.
  // A fill under white text, darker if need be.
  const accent = shift(c, false, (x) => contrast(x, WHITE) >= 4.8);
  // Text on the groups and the shell, lighter in dark, darker in light.
  const readable = (x: Rgb) => contrast(x, group) >= 5.3 && contrast(x, shell) >= 5.3;
  const primary = shift(c, dark, readable);
  // Text on it: in dark a near-black of the same hue, in light white.
  const [, , h] = toOklch(primary);
  const onPrimary = dark ? fromOklch([0.2, 0.04, h]) : WHITE;
  const chart = shift(c, dark, (x) => contrast(x, group) >= 4.5);
  return { primary: hex(primary), onPrimary: hex(onPrimary), accent: hex(accent), chart: hex(chart) };
}

/** `#rrggbb` to red, green and blue (0–1); `null` if it isn't one. */
export function parseHex(s: string): Rgb | null {
  const m = /^#?([0-9a-f]{6})$/i.exec(s.trim());
  if (!m) return null;
  const n = parseInt(m[1]!, 16);
  return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
}
