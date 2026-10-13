#!/bin/sh
# Arma los instaladores de macOS (spec 0013): una versión por arquitectura, Apple Silicon e Intel,
# cada una con ProxyRR.app (la CLI adentro), su .dmg y su tarball para install.sh, más SHA256SUMS,
# en dist/. Corre igual en una Mac cualquiera y en CI.
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
DIST="$ROOT/dist"
ENTITLEMENTS="$ROOT/crates/proxyrr-desktop/macos/entitlements.plist"

sign() {
  if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
    codesign --force --options runtime --timestamp --entitlements "$ENTITLEMENTS" \
      --sign "$APPLE_SIGNING_IDENTITY" "$@"
  else
    codesign --force --sign - "$@"
  fi
}

can_notarize() {
  [ -n "${APPLE_SIGNING_IDENTITY:-}" ] && [ -n "${APPLE_API_ISSUER:-}" ] &&
    [ -n "${APPLE_API_KEY:-}" ] && [ -n "${APPLE_API_KEY_PATH:-}" ]
}

notarize() {
  xcrun notarytool submit "$1" --key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY" \
    --issuer "$APPLE_API_ISSUER" --wait
}

# build <target de rust> <etiqueta pública> <arquitectura de lipo>
build() {
  target=$1
  label=$2
  arch=$3
  release="$ROOT/target/$target/release"
  app="$release/bundle/macos/ProxyRR.app"
  dmg="$DIST/ProxyRR_${VERSION}_${label}.dmg"
  tarball="$DIST/proxyrr-${VERSION}-macos-${label}.tar.gz"

  echo "==> [$label] CLI"
  cargo build --release --locked -p proxyrr-cli --target "$target"

  echo "==> [$label] App"
  # Tauri firmaría por su cuenta si ve las variables de Apple; la firma la hace este script, al final,
  # porque copiar la CLI dentro del bundle invalida cualquier sello anterior.
  (cd crates/proxyrr-desktop && env -u APPLE_SIGNING_IDENTITY -u APPLE_CERTIFICATE \
    cargo tauri build --target "$target" --bundles app -- --locked)
  cp "$release/proxyrr" "$app/Contents/MacOS/proxyrr"

  echo "==> [$label] Firma ($( [ -n "${APPLE_SIGNING_IDENTITY:-}" ] && echo "Developer ID" || echo "ad-hoc, sin notarizar"))"
  # De adentro hacia afuera: primero la CLI, después el bundle que la contiene.
  sign "$app/Contents/MacOS/proxyrr"
  sign "$app"

  if can_notarize; then
    echo "==> [$label] Notarización de la app"
    ditto -c -k --keepParent "$app" "$DIST/notarize-app-$label.zip"
    notarize "$DIST/notarize-app-$label.zip"
    rm "$DIST/notarize-app-$label.zip"
    xcrun stapler staple "$app"
    "$ROOT/scripts/check-macos-bundle.sh" "$app" --arch "$arch" --signed
  else
    "$ROOT/scripts/check-macos-bundle.sh" "$app" --arch "$arch"
  fi

  echo "==> [$label] .dmg"
  stage="$DIST/dmg-stage-$label"
  rm -rf "$stage"
  mkdir -p "$stage"
  ditto "$app" "$stage/ProxyRR.app"
  ln -s /Applications "$stage/Aplicaciones"
  hdiutil create -quiet -volname "ProxyRR $VERSION" -srcfolder "$stage" -ov -format UDZO "$dmg"
  rm -rf "$stage"
  if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
    sign "$dmg"
  fi
  if can_notarize; then
    notarize "$dmg"
    xcrun stapler staple "$dmg"
  fi

  echo "==> [$label] Tarball (para install.sh)"
  # `tar` de macOS conserva los atributos y la firma del bundle.
  tar -C "$(dirname "$app")" -czf "$tarball" ProxyRR.app
}

echo "==> ProxyRR $VERSION"
rm -rf "$DIST"
mkdir -p "$DIST"

build aarch64-apple-darwin apple-silicon arm64
build x86_64-apple-darwin intel x86_64

cp "$ROOT/scripts/install.sh" "$DIST/install.sh"
(cd "$DIST" && shasum -a 256 ./*.dmg ./*.tar.gz | sed 's| \./| |' > SHA256SUMS)

echo "==> Listo en dist/:"
ls -lh "$DIST"
