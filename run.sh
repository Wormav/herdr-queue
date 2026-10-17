#!/bin/sh
set -eu

PROJECT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$PROJECT_DIR"

if [ ! -x .venv/bin/python ]; then
    if ! command -v uv >/dev/null 2>&1; then
        echo "Installez uv, puis exécutez : uv sync --frozen --no-dev" >&2
        exit 1
    fi
    uv sync --frozen --no-dev
fi

exec .venv/bin/python -m herdr_drafts "$@"
