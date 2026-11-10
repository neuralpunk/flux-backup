#!/bin/sh
# Build the fuzz targets with libFuzzer and AddressSanitizer, as `cargo fuzz build` does,
# without needing cargo-fuzz. Needs a nightly toolchain (FLUX_FUZZ_TOOLCHAIN, default
# "nightly") and a C++ compiler.
set -eu
cd "$(dirname "$0")"
export RUSTFLAGS="-Cpasses=sancov-module -Cllvm-args=-sanitizer-coverage-level=4 \
-Cllvm-args=-sanitizer-coverage-inline-8bit-counters -Cllvm-args=-sanitizer-coverage-pc-table \
-Cllvm-args=-sanitizer-coverage-trace-compares --cfg fuzzing -Zsanitizer=address \
-Cdebug-assertions -Coverflow-checks"
cargo "+${FLUX_FUZZ_TOOLCHAIN:-nightly}" build --release --target x86_64-unknown-linux-gnu --bins "$@"
echo "built in $(pwd)/target/x86_64-unknown-linux-gnu/release/"
