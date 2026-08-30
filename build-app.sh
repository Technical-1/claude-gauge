#!/usr/bin/env bash
#
# Build, sign, notarize and staple Claude Gauge.app.
#
#   ./build-app.sh              full pipeline
#   ./build-app.sh --no-sign    bundle only, unsigned — for local iteration
#   ./build-app.sh --no-notarize  signed but not notarized (will NOT run elsewhere)
#   ./build-app.sh --icon       also re-render the icon from assets/icon.html
#   ./build-app.sh --install    copy the finished app into /Applications
#   ./build-app.sh --universal  build for arm64 AND x86_64 (what a release ships)
#
# Credentials live in the notarytool keychain profile below — never in this repo.
set -euo pipefail
cd "$(dirname "$0")"

APP_NAME="Claude Gauge"
BUNDLE_ID="com.technical1.claude-gauge"
# Signing identity and notary profile, in precedence order:
#   1. SIGN_IDENTITY / NOTARY_PROFILE in the environment
#   2. ~/.apple-signing/, the machine-local credential store
#   3. error (identity) or a sensible default (profile)
#
# Nothing is hardcoded here on purpose. A baked-in identity means someone else's
# build silently tries to sign as this file's author and fails somewhere
# unhelpful; and the store already holds these, so repeating them invites drift.
#
# Read, never echoed. Everything under ~/.apple-signing/ may be substituted
# into a command but must not be printed.
# The `if` form matters: `[ -f x ] && cat x` returns 1 when the file is absent,
# and under `set -e` that kills the script inside a ${VAR:-$(read_secret ...)}
# substitution before anything is printed. Absent is not an error here — it just
# means fall through to the next source.
read_secret() {
  if [ -f "$HOME/.apple-signing/$1" ]; then cat "$HOME/.apple-signing/$1"; fi
}

IDENTITY="${SIGN_IDENTITY:-$(read_secret APPLE_SIGNING_IDENTITY)}"
PROFILE="${NOTARY_PROFILE:-$(read_secret APPLE_NOTARY_PROFILE)}"
PROFILE="${PROFILE:-claude-gauge}"
VERSION="$(awk -F'"' '/^version =/{print $2; exit}' Cargo.toml)"

APP="dist/${APP_NAME}.app"
# Two different zips on purpose. The submission copy goes to Apple BEFORE the
# ticket exists; the release copy is made AFTER stapling so it carries the ticket
# inside the bundle. Shipping the submission zip means every recipient's first
# launch has to reach Apple to verify — slow at best, a failure offline.
SUBMIT_ZIP="dist/.submit-${VERSION}.zip"
DIST_ZIP="dist/${APP_NAME// /-}-${VERSION}.zip"
CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"

NOTARIZE=1; ICON=0; INSTALL=0; SIGN=1; UNIVERSAL=0
for a in "$@"; do case "$a" in
  --universal) UNIVERSAL=1 ;;
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
# A plain `cargo build --release` produces a HOST-ARCH binary. On Apple
# Silicon that is arm64 only, and an Intel Mac cannot run it at all — so a
# release built without this flag is unusable by half its potential audience.
# Local iteration stays single-arch because it is twice as fast and only has to
# run on this machine.
if [ "$UNIVERSAL" = 1 ]; then
  say "Building universal binary (v$VERSION)"
  rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null 2>&1 || true
  # `rustup target add` can succeed while the build still fails, because
  # rustup installs into ITS toolchain and a Homebrew-installed rustc — which
  # ships host-only std — may be the one on PATH. `rustup target list
  # --installed` then lies: it reports the target while the active compiler has
  # no std for it, and the build dies deep in a dependency with a misleading
  # "can't find crate for std". Ask the compiler that will actually run.
  for t in aarch64-apple-darwin x86_64-apple-darwin; do
    libdir="$(rustc --print target-libdir --target "$t" 2>/dev/null || true)"
    if [ -z "$libdir" ] || [ ! -d "$libdir" ]; then
      echo "The active rustc has no std for $t." >&2
      echo "  active: $(command -v rustc) — $(rustc --version)" >&2
      echo "  A rustup-managed toolchain is needed for a universal build;" >&2
      echo "  a Homebrew rustc only ships its own architecture." >&2
      echo "  Build without --universal for a local, this-machine-only app." >&2
      exit 1
    fi
  done
  cargo build --release --target aarch64-apple-darwin
  cargo build --release --target x86_64-apple-darwin
  mkdir -p target/universal
  lipo -create -output target/universal/claude-gauge \
    target/aarch64-apple-darwin/release/claude-gauge \
    target/x86_64-apple-darwin/release/claude-gauge
  BIN=target/universal/claude-gauge
else
  say "Building release binary (v$VERSION, this machine's architecture only)"
  cargo build --release
  BIN=target/release/claude-gauge
fi

# ── bundle ──────────────────────────────────────────────────────────────────
say "Assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/claude-gauge"
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
  <key>CFBundleExecutable</key>            <string>claude-gauge</string>
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
  echo "No signing identity. Either:" >&2
  echo "  put it in ~/.apple-signing/APPLE_SIGNING_IDENTITY" >&2
  echo "  export SIGN_IDENTITY=\"Developer ID Application: Your Name (TEAMID)\"" >&2
  echo "  ...or build unsigned with --no-sign" >&2
  exit 1
fi
say "Signing"
codesign --force --options runtime --timestamp \
  --sign "$IDENTITY" "$APP/Contents/MacOS/claude-gauge"
codesign --force --options runtime --timestamp \
  --sign "$IDENTITY" "$APP"
codesign --verify --strict --verbose=2 "$APP"
fi

# ── notarize ────────────────────────────────────────────────────────────────
if [ "$NOTARIZE" = 1 ]; then
  say "Notarizing (this takes a few minutes)"
  # A fresh CI runner has no keychain profile, so fall back to the API key trio
  # when it is supplied by the environment. Locally the profile is used.
  if [ -n "${APPLE_API_KEY_PATH:-}" ] && [ -n "${APPLE_API_KEY_ID:-}" ] && [ -n "${APPLE_API_ISSUER_ID:-}" ]; then
    NOTARY_AUTH=(--key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER_ID")
  else
    NOTARY_AUTH=(--keychain-profile "$PROFILE")
  fi
  rm -f "$SUBMIT_ZIP"
  ditto -c -k --keepParent "$APP" "$SUBMIT_ZIP"
  if ! xcrun notarytool submit "$SUBMIT_ZIP" "${NOTARY_AUTH[@]}" --wait; then
    echo "notarization failed — fetching log for the most recent submission" >&2
    id=$(xcrun notarytool history "${NOTARY_AUTH[@]}" 2>/dev/null \
         | awk '/id: /{print $2; exit}')
    [ -n "$id" ] && xcrun notarytool log "$id" "${NOTARY_AUTH[@]}" >&2
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
