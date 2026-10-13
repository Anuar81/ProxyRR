#!/bin/sh
# Arma el instalador de macOS (spec 0013): ProxyRR.app universal con la CLI adentro, el .dmg, el
# tarball para install.sh y SHA256SUMS, en dist/. Corre igual en una Mac cualquiera y en CI.
#
#   scripts/build-macos.sh
#
# Sin variables de Apple firma ad-hoc (gratis, sin notarizar). Con estas variables firma con
# Developer ID y notariza:
#   APPLE_SIGNING_IDENTITY   "Developer ID Application: Nombre (TEAMID)" (ya en el llavero)
#   APPLE_API_ISSUER, APPLE_API_KEY, APPLE_API_KEY_PATH   API key de App Store Connect (notarytool)
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

[ "$(uname -s)" = Darwin ] || { echo "build-macos.sh: solo corre en macOS" >&2; exit 1; }

VERSION=$(cargo pkgid -p proxyrr-cli | sed 's/.*[#@]//')
TARGETS="aarch64-apple-darwin x86_64-apple-darwin"
UNIVERSAL="$ROOT/target/universal-apple-darwin/release"
DIST="$ROOT/dist"
APP="$UNIVERSAL/bundle/macos/ProxyRR.app"
DMG="$DIST/ProxyRR_${VERSION}_universal.dmg"
TARBALL="$DIST/proxyrr-${VERSION}-macos-universal.tar.gz"
ENTITLEMENTS="$ROOT/crates/proxyrr-desktop/macos/entitlements.plist"

echo "==> ProxyRR $VERSION"
rm -rf "$DIST"
mkdir -p "$DIST" "$UNIVERSAL"

echo "==> CLI universal"
for t in $TARGETS; do
  cargo build --release --locked -p proxyrr-cli --target "$t"
done
lipo -create -output "$UNIVERSAL/proxyrr" \
  "$ROOT/target/aarch64-apple-darwin/release/proxyrr" \
  "$ROOT/target/x86_64-apple-darwin/release/proxyrr"

echo "==> App universal"
# Tauri firmaría por su cuenta si ve las variables de Apple; la firma la hace este script, al final,
# porque copiar la CLI dentro del bundle invalida cualquier sello anterior.
(cd crates/proxyrr-desktop && env -u APPLE_SIGNING_IDENTITY -u APPLE_CERTIFICATE \
  cargo tauri build --target universal-apple-darwin --bundles app -- --locked)
cp "$UNIVERSAL/proxyrr" "$APP/Contents/MacOS/proxyrr"

sign() {
  if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
    codesign --force --options runtime --timestamp --entitlements "$ENTITLEMENTS" \
      --sign "$APPLE_SIGNING_IDENTITY" "$@"
  else
    codesign --force --sign - "$@"
  fi
}

notarize() {
  xcrun notarytool submit "$1" --key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY" \
    --issuer "$APPLE_API_ISSUER" --wait
}

can_notarize() {
  [ -n "${APPLE_SIGNING_IDENTITY:-}" ] && [ -n "${APPLE_API_ISSUER:-}" ] &&
    [ -n "${APPLE_API_KEY:-}" ] && [ -n "${APPLE_API_KEY_PATH:-}" ]
}

echo "==> Firma ($( [ -n "${APPLE_SIGNING_IDENTITY:-}" ] && echo "Developer ID" || echo "ad-hoc, sin notarizar"))"
# De adentro hacia afuera: primero la CLI, después el bundle que la contiene.
sign "$UNIVERSAL/proxyrr"
sign "$APP/Contents/MacOS/proxyrr"
sign "$APP"
codesign --verify --deep --strict "$APP"

if can_notarize; then
  echo "==> Notarización de la app"
  ditto -c -k --keepParent "$APP" "$DIST/notarize-app.zip"
  notarize "$DIST/notarize-app.zip"
  rm "$DIST/notarize-app.zip"
  xcrun stapler staple "$APP"
fi

echo "==> Chequeos del bundle"
if can_notarize; then
  "$ROOT/scripts/check-macos-bundle.sh" "$APP" --signed
else
  "$ROOT/scripts/check-macos-bundle.sh" "$APP"
fi

echo "==> .dmg"
STAGE="$DIST/dmg-stage"
mkdir -p "$STAGE"
ditto "$APP" "$STAGE/ProxyRR.app"
ln -s /Applications "$STAGE/Aplicaciones"
hdiutil create -quiet -volname "ProxyRR $VERSION" -srcfolder "$STAGE" -ov -format UDZO "$DMG"
rm -rf "$STAGE"
if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
  sign "$DMG"
fi
if can_notarize; then
  notarize "$DMG"
  xcrun stapler staple "$DMG"
fi

echo "==> Tarball (para install.sh)"
# `tar` de macOS conserva los atributos y la firma del bundle.
tar -C "$(dirname "$APP")" -czf "$TARBALL" ProxyRR.app
cp "$ROOT/scripts/install.sh" "$DIST/install.sh"

(cd "$DIST" && shasum -a 256 "$(basename "$DMG")" "$(basename "$TARBALL")" > SHA256SUMS)

echo "==> Listo en dist/:"
ls -lh "$DIST"
