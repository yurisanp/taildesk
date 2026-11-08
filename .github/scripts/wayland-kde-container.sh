#!/usr/bin/env bash
# Dentro do container archlinux do job "Wayland KDE": dependências, toolchain, build do
# taildesk-cli e o teste (`wayland-kde.sh`). O cache do cargo fica em /cache (volume do host).
set -euo pipefail

pacman -Syu --noconfirm --needed base-devel git curl fontconfig pipewire wireplumber clang nasm \
    cmake libcap kwin xdg-desktop-portal xdg-desktop-portal-kde flatpak mesa > /tmp/pacman.log ||
    { tail -50 /tmp/pacman.log; exit 1; }
pacman -Q kwin xdg-desktop-portal xdg-desktop-portal-kde pipewire wireplumber mesa

export CARGO_HOME=/cache/cargo RUSTUP_HOME=/cache/rustup CARGO_TARGET_DIR=/cache/target
export PATH="$CARGO_HOME/bin:$PATH"
if ! command -v rustup > /dev/null; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
        sh -s -- -y --profile minimal --default-toolchain none --no-modify-path
fi
rustup toolchain install
cargo build -p taildesk-cli

.github/scripts/wayland-kde.sh "$CARGO_TARGET_DIR/debug/taildesk-cli" 2
