#!/usr/bin/env bash
# Atualiza o pacote taildesk-bin no AUR para uma versão nova e envia.
#
#   packaging/arch/update-aur.sh <versão sem o "v"> [sha256]
#
# Reescreve pkgver e sha256sums do packaging/arch/taildesk-bin/PKGBUILD, gera o .SRCINFO
# (makepkg --printsrcinfo) e faz push para ssh://aur@aur.archlinux.org/taildesk-bin.git.
# Precisa de makepkg (Arch) e da chave SSH do mantenedor já no agente/known_hosts.
# Sem o sha, baixa o tarball da release e calcula.
set -euo pipefail

version="${1:?uso: update-aur.sh <versão> [sha256]}"
sha="${2:-}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
pkgdir="$root/packaging/arch/taildesk-bin"
url="https://github.com/yurisanp/taildesk/releases/download/v$version/taildesk-$version-linux-x86_64.tar.gz"

if [[ -z "$sha" ]]; then
  echo "baixando $url para o sha256…"
  sha="$(curl -fsSL "$url" | sha256sum | cut -d' ' -f1)"
fi

sed -i -E "s/^pkgver=.*/pkgver=$version/; s/^pkgrel=.*/pkgrel=1/; s/^sha256sums=.*/sha256sums=('$sha')/" \
  "$pkgdir/PKGBUILD"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
git clone "ssh://aur@aur.archlinux.org/taildesk-bin.git" "$work"
cp "$pkgdir/PKGBUILD" "$work/PKGBUILD"
cd "$work"
makepkg --printsrcinfo > .SRCINFO
git add PKGBUILD .SRCINFO
if git diff --cached --quiet; then
  echo "nada mudou no AUR."
  exit 0
fi
git -c user.name="TailDesk release" -c user.email="yurisantp@gmail.com" \
  commit -m "taildesk-bin $version"
git push origin master
echo "taildesk-bin $version enviado ao AUR."
