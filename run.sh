#!/bin/sh
set -eu

PROJECT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$PROJECT_DIR"

if [ -x bin/herdr-queue ]; then
    exec bin/herdr-queue "$@"
fi

if [ -x target/release/herdr-queue ]; then
    exec target/release/herdr-queue "$@"
fi

sh install.sh
exec bin/herdr-queue "$@"
