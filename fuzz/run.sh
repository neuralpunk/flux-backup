#!/bin/sh
# Run every fuzz target for SECONDS (default 300) in parallel; crashes go to
# artifacts/<target>/. Build first with ./build.sh.
cd "$(dirname "$0")"
T=${1:-300}
B=target/x86_64-unknown-linux-gnu/release
for t in vault_pack vault_index vault_tree vault_json mirror_manifest snapshot_meta profile helper_protocol; do
  mkdir -p corpus/$t artifacts/$t
  $B/$t corpus/$t -max_total_time="$T" -rss_limit_mb=2048 -timeout=10 -max_len=65536 \
    -artifact_prefix=artifacts/$t/ -print_final_stats=1 > artifacts/$t.log 2>&1 &
done
wait
for t in vault_pack vault_index vault_tree vault_json mirror_manifest snapshot_meta profile helper_protocol; do
  printf '%-16s %s  crashes: %s\n' "$t" "$(grep -E '^stat::number_of_executed_units' artifacts/$t.log | awk '{print $2}') runs" "$(ls artifacts/$t | wc -l)"
done
