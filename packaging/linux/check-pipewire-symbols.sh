#!/usr/bin/env bash
# Confere que os binários só pedem à libpipewire funções que a biblioteca
# indicada exporta (no build de release: a 0.3.48 do Ubuntu 22.04).
#
# Uso: packaging/linux/check-pipewire-symbols.sh <libpipewire-0.3.so.0> <binário>...
set -euo pipefail

lib="$1"; shift
exportados="$(nm -D --defined-only "$lib" | awk '{print $NF}' | sort -u)"
falta=0
for bin in "$@"; do
  while read -r simbolo; do
    if ! grep -qxF "$simbolo" <<<"$exportados"; then
      echo "$bin: $simbolo não existe em $lib" >&2
      falta=1
    fi
  done < <(nm -D --undefined-only "$bin" | awk '{print $NF}' | sed 's/@.*//' | grep -E '^pw_' | sort -u)
done
[ "$falta" -eq 0 ] && echo "símbolos da PipeWire presentes em $lib"
exit "$falta"
