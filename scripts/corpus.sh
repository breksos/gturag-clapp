#!/usr/bin/env bash
# Rebuild the corpus index from forms/ — incrementally, in one command.
#
#   scripts/corpus.sh                 # embed only what changed since the last corpus.gtu
#   scripts/corpus.sh --fresh         # embed everything again
#   scripts/corpus.sh --built 2026-09-02   # pin the date (default: today)
#
# This is the whole update loop after publishing: edit forms/ (add, remove, or re-fetch a
# document), run this, commit corpus.gtu. Every installed app picks it up with `<cli> sync`
# — no release, no reinstall. A removal or a metadata edit needs no model at all; only
# documents whose text changed are embedded, from the model cache every clapp of this
# family shares.
set -euo pipefail
cd "$(dirname "$0")/.."

CLI=$(node -p "require('./clatch.json').connector.cli")
BIN=""
for c in "src-tauri/target/release/$CLI" "src-tauri/target/release/$CLI.exe" "$(command -v "$CLI" || true)"; do
    [ -n "$c" ] && [ -x "$c" ] && { BIN="$c"; break; }
done
[ -n "$BIN" ] || { echo "corpus.sh: no $CLI binary — run \`npm run build\` first" >&2; exit 1; }

# The same shared cache the app reads the model from, so a maintainer's machine that has
# run the app never downloads the model twice. $CLATCH_MODELS_DIR overrides, as in the app.
MODEL_ID="intfloat--multilingual-e5-small"
case "$(uname -s)" in
    Darwin*) CACHE="${CLATCH_MODELS_DIR:-$HOME/Library/Caches/clatch/models}" ;;
    Linux*)  CACHE="${CLATCH_MODELS_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/clatch/models}" ;;
    *)       CACHE="${CLATCH_MODELS_DIR:-${LOCALAPPDATA:-$HOME}/clatch/models}" ;;
esac
MODEL="$CACHE/$MODEL_ID"

ARGS=(forms --out corpus.gtu)
BUILT=""
while [ $# -gt 0 ]; do
    case "$1" in
        --built) BUILT="$2"; shift 2 ;;
        *) ARGS+=("$1"); shift ;;
    esac
done
ARGS+=(--built "${BUILT:-$(date +%F)}")
# Only offered when present: a build that embeds nothing must not fail for want of a model
# it would never load, and one that must embed says so itself.
[ -f "$MODEL/model.safetensors" ] && ARGS+=(--model "$MODEL")

exec "$BIN" index-corpus "${ARGS[@]}"
