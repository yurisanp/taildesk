#!/usr/bin/env python3
"""Resumo dos resultados JUnit (XML do Gradle) em Markdown, para o GITHUB_STEP_SUMMARY.

Uso: resumo_testes.py <pasta com os XML> [mais pastas...]
"""
import sys
import xml.etree.ElementTree as ET
from pathlib import Path


def main() -> int:
    total = falhas = ignorados = 0
    quebrados = []
    for pasta in sys.argv[1:] or ["."]:
        for xml in sorted(Path(pasta).rglob("TEST-*.xml")):
            try:
                raiz = ET.parse(xml).getroot()
            except ET.ParseError:
                continue
            for caso in raiz.iter("testcase"):
                total += 1
                if caso.find("skipped") is not None:
                    ignorados += 1
                erro = caso.find("failure")
                if erro is None:
                    erro = caso.find("error")
                if erro is not None:
                    falhas += 1
                    msg = (erro.get("message") or "").splitlines()[0:1]
                    quebrados.append(f"{caso.get('classname')}.{caso.get('name')}: {' '.join(msg)}")
    status = "✅" if falhas == 0 else "❌"
    print(f"### {status} Testes JVM do Android\n")
    print(f"| Total | Falhas | Ignorados |\n|---:|---:|---:|\n| {total} | {falhas} | {ignorados} |\n")
    for q in quebrados[:50]:
        print(f"- `{q}`")
    return 0


if __name__ == "__main__":
    sys.exit(main())
