#!/usr/bin/env bash
# Instala só os cabeçalhos de uma versão mais nova da PipeWire num prefixo, com
# .pc próprios. O crate pipewire 0.10 não compila com os cabeçalhos 0.3.48 do
# Ubuntu 22.04, mas o build de release precisa desse Ubuntu (glibc 2.35).
# O binário continua ligando a libpipewire-0.3.so.0 do sistema (ABI estável);
# só funções presentes na 0.3.48 são usadas (conferido por check-pipewire-symbols.sh).
#
# Uso: packaging/linux/pipewire-headers.sh <versão> <prefixo>
# Depois: export PKG_CONFIG_PATH=<prefixo>/lib/pkgconfig
set -euo pipefail

versao="$1"
prefixo="$(realpath -m "$2")"
fonte="$(mktemp -d)"
trap 'rm -rf "$fonte"' EXIT

curl -fsSL "https://gitlab.freedesktop.org/pipewire/pipewire/-/archive/$versao/pipewire-$versao.tar.gz" \
  | tar -xz -C "$fonte" --strip-components=1

IFS=. read -r maior menor micro <<<"$versao"
mkdir -p "$prefixo/include/spa-0.2" "$prefixo/include/pipewire-0.3" "$prefixo/lib/pkgconfig"
cp -r "$fonte/spa/include/spa" "$prefixo/include/spa-0.2/"
(cd "$fonte/src" && find pipewire -name '*.h' -exec cp --parents {} "$prefixo/include/pipewire-0.3/" \;)
sed -e "s/@PIPEWIRE_VERSION_MAJOR@/$maior/g" -e "s/@PIPEWIRE_VERSION_MINOR@/$menor/g" \
    -e "s/@PIPEWIRE_VERSION_MICRO@/$micro/g" -e "s/@PIPEWIRE_API_VERSION@/\"0.3\"/g" \
    "$fonte/src/pipewire/version.h.in" >"$prefixo/include/pipewire-0.3/pipewire/version.h"

# A biblioteca de link (libpipewire-0.3.so) vem do libpipewire-0.3-dev do sistema.
libdir="$(pkg-config --variable=libdir libpipewire-0.3 2>/dev/null || echo /usr/lib/x86_64-linux-gnu)"
cat >"$prefixo/lib/pkgconfig/libspa-0.2.pc" <<PC
includedir=$prefixo/include

Name: libspa
Description: Simple Plugin API (só cabeçalhos $versao)
Version: 0.2
Cflags: -I\${includedir}/spa-0.2 -D_REENTRANT
PC
cat >"$prefixo/lib/pkgconfig/libpipewire-0.3.pc" <<PC
includedir=$prefixo/include
libdir=$libdir

Name: libpipewire
Description: PipeWire Interface (cabeçalhos $versao, biblioteca do sistema)
Version: $versao
Requires: libspa-0.2
Libs: -L\${libdir} -lpipewire-0.3
Cflags: -I\${includedir}/pipewire-0.3 -D_REENTRANT
PC
echo "cabeçalhos PipeWire $versao em $prefixo"
