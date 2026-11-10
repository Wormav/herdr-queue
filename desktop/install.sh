#!/bin/sh
set -eu

source_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
app_home=${HERDR_APP_HOME:-"$HOME/.config/herdr-app"}

mkdir -p "$app_home/layouts" "$app_home/mods/herdr-queue" "$app_home/mods/tabs-queue"
cp "$source_dir/layouts/main-queue.json" "$app_home/layouts/main-queue.json"
cp "$source_dir/mods/herdr-queue/"*.js "$source_dir/mods/herdr-queue/mod.json" "$source_dir/mods/herdr-queue/"*.svg "$app_home/mods/herdr-queue/"
cp "$source_dir/mods/tabs-queue/"*.js "$source_dir/mods/tabs-queue/mod.json" "$app_home/mods/tabs-queue/"

printf '%s\n' "Herdr Queue Desktop installed in $app_home"
printf '%s\n' "Switch to the main-queue layout in Herdr Desktop."
