#!/usr/bin/env bash
# Build a release into target/dist and (optionally) upload it.
#
#   scripts/release-macos.sh                 # build, sign, notarize, staple
#   scripts/release-macos.sh --upload v0.1.1 # ...and attach to that GitHub release
#   NOTARIZE=0 scripts/release-macos.sh      # sign only, with an Apple Development identity
#
# A notarized release needs a "Developer ID Application" identity in the
# keychain and notarytool credentials stored once with:
#   xcrun notarytool store-credentials copypasta --apple-id <you> --team-id <TEAM>
# Until those exist, NOTARIZE=0 builds what install.sh installs today.
# Env: SIGN_ID (default: first Developer ID Application, or first Apple
# Development with NOTARIZE=0), NOTARY_PROFILE (default copypasta).
set -euo pipefail
cd "$(dirname "$0")/.."

NOTARIZE="${NOTARIZE:-1}"
PROFILE="${NOTARY_PROFILE:-copypasta}"
if [ "$NOTARIZE" = 1 ]; then
  SIGN_ID="${SIGN_ID:-$(security find-identity -v -p codesigning | awk -F'"' '/Developer ID Application/ {print $2; exit}')}"
  [ -n "$SIGN_ID" ] || { echo "error: no 'Developer ID Application' identity in the keychain (NOTARIZE=0 signs without one)" >&2; exit 1; }
  xcrun notarytool history --keychain-profile "$PROFILE" >/dev/null 2>&1 \
    || { echo "error: notarytool profile '$PROFILE' missing; run xcrun notarytool store-credentials $PROFILE …" >&2; exit 1; }
else
  SIGN_ID="${SIGN_ID:-$(security find-identity -v -p codesigning | awk -F'"' '/Apple Development/ {print $2; exit}')}"
  [ -n "$SIGN_ID" ] || { echo "error: no 'Apple Development' identity in the keychain" >&2; exit 1; }
fi

DIST=target/dist
rm -rf "$DIST" && mkdir -p "$DIST"
# Keep this machine's home folder (and so the user name) out of the binaries:
# they carry the source paths of dependencies under ~/.cargo.
export RUSTFLAGS="--remap-path-prefix=$HOME=~ ${RUSTFLAGS:-}"
cargo build --release -p copypasta-app -p copypasta
for bin in target/release/copypasta-app target/release/copypasta; do
  if strings -a "$bin" | grep -q -F "$HOME"; then
    echo "error: $bin still contains $HOME; not releasing it" >&2; exit 1
  fi
done

cp target/release/copypasta "$DIST/copypasta"
if [ "$NOTARIZE" = 1 ]; then
  SIGN_ID="$SIGN_ID" HARDENED=1 scripts/bundle-macos.sh "$DIST/Copypasta.app"
  codesign --force --sign "$SIGN_ID" --identifier dev.base698.copypasta.cli --options runtime --timestamp "$DIST/copypasta"

  notarize() {
    echo "notarizing $1…"
    xcrun notarytool submit "$1" --keychain-profile "$PROFILE" --wait | tee "$DIST/notary.log"
    grep -q "status: Accepted" "$DIST/notary.log" || { echo "error: notarization failed for $1" >&2; exit 1; }
  }

  # App: notarize, then staple the ticket to the bundle that gets zipped.
  ditto -c -k --keepParent "$DIST/Copypasta.app" "$DIST/notarize-app.zip"
  notarize "$DIST/notarize-app.zip"
  xcrun stapler staple "$DIST/Copypasta.app"
  # CLI: bare binaries can't be stapled; Gatekeeper fetches the ticket online.
  ditto -c -k "$DIST/copypasta" "$DIST/notarize-cli.zip"
  notarize "$DIST/notarize-cli.zip"
  rm -f "$DIST"/notarize-*.zip
else
  # The CLI keeps the ad-hoc signature the linker gave it.
  SIGN_ID="$SIGN_ID" scripts/bundle-macos.sh "$DIST/Copypasta.app"
fi

# Archives carry no extended attributes and no owner: neither says anything
# about the app, both say something about the machine that built it.
ditto -c -k --keepParent --norsrc --noextattr "$DIST/Copypasta.app" "$DIST/Copypasta-macos-arm64.zip"
/usr/bin/tar -C "$DIST" --uid 0 --gid 0 --uname root --gname wheel --no-xattrs --no-mac-metadata \
  -czf "$DIST/copypasta-cli-macos-arm64.tar.gz" copypasta
(cd "$DIST" && shasum -a 256 Copypasta-macos-arm64.zip copypasta-cli-macos-arm64.tar.gz > SHA256SUMS)
if [ "$NOTARIZE" = 1 ]; then
  spctl -a -vv -t exec "$DIST/Copypasta.app"
fi

if [ "${1:-}" = --upload ]; then
  tag="${2:?usage: --upload <tag>}"
  gh release upload "$tag" --clobber "$DIST/Copypasta-macos-arm64.zip" "$DIST/copypasta-cli-macos-arm64.tar.gz" "$DIST/SHA256SUMS"
fi
echo "release artifacts in $DIST"
