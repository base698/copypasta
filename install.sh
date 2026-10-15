#!/bin/sh
# Install the latest Copypasta release (macOS, Apple Silicon):
#
#   curl -fsSL https://raw.githubusercontent.com/base698/copypasta/main/install.sh | sh
#
# Downloads with curl (which does not quarantine files), checks SHA256SUMS
# and the code signature, installs ~/Applications/Copypasta.app and
# ~/.local/bin/copypasta, and starts it at login.
#
# Env: COPYPASTA_VERSION=v0.1.0 (default latest), COPYPASTA_APP_DIR,
#      COPYPASTA_BIN_DIR, COPYPASTA_NO_AGENT=1 (don't touch launchd).
set -eu

REPO="base698/copypasta"
VERSION="${COPYPASTA_VERSION:-latest}"
APP_DIR="${COPYPASTA_APP_DIR:-$HOME/Applications}"
BIN_DIR="${COPYPASTA_BIN_DIR:-$HOME/.local/bin}"
LABEL="dev.base698.copypasta"
APP="$APP_DIR/Copypasta.app"

say() { printf '\033[1;33m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = Darwin ] || die "this installer is macOS-only (Linux support is on the roadmap)"
[ "$(uname -m)" = arm64 ] || die "release builds are Apple Silicon only; build from source on Intel"

if [ "$VERSION" = latest ]; then
  base="https://github.com/$REPO/releases/latest/download"
else
  base="https://github.com/$REPO/releases/download/$VERSION"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cd "$tmp"
say "downloading Copypasta ($VERSION)"
for f in Copypasta-macos-arm64.zip copypasta-cli-macos-arm64.tar.gz SHA256SUMS; do
  curl -fsSL --retry 2 -o "$f" "$base/$f" || die "download failed: $base/$f"
done
shasum -a 256 -c SHA256SUMS >/dev/null || die "checksum mismatch"

ditto -x -k Copypasta-macos-arm64.zip "$tmp/app"
tar -xzf copypasta-cli-macos-arm64.tar.gz -C "$tmp"
codesign --verify --strict "$tmp/app/Copypasta.app" || die "app signature is invalid"
codesign --verify --strict "$tmp/copypasta" || die "CLI signature is invalid"
xattr -dr com.apple.quarantine "$tmp/app/Copypasta.app" "$tmp/copypasta" 2>/dev/null || true

if [ -z "${COPYPASTA_NO_AGENT:-}" ]; then
  launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
  for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    launchctl print "gui/$(id -u)/$LABEL" >/dev/null 2>&1 || break
    sleep 0.1
  done
fi

say "installing $APP"
mkdir -p "$APP_DIR" "$BIN_DIR"
rm -rf "$APP"
mv "$tmp/app/Copypasta.app" "$APP"
rm -f "$BIN_DIR/copypasta"
mv "$tmp/copypasta" "$BIN_DIR/copypasta"

if [ -z "${COPYPASTA_NO_AGENT:-}" ]; then
  mkdir -p "$HOME/Library/LaunchAgents" "$HOME/Library/Logs"
  plist="$HOME/Library/LaunchAgents/$LABEL.plist"
  cat > "$plist" <<PLIST
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
  launchctl bootstrap "gui/$(id -u)" "$plist"
  say "started (log: ~/Library/Logs/copypasta.log)"
fi

case ":$PATH:" in *":$BIN_DIR:"*) ;; *) say "add $BIN_DIR to your PATH to use the copypasta CLI" ;; esac
say "done — press ⌃⌘V. macOS will ask for Accessibility and Input Monitoring the first time."
