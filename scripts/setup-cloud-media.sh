#!/usr/bin/env bash
# Rootless Debian 13 cloud setup. Native Windows/Linux deployment uses docs/media.md.
set -euo pipefail
if ! test -r /usr/share/keyrings/debian-archive-keyring.gpg; then
    echo 'This helper needs the Debian archive keyring; use the platform setup in docs/media.md.' >&2
    exit 1
fi
media_prefix=/workspace/.media
apt_prefix=/workspace/.media-apt
mkdir -p "$apt_prefix/lists/partial" "$apt_prefix/archives/partial" "$apt_prefix/empty" "$media_prefix"
cat > "$apt_prefix/sources.list" <<'EOF'
deb [signed-by=/usr/share/keyrings/debian-archive-keyring.gpg] https://deb.debian.org/debian trixie main
deb [signed-by=/usr/share/keyrings/debian-archive-keyring.gpg] https://security.debian.org/debian-security trixie-security main
deb [signed-by=/usr/share/keyrings/debian-archive-keyring.gpg] https://deb.debian.org/debian trixie-updates main
EOF
cat > "$apt_prefix/apt.conf" <<'EOF'
Dir::Etc::main "/workspace/.media-apt/none";
Dir::Etc::parts "/workspace/.media-apt/empty";
Dir::Etc::sourcelist "/workspace/.media-apt/sources.list";
Dir::Etc::sourceparts "/workspace/.media-apt/empty";
Dir::State::lists "/workspace/.media-apt/lists";
Dir::Cache::archives "/workspace/.media-apt/archives";
APT::Sandbox::User "agent";
Acquire::Languages "none";
EOF
APT_CONFIG="$apt_prefix/apt.conf" /usr/bin/apt-get update
# Only download authenticated archives. No dpkg installation/system mutation occurs;
# download-only needs no frontend lock against the read-only system database.
APT_CONFIG="$apt_prefix/apt.conf" /usr/bin/apt-get -o Debug::NoLocking=true \
  --download-only -y --no-install-recommends install libgstreamer1.0-dev \
  libgstreamer-plugins-base1.0-dev libgstrtspserver-1.0-dev \
  gstreamer1.0-tools gstreamer1.0-libav libxkbcommon0 libxkbcommon-x11-0 libgl1 xvfb
python3 - <<'PY'
import pathlib, subprocess
root = pathlib.Path('/workspace/.media')
for package in pathlib.Path('/workspace/.media-apt/archives').glob('*.deb'):
    subprocess.run(['dpkg-deb', '-x', str(package), str(root)], check=True)
for directory in [root/'usr/lib/x86_64-linux-gnu/pkgconfig', root/'usr/share/pkgconfig']:
    for pc in directory.glob('*.pc'):
        data = pc.read_text().replace('prefix=/usr', f'prefix={root}/usr').replace('includedir=/usr', f'includedir={root}/usr').replace('libdir=/usr', f'libdir={root}/usr')
        pc.write_text(data)
for link in (root/'usr/lib/x86_64-linux-gnu').glob('*.so'):
    if link.is_symlink() and not link.exists():
        target = pathlib.Path('/usr/lib/x86_64-linux-gnu')/link.readlink().name
        if target.exists():
            link.unlink()
            link.symlink_to(target)
PY
export PKG_CONFIG_PATH="$media_prefix/usr/lib/x86_64-linux-gnu/pkgconfig:$media_prefix/usr/share/pkgconfig"
export LD_LIBRARY_PATH="$media_prefix/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export GST_PLUGIN_PATH="$media_prefix/usr/lib/x86_64-linux-gnu/gstreamer-1.0"
export XDG_CACHE_HOME=/workspace/.media-cache
pkg-config --modversion gstreamer-1.0 gstreamer-app-1.0 gstreamer-rtsp-server-1.0
"$media_prefix/usr/bin/gst-inspect-1.0" avdec_h264 >/dev/null
"$media_prefix/usr/bin/gst-inspect-1.0" x264enc >/dev/null
echo 'Cloud media prerequisites ready. Activate the prefix as documented in docs/media.md.'
