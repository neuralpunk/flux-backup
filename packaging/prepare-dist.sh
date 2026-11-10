#!/bin/sh
# Build the command-line parts of a release into $TARGET/dist, for the app's packages
# (tauri.conf.json runs this before bundling) and for install.sh:
#
#   dist/flux, dist/flux-helper          release binaries
#   dist/flux-vault-recover              reads Vaults without Flux; static (musl) when the
#                                        target is installed, so it runs on any Linux
#   dist/man/*.1.gz                      man pages
#   dist/completions/{flux.bash,_flux,flux.fish}
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
target=${CARGO_TARGET_DIR:-$root/target}
dist=$target/dist

cargo build --release --locked --manifest-path "$root/Cargo.toml" -p flux-cli -p flux-helper

# The recovery tool is kept next to Vaults on drives, to be run on whatever computer the
# drive is plugged into years from now: static when possible.
musl=$(uname -m)-unknown-linux-musl
if rustup target list --installed 2>/dev/null | grep -qx "$musl"; then
    cargo build --release --locked --manifest-path "$root/Cargo.toml" -p flux-vault-recover --target "$musl"
    recover=$target/$musl/release/flux-vault-recover
else
    echo "note: $musl isn't installed (rustup target add $musl): flux-vault-recover is built dynamically linked" >&2
    cargo build --release --locked --manifest-path "$root/Cargo.toml" -p flux-vault-recover
    recover=$target/release/flux-vault-recover
fi

rm -rf "$dist"
mkdir -p "$dist/man" "$dist/completions"
cp "$target/release/flux" "$target/release/flux-helper" "$recover" "$dist/"

"$dist/flux" manpages "$dist/man" 2>/dev/null
gzip -9n "$dist"/man/*.1

"$dist/flux" completions bash > "$dist/completions/flux.bash"
"$dist/flux" completions zsh > "$dist/completions/_flux"
"$dist/flux" completions fish > "$dist/completions/flux.fish"

echo "prepared $dist"
