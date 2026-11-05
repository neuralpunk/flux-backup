#!/bin/sh
# Install Flux Backup from a source tree, for distributions and `make install`-style
# setups. Run packaging/prepare-dist.sh first (and `npm run tauri build` in app/ for the
# desktop app, which is installed when it was built).
#
#   PREFIX=/usr DESTDIR=/tmp/pkg packaging/install.sh
#
# Installs: flux, flux-backup (if built), flux-helper in libexec, the polkit policy, man
# pages, shell completions, the .desktop file, AppStream metadata and icons.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
target=${CARGO_TARGET_DIR:-$root/target}
dist=$target/dist
PREFIX=${PREFIX:-/usr/local}
DESTDIR=${DESTDIR:-}
d=$DESTDIR$PREFIX

[ -x "$dist/flux" ] || { echo "run packaging/prepare-dist.sh first" >&2; exit 1; }

install -Dm755 "$dist/flux" "$d/bin/flux"
install -Dm755 "$dist/flux-helper" "$d/libexec/flux-backup/flux-helper"
for m in "$dist"/man/*.1.gz; do
    install -Dm644 "$m" "$d/share/man/man1/$(basename "$m")"
done
install -Dm644 "$dist/completions/flux.bash" "$d/share/bash-completion/completions/flux"
install -Dm644 "$dist/completions/_flux" "$d/share/zsh/site-functions/_flux"
install -Dm644 "$dist/completions/flux.fish" "$d/share/fish/vendor_completions.d/flux.fish"

# The policy names the helper's path; point it at this prefix.
policy=io.github.neuralpunk.FluxBackup.helper.policy
mkdir -p "$d/share/polkit-1/actions"
sed "s|/usr/libexec/flux-backup/flux-helper|$PREFIX/libexec/flux-backup/flux-helper|" \
    "$root/packaging/polkit/$policy" > "$d/share/polkit-1/actions/$policy"
chmod 644 "$d/share/polkit-1/actions/$policy"

if [ -x "$target/release/flux-backup" ]; then
    install -Dm755 "$target/release/flux-backup" "$d/bin/flux-backup"
    sed 's/{{name}}/Flux Backup/; s/{{exec}}/flux-backup/g; s/{{icon}}/flux-backup/' \
        "$root/packaging/linux/flux-backup.desktop" > "$target/flux-backup.desktop"
    install -Dm644 "$target/flux-backup.desktop" "$d/share/applications/flux-backup.desktop"
    install -Dm644 "$root/packaging/linux/io.github.neuralpunk.FluxBackup.metainfo.xml" \
        "$d/share/metainfo/io.github.neuralpunk.FluxBackup.metainfo.xml"
    icons=$root/app/src-tauri/icons
    extra=$root/packaging/linux/icons
    hicolor=$d/share/icons/hicolor
    for n in 16 24 48; do
        install -Dm644 "$extra/${n}x${n}/flux-backup.png" "$hicolor/${n}x${n}/apps/flux-backup.png"
    done
    install -Dm644 "$icons/32x32.png" "$hicolor/32x32/apps/flux-backup.png"
    install -Dm644 "$icons/64x64.png" "$hicolor/64x64/apps/flux-backup.png"
    install -Dm644 "$icons/128x128.png" "$hicolor/128x128/apps/flux-backup.png"
    install -Dm644 "$icons/256x256.png" "$hicolor/256x256/apps/flux-backup.png"
    install -Dm644 "$icons/icon.png" "$hicolor/512x512/apps/flux-backup.png"
    install -Dm644 "$extra/flux-backup.svg" "$hicolor/scalable/apps/flux-backup.svg"
    install -Dm644 "$extra/flux-backup-symbolic.svg" "$hicolor/symbolic/apps/flux-backup-symbolic.svg"
fi
echo "installed into $d"
