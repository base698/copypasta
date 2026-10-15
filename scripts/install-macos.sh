#!/usr/bin/env bash
# Build, bundle, sign and (re)start Copypasta as a login agent.
#   scripts/install-macos.sh            # build + install + restart
#   SIGN_ID="Apple Development: …"      # override the signing identity
set -euo pipefail
cd "$(dirname "$0")/.."

APP="$HOME/Applications/Copypasta.app"
BIN_DIR="$HOME/.local/bin"
LABEL="dev.base698.copypasta"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
SIGN_ID="${SIGN_ID:-$(security find-identity -v -p codesigning | awk -F'"' '/Apple Development/ {print $2; exit}')}"

cargo build --release -p copypasta-app -p copypasta

mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$BIN_DIR" "$HOME/Library/Logs"
# Never overwrite signed binaries in place: macOS kills the replaced binary.
rm -f "$APP/Contents/MacOS/Copypasta" "$BIN_DIR/copypasta"
cp target/release/copypasta-app "$APP/Contents/MacOS/Copypasta"
cp target/release/copypasta "$BIN_DIR/copypasta"

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
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSAppleEventsUsageDescription</key><string>Copypasta asks your browser for the active tab's URL so it can show where you copied from and pasted into.</string>
</dict></plist>
PLIST

if [ -n "$SIGN_ID" ]; then
  codesign --force --sign "$SIGN_ID" --identifier "$LABEL" "$APP"
else
  echo "warning: no Apple Development identity; ad-hoc signing (permissions reset on every rebuild)" >&2
  codesign --force --sign - --identifier "$LABEL" "$APP"
fi

cat > "$PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key><array><string>$APP/Contents/MacOS/Copypasta</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ProcessType</key><string>Interactive</string>
  <key>StandardErrorPath</key><string>$HOME/Library/Logs/copypasta.log</string>
  <key>StandardOutPath</key><string>$HOME/Library/Logs/copypasta.log</string>
</dict></plist>
PLIST

launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
for _ in $(seq 1 50); do launchctl print "gui/$(id -u)/$LABEL" >/dev/null 2>&1 || break; sleep 0.1; done
launchctl bootstrap "gui/$(id -u)" "$PLIST"
echo "installed $APP (signed: ${SIGN_ID:-ad-hoc}); CLI at $BIN_DIR/copypasta; log ~/Library/Logs/copypasta.log"
