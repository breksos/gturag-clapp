#!/usr/bin/env sh
# Assemble the Clatch DEPOT — the folder `clatch validate`, `clatch install` and
# `clatch pack` consume:
#
#   <depot>/clatch.json              the manifest (verbatim; cliBin gains .exe on Windows)
#   <depot>/bin/<cli>[.exe]          the ONE binary — the GUI *and* the agent CLI
#   <depot>/assets/icon.png          the icon clatch.json declares
#   <depot>/THIRD_PARTY_NOTICES.md   when the repo has one
#   …plus whatever app_extras() adds
#
# Prints exactly ONE line on stdout — the depot path — and sends every other word to
# stderr, so this is a legal thing to write:
#
#   clatch validate "$(npm run -s package)"
#
# The script is identical in shape in every clapp: one small per-app header block,
# then a body that is byte-identical everywhere. Fix a bug once, copy it four times.
#
# CROSS-PLATFORM. Written for macOS, Linux, and Windows under Git Bash / MSYS2 — the
# shell Git for Windows installs, which is also what `npm` already assumes there, so
# no second implementation is needed. HONESTY: the Windows branches (.exe suffix,
# node.exe vendoring, the cliBin rewrite, skipping chmod) are reasoned from the
# platform's rules and from Clatch's own installer, NOT executed — this repo's
# packaging has only ever been RUN on macOS. A green macOS run is not a Windows
# guarantee. Anyone with a Windows box should run this and report back.
set -eu
. "$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)/lib.sh"

# ── this clapp ──────────────────────────────────────────────────────────────────
DEPOT=pkg                # the assembled Clatch depot. NEVER dist/: that name belongs
                         # to the frontend bundle (src-tauri/tauri.conf.json's
                         # build.frontendDist), and the two used to delete each other.
bin_src()      { printf '%s' "$ROOT/src-tauri/target/release/$CLI$EXE"; }
build_binary() { tauri_build && assert_frontend_embedded "$(bin_src)"; }

# The index ships INSIDE the depot, so a first run is never blocked on the network for
# anything but the model: clappkit::paths::install_root() finds it beside clatch.json.
# `gturag sync` can still fetch a newer one from the repository, which is how the corpus
# is refreshed without a release.
#
# Linux too: the shared body below copies clatch.json verbatim there, because the family
# ships macOS and Windows, where it rewrites it. A depot names exactly ONE launch OS, its
# own (clappkit/docs/format.md), so this app, which also ships Linux, rewrites it here.
app_extras() {
  [ -f "$ROOT/corpus.gtu" ] || fail "corpus.gtu is missing. Build it: npm run corpus"
  cp "$ROOT/corpus.gtu" "$DIST/corpus.gtu"
  ok "corpus.gtu, $(du -h "$DIST/corpus.gtu" | awk '{print $1}')"
  if [ "$OS" = linux ]; then
    install_manifest "$ROOT/clatch.json" "$DIST/clatch.json" "bin/$CLI"
  fi
}

# macOS: lib.sh names the bundle DIRECTORY after the display name, and this app's name is
# "GTÜ Formlar": a space and a non-ASCII letter. Every component of connector.cliBin must be
# a safe segment, [A-Za-z0-9._-], because the value is interpolated into an exec shim
# (clappkit/docs/format.md), and the depot's cliBin points inside this bundle. So the
# directory takes the cli name, and the name a person reads stays whole in CFBundleName and
# CFBundleDisplayName. Same arguments and output as lib.sh's, so the shared body below calls
# it unchanged. The icon is the committed .icns that scripts/icon.py derives, already inset
# to the macOS 824/1024 grid, so there is no dock-icon step.
macos_app_bundle() { # <dist> <cli> <display-name> <id> <version> <src-icon-png> -> prints inner exec path
  _d=$1; _cli=$2; _name=$3; _id=$4; _ver=$5
  _icns="$ROOT/src-tauri/icons/icon.icns"
  [ -f "$_icns" ] || fail "src-tauri/icons/icon.icns is missing. Regenerate it: python3 scripts/icon.py"
  _app="$_d/bin/$_cli.app"
  mkdir -p "$_app/Contents/MacOS" "$_app/Contents/Resources"
  mv "$_d/bin/$_cli" "$_app/Contents/MacOS/$_cli"
  chmod +x "$_app/Contents/MacOS/$_cli"
  cp "$_icns" "$_app/Contents/Resources/$_cli.icns"
  cat > "$_app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleExecutable</key><string>$_cli</string>
  <key>CFBundleIdentifier</key><string>$_id</string>
  <key>CFBundleName</key><string>$_name</string>
  <key>CFBundleDisplayName</key><string>$_name</string>
  <key>CFBundleIconFile</key><string>$_cli</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleShortVersionString</key><string>$_ver</string>
  <key>CFBundleVersion</key><string>$_ver</string>
  <key>LSMinimumSystemVersion</key><string>10.15</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
  plutil -lint "$_app/Contents/Info.plist" >/dev/null 2>&1 || fail "the generated Info.plist is not valid"
  # Ad-hoc signed with the bundle id as its identifier, like every clapp's bundle: macOS
  # keys a permission to the signature, so a grant survives a move and names the app.
  codesign --force --sign - --identifier "$_id" "$_app" >/dev/null 2>&1 \
    || note "codesign unavailable - the bundle ships unsigned"
  printf 'bin/%s.app/Contents/MacOS/%s\n' "$_cli" "$_cli"
}

