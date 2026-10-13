#!/usr/bin/env sh
set -eu
package_directory=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
if ! "$package_directory/vidfish" --media-check >/dev/null; then
    echo 'Install GStreamer runtime plugins (base, good, bad, ugly, libav) and your OpenGL/X11/Wayland runtime. See docs/media.md.' >&2
    exit 1
fi
exec "$package_directory/vidfish" "$@"
