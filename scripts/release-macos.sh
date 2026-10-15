#!/usr/bin/env bash
# Build a notarized release into target/dist and (optionally) upload it.
#
#   scripts/release-macos.sh                 # build, sign, notarize, staple
#   scripts/release-macos.sh --upload v0.1.1 # ...and attach to that GitHub release
#
# Needs a "Developer ID Application" identity in the keychain and notarytool
# credentials stored once with:
#   xcrun notarytool store-credentials copypasta --apple-id <you> --team-id <TEAM>
# Env: SIGN_ID (default: first Developer ID Application), NOTARY_PROFILE (default copypasta).
set -euo pipefail
cd "$(dirname "$0")/.."

SIGN_ID="${SIGN_ID:-$(security find-identity -v -p codesigning | awk -F'"' '/Developer ID Application/ {print $2; exit}')}"
PROFILE="${NOTARY_PROFILE:-copypasta}"
[ -n "$SIGN_ID" ] || { echo "error: no 'Developer ID Application' identity in the keychain" >&2; exit 1; }
xcrun notarytool history --keychain-profile "$PROFILE" >/dev/null 2>&1 \
  || { echo "error: notarytool profile '$PROFILE' missing; run xcrun notarytool store-credentials $PROFILE …" >&2; exit 1; }

DIST=target/dist
rm -rf "$DIST" && mkdir -p "$DIST"
cargo build --release -p copypasta-app -p copypasta

SIGN_ID="$SIGN_ID" HARDENED=1 scripts/bundle-macos.sh "$DIST/Copypasta.app"
cp target/release/copypasta "$DIST/copypasta"
codesign --force --sign "$SIGN_ID" --identifier dev.base698.copypasta.cli --options runtime --timestamp "$DIST/copypasta"

notarize() {
  echo "notarizing $1…"
  xcrun notarytool submit "$1" --keychain-profile "$PROFILE" --wait | tee "$DIST/notary.log"
  grep -q "status: Accepted" "$DIST/notary.log" || { echo "error: notarization failed for $1" >&2; exit 1; }
}

# App: notarize, staple the ticket, then zip the stapled bundle.
ditto -c -k --keepParent "$DIST/Copypasta.app" "$DIST/notarize-app.zip"
notarize "$DIST/notarize-app.zip"
xcrun stapler staple "$DIST/Copypasta.app"
ditto -c -k --keepParent "$DIST/Copypasta.app" "$DIST/Copypasta-macos-arm64.zip"

# CLI: bare binaries can't be stapled; Gatekeeper fetches the ticket online.
ditto -c -k "$DIST/copypasta" "$DIST/notarize-cli.zip"
notarize "$DIST/notarize-cli.zip"
tar -C "$DIST" -czf "$DIST/copypasta-cli-macos-arm64.tar.gz" copypasta

rm -f "$DIST"/notarize-*.zip
(cd "$DIST" && shasum -a 256 Copypasta-macos-arm64.zip copypasta-cli-macos-arm64.tar.gz > SHA256SUMS)
spctl -a -vv -t exec "$DIST/Copypasta.app"

if [ "${1:-}" = --upload ]; then
  tag="${2:?usage: --upload <tag>}"
  gh release upload "$tag" --clobber "$DIST/Copypasta-macos-arm64.zip" "$DIST/copypasta-cli-macos-arm64.tar.gz" "$DIST/SHA256SUMS"
fi
echo "release artifacts in $DIST"
