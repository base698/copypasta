#!/usr/bin/env bash
# Assemble and sign Copypasta.app from target/release.
#   scripts/bundle-macos.sh <path/to/Copypasta.app>
# Env: SIGN_ID (identity; default: first Apple Development), HARDENED=1 for
# hardened runtime + secure timestamp (required for notarization).
set -euo pipefail
cd "$(dirname "$0")/.."

APP="$1"
LABEL="dev.base698.copypasta"
VERSION="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
SIGN_ID="${SIGN_ID:-$(security find-identity -v -p codesigning | awk -F'"' '/Apple Development/ {print $2; exit}')}"

mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
# Never overwrite a signed binary in place: macOS kills the replaced binary.
rm -f "$APP/Contents/MacOS/Copypasta"
cp target/release/copypasta-app "$APP/Contents/MacOS/Copypasta"

if [ ! -f "$APP/Contents/Resources/icon.icns" ] || [ crates/copypasta-app/icons/icon.png -nt "$APP/Contents/Resources/icon.icns" ]; then
  iconset="$(mktemp -d)/icon.iconset"; mkdir -p "$iconset"
  for s in 16 32 128 256 512; do
    sips -z $s $s crates/copypasta-app/icons/icon.png --out "$iconset/icon_${s}x${s}.png" >/dev/null
    sips -z $((s*2)) $((s*2)) crates/copypasta-app/icons/icon.png --out "$iconset/icon_${s}x${s}@2x.png" >/dev/null
  done
  iconutil -c icns "$iconset" -o "$APP/Contents/Resources/icon.icns"
fi

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>$LABEL</string>
  <key>CFBundleName</key><string>Copypasta</string>
  <key>CFBundleDisplayName</key><string>Copypasta</string>
  <key>CFBundleExecutable</key><string>Copypasta</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSAppleEventsUsageDescription</key><string>Copypasta asks your browser for the active tab's URL so it can show where you copied from and pasted into.</string>
  <key>NSDesktopFolderUsageDescription</key><string>Copypasta adds the screenshots you save here to your clipboard history.</string>
  <key>NSDocumentsFolderUsageDescription</key><string>Copypasta adds the screenshots you save here to your clipboard history.</string>
  <key>NSDownloadsFolderUsageDescription</key><string>Copypasta adds the screenshots you save here to your clipboard history.</string>
</dict></plist>
PLIST

if [ -z "$SIGN_ID" ]; then
  echo "warning: no signing identity; ad-hoc signing (permissions reset on every rebuild)" >&2
  SIGN_ID="-"
fi
flags=(--force --sign "$SIGN_ID" --identifier "$LABEL")
if [ "${HARDENED:-0}" = 1 ]; then
  flags+=(--options runtime --timestamp --entitlements scripts/entitlements.plist)
fi
codesign "${flags[@]}" "$APP"
codesign --verify --strict "$APP"
echo "bundled $APP (signed: $SIGN_ID${HARDENED:+, hardened})"
