# TailDesk no COPR (Fedora)

O [COPR](https://copr.fedorainfracloud.org/) hospeda um repositório `.rpm` do TailDesk para
Fedora. O COPR compila do código-fonte nos builders dele (que têm rede para o `cargo`), a
partir de `packaging/copr/taildesk.spec`.

## Configuração (uma vez, feita pelo Yuri)

1. Crie o projeto em https://copr.fedorainfracloud.org/ → **New Project**: nome `taildesk`,
   chroot `fedora-rawhide-x86_64` e os `fedora-N-x86_64` desejados.
2. Em **Packages → New package → SCM**:
   - Clone URL: `https://github.com/yurisanp/taildesk`
   - Subdirectory: (vazio)
   - Spec File: `packaging/copr/taildesk.spec`
   - Build method: `rpkg`
3. O `Version` do spec vem da macro `taildesk_version`. Como o COPR tira a versão da tag/commit,
   deixe a build automática por webhook (Settings → Integrations) ou dispare a cada release.

Quem usa instala com:

```bash
sudo dnf copr enable yurisanp/taildesk
sudo dnf install taildesk
```

## Publicar por linha de comando (opcional, no CI)

O job `copr` do `release.yml` envia a build quando o secret `COPR_TOKEN` existe (o conteúdo do
arquivo `~/.config/copr` que o COPR gera em **API**). Sem o secret, o job é pulado.

```bash
copr-cli build yurisanp/taildesk <srpm>
```
