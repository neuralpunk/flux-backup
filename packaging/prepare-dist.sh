#!/bin/sh
# Build the command-line parts of a release into $TARGET/dist, for the app's packages
# (tauri.conf.json runs this before bundling) and for install.sh:
#
#   dist/flux, dist/flux-helper          release binaries
#   dist/man/*.1.gz                      man pages
#   dist/completions/{flux.bash,_flux,flux.fish}
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
target=${CARGO_TARGET_DIR:-$root/target}
dist=$target/dist

cargo build --release --locked --manifest-path "$root/Cargo.toml" -p flux-cli -p flux-helper

rm -rf "$dist"
mkdir -p "$dist/man" "$dist/completions"
cp "$target/release/flux" "$target/release/flux-helper" "$dist/"

"$dist/flux" manpages "$dist/man" 2>/dev/null
gzip -9n "$dist"/man/*.1

"$dist/flux" completions bash > "$dist/completions/flux.bash"
"$dist/flux" completions zsh > "$dist/completions/_flux"
"$dist/flux" completions fish > "$dist/completions/flux.fish"

echo "prepared $dist"
