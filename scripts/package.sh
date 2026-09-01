#!/usr/bin/env bash
# Assemble pkg/ — the depot Clatch installs — and then the .clapp.
#
# PLAYBOOK §1: a .clapp ships REAL FILES. The depot is unpacked on someone else's machine,
# where a symlink into this source tree resolves to nothing. Everything here copies.
#
# PLAYBOOK §5: pkg/ and *.clapp are derived and gitignored. The committed sources are the
# truth; the depot is a copy you refresh.
#
#   scripts/package.sh          build, assemble, and pack
#   scripts/package.sh --skip-build   assemble from whatever is already in target/release
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"
PKG="$ROOT/pkg"

# Identity is read from the manifest, never duplicated into the script.
ID=$(node -p "require('./clatch.json').id")
CLI=$(node -p "require('./clatch.json').connector.cli")
VERSION=$(node -p "require('./clatch.json').version")

case "$(uname -s)" in
    Darwin*) OS=macos ;;
    Linux*)  OS=linux ;;
    *)       OS=windows ;;
esac
ARCH=$(uname -m)
case "$ARCH" in
    x86_64|amd64) ARCH=x64 ;;
    arm64|aarch64) ARCH=arm64 ;;
esac
EXE=""
[ "$OS" = windows ] && EXE=".exe"

if [ "${1:-}" != "--skip-build" ]; then
    # Never a bare `cargo build`: without Tauri's custom-protocol feature the binary loads
    # the dev URL and the window comes up white.
    echo "→ building"
    npm run build
fi

BIN="$ROOT/src-tauri/target/release/${CLI}${EXE}"
[ -f "$BIN" ] || { echo "package.sh: $BIN is missing — build first" >&2; exit 1; }

echo "→ assembling pkg/"
rm -rf "$PKG"
mkdir -p "$PKG/bin" "$PKG/assets"
cp "$BIN" "$PKG/bin/"
cp "$ROOT/assets/icon.png" "$PKG/assets/"
cp "$ROOT/THIRD_PARTY_NOTICES.md" "$PKG/"

# The index ships INSIDE the depot, so a first run is never blocked on the network for
# anything but the model. `clappkit::paths::install_root()` finds it here by walking up to
# clatch.json. `gturag sync` can still fetch a newer one from the repository, which is what
# lets the corpus be refreshed without rebuilding the app.
[ -f "$ROOT/corpus.gtu" ] || {
    echo "package.sh: corpus.gtu is missing — build it first:" >&2
    echo "  gturag index-corpus forms --model <dir> --built \$(date +%F)" >&2
    exit 1
}
cp "$ROOT/corpus.gtu" "$PKG/"

# Rewrite the manifest for THIS platform: a .clapp is per-OS-arch, so the depot advertises
# only the binary it actually carries, and `cliBin` points at where that binary really is.
# Scripts that hardcode `bin/<cli>` are wrong on one of the three platforms (PLAYBOOK §12) —
# so write the answer here, and let verify.sh READ it back rather than guess.
# RELATIVE paths past this point, everywhere a Windows binary is involved. Git Bash hands
# out `/c/Users/…`; node.exe and clatch.exe are native and read that as `C:\c\Users\…`.
# The script already cd'd to the repo root, so a relative path is correct in both worlds
# and needs no cygpath.
node -e "
  const m = require('./clatch.json');
  const os = '$OS', exe = '$EXE', cli = '$CLI';
  m.launch = { [os]: 'bin/' + cli + exe, args: ['app'] };
  m.connector.cliBin = 'bin/' + cli + exe;
  require('fs').writeFileSync('pkg/clatch.json', JSON.stringify(m, null, 2) + '\n');
"

# Validate when we can, say so when we cannot. `clatch` is on a developer's machine and is
# NOT on a CI runner, and requiring it there is what made every platform's Package step fail
# on the v0.1.0 release run. The depot is still checked below by the smoke test, which is
# the part that catches packaging mistakes anyway.
if command -v clatch >/dev/null 2>&1; then
    echo "→ validating"
    clatch validate pkg
else
    echo "→ validating — skipped, no clatch on PATH (expected on CI)"
fi

echo "→ packing"
rm -f "$ROOT"/*.clapp "$ROOT"/*.clapp.sha256
FINAL="$ROOT/${ID}-${OS}-${ARCH}.clapp"
# A .clapp IS a zip rooted at clatch.json, so `clatch pack` is a convenience rather than a
# format. Zip it ourselves when clatch is absent — and Git Bash on the Windows runner has
# no `zip`, only 7z (PLAYBOOK §9).
if command -v clatch >/dev/null 2>&1; then
    clatch pack pkg
    packed=$(ls -t "$ROOT"/*.clapp 2>/dev/null | head -n 1)
    [ -n "$packed" ] && FINAL="$packed"
elif command -v zip >/dev/null 2>&1; then
    ( cd pkg && zip -qr "$FINAL" . -x '*.DS_Store' )
else
    ( cd pkg && 7z a -tzip -bso0 -bsp0 "$FINAL" . >/dev/null )
fi
[ -s "$FINAL" ] || { echo "package.sh: produced no .clapp" >&2; exit 1; }
# Hash the BASENAME, from the file's own directory. `sha256sum "$FINAL"` records whatever
# path it was given — under Git Bash that is `/c/Users/…`, which makes `sha256sum -c` fail
# on every machine but this one, looking for a path that does not exist there. Clatch reads
# only the hash and did not notice; a human running the standard check would have.
( cd "$(dirname "$FINAL")" && base=$(basename "$FINAL") \
    && { sha256sum "$base" > "$base.sha256" 2>/dev/null \
         || shasum -a 256 "$base" > "$base.sha256"; } )

echo
echo "  $(basename "$FINAL")  $(du -h "$FINAL" | cut -f1)  (v$VERSION)"
echo "  install with:  clatch install $FINAL"
