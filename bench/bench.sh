#!/usr/bin/env bash
# Compare Flux's Mirror engine with rsync --link-dest (the approach Back in Time uses).
#
# usage: bench/bench.sh [WORKDIR]   (default: a temp dir; tmpfs keeps disk effects out)
# Needs: a release build (cargo build --release), rsync, python3.
set -euo pipefail

FLUX="$(cd "$(dirname "$0")/.." && pwd)/target/release/flux"
WORK="${1:-$(mktemp -d)}"
SMALL=${SMALL:-100000}   # small files (1-8 KiB) in 1000 directories
LARGE=${LARGE:-20}       # large files of 20 MiB
mkdir -p "$WORK"
SRC="$WORK/src"; FT="$WORK/flux-target"; RT="$WORK/rsync-target"
rm -rf "$SRC" "$FT" "$RT" "$WORK/home"; mkdir -p "$FT" "$RT"

echo "Generating $SMALL small + $LARGE large files in $SRC ..."
python3 -I - "$SRC" "$SMALL" "$LARGE" <<'PY'
import os, random, sys
src, small, large = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
rnd = random.Random(42)
for i in range(small):
    d = os.path.join(src, f"d{i % 1000:03d}", f"s{(i // 1000) % 10}")
    os.makedirs(d, exist_ok=True)
    with open(os.path.join(d, f"f{i}.txt"), "wb") as f:
        f.write(rnd.randbytes(rnd.randint(1024, 8192)))
os.makedirs(os.path.join(src, "big"), exist_ok=True)
for i in range(large):
    with open(os.path.join(src, "big", f"b{i}.bin"), "wb") as f:
        f.write(os.urandom(20 << 20))
PY
du -sh "$SRC" | cut -f1 | sed 's/^/Source size: /'

export FLUX_HOME="$WORK/home"
"$FLUX" profile create bench -t "$FT" -s "$SRC" >/dev/null
# Force a snapshot even when nothing changed, to compare like with like.
sed -i 's/^snapshot_if_unchanged = false/snapshot_if_unchanged = true/' "$FLUX_HOME/config/profiles/bench.toml"

t() { local s=$(date +%s.%N); "$@" >/dev/null; echo "$(date +%s.%N) - $s" | bc; }
rs() { # rsync snapshot N (link-dest to N-1)
  local n=$1; local ld=(); [ "$n" -gt 1 ] && ld=(--link-dest="$RT/$((n-1))")
  rsync -aHAX --delete "${ld[@]}" "$SRC/" "$RT/$n/"
}
modify() { # touch 1% of the small files and one large file
  python3 -I - "$SRC" "$SMALL" <<'PY'
import os, sys
src, small = sys.argv[1], int(sys.argv[2])
for i in range(0, small, 100):
    p = os.path.join(src, f"d{i % 1000:03d}", f"s{(i // 1000) % 10}", f"f{i}.txt")
    with open(p, "ab") as f: f.write(b"changed")
with open(os.path.join(src, "big", "b0.bin"), "r+b") as f: f.write(b"changed")
PY
}
drop() { sync; }

printf "\n%-34s %10s %10s\n" "scenario" "flux (s)" "rsync (s)"
drop; f=$(t "$FLUX" backup bench); drop; r=$(t rs 1)
printf "%-34s %10.2f %10.2f\n" "1. initial full backup" "$f" "$r"
drop; f=$(t "$FLUX" backup bench); drop; r=$(t rs 2)
printf "%-34s %10.2f %10.2f\n" "2. no changes (new snapshot)" "$f" "$r"
modify
drop; f=$(t "$FLUX" backup bench); drop; r=$(t rs 3)
printf "%-34s %10.2f %10.2f\n" "3. 1% small files + 1 big changed" "$f" "$r"
sed -i 's/^snapshot_if_unchanged = true/snapshot_if_unchanged = false/' "$FLUX_HOME/config/profiles/bench.toml"
f=$(t "$FLUX" backup bench)
printf "%-34s %10.2f %10s\n" "4. no changes (skip, Flux default)" "$f" "n/a"

[ -z "${KEEP:-}" ] && { flux_ro_fix() { chmod -R u+w "$1" 2>/dev/null || true; }; flux_ro_fix "$FT"; rm -rf "$SRC" "$FT" "$RT" "$WORK/home"; }
