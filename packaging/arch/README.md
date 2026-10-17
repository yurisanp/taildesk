# TailDesk no AUR (Arch Linux)

Dois pacotes:

- **`taildesk-bin`** (`taildesk-bin/PKGBUILD`): instala o binário do GitHub Releases. É o
  recomendado. Atualizado a cada versão por `packaging/arch/update-aur.sh`.
- **`taildesk-git`** (`taildesk-git/PKGBUILD`): compila a `main` do git (versão de
  desenvolvimento).

Instalar com um AUR helper, por exemplo:

```bash
paru -S taildesk-bin   # ou taildesk-git
```

## Publicar (feito pelo Yuri, uma vez por versão)

O job `aur` do `release.yml` roda `update-aur.sh` quando o secret `AUR_SSH_KEY` existe (chave
SSH privada cadastrada em https://aur.archlinux.org/ → My Account). Sem o secret, o job é
pulado e dá para enviar à mão:

```bash
packaging/arch/update-aur.sh 0.2.0        # baixa o tarball da release e calcula o sha
```

O `taildesk-git` muda pouco (só quando o PKGBUILD muda); envie-o à mão quando precisar.
