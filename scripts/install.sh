#!/bin/sh
set -eu
bundle=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
case "$(uname -sm)" in 'Linux x86_64') ;; *) echo 'Vidfish requires Linux x86_64.' >&2; exit 1;; esac
prefix=${HOME:?}/.local
if [ "$#" -gt 0 ]; then
    if [ "$#" -ne 2 ] || [ "$1" != '--prefix' ]; then echo 'Usage: ./install.sh [--prefix /absolute/path]' >&2; exit 1; fi
    prefix=$2
fi
case "$prefix" in /*) ;; *) echo 'Install prefix must be absolute.' >&2; exit 1;; esac
exec "$bundle/vidfish-maintenance" install --bundle "$bundle" --prefix "$prefix"
