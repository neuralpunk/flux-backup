#!/usr/bin/env bash
# Vault throughput: a first backup, a full check and a restore, timed, with the source and
# the repository out of the page cache before each step. Made-up data only (random, so it
# doesn't compress, like photos and video).
#
# usage: bench/vault-perf.sh TARGET [WORKDIR]
#   TARGET   where the test Vault goes (the drive to measure); TARGET/vault is replaced
#   WORKDIR  test files, the profile and the restore (default ~/flux-perf-testdata);
#            the files are made once and kept for the next run
# env:
#   FLUX_BINS="old/flux new/flux"  builds to compare (default: target/release/flux); with
#                                  more than one they take turns, so a drive or CPU that
#                                  warms up over the runs doesn't favour one
#   RUNS=1                         rounds
#   STEPS="backup check restore"   what to time
#   MEDIUM=25000 HALF=25000 SMALL=100000 LARGE="1 2 3 4 5 5 3 2"
#                                  1 MiB files, 512 KiB files, 4–64 KiB files, and the
#                                  sizes in GiB of the large ones (default 65.5 GiB in all)
# Needs python3 and GNU time. No root: the page cache is emptied file by file
# (posix_fadvise), which leaves no dirty data behind as long as `sync` ran first.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="${1:?usage: bench/vault-perf.sh TARGET [WORKDIR]}"
WORK="${2:-$HOME/flux-perf-testdata}"
read -r -a BINS <<<"${FLUX_BINS:-$ROOT/target/release/flux}"
RUNS=${RUNS:-1}
STEPS=${STEPS:-backup check restore}
MEDIUM=${MEDIUM:-25000} HALF=${HALF:-25000} SMALL=${SMALL:-100000}
LARGE=${LARGE:-1 2 3 4 5 5 3 2}
SRC="$WORK/src" VT="$TARGET/vault" OUT="$WORK/restore"
SPEC="$MEDIUM $HALF $SMALL $LARGE"

export FLUX_HOME="$WORK/home" FLUX_PASSWORD=bench
export FLUX_SYSTEMD_UNIT_DIR="$WORK/home/units" FLUX_SYSTEMCTL=true FLUX_SECRET_TOOL=false

py() { python3 -I - "$@"; }

if [ "$(cat "$SRC/.spec" 2>/dev/null)" != "$SPEC" ]; then
  echo "Making test files in $SRC ($SPEC) ..."
  rm -rf "$SRC"; mkdir -p "$SRC"
  py "$SRC" "$MEDIUM" "$HALF" "$SMALL" $LARGE <<'PY'
import os, random, sys
from concurrent.futures import ThreadPoolExecutor
root, medium, half, small = sys.argv[1], *map(int, sys.argv[2:5])
large = [int(g) for g in sys.argv[5:]]
rnd = random.Random(7)
small_sizes = [rnd.randint(4 << 10, 64 << 10) for _ in range(small)]
def write(path, size):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        while size:
            n = min(size, 16 << 20)
            f.write(os.urandom(n)); size -= n
jobs = [(f"1m/d{i // 250:03d}/f{i:05d}.bin", 1 << 20) for i in range(medium)]
jobs += [(f"512k/d{i // 250:03d}/f{i:05d}.bin", 512 << 10) for i in range(half)]
jobs += [(f"small/d{i // 500:03d}/s{i:06d}.bin", s) for i, s in enumerate(small_sizes)]
jobs += [(f"large/big{i}.bin", g << 30) for i, g in enumerate(large)]
with ThreadPoolExecutor(8) as ex:
    list(ex.map(lambda j: write(os.path.join(root, j[0]), j[1]), jobs, chunksize=64))
PY
  echo "$SPEC" > "$SRC/.spec"
fi
GIB=$(du -sb --exclude=.spec "$SRC" | awk '{printf "%.1f", $1 / 2^30}')
echo "Source: $GIB GiB in $(find "$SRC" -type f ! -name .spec | wc -l) files; Vault: $VT"

evict() { # drop these trees' clean pages from the page cache
  sync
  py "$@" <<'PY'
import os, sys
for top in sys.argv[1:]:
    for d, _, files in os.walk(top):
        for n in files:
            try:
                fd = os.open(os.path.join(d, n), os.O_RDONLY)
            except OSError:
                continue
            os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED); os.close(fd)
PY
}

# timed LABEL CMD...: wall seconds, CPU seconds (user+sys) and page faults of CMD
timed() {
  local label=$1; shift
  local tf; tf=$(mktemp)
  local s e; s=$(date +%s.%N)
  /usr/bin/time -f "%U %S %R" -o "$tf" "$@" >"$WORK/last-$label.log" 2>&1 \
    || { echo "  $label FAILED (see $WORK/last-$label.log)"; rm -f "$tf"; return 1; }
  e=$(date +%s.%N)
  read -r u sy f < <(tail -1 "$tf"); rm -f "$tf"
  awk -v l="$label" -v w="$(echo "$e - $s" | bc)" -v u="$u" -v s="$sy" -v f="$f" -v g="$GIB" \
    'BEGIN { printf "  %-8s %7.1f s  %6.0f MiB/s   cpu %6.0f s (user %5.0f, sys %5.0f)   faults %5.2fM\n",
             l, w, g * 1024 / w, u + s, u, s, f / 1e6 }'
}

# What Flux recorded about the last backup: phases and the work in each step.
perf_line() {
  py "$FLUX_HOME/state/logs/perf/runs.jsonl" <<'PY'
import json, sys
p = json.loads(open(sys.argv[1]).read().splitlines()[-1]).get("performance") or {}
ph = {x["name"]: x["secs"] for x in p.get("phases", [])}
st = ", ".join(f'{x["name"].split()[0].lower()} {x["secs"]:.0f}' for x in p.get("stages", []))
print(f'           copying {ph.get("Copying", 0):.1f} s at {p.get("throughput", 0) / 2**20:.0f} MiB/s;'
      f' thread-seconds: {st}; {p.get("writes", 0)} writes, {p.get("write_ms") or 0:.0f} ms each')
PY
}

for ((r = 1; r <= RUNS; r++)); do
  for bin in "${BINS[@]}"; do
    echo "run $r: $bin ($("$bin" --version))"
    rm -rf "$VT" "$FLUX_HOME" "$OUT"; mkdir -p "$VT"
    "$bin" profile create perf -t "$VT" -s "$SRC" --engine vault >/dev/null
    sed -i 's/^notify = true/notify = false/' "$FLUX_HOME/config/profiles/perf.toml"
    for step in $STEPS; do
      case $step in
        backup)
          evict "$SRC"
          timed backup "$bin" backup perf && perf_line ;;
        check)
          evict "$VT"
          timed check "$bin" check perf --read-data ;;
        restore)
          evict "$VT"; rm -rf "$OUT"; mkdir -p "$OUT"
          timed restore "$bin" restore perf "$SRC" --to "$OUT"
          rm -rf "$OUT" ;;
      esac
    done
  done
done
rm -rf "$VT"
