#!/usr/bin/env bash
set -euo pipefail
apt_command() {
    local log
    log=$(mktemp)
    if sudo apt-get "$@" >"$log" 2>&1; then
        cat "$log"
        rm -f "$log"
    else
        cat "$log"
        # Surface the actual prerequisite failure through the checks API too.
        if [ -n "${GITHUB_ACTIONS:-}" ]; then
            local message
            message=$(tail -n 15 "$log")
            message=${message//'%'/'%25'}
            message=${message//$'\r'/'%0D'}
            message=${message//$'\n'/'%0A'}
            echo "::error title=Linux media prerequisites failed::$message"
        fi
        rm -f "$log"
        return 1
    fi
}
apt_command update
apt_command install -y libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  libgstrtspserver-1.0-dev gstreamer1.0-tools gstreamer1.0-plugins-good \
  gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly gstreamer1.0-libav xvfb xauth
