# 01 — Tailscale: contexto técnico para uma ferramenta de acesso remoto

## 1. Modelo mental

Tailscale = **plano de controle centralizado** + **plano de dados em malha (mesh)**.

| Conceito | O que é | Relevância para o produto |
|---|---|---|
| **Tailnet** | Rede privada lógica de uma organização/usuário. | Fronteira de confiança: "quem está na tailnet e passa nas policies pode tentar conectar". |
| **Coordination server** (control plane) | Servidor (login.tailscale.com ou **Headscale** self-hosted) que distribui chaves públicas WireGuard, endpoints e policies. Não carrega tráfego. | Substitui o *rendezvous server* do RustDesk (hbbs). |
| **WireGuard** (data plane) | Túneis cifrados ponta a ponta entre nós; chave privada nunca sai do nó. | Mídia já trafega cifrada E2E entre os dois nós. |
| **NAT traversal** | STUN/ICE-like + protocolo *disco* para abrir caminho direto UDP entre nós atrás de NAT. | Latência mínima; é o caminho "bom" para vídeo. |
| **DERP** | Relays cifrados via HTTPS usados quando não dá conexão direta (ou enquanto ela é estabelecida). Só repassam pacotes já cifrados. | Substitui o *relay server* do RustDesk (hbbr). **Throughput limitado** — vídeo de alta taxa (PyroWave) não é viável via DERP. |
| **IPs 100.x.y.z / fd7a:115c:a1e0::/48** | Endereços estáveis por nó na tailnet (CGNAT range). | Endereço do host remoto = IP/hostname tailnet. |
| **MagicDNS** | Nomes `host.tailnet-xxxx.ts.net`. | Conectar por nome em vez de IP. |
| **ACLs / Grants** | Policy central (HuJSON) aplicada **em cada nó** no momento da decriptação. | Controle de quem pode alcançar a porta do serviço remoto. |
| **App capabilities** | Grants com campo `app` que transportam permissões de aplicação (JSON livre) até o nó destino. | **Autorização dentro do app** (ver controle total vs. só visualizar, transferir arquivos etc.). |
| **Tags** | Identidade de máquina (`tag:remote-host`) em vez de usuário. | Hosts não-assistidos (servidores, quiosques, PDVs). |
| **Auth keys** | Chaves para registrar nós sem login interativo (reutilizáveis, efêmeras, pré-autorizadas, com tags). | Provisionamento em massa de hosts. |
| **Ephemeral nodes** | Nós removidos automaticamente ao ficarem offline. | Sessões de suporte avulsas ("cliente instala, conecta, some"). |
| **Tailnet Lock** | Assinatura de chaves de nós por chaves confiáveis da organização. | Defesa contra comprometimento do control plane. |
| **Headscale** | Implementação open-source do control server. | Opção self-hosted (relevante p/ empresas que já fazem self-host, ex.: GitLab interno). |

Leitura obrigatória: *How Tailscale works* e *How NAT traversal works* (blog oficial).

## 2. Três formas de integrar o Tailscale em um app Rust

> **Decisão do Yuri (15/10/2026): o TailDesk usa só o modo sistema (§2.1).** O Tailscale precisa estar instalado e rodando na máquina; o modo embutido (§2.2) foi avaliado e **não será usado**. §2.2 fica como registro da avaliação.

### 2.1 Modo **sistema** — usa o cliente Tailscale já instalado
O app escuta/conecta em sockets normais do SO no IP da interface `tailscale0`/utun. Para identidade e capabilities consulta a **LocalAPI** do `tailscaled`.

- **Prós**: produção-ready hoje, P2P/NAT traversal maduro, MagicDNS, funciona em **Android e iOS** (app oficial com VpnService / NetworkExtension), throughput de kernel/wireguard-go otimizado.
- **Contras**: depende do usuário ter o Tailscale instalado e logado; o "nó" é o dispositivo, não o app; LocalAPI tem transporte diferente por SO.
- **LocalAPI** (HTTP sobre socket local; usada pelo CLI `tailscale`):
  - Linux: unix socket `/var/run/tailscale/tailscaled.sock` (ou `/run/tailscale/tailscaled.sock`).
  - Windows: named pipe `\\.\pipe\ProtectedPrefix\Administrators\Tailscale\tailscaled`.
  - macOS: depende da variante (app da App Store/Standalone usa porta TCP local + token em arquivo; `tailscaled` open-source usa unix socket).
  - Endpoints úteis: `GET /localapi/v0/status` (peers, self, online, endpoints), `GET /localapi/v0/whois?addr=100.x.y.z:port` (**UserProfile + Node + CapMap** do peer que conectou).
  - Header obrigatório: `Host: local-tailscaled.sock` (o servidor rejeita outros hosts) e, em alguns builds, Basic auth com token.
  - **Não há crate Rust oficial** — escrever um cliente fino (`hyper` + `hyperlocal`/named pipe). É exatamente isso que `tailscale whois` faz.
