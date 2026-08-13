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

echo "→ validating"
clatch validate pkg

echo "→ packing"
rm -f "$ROOT"/*.clapp "$ROOT"/*.clapp.sha256
# `clatch pack <dir>` builds the host-platform depot and names its own output. Let it fail
# LOUDLY: a packaging step that falls back to "just zip it" ships a subtly different
# artifact on exactly the machines nobody watches (PLAYBOOK, field notes).
clatch pack pkg

FINAL=$(ls -t "$ROOT"/*.clapp 2>/dev/null | head -n 1)
if [ -z "$FINAL" ]; then
    # It may write beside the depot instead of into the cwd.
    FINAL=$(ls -t "$PKG"/../*.clapp "$PKG"/*.clapp 2>/dev/null | head -n 1)
fi
[ -n "$FINAL" ] || { echo "package.sh: clatch pack produced no .clapp" >&2; exit 1; }
sha256sum "$FINAL" > "$FINAL.sha256" 2>/dev/null || shasum -a 256 "$FINAL" > "$FINAL.sha256"

echo
echo "  $(basename "$FINAL")  $(du -h "$FINAL" | cut -f1)  (v$VERSION)"
echo "  install with:  clatch install $FINAL"
