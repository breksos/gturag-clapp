#!/usr/bin/env bash
# Prove the two surfaces talk: build → package → validate → CLI ⇄ GUI round-trip.
#
# `clatch validate` reads the manifest and nothing reads the code (PLAYBOOK §4). This
# script is what closes that gap — plus the check PLAYBOOK §9 calls the better half:
# a CLI that FAILS with the app's own "not running" sentence has proved it got as far as
# dialling its socket, which means the two-surface wiring survived packaging.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"

echo "── 1. tests ─────────────────────────────────────────"
# `cargo build` cannot see #[cfg(test)] — test code rots silently while everything looks
# green (PLAYBOOK, field notes).
( cd src-tauri && cargo test --quiet )

echo
echo "── 2. package ───────────────────────────────────────"
# Through `bash`, not as a bare path. The scripts carry the executable bit in the index
# now, but a source zip from a GitHub release drops it, and so does a clone with
# core.fileMode=false — and the symptom is `Permission denied` on step 2, on every
# platform except the Windows one this repo is authored on. Same idiom release.yml uses.
bash scripts/package.sh

echo
echo "── 3. the depot's own manifest ──────────────────────"
# Read cliBin OUT of the packaged manifest. verify.sh once looked for bin/<cli> and broke
# the day macOS packaging moved the binary into a .app bundle (PLAYBOOK §12).
CLI_BIN=$(node -p "require('$ROOT/pkg/clatch.json').connector.cliBin")
BIN="$ROOT/pkg/$CLI_BIN"
[ -f "$BIN" ] || { echo "verify: the manifest points at $CLI_BIN, which is not there" >&2; exit 1; }
echo "  cliBin = $CLI_BIN"

echo
echo "── 4. the binary runs at all ────────────────────────"
# The cheapest missing-DLL check is not reading the import table, it is running the
# binary: Windows resolves a PE's imports at process start, so an exe with an unsatisfied
# dependency cannot print --help at all.
"$BIN" --help | head -n 1

echo
echo "── 5. the CLI dials its socket ──────────────────────"
# With no app running this MUST fail, and with the app's own sentence — not a panic, not
# a hang, not exit 0.
if OUT=$("$BIN" status 2>&1); then
    echo "verify: \`status\` succeeded with no app running — is a stale instance up?" >&2
    echo "$OUT" >&2
    exit 1
fi
case "$OUT" in
    *"not running"*) echo "  $OUT" ;;
    *) echo "verify: expected the app's \"not running\" sentence, got:" >&2
       echo "$OUT" >&2; exit 1 ;;
esac

echo
echo "✓ verified — now the real path:"
echo "    clatch install $(ls -t "$ROOT"/*.clapp | head -n 1)"
echo "    clatch run $(node -p "require('$ROOT/clatch.json').id")"
echo "    $(node -p "require('$ROOT/clatch.json').connector.cli") status"
