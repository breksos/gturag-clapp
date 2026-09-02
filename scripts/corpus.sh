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

# The model, from wherever the app itself would read it — the same candidate order as
# provision.rs, so a maintainer's machine that has run the app never downloads it twice:
# the launcher's announced store, then the conventional shared location, then this app's
# own copy. The slug is MODEL_ID with anything unsafe in a path folded to `-`.
SLUG="intfloat-multilingual-e5-small"
case "$(uname -s)" in
    Darwin*|Linux*) OWN="$HOME/.$CLI/model" ;;
    *)              OWN="${LOCALAPPDATA:-$HOME}/$CLI/model" ;;
esac
MODEL=""
for d in "${CLATCH_ASSETS_DIR:+$CLATCH_ASSETS_DIR/$SLUG}" "$HOME/.clatch/shared/$SLUG" "$OWN"; do
    [ -n "$d" ] && [ -f "$d/model.safetensors" ] && { MODEL="$d"; break; }
done

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
[ -n "$MODEL" ] && ARGS+=(--model "$MODEL")

exec "$BIN" index-corpus "${ARGS[@]}"
