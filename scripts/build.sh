#!/usr/bin/env sh
# `npm run build` — build the SHIPPABLE binary. The word "build" means the same thing
# in every clapp: run this and the release binary is current, with the frontend
# EMBEDDED (not fetched from a dev URL). It prints that binary's path.
#
# RE-ENTRANCY: tauri.conf.json's beforeBuildCommand calls back into npm. Two markers say
# "you are that inner call" — TAURI_ENV_PLATFORM (exported by the Tauri v2 CLI) and
# CLAPP_FRONTEND_ONLY (set by lib.sh's tauri_build). Either one means: frontend, then stop.
set -eu
. "$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)/lib.sh"

if [ -n "${TAURI_ENV_PLATFORM:-}" ] || [ -n "${CLAPP_FRONTEND_ONLY:-}" ]; then
  exec npm run --silent build:web
fi

tauri_build
printf '%s\n' "$ROOT/src-tauri/target/release/$(manifest connector.cli)$(exe_suffix)"
