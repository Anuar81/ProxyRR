#!/bin/sh
# Verifica un ProxyRR.app armado (spec 0013, criterios 3 y 4; con --signed también el 9).
#
#   scripts/check-macos-bundle.sh [ruta/a/ProxyRR.app] [--signed]
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
APP="$ROOT/target/universal-apple-darwin/release/bundle/macos/ProxyRR.app"
SIGNED=no
for arg in "$@"; do
  case "$arg" in
    --signed) SIGNED=yes ;;
    *) APP=$arg ;;
  esac
done

VERSION=$(cd "$ROOT" && cargo pkgid -p proxyrr-cli | sed 's/.*[#@]//')
PLIST="$APP/Contents/Info.plist"
FAILS=0

ok() { echo "  ✅ $1"; }
fail() { echo "  ❌ $1" >&2; FAILS=$((FAILS + 1)); }
expect() { # nombre, obtenido, esperado
  if [ "$2" = "$3" ]; then ok "$1 = $2"; else fail "$1 = '$2' (esperaba '$3')"; fi
}
plist() { /usr/libexec/PlistBuddy -c "Print :$1" "$PLIST" 2>/dev/null || echo "<falta>"; }
archs() { lipo -archs "$1" 2>/dev/null | tr ' ' '\n' | sort | tr '\n' ' ' | sed 's/ $//'; }

echo "Revisando $(basename "$APP") (versión $VERSION)"
[ -f "$PLIST" ] || { echo "  ❌ no existe $PLIST" >&2; exit 1; }

expect CFBundleIdentifier "$(plist CFBundleIdentifier)" com.proxyrr.desktop
expect CFBundleShortVersionString "$(plist CFBundleShortVersionString)" "$VERSION"
expect LSMinimumSystemVersion "$(plist LSMinimumSystemVersion)" 13.0
ICON=$(plist CFBundleIconFile)
if [ -f "$APP/Contents/Resources/$ICON" ] || [ -f "$APP/Contents/Resources/$ICON.icns" ]; then
  ok "ícono $ICON"
else
  fail "ícono '$ICON' no está en Resources"
fi

MAIN="$APP/Contents/MacOS/$(plist CFBundleExecutable)"
CLI="$APP/Contents/MacOS/proxyrr"
expect "arquitecturas de la app" "$(archs "$MAIN")" "arm64 x86_64"
if [ -x "$CLI" ]; then
  expect "arquitecturas de la CLI" "$(archs "$CLI")" "arm64 x86_64"
  expect "proxyrr --version" "$("$CLI" --version 2>&1)" "proxyrr $VERSION"
else
  fail "falta la CLI en Contents/MacOS/proxyrr"
fi

if codesign --verify --deep --strict "$APP" 2>/dev/null; then
  ok "firma válida (codesign --verify --deep --strict)"
else
  fail "firma inválida: $(codesign --verify --deep --strict "$APP" 2>&1 | head -1)"
fi

if [ "$SIGNED" = yes ]; then
  if spctl -a -vv -t exec "$APP" 2>&1 | grep -q "Notarized Developer ID"; then
    ok "Gatekeeper: Notarized Developer ID"
  else
    fail "Gatekeeper no la acepta como notarizada: $(spctl -a -vv -t exec "$APP" 2>&1 | head -2)"
  fi
  if xcrun stapler validate "$APP" >/dev/null 2>&1; then ok "ticket grapado"; else fail "sin ticket grapado"; fi
fi

if [ "$FAILS" -gt 0 ]; then
  echo "$FAILS chequeo(s) fallaron" >&2
  exit 1
fi
echo "Todo bien."
