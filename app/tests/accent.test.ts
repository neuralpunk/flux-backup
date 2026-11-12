import { test } from "node:test";
import assert from "node:assert/strict";
import { accentTokens, contrast, parseHex, hex } from "../src/lib/accent.ts";

// GNOME's nine accent colours, and a few a KDE user might pick.
const GNOME = ["#3584e4", "#2190a4", "#3a944a", "#c88800", "#ed5b00", "#e62d42", "#d56199", "#9141ac", "#6f8396"];
const OTHERS = ["#ffff00", "#00ff00", "#000000", "#ffffff", "#1d99f3", "#808080"];
const SURFACES = { dark: ["#343438", "#2e2e32"], light: ["#ffffff", "#ebebed"] } as const;
const rgb = (s: string) => parseHex(s)!;

test("every accent is readable where it's used, in both themes", () => {
  for (const c of [...GNOME, ...OTHERS]) {
    for (const theme of ["dark", "light"] as const) {
      const t = accentTokens(rgb(c), theme);
      const at = `${c} in ${theme}`;
      assert.ok(contrast(rgb(t.accent), [1, 1, 1]) >= 4.8, `white on the fill, ${at}: ${t.accent}`);
      for (const s of SURFACES[theme]) {
        assert.ok(contrast(rgb(t.primary), rgb(s)) >= 5.3, `text on ${s}, ${at}: ${t.primary}`);
      }
      assert.ok(contrast(rgb(t.onPrimary), rgb(t.primary)) >= 4.5, `text on the primary, ${at}`);
      assert.ok(contrast(rgb(t.chart), rgb(SURFACES[theme][0])) >= 4.5, `the chart on a group, ${at}: ${t.chart}`);
    }
  }
});

test("a colour readable as it is stays as it is", () => {
  // GNOME's blue under white text needs darkening; a dark blue doesn't.
  assert.equal(accentTokens(rgb("#1a5aac"), "light").accent, "#1a5aac");
  assert.equal(accentTokens(rgb("#1a5aac"), "light").primary, "#1a5aac");
  assert.notEqual(accentTokens(rgb("#3584e4"), "dark").accent, "#3584e4");
});

test("the hue is kept", () => {
  // Teal stays teal: green and blue above red, in every token.
  const t = accentTokens(rgb("#2190a4"), "dark");
  for (const v of [t.primary, t.accent, t.chart]) {
    const [r, g, b] = rgb(v);
    assert.ok(g > r && b > r, `${v} is still teal`);
  }
});

test("hex in and out", () => {
  assert.deepEqual(parseHex("#ff8000"), [1, 128 / 255, 0]);
  assert.equal(parseHex("orange"), null);
  assert.equal(hex([0.12941177189350128, 0.5647059082984924, 0.6431372761726379]), "#2190a4");
});
