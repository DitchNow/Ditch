#!/bin/sh
set -eu
workspace=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
exec node "$workspace/scripts/prepare-community-archive.mjs" production "$@"
