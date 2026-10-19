#!/usr/bin/env python3
"""Gera crates/taildesk-ui/ui/icones/taildesk-64.png (ícone da bandeja e da janela) a partir do
desenho de packaging/linux/io.github.yurisanp.TailDesk.svg, refeito aqui com Pillow para não depender de rsvg. A
bandeja precisa de pixels: o Slint não converte SVG para o ícone da bandeja.

    python3 packaging/icone-bandeja.py
"""
from pathlib import Path

from PIL import Image, ImageDraw

K = 16  # supersampling
N = 256 * K
raiz = Path(__file__).resolve().parent.parent


def s(v: float) -> int:
    return int(v * K)


im = Image.new("RGBA", (N, N), (0, 0, 0, 0))
d = ImageDraw.Draw(im)
d.rounded_rectangle([s(8), s(8), s(248), s(248)], radius=s(56), fill="#1f2937")
# Tela: retângulo com contorno de 14 px.
d.rounded_rectangle([s(37), s(53), s(219), s(179)], radius=s(21), fill="#e5e7eb")
d.rounded_rectangle([s(51), s(67), s(205), s(165)], radius=s(7), fill="#1f2937")
d.rounded_rectangle([s(93), s(193), s(163), s(207)], radius=s(7), fill="#e5e7eb")
for cx in (98, 128, 158):
    d.ellipse([s(cx - 12), s(104), s(cx + 12), s(128)], fill="#60a5fa")
pequeno = im.resize((64, 64), Image.LANCZOS)
pequeno.save(raiz / "crates/taildesk-ui/ui/icones/taildesk-64.png", optimize=True)
