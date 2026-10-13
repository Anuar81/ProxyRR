#!/bin/sh
# Test de scripts/install.sh (spec 0013, criterio 6), sin red: una "release" falsa en file://.
#
#   scripts/test-install.sh
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK=$(mktemp -d "${TMPDIR:-/tmp}/proxyrr-test-install.XXXXXX")
trap 'rm -rf "$WORK"' EXIT INT TERM
FAILS=0
ok() { echo "  ✅ $1"; }
fail() { echo "  ❌ $1" >&2; FAILS=$((FAILS + 1)); }

# Release falsa: por arquitectura, un .app con una "CLI" que imprime su versión y su arquitectura.
make_release() { # dir versión
  dir=$1
  mkdir -p "$dir"
  for label in apple-silicon intel; do
    mkdir -p "$WORK/pkg/ProxyRR.app/Contents/MacOS"
    printf '#!/bin/sh\necho "proxyrr %s %s"\n' "$2" "$label" > "$WORK/pkg/ProxyRR.app/Contents/MacOS/proxyrr"
    chmod +x "$WORK/pkg/ProxyRR.app/Contents/MacOS/proxyrr"
    tar -C "$WORK/pkg" -czf "$dir/proxyrr-$2-macos-$label.tar.gz" ProxyRR.app
    rm -rf "$WORK/pkg"
  done
  (cd "$dir" && shasum -a 256 proxyrr-*.tar.gz > SHA256SUMS)
}

BASE="$WORK/releases"
make_release "$BASE/latest/download" 9.9.9
make_release "$BASE/download/v1.2.3" 1.2.3
HERE=$([ "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" = 1 ] && echo apple-silicon || echo intel)

run_install() { # nombre-del-caso, variables extra...
  case_dir="$WORK/$1"
  shift
  mkdir -p "$case_dir"
  env PROXYRR_BASE_URL="file://$BASE" PROXYRR_APPS_DIR="$case_dir/Apps" \
    PROXYRR_BIN_DIR="$case_dir/bin" "$@" sh "$ROOT/scripts/install.sh" > "$case_dir/log" 2>&1
}

echo "install.sh"

if run_install latest && [ "$("$WORK/latest/bin/proxyrr")" = "proxyrr 9.9.9 $HERE" ]; then
  ok "última release: instala la versión de esta Mac ($HERE) y enlaza la CLI"
else
  fail "última release: $(cat "$WORK/latest/log")"
fi

for label in apple-silicon intel; do
  if run_install "arch-$label" PROXYRR_ARCH=$label && [ "$("$WORK/arch-$label/bin/proxyrr")" = "proxyrr 9.9.9 $label" ]; then
    ok "PROXYRR_ARCH=$label baja ese tarball"
  else
    fail "PROXYRR_ARCH=$label: $(cat "$WORK/arch-$label/log")"
  fi
done

if run_install bad-arch PROXYRR_ARCH=universal; then
  fail "PROXYRR_ARCH inválida: no falló"
else
  ok "PROXYRR_ARCH inválida: falla con mensaje"
fi

if run_install pinned PROXYRR_VERSION=1.2.3 && [ "$("$WORK/pinned/bin/proxyrr")" = "proxyrr 1.2.3 $HERE" ]; then
  ok "PROXYRR_VERSION elige la release"
else
  fail "PROXYRR_VERSION: $(cat "$WORK/pinned/log")"
fi

# Reinstalar reemplaza la versión anterior.
if run_install latest PROXYRR_VERSION=1.2.3 && [ "$("$WORK/latest/bin/proxyrr")" = "proxyrr 1.2.3 $HERE" ]; then
  ok "reinstalar reemplaza ProxyRR.app"
else
  fail "reinstalar: $(cat "$WORK/latest/log")"
fi

# Checksum que no coincide: aborta sin tocar el destino.
printf 'basura' >> "$BASE/latest/download/proxyrr-9.9.9-macos-$HERE.tar.gz"
if run_install tampered; then
  fail "checksum malo: instaló igual"
elif [ -e "$WORK/tampered/Apps/ProxyRR.app" ] || [ -e "$WORK/tampered/bin/proxyrr" ]; then
  fail "checksum malo: tocó el destino"
elif grep -q "no coincide" "$WORK/tampered/log"; then
  ok "checksum malo: aborta sin instalar nada"
else
  fail "checksum malo: mensaje inesperado: $(cat "$WORK/tampered/log")"
fi

if run_install missing PROXYRR_VERSION=0.0.0; then
  fail "versión inexistente: no falló"
else
  ok "versión inexistente: falla con mensaje ($(tail -1 "$WORK/missing/log"))"
fi

if [ "$FAILS" -gt 0 ]; then
  echo "$FAILS caso(s) fallaron" >&2
  exit 1
fi
echo "Todo bien."
