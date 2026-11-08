#!/usr/bin/env bash
# Teste do caminho Wayland nativo sem máquina real (docs/context/18-testes-multiplataforma.md §4.2):
# KWin sem tela (`kwin_wayland --virtual`, N monitores), PipeWire + WirePlumber,
# xdg-desktop-portal + backend KDE e a pré-autorização `kde-authorized` (Plasma ≥ 6.3), que
# troca o diálogo "Pedido de controle remoto" por um "sim" gravado no permission store.
# Roda o `taildesk-cli portal` (RemoteDesktop + ScreenCast numa sessão, PipeWire e libei) e
# falha se algum monitor não entregar quadros ou se o teclado/mouse não vierem.
#
# Uso: wayland-kde.sh <taildesk-cli> [monitores]   (sem dbus: o script se reabre em
# `dbus-run-session`). Precisa de: kwin, xdg-desktop-portal(-kde), pipewire, wireplumber, flatpak.
set -euo pipefail

CLI=$(realpath "$1")
MONITORES=${2:-2}

if [[ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]]; then
    exec dbus-run-session -- "$0" "$@"
fi

XDG_RUNTIME_DIR=$(mktemp -d)
chmod 700 "$XDG_RUNTIME_DIR"
LOGS=${TAILDESK_LOGS:-$XDG_RUNTIME_DIR/logs}
mkdir -p "$LOGS"
export XDG_RUNTIME_DIR
export XDG_CURRENT_DESKTOP=KDE XDG_SESSION_TYPE=wayland XDG_SESSION_DESKTOP=KDE
export WAYLAND_DISPLAY=wayland-taildesk QT_QPA_PLATFORM=wayland
export KDE_FULL_SESSION=true KDE_SESSION_VERSION=6
export QT_FORCE_STDERR_LOGGING=1

# O ScreenCast do KWin só existe com composição OpenGL, e o KWin só usa OpenGL com um render node.
# Sem GPU: módulo vgem no kernel do host (`modprobe vgem`) + Mesa llvmpipe.
if ! compgen -G "/dev/dri/renderD*" > /dev/null; then
    echo "::error::sem /dev/dri/renderD*: carregue o vgem no host e passe --device /dev/dri"
    exit 1
fi

# O pacote dá cap_sys_nice ao kwin_wayland, e o container do CI não tem essa capability: sem
# tirar, o exec falha com EPERM.
if command -v getcap > /dev/null && getcap "$(command -v kwin_wayland)" | grep -q cap_; then
    setcap -r "$(command -v kwin_wayland)" 2> /dev/null || true
fi

pids=()
fim() {
    for p in "${pids[@]}"; do kill "$p" 2> /dev/null || true; done
    wait 2> /dev/null || true
}
trap fim EXIT

no_barramento() { # sem ativar o serviço, ao contrário de `busctl status`
    busctl --user --acquired --no-pager --no-legend list | grep -q "^$1 "
}

esperar() { # esperar <descrição> <comando…>
    local o=$1
    shift
    for _ in $(seq 100); do
        if "$@" > /dev/null 2>&1; then return 0; fi
        sleep 0.2
    done
    echo "::error::$o não subiu"
    tail -50 "$LOGS"/*.log || true
    exit 1
}

pipewire > "$LOGS/pipewire.log" 2>&1 &
pids+=($!)
esperar "pipewire" test -S "$XDG_RUNTIME_DIR/pipewire-0"
wireplumber > "$LOGS/wireplumber.log" 2>&1 &
pids+=($!)

kwin_wayland --virtual --no-lockscreen --no-global-shortcuts --no-kactivities \
    --width 1920 --height 1080 --output-count "$MONITORES" --socket "$WAYLAND_DISPLAY" \
    > "$LOGS/kwin.log" 2>&1 &
pids+=($!)
esperar "kwin_wayland" test -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY"
sleep 1
if grep -q "Found no render device" "$LOGS/kwin.log"; then
    echo "::error::o KWin não achou o render node (composição QPainter não tem ScreenCast)"
    cat "$LOGS/kwin.log"
    exit 1
fi

# Os portais são ativados pelo D-Bus e herdam este ambiente.
dbus-update-activation-environment WAYLAND_DISPLAY XDG_CURRENT_DESKTOP XDG_SESSION_TYPE \
    XDG_SESSION_DESKTOP XDG_RUNTIME_DIR QT_QPA_PLATFORM KDE_FULL_SESSION KDE_SESSION_VERSION

# App sem app id (processo do host, não Flatpak) = id vazio na tabela do KDE.
flatpak permission-set kde-authorized remote-desktop "" yes

# O backend KDE sobe à mão (log próprio); o xdg-desktop-portal vem pela ativação do D-Bus.
/usr/lib/xdg-desktop-portal-kde \
    > "$LOGS/portal-kde.log" 2>&1 &
pids+=($!)
esperar "xdg-desktop-portal-kde" no_barramento org.freedesktop.impl.portal.desktop.kde

saida="$LOGS/taildesk-cli.log"
set +e
timeout 90 "$CLI" portal 2>&1 | tee "$saida"
codigo=${PIPESTATUS[0]}
set -e

falhou=0
if [[ $codigo -ne 0 ]]; then
    echo "::error::taildesk-cli portal saiu com $codigo"
    falhou=1
fi
achados=$(grep -c '^monitor [0-9]*: node PipeWire' "$saida" || true)
if [[ $achados -ne $MONITORES ]]; then
    echo "::error::o portal liberou $achados monitores, esperava $MONITORES"
    falhou=1
fi
if ! grep -q '^teclado: sim, mouse: sim' "$saida"; then
    echo "::error::o portal não liberou teclado e mouse"
    falhou=1
fi
medidos=$(grep -Ec '^monitor [0-9]+: [1-9][0-9]* quadros' "$saida" || true)
if [[ $medidos -ne $MONITORES ]]; then
    echo "::error::$medidos de $MONITORES monitores entregaram quadros"
    falhou=1
fi
if [[ $falhou -ne 0 ]]; then
    for f in "$LOGS"/*.log; do
        echo "== $f"
        tail -40 "$f"
    done
fi
exit $falhou
