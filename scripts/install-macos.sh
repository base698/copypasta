#!/usr/bin/env bash
# Build from source, bundle, sign and (re)start Copypasta as a login agent.
#   scripts/install-macos.sh            # build + install + restart
#   SIGN_ID="Apple Development: …"      # override the signing identity
set -euo pipefail
cd "$(dirname "$0")/.."

APP="$HOME/Applications/Copypasta.app"
BIN_DIR="$HOME/.local/bin"
LABEL="dev.base698.copypasta"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"

cargo build --release -p copypasta-app -p copypasta
scripts/bundle-macos.sh "$APP"

mkdir -p "$BIN_DIR" "$HOME/Library/Logs"
rm -f "$BIN_DIR/copypasta"
cp target/release/copypasta "$BIN_DIR/copypasta"

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
# bootout doesn't always take the app down with it; make sure no old copy
# keeps capturing (and holding the hotkey) with stale code.
pkill -f "$APP/Contents/MacOS/Copypasta" 2>/dev/null || true
for _ in $(seq 1 30); do pgrep -f "$APP/Contents/MacOS/Copypasta" >/dev/null || break; sleep 0.1; done
pkill -9 -f "$APP/Contents/MacOS/Copypasta" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$PLIST"
echo "installed $APP; CLI at $BIN_DIR/copypasta; log ~/Library/Logs/copypasta.log"