- **Versão**: `tailscaled` / cliente Tailscale **mínimo 1.102**; estável em 15/10/2026 = 1.104.1. Testes de contrato da LocalAPI contra a mínima e a estável.
- **Caminho**: o `status` diz se o peer está direto (`CurAddr`) ou via DERP (`Relay`); é a pista de caminho da seleção adaptativa (`14-selecao-adaptativa.md`).
- **Ponto de atenção**: a LocalAPI é interna/não-versionada formalmente. Isolar atrás de uma trait e ter testes de contrato.

### 2.2 Modo **embutido** — `tailscale` crate (tailscale-rs) — **não adotado**
"tsnet para Rust": o processo vira um nó próprio da tailnet com netstack userspace, sem precisar do Tailscale instalado. Avaliado em 15/10/2026 (crate 0.6.1) e descartado pelo Yuri: o TailDesk exige o Tailscale instalado. Pesou também o estado da crate: pre-1.0, conexão direta só "em muitos casos" e sem informar o caminho, sem `whois` de usuário, sem MagicDNS, app capabilities nem DERP privado.

### 2.3 Modo `libtailscale` (C) / `tsnet` (Go) — **não recomendado**
`libtailscale` sobe um runtime Go inteiro dentro do processo; o próprio time do Tailscale descreve conflitos de runtime. Também fica de fora.

## 3. Integração no TailDesk

```
trait Overlay  ── SystemOverlay (tailscaled + LocalAPI) → host e controlador (único em produção)
               ── LanOverlay    (sockets crus)          → só desenvolvimento e testes
```

Ao abrir, o app consulta a LocalAPI. Tailscale ausente, parado, deslogado ou abaixo de 1.102 vira uma tela com o motivo e o link de instalação (`10-plano-windows-linux.md` §2.1). O app não faz login nem muda preferências do Tailscale; isso fica com o cliente oficial.

## 4. Políticas (tailnet policy file) sugeridas

```jsonc
{
  "tagOwners": {
    "tag:remote-host":   ["group:it-admins"],
    "tag:remote-client": ["group:it-admins"]
  },
  "grants": [
    {
      // rede: técnicos alcançam a porta do serviço nos hosts
      "src": ["group:suporte"],
      "dst": ["tag:remote-host"],
      "ip":  ["udp:7420"],
      // aplicação: o que podem fazer dentro do halyard
      "app": {
        "aglets.com.br/cap/halyard": [
          { "view": true, "control": true, "clipboard": true, "files": true, "audio": true }
        ]
      }
    },
    {
      "src": ["group:auditoria"],
      "dst": ["tag:remote-host"],
      "ip":  ["udp:7420"],
      "app": { "aglets.com.br/cap/halyard": [ { "view": true } ] }
    }
  ]
}
```

- Nomes de capability seguem `{dominio}/{caminho}`; `tailscale.com`/`tailscale.io` são reservados.
- O app lê as capabilities do peer via LocalAPI `whois`.
- **Mesmo com policy, o host deve pedir consentimento local** em modo "assistido" (UX tipo "Fulano quer controlar sua tela — Permitir?"). Modo "não assistido" só para `tag:remote-host` explicitamente configurado.

## 5. Provisionamento

| Cenário | Como |
|---|---|
| Host fixo da empresa | Tailscale instalado e registrado com auth key reutilizável, **pré-autorizada**, com `tag:remote-host` (`tailscale up --auth-key=...`). O TailDesk não guarda a chave. |
| Suporte avulso a cliente | Cliente instala o Tailscale e entra na tailnet (convite ou auth key efêmera com `tag:support-session`). |
| Técnico (cliente) | Login interativo (SSO) no cliente Tailscale. |

API REST do Tailscale (v2) pode ser chamada com `reqwest`; há crate comunitário `tailscale-client` (pequeno, opcional).

## 6. Portas e pegadinhas

- Use uma porta fixa na tailnet: `7420/udp` (endpoint QUIC único para controle e mídia; ver `07-protocolo.md`). O firewall local (Windows, firewalld no Fedora) pode precisar liberar essa porta na interface do Tailscale.
- **MTU**: WireGuard na tailnet usa MTU ~1280. Datagramas de mídia devem caber em **≤ 1200 bytes de payload** (sem fragmentação IP). QUIC já respeita 1200 por padrão.
- DERP: detectar caminho relayed (`status` mostra `CurAddr` vazio/`Relay`) e medir RTT/banda. Em DERP: limitar bitrate, **desabilitar PyroWave**, preferir AV1/H.265.
- Key expiry: nós com login de usuário expiram (default 180 dias); hosts com tag normalmente têm expiry desabilitado.
