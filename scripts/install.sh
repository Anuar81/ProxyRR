#!/bin/sh
# Instala ProxyRR en macOS (spec 0013) desde las releases de GitHub:
#
#   curl -fsSL https://github.com/Anuar81/ProxyRR/releases/latest/download/install.sh | sh
#
# Baja el tarball, verifica su SHA-256 contra SHA256SUMS, deja ProxyRR.app en /Applications (o en
# ~/Applications si no hay permiso) y enlaza la CLI `proxyrr`. No usa sudo y NO instala la CA: eso se
# hace después desde la app (Certificado… → Este equipo) o con `proxyrr ca install`.
#
# Variables opcionales:
#   PROXYRR_VERSION    versión a instalar (p. ej. 0.1.0); por defecto, la última release
#   PROXYRR_ARCH       apple-silicon o intel; por defecto, la de esta Mac
#   PROXYRR_APPS_DIR   dónde dejar ProxyRR.app
#   PROXYRR_BIN_DIR    dónde enlazar la CLI
#   PROXYRR_BASE_URL   base de las releases (por defecto https://github.com/Anuar81/ProxyRR/releases)
set -eu

BASE=${PROXYRR_BASE_URL:-https://github.com/Anuar81/ProxyRR/releases}
if [ -n "${PROXYRR_VERSION:-}" ]; then
  URL="$BASE/download/v${PROXYRR_VERSION#v}"
else
  URL="$BASE/latest/download"
fi

say() { printf '%s\n' "$*"; }
die() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = Darwin ] || die "por ahora solo hay instalador para macOS"
command -v curl >/dev/null || die "falta curl"
command -v shasum >/dev/null || die "falta shasum"

TMP=$(mktemp -d "${TMPDIR:-/tmp}/proxyrr-install.XXXXXX")
trap 'rm -rf "$TMP"' EXIT INT TERM

# Arquitectura real de la Mac: `sysctl hw.optional.arm64` da 1 en Apple Silicon aunque esta terminal
# corra bajo Rosetta (donde `uname -m` diría x86_64).
if [ -n "${PROXYRR_ARCH:-}" ]; then
  LABEL=$PROXYRR_ARCH
elif [ "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" = 1 ]; then
  LABEL=apple-silicon
else
  LABEL=intel
fi
case "$LABEL" in
  apple-silicon | intel) ;;
  *) die "PROXYRR_ARCH tiene que ser apple-silicon o intel" ;;
esac

say "Bajando la lista de archivos de $URL"
curl -fsSL "$URL/SHA256SUMS" -o "$TMP/SHA256SUMS" || die "no pude bajar SHA256SUMS de $URL"
NAME=$(awk -v want="-macos-$LABEL.tar.gz" '$2 ~ /^proxyrr-/ && substr($2, length($2) - length(want) + 1) == want { print $2; exit }' "$TMP/SHA256SUMS")
[ -n "$NAME" ] || die "esta release no tiene versión para Mac $LABEL"

say "Bajando $NAME (Mac $LABEL)"
curl -fsSL "$URL/$NAME" -o "$TMP/$NAME" || die "no pude bajar $NAME"

say "Verificando SHA-256"
(cd "$TMP" && grep " $NAME\$" SHA256SUMS | shasum -a 256 -c -s -) ||
  die "el SHA-256 de $NAME no coincide con SHA256SUMS: no instalo nada"

mkdir "$TMP/x"
tar -xzf "$TMP/$NAME" -C "$TMP/x"
[ -x "$TMP/x/ProxyRR.app/Contents/MacOS/proxyrr" ] || die "el tarball no trae ProxyRR.app con la CLI"

if [ -n "${PROXYRR_APPS_DIR:-}" ]; then
  APPS=$PROXYRR_APPS_DIR
elif [ -w /Applications ]; then
  APPS=/Applications
else
  APPS="$HOME/Applications"
fi
mkdir -p "$APPS"
# Reemplaza una versión anterior; solo toca ProxyRR.app.
rm -rf "$APPS/ProxyRR.app"
mv "$TMP/x/ProxyRR.app" "$APPS/ProxyRR.app"
say "App instalada en $APPS/ProxyRR.app"

if [ -n "${PROXYRR_BIN_DIR:-}" ]; then
  BIN=$PROXYRR_BIN_DIR
elif [ -d /usr/local/bin ] && [ -w /usr/local/bin ]; then
  BIN=/usr/local/bin
else
  BIN="$HOME/.local/bin"
fi
mkdir -p "$BIN"
ln -sf "$APPS/ProxyRR.app/Contents/MacOS/proxyrr" "$BIN/proxyrr"
say "CLI enlazada en $BIN/proxyrr"
case ":$PATH:" in
  *":$BIN:"*) ;;
  *) say "Ojo: $BIN no está en tu PATH. Agregalo, p. ej.: echo 'export PATH=\"$BIN:\$PATH\"' >> ~/.zshrc" ;;
esac

say ""
say "Listo. Abrí ProxyRR desde $APPS, o probá: $BIN/proxyrr --version"
say "Para ver HTTPS descifrado, instalá la CA desde la app (Certificado… → Este equipo)."
