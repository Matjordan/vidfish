#!/usr/bin/env bash
set -euo pipefail
: "${VIDFISH_VERSION:?VIDFISH_VERSION must be set}"
case "$VIDFISH_VERSION" in *[!0-9A-Za-z.+-]*) echo 'Invalid version' >&2; exit 1;; esac
root="dist/linux/vidfish-${VIDFISH_VERSION}"
mkdir -p "$root/docs"
cp target/release/vidfish README.md "$root/"
cp docs/media.md "$root/docs/"
cp scripts/launch-vidfish.sh "$root/"
chmod +x "$root/launch-vidfish.sh"
test "$("$root/vidfish" --version)" = "vidfish $VIDFISH_VERSION"
"$root/launch-vidfish.sh" --check
tar -C dist/linux -czf "vidfish-${VIDFISH_VERSION}-linux-x86_64.tar.gz" "vidfish-${VIDFISH_VERSION}"
