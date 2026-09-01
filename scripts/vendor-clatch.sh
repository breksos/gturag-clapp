#!/usr/bin/env sh
# Refresh vendor/clatch from the clatch repository at a tag.
#
# WHY THIS EXISTS. clappkit depends on four crates from github.com/arfium/clatch over
# ssh://, which needs a key this machine, a CI runner, or a checkout five years from now
# may not have — PLAYBOOK §8, a dependency you cannot reach is not a dependency. So the
# four crates are copied into this repo at the tag clappkit pins, and
# src-tauri/Cargo.toml's [patch] points cargo at the copy. `cargo build --offline
# --locked` is the test that it worked.
#
#   scripts/vendor-clatch.sh v0.4.3                              # clones over HTTPS
#   CLATCH_REPO=/path/to/clatch scripts/vendor-clatch.sh v0.4.4  # from a local checkout
#
# After running it: bump the tag in clappkit's Cargo.toml too, or the patch will be
# redirecting a version nobody asked for — and re-run `npm run verify`.
set -eu
ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
TAG="${1:-}"
URL="https://github.com/arfium/clatch.git"
CRATES="clatch-core clatch-ipc clatch-pipe clatch-registry"

[ -n "$TAG" ] || { echo "usage: scripts/vendor-clatch.sh <tag>   (e.g. v0.4.3)" >&2; exit 1; }

if [ -n "${CLATCH_REPO:-}" ]; then
  CLATCH="$CLATCH_REPO"
  [ -d "$CLATCH/.git" ] || { echo "no clatch checkout at $CLATCH" >&2; exit 1; }
  git -C "$CLATCH" rev-parse -q --verify "refs/tags/$TAG" >/dev/null \
    || { echo "$CLATCH has no tag $TAG" >&2; exit 1; }
else
  # The repository is public over HTTPS, so the default needs no key and no sibling
  # checkout: a shallow clone of just the tag, thrown away afterwards.
  TMP="$(mktemp -d)"
  trap 'rm -rf "$TMP"' EXIT INT TERM
  git clone --quiet --depth 1 --branch "$TAG" "$URL" "$TMP/clatch"
  CLATCH="$TMP/clatch"
fi

rm -rf "$ROOT/vendor/clatch"
mkdir -p "$ROOT/vendor/clatch"
for c in $CRATES; do
  git -C "$CLATCH" archive "$TAG" "crates/$c" | tar -x -C "$ROOT/vendor/clatch"
done
git -C "$CLATCH" archive "$TAG" LICENSE | tar -x -C "$ROOT/vendor/clatch"

# The crates use workspace inheritance, so they need a workspace root: upstream's, with
# `members` trimmed to what was copied and agent-engine (not vendored) dropped.
git -C "$CLATCH" show "$TAG:Cargo.toml" | awk -v tag="$TAG" '
  /^members  =/ { print "members  = [\"crates/clatch-core\", \"crates/clatch-ipc\", \"crates/clatch-pipe\", \"crates/clatch-registry\"]"; next }
  /^agent-engine =/ { next }
  { print }
' > "$ROOT/vendor/clatch/Cargo.toml.body"
{
  printf '# VENDORED — do not edit by hand. Copied verbatim from github.com/arfium/clatch at\n'
  printf '# tag %s by scripts/vendor-clatch.sh, so that building this app needs no access to\n' "$TAG"
  printf '# that repository: no deploy key, no SSH agent on a CI runner, nothing to configure\n'
  printf '# before `cargo build`. src-tauri/Cargo.toml [patch] points cargo here.\n'
  cat "$ROOT/vendor/clatch/Cargo.toml.body"
} > "$ROOT/vendor/clatch/Cargo.toml"
rm -f "$ROOT/vendor/clatch/Cargo.toml.body"

# Record what is in there, so nobody has to diff against upstream to find out.
{
  printf 'tag:    %s\n' "$TAG"
  printf 'source: %s\n' "$URL"
  printf 'date:   %s\n' "$(date +%Y-%m-%d)"
} > "$ROOT/vendor/clatch/VENDORED"

printf '\nvendored %s:\n' "$TAG"
for c in $CRATES; do printf '  %-18s %s\n' "$c" "$(du -sh "$ROOT/vendor/clatch/crates/$c" | cut -f1)"; done
printf '\nnow: (cd src-tauri && cargo build --offline --locked) && npm run verify\n'
