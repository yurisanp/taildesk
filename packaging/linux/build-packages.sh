#!/usr/bin/env bash
# Gera os pacotes Linux do TailDesk a partir de um build release já feito.
#
#   cargo build --release --locked -p taildesk -p taildesk-cli
#   packaging/linux/build-packages.sh <versão> <pasta de saída>
#
# Saída: .tar.gz, .AppImage (+ .zsync), .deb e .rpm (x86_64). Precisa de cargo-deb,
# cargo-generate-rpm e zsyncmake no PATH; o appimagetool é baixado se não existir (APPIMAGETOOL aponta para um já baixado).
set -euo pipefail

version="${1:?uso: build-packages.sh <versão> <pasta de saída>}"
out="$(mkdir -p "${2:?uso: build-packages.sh <versão> <pasta de saída>}" && cd "$2" && pwd)"
root="$(cd "$(dirname "$0")/../.." && pwd)"
arch=x86_64
rel="$root/target/release"
here="$root/packaging/linux"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

for bin in taildesk taildesk-cli; do
  [[ -x "$rel/$bin" ]] || { echo "falta $rel/$bin: rode cargo build --release primeiro" >&2; exit 1; }
done

# Árvore comum (prefixo /usr), usada pelo tar.gz e pelo AppImage.
tree="$work/taildesk-$version-linux-$arch"
install -Dm755 "$rel/taildesk"     "$tree/usr/bin/taildesk"
install -Dm755 "$rel/taildesk-cli" "$tree/usr/bin/taildesk-cli"
install -Dm644 "$here/taildesk.desktop" "$tree/usr/share/applications/taildesk.desktop"
install -Dm644 "$here/taildesk.svg"     "$tree/usr/share/icons/hicolor/scalable/apps/taildesk.svg"
install -Dm644 "$here/io.github.yurisanp.TailDesk.metainfo.xml" "$tree/usr/share/metainfo/io.github.yurisanp.TailDesk.metainfo.xml"
install -Dm644 "$root/LICENSE-MIT"      "$tree/usr/share/licenses/taildesk/LICENSE-MIT"
install -Dm644 "$root/LICENSE-APACHE"   "$tree/usr/share/licenses/taildesk/LICENSE-APACHE"
# PyroWave (opcional): packaging/pyrowave/build.sh target/release deixa a biblioteca ao lado
# dos binários; o app a procura em ../lib a partir do executável.
if [[ -f "$rel/libpyrowave-shared.so.1" ]]; then
  install -Dm755 "$rel/libpyrowave-shared.so.1" "$tree/usr/lib/libpyrowave-shared.so.1"
  for l in "$rel"/licencas-pyrowave/*; do
    install -Dm644 "$l" "$tree/usr/share/licenses/taildesk/$(basename "$l")"
  done
fi

# 1. tar.gz (também é a fonte do pacote taildesk-bin do AUR)
tar -C "$work" -czf "$out/taildesk-$version-linux-$arch.tar.gz" "$(basename "$tree")"

# 2. AppImage: AppDir = árvore + AppRun + .desktop e ícone na raiz
appdir="$work/TailDesk.AppDir"
cp -a "$tree" "$appdir"
cp "$here/taildesk.desktop" "$appdir/taildesk.desktop"
cp "$here/taildesk.svg" "$appdir/taildesk.svg"
ln -s taildesk.svg "$appdir/.DirIcon"
cat > "$appdir/AppRun" <<'APPRUN'
#!/bin/sh
# `TailDesk.AppImage cli ...` roda a CLI; sem argumento "cli" abre o app.
here="$(dirname "$(readlink -f "$0")")"
if [ "${1:-}" = "cli" ]; then shift; exec "$here/usr/bin/taildesk-cli" "$@"; fi
exec "$here/usr/bin/taildesk" "$@"
APPRUN
chmod 755 "$appdir/AppRun"
tool="${APPIMAGETOOL:-$work/appimagetool}"
if [[ ! -x "$tool" ]]; then
  curl -fsSL -o "$tool" "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$arch.AppImage"
  chmod 755 "$tool"
fi
# Update information (spec do AppImage, seção ELF .upd_info): AppImageUpdate, Gear Lever e o
# próprio TailDesk acham a versão nova pelo .zsync da release mais recente no GitHub.
# Build de pre-release (versão com "-") segue latest-all: a mais nova, estável ou pre-release. TAILDESK_UPDATE_INFO="" desliga.
channel=latest
[[ "$version" == *-* ]] && channel=latest-all
updinfo="${TAILDESK_UPDATE_INFO-gh-releases-zsync|yurisanp|taildesk|$channel|TailDesk-*$arch.AppImage.zsync}"
updargs=()
if [[ -n "$updinfo" ]]; then
  command -v zsyncmake >/dev/null || { echo "falta zsyncmake (pacote zsync) para gerar o .zsync" >&2; exit 1; }
  updargs=(--updateinformation "$updinfo")
fi
# Sem FUSE no CI: extrair e rodar. O .zsync sai na pasta atual, por isso o cd.
(cd "$out" && APPIMAGE_EXTRACT_AND_RUN=1 ARCH=$arch "$tool" --no-appstream "${updargs[@]}" \
  "$appdir" "$out/TailDesk-$version-$arch.AppImage")

# 3. .deb e 4. .rpm (metadados em apps/taildesk/Cargo.toml)
# Pre-release usa "~" (0.1.0~beta.1): ordena antes de 0.1.0 no dpkg e no rpm, que não aceita "-".
pkgver="${version//-/\~}"
cd "$root"
cargo deb --no-build --no-strip -p taildesk --deb-version "$pkgver" -o "$out/taildesk_${pkgver}_amd64.deb"
cargo generate-rpm -p apps/taildesk --set-metadata "version = \"$pkgver\"" -o "$out/taildesk-$pkgver-1.$arch.rpm"

ls -l "$out"
