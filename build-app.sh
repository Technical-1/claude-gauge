#!/usr/bin/env bash
#
# Build, sign, notarize and staple Claude Usage.app.
#
#   ./build-app.sh              full pipeline
#   ./build-app.sh --no-sign    bundle only, unsigned — for local iteration
#   ./build-app.sh --no-notarize  signed but not notarized (will NOT run elsewhere)
#   ./build-app.sh --icon       also re-render the icon from assets/icon.html
#   ./build-app.sh --install    copy the finished app into /Applications
#
# Credentials live in the notarytool keychain profile below — never in this repo.
set -euo pipefail
cd "$(dirname "$0")"

APP_NAME="Claude Usage"
BUNDLE_ID="com.technical1.claude-usage"
# No default identity on purpose: a hardcoded one means someone else's build
# silently tries to sign as its author and fails somewhere confusing.
IDENTITY="${SIGN_IDENTITY:-}"
PROFILE="${NOTARY_PROFILE:-claude-usage}"
VERSION="$(awk -F'"' '/^version =/{print $2; exit}' Cargo.toml)"

APP="dist/${APP_NAME}.app"
# Two different zips on purpose. The submission copy goes to Apple BEFORE the
# ticket exists; the release copy is made AFTER stapling so it carries the ticket
# inside the bundle. Shipping the submission zip means every recipient's first
# launch has to reach Apple to verify — slow at best, a failure offline.
SUBMIT_ZIP="dist/.submit-${VERSION}.zip"
DIST_ZIP="dist/${APP_NAME// /-}-${VERSION}.zip"
CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"

NOTARIZE=1; ICON=0; INSTALL=0; SIGN=1
for a in "$@"; do case "$a" in
  --no-notarize) NOTARIZE=0 ;; --icon) ICON=1 ;; --install) INSTALL=1 ;;
  --no-sign) SIGN=0; NOTARIZE=0 ;;
  *) echo "unknown flag: $a" >&2; exit 2 ;;
esac; done

say() { printf '\n\033[1m==> %s\033[0m\n' "$1"; }

# ── icon ────────────────────────────────────────────────────────────────────
# Off by default: assets/icon.icns is committed, and re-rendering needs Chrome.
if [ "$ICON" = 1 ]; then
  say "Rendering icon at 2048px"
  # dist/ is normally created by the bundle step further down, which runs AFTER
  # this one. On a clean tree it does not exist yet and the render silently
  # writes nowhere — only a from-scratch build reveals it.
  mkdir -p dist
  "$CHROME" --headless --disable-gpu --no-sandbox \
    --screenshot=dist/icon-2048.png --window-size=1024,1024 \
    --force-device-scale-factor=2 --default-background-color=00000000 \
    --hide-scrollbars --virtual-time-budget=3000 \
    "file://$PWD/assets/icon.html" >/dev/null 2>&1
  rm -rf dist/icon.iconset && mkdir -p dist/icon.iconset
  for spec in 16:16x16 32:16x16@2x 32:32x32 64:32x32@2x 128:128x128 \
              256:128x128@2x 256:256x256 512:256x256@2x 512:512x512 1024:512x512@2x; do
    magick dist/icon-2048.png -filter Lanczos -resize "${spec%%:*}x${spec%%:*}" \
      -strip "dist/icon.iconset/icon_${spec##*:}.png"
  done
  iconutil -c icns dist/icon.iconset -o assets/icon.icns
fi

# ── build ───────────────────────────────────────────────────────────────────
say "Building release binary (v$VERSION)"
cargo build --release

# ── bundle ──────────────────────────────────────────────────────────────────
say "Assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/claude-usage "$APP/Contents/MacOS/claude-usage"
cp assets/icon.icns            "$APP/Contents/Resources/icon.icns"

