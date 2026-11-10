// Restore tests sized by what they must prove (the arithmetic itself is in
// crates/flux-runner/src/sample.rs; here, only what the settings page shows).

import type { TestGuarantee } from "./types";

/** "1%" or "99.9%" as a fraction of one. */
export function fraction(percent: string | undefined, fallback: number): number {
  const n = Number.parseFloat((percent ?? "").replace("%", ""));
  return Number.isFinite(n) ? n / 100 : fallback;
}

/**
 * The most files a test reads to catch damage to a share `damage` of the files with a
 * certainty `certainty`: n ≥ ln(1 − c) / ln(1 − d). A backup of any size needs no more
 * (the test works out the exact number for its files, which is lower for a small one).
 */
export function sampleBound(damage: string | undefined, certainty: string | undefined): number {
  const d = Math.min(Math.max(fraction(damage, 0.01), 1e-6), 0.999999);
  const c = Math.min(Math.max(fraction(certainty, 0.99), 0), 0.999999);
  return Math.max(1, Math.ceil(Math.log(1 - c) / Math.log(1 - d) - 1e-9));
}

/** What a test that passed proves, as a sentence. */
export function guaranteeWords(g: TestGuarantee): string {
  const files = g.files.toLocaleString("en-US");
  return g.damaged <= 1
    ? `If even one of its ${files} files were damaged, this test would have found it (${g.certainty} certain).`
    : `If ${g.damage} or more of its ${files} files were damaged, this test would have found it (${g.certainty} certain).`;
}