# ── everything below this line is byte-identical in every clapp ──────────────────

OS="$(host_os)"
EXE="$(exe_suffix)"
CLI="$(manifest connector.cli)"  || fail "clatch.json: connector.cli is missing"
ID="$(manifest id)"              || fail "clatch.json: id is missing"
VERSION="$(manifest version)"    || fail "clatch.json: version is missing"
ICON="$(manifest icon)"          || ICON="assets/icon.png"
DIST="$ROOT/$DEPOT"

step "build — $ID $VERSION ($OS/$(uname -m))"
build_binary || fail "the build failed — run it directly to see why"
BIN="$(bin_src)"
[ -f "$BIN" ] || fail "the build produced no binary at $BIN"
ok "$(du -h "$BIN" | awk '{print $1}')"

step "assemble $DEPOT/"
rm -rf "$DIST"
mkdir -p "$DIST/bin" "$DIST/$(dirname -- "$ICON")"

# ONE binary, two roles: `<cli> app` is the GUI Clatch launches, `<cli> <verb>` is the
# agent's CLI. launch.<os> and connector.cliBin in clatch.json both resolve to it.
cp "$BIN" "$DIST/bin/$CLI$EXE"
[ -n "$EXE" ] || chmod +x "$DIST/bin/$CLI"

# Everything the manifest promises: the icon, and a banner or photos if declared.
manifest_assets | while IFS= read -r asset; do
  [ -n "$asset" ] || continue
  [ -f "$ROOT/$asset" ] || fail "clatch.json declares $asset, which is not in this repo"
  mkdir -p "$DIST/$(dirname -- "$asset")"
  cp "$ROOT/$asset" "$DIST/$asset"
done
if [ -f "$ROOT/THIRD_PARTY_NOTICES.md" ]; then
  cp "$ROOT/THIRD_PARTY_NOTICES.md" "$DIST/THIRD_PARTY_NOTICES.md"
fi

# On macOS the binary moves into a real .app bundle so the Dock has an icon and a name to
# read (see macos_app_bundle() in lib.sh — a bare executable has neither).
INNER=""
if [ "$OS" = macos ]; then
  NAME="$(manifest name)" || NAME="$CLI"
  INNER="$(macos_app_bundle "$DIST" "$CLI" "$NAME" "$ID" "$VERSION" "$ROOT/$ICON")"
fi

# The manifest. The DEPOT copy's connector.cliBin is rewritten per host — to the .exe name
# on Windows, into the bundle on macOS — see install_manifest() in lib.sh for why, and why
# the repo copy must stay the plain POSIX form.
if [ -n "$INNER" ]; then
  install_manifest "$ROOT/clatch.json" "$DIST/clatch.json" "$INNER" macos "$INNER"
elif [ -n "$EXE" ]; then
  install_manifest "$ROOT/clatch.json" "$DIST/clatch.json" "bin/$CLI$EXE"
else
  cp "$ROOT/clatch.json" "$DIST/clatch.json"
fi

app_extras

# The depot must contain everything the manifest promises, or the failure surfaces on
# a user's machine at `clatch install` instead of here. Same pair Clatch checks.
step "self-check"
[ -x "$DIST/${INNER:-bin/$CLI$EXE}" ] || fail "$DEPOT/${INNER:-bin/$CLI$EXE} is missing or not executable"
for declared in "$(manifest_path "$DIST/clatch.json" cliBin "$OS")" \
                "$(manifest_path "$DIST/clatch.json" launch "$OS")" \
                $(manifest_assets); do
  [ -z "$declared" ] || [ -e "$DIST/$declared" ] \
    || fail "the manifest declares $declared, which is not in the depot"
done
ok "manifest and files agree"

printf '%s\n' "$DIST"
