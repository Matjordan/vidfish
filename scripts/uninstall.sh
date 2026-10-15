#!/bin/sh
set -eu
bundle=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
prefix=${HOME:?}/.local
if [ "$#" -gt 0 ]; then
    if [ "$#" -ne 2 ] || [ "$1" != '--prefix' ]; then echo 'Usage: ./uninstall.sh [--prefix /absolute/path]' >&2; exit 1; fi
    prefix=$2
fi
exec "$bundle/vidfish-maintenance" uninstall --prefix "$prefix"
