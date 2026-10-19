#!/usr/bin/env bash
# Gera packaging/flatpak/cargo-sources.json a partir do Cargo.lock: a lista de crates que o
# flatpak-builder baixa antes do build (o build do Flatpak roda sem rede, como o Flathub exige).
#
#   packaging/flatpak/gerar-fontes.sh
#
# Usa o flatpak-cargo-generator (MIT, flatpak/flatpak-builder-tools) numa versão fixa, num
# ambiente Python próprio. Rode de novo sempre que o Cargo.lock mudar.
set -euo pipefail

COMMIT=74697c75b630d7330e77250fc13cb5ea688d9479
SHA256=0a2db6be87d75910facef28ab46d4d6460802e8419ab850d0caa6a364d26b380

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
work="${FLATPAK_CARGO_WORK:-$(mktemp -d)}"

script="$work/flatpak-cargo-generator.py"
if [[ ! -f "$script" ]]; then
  curl -fsSL -o "$script" \
    "https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/$COMMIT/cargo/flatpak-cargo-generator.py"
fi
echo "$SHA256  $script" | sha256sum -c --quiet -

if [[ ! -x "$work/venv/bin/python" ]]; then
  python3 -m venv "$work/venv"
  "$work/venv/bin/pip" install --quiet 'aiohttp>=3.9.5,<4' 'PyYAML>=6.0.2,<7' 'tomlkit>=0.13.3,<1'
fi
"$work/venv/bin/python" -I "$script" "$root/Cargo.lock" -o "$here/cargo-sources.json"
echo "gerado: $here/cargo-sources.json"
