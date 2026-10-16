#!/usr/bin/env bash
# Compila a biblioteca do PyroWave (MIT, github.com/Themaister/pyrowave) numa versão fixa e põe
# em <pasta de saída> o arquivo que o TailDesk abre em tempo de execução:
# libpyrowave-shared.so.1 (Linux) ou libpyrowave-shared-1.dll (Windows, com o MSVC no PATH).
# Também copia as licenças (PyroWave e Granite) para <pasta de saída>/licencas-pyrowave.
#
#   packaging/pyrowave/build.sh <pasta de saída>
#
# Precisa de git, cmake (≥ 3.27 no Windows), ninja e um compilador C++.
set -euo pipefail

PYROWAVE_COMMIT=a0d2e71e98b85f6a23f62407df21a39e9745b875

out="$(mkdir -p "${1:?uso: build.sh <pasta de saída>}" && cd "$1" && pwd)"
work="${PYROWAVE_WORK:-$(mktemp -d)}"
src="$work/pyrowave"

if [[ ! -d "$src/.git" ]]; then
  git clone --quiet https://github.com/Themaister/pyrowave "$src"
fi
git -C "$src" fetch --quiet origin "$PYROWAVE_COMMIT" 2>/dev/null || true
git -C "$src" checkout --quiet "$PYROWAVE_COMMIT"
(cd "$src" && bash checkout_granite.sh >/dev/null)

cmake -S "$src" -B "$src/build" -G Ninja -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="$src/build/output" >/dev/null
cmake --build "$src/build" --target pyrowave-shared
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) cp "$src"/build/*pyrowave-shared*.dll "$out/" ;;
  *) cp -L "$src"/build/libpyrowave-shared.so.1 "$out/" ;;
esac

mkdir -p "$out/licencas-pyrowave"
cp "$src/LICENSE" "$out/licencas-pyrowave/PyroWave-LICENSE"
cp "$src/Granite/LICENSE" "$out/licencas-pyrowave/Granite-LICENSE"
echo "PyroWave $PYROWAVE_COMMIT em $out"
