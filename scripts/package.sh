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
cp "$ROOT/assets/icon.png" "$PKG/assets/"
# The library detail page draws this behind the identity text. Declared in the
# manifest, so it must be here: a declared asset that is missing fails validate and
# install both.
cp "$ROOT/assets/banner.png" "$PKG/assets/"
cp "$ROOT/THIRD_PARTY_NOTICES.md" "$PKG/"

# On macOS the binary goes inside a real .app bundle; everywhere else it sits in bin/.
#
# icons.md: "a bare executable has no icon identity, so the Dock falls back to a generic
# terminal tile" — and no runtime call fixes that, because the Dock reads the bundle. The
# .icns is the same mark as the library tile, inset to the 824/1024 macOS grid by
# scripts/icon.py, so the shelf gets full-bleed and the Dock gets its margin from one
# source.
#
# The DIRECTORY is named after `connector.cli`, never after the display name — PLAYBOOK
# §12b: format.md limits every component of `cliBin` to [A-Za-z0-9._-], and this app's
# name is "GTÜ Formlar", which carries both a space and a non-ASCII letter. What a person
# reads is CFBundleName/CFBundleDisplayName below, in full. The assertion further down
# would catch a regression, but the point is not to need it.
if [ "$OS" = macos ]; then
    ICNS="$ROOT/src-tauri/icons/icon.icns"
    [ -f "$ICNS" ] || { echo "package.sh: $ICNS is missing — run scripts/icon.py" >&2; exit 1; }
    APP="$PKG/bin/$CLI.app"
    LAUNCH_REL="bin/$CLI.app/Contents/MacOS/$CLI"
    mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
    cp "$BIN" "$APP/Contents/MacOS/$CLI"
    cp "$ICNS" "$APP/Contents/Resources/icon.icns"
    NAME=$(node -p "require('$ROOT/clatch.json').name")
    cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>$CLI</string>
  <key>CFBundleIdentifier</key><string>$ID</string>
  <key>CFBundleName</key><string>$NAME</string>
  <key>CFBundleDisplayName</key><string>$NAME</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>10.15</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
else
    LAUNCH_REL="bin/$CLI$EXE"
    cp "$BIN" "$PKG/bin/"
fi

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
  const os = '$OS', rel = '$LAUNCH_REL';
  m.launch = { [os]: rel, args: ['app'] };
  m.connector.cliBin = rel;
  require('fs').writeFileSync('pkg/clatch.json', JSON.stringify(m, null, 2) + '\n');
"

# Assert every component of what the manifest points at is a safe segment — PLAYBOOK §12b.
# format.md limits cliBin and launch to [A-Za-z0-9._-] because the value is interpolated
# into an exec shim, and the launcher enforces that at INSTALL, in front of a user. A depot
# whose files are all present is not the same thing as a depot that installs; checking here
# turns that into a build failure we see instead of a refusal they see.
node -e "
  const m = require('./pkg/clatch.json');
  const safe = /^[A-Za-z0-9._-]+\$/;
  const check = (label, value) => {
    for (const part of String(value).split('/')) {
      if (!safe.test(part)) {
        console.error(\`package.sh: \${label} component '\${part}' is not a safe segment\`);
        process.exit(1);
      }
    }
  };
  check('connector.cliBin', m.connector.cliBin);
  for (const [os, cmd] of Object.entries(m.launch)) {
    if (os !== 'args') check(\`launch.\${os}\`, cmd);
  }
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
