// The fixed magnitude ramp for history bars and snapshot marks: how much a backup copied
// (or stored), on a log scale from 1 MiB (or nothing changed) to 10 GiB or more. Fixed, not
// relative to a backup, so marks compare across backups.
const LO = Math.log10(1024 ** 2);
const HI = Math.log10(10 * 1024 ** 3);

/** 0 (up to 1 MiB, or nothing changed) to 1 (10 GiB or more). */
export function ramp(bytes: number): number {
  if (bytes <= 1024 ** 2) return 0;
  return Math.min(1, (Math.log10(bytes) - LO) / (HI - LO));
}