# LSUIElement duplicates the runtime ActivationPolicy::Accessory call on purpose:
# the plist keeps the app out of the Dock from the instant it launches, before
# any Rust runs. Without it there is a brief Dock-icon flash at login.
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key>                  <string>${APP_NAME}</string>
  <key>CFBundleDisplayName</key>           <string>${APP_NAME}</string>
  <key>CFBundleIdentifier</key>            <string>${BUNDLE_ID}</string>
  <key>CFBundleExecutable</key>            <string>claude-usage</string>
  <key>CFBundleIconFile</key>              <string>icon</string>
  <key>CFBundlePackageType</key>           <string>APPL</string>
  <key>CFBundleInfoDictionaryVersion</key> <string>6.0</string>
  <key>CFBundleShortVersionString</key>    <string>${VERSION}</string>
  <key>CFBundleVersion</key>               <string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key>        <string>11.0</string>
  <key>LSUIElement</key>                   <true/>
  <key>NSHighResolutionCapable</key>       <true/>
  <key>NSHumanReadableCopyright</key>      <string>© 2026 Jacob Kanfer</string>
</dict></plist>
PLIST

# ── sign ────────────────────────────────────────────────────────────────────
# Inner Mach-O first, then the bundle. That ordering is what --deep used to do
# badly; done explicitly it is the supported form. Hardened runtime is required
# for notarization. No entitlements: the app only shells out to /usr/bin/security
# and makes an HTTPS request, neither of which needs one.
if [ "$SIGN" = 0 ]; then
  say "Skipping signing (--no-sign): local iteration build"
else
if [ -z "$IDENTITY" ]; then
  echo "SIGN_IDENTITY is not set. Either:" >&2
  echo "  export SIGN_IDENTITY=\"Developer ID Application: Your Name (TEAMID)\"" >&2
  echo "  ...or build unsigned with --no-sign" >&2
  exit 1
fi
say "Signing"
codesign --force --options runtime --timestamp \
  --sign "$IDENTITY" "$APP/Contents/MacOS/claude-usage"
codesign --force --options runtime --timestamp \
  --sign "$IDENTITY" "$APP"
codesign --verify --strict --verbose=2 "$APP"
fi

# ── notarize ────────────────────────────────────────────────────────────────
if [ "$NOTARIZE" = 1 ]; then
  say "Notarizing (this takes a few minutes)"
  rm -f "$SUBMIT_ZIP"
  ditto -c -k --keepParent "$APP" "$SUBMIT_ZIP"
  if ! xcrun notarytool submit "$SUBMIT_ZIP" --keychain-profile "$PROFILE" --wait; then
    echo "notarization failed — fetching log for the most recent submission" >&2
    id=$(xcrun notarytool history --keychain-profile "$PROFILE" 2>/dev/null \
         | awk '/id: /{print $2; exit}')
    [ -n "$id" ] && xcrun notarytool log "$id" --keychain-profile "$PROFILE" >&2
    exit 1
  fi
  say "Stapling"
  xcrun stapler staple "$APP"

  # The only check that means anything. A bundle always launches on the machine
  # that built it, notarized or not, so local launch proves nothing.
  say "Verifying"
  spctl -a -vvv -t install "$APP"
  xcrun stapler validate "$APP"

  # Release artifact, built from the stapled bundle.
  say "Packaging $DIST_ZIP"
  rm -f "$SUBMIT_ZIP" "$DIST_ZIP"
  ditto -c -k --keepParent "$APP" "$DIST_ZIP"

  # The only check that reflects a recipient's experience: mark the artifact as
  # downloaded from the internet, unpack it somewhere else, and ask Gatekeeper.
  # Verifying the app we just built in place proves nothing about that.
  say "Verifying as a download"
  tmp="$(mktemp -d)"
  cp "$DIST_ZIP" "$tmp/dl.zip"
  xattr -w com.apple.quarantine "0083;00000000;Safari;" "$tmp/dl.zip"
  ditto -x -k "$tmp/dl.zip" "$tmp/out"
  spctl -a -vvv -t install "$tmp/out/${APP_NAME}.app"
  xcrun stapler validate "$tmp/out/${APP_NAME}.app"
  rm -rf "$tmp"
fi

# ── install ─────────────────────────────────────────────────────────────────
if [ "$INSTALL" = 1 ]; then
  say "Installing to /Applications"
  pkill -f "/Applications/${APP_NAME}.app" 2>/dev/null || true
  rm -rf "/Applications/${APP_NAME}.app"
  ditto "$APP" "/Applications/${APP_NAME}.app"
fi

say "Done: $APP"
