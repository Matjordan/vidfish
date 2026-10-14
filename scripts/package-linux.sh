#!/usr/bin/env bash
set -euo pipefail
: "${VIDFISH_VERSION:?VIDFISH_VERSION must be set}"
case "$VIDFISH_VERSION" in *[!0-9A-Za-z.+-]*) echo 'Invalid version' >&2; exit 1;; esac
root="dist/linux/vidfish-${VIDFISH_VERSION}"
mkdir -p "$root/docs" "$root/assets"
cp target/release/vidfish target/release/vidfish-maintenance README.md "$root/"
cp docs/media.md docs/cameras.md docs/recording.md docs/monitoring.md docs/events.md docs/sessions.md "$root/docs/"
cp scripts/launch-vidfish.sh scripts/install.sh scripts/uninstall.sh "$root/"
cp assets/vidfish.* "$root/assets/"
chmod +x "$root/"*.sh
python3 scripts/make-update-payload.py "$root" "$VIDFISH_VERSION" linux-x86_64
test "$("$root/vidfish" --version)" = "vidfish $VIDFISH_VERSION"
"$root/launch-vidfish.sh" --check
tar -C dist/linux -czf "vidfish-${VIDFISH_VERSION}-linux-x86_64.tar.gz" "vidfish-${VIDFISH_VERSION}"
