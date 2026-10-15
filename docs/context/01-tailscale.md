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
| **MagicDNS** | Nomes `host.tailnet-xxxx.ts.net`. | Conectar por nome em vez de IP (no modo embutido, só por IP/`peer_by_name`). |
| **ACLs / Grants** | Policy central (HuJSON) aplicada **em cada nó** no momento da decriptação. | Controle de quem pode alcançar a porta do serviço remoto. |
| **App capabilities** | Grants com campo `app` que transportam permissões de aplicação (JSON livre) até o nó destino. | **Autorização dentro do app** (ver controle total vs. só visualizar, transferir arquivos etc.). |
| **Tags** | Identidade de máquina (`tag:remote-host`) em vez de usuário. | Hosts não-assistidos (servidores, quiosques, PDVs). |
| **Auth keys** | Chaves para registrar nós sem login interativo (reutilizáveis, efêmeras, pré-autorizadas, com tags). | Provisionamento em massa de hosts. |
| **Ephemeral nodes** | Nós removidos automaticamente ao ficarem offline. | Sessões de suporte avulsas ("cliente instala, conecta, some"). |
| **Tailnet Lock** | Assinatura de chaves de nós por chaves confiáveis da organização. | Defesa contra comprometimento do control plane. |
| **Headscale** | Implementação open-source do control server. | Opção self-hosted (relevante p/ empresas que já fazem self-host, ex.: GitLab interno). |

Leitura obrigatória: *How Tailscale works* e *How NAT traversal works* (blog oficial).

## 2. Três formas de integrar o Tailscale em um app Rust

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
- **Classe LAN**: por decisão do Yuri, o modo sistema é o que garante caminho direto/LAN agora; o embutido entra quando o G0 mostrar com que frequência ele fica direto.
- **Ponto de atenção**: a LocalAPI é interna/não-versionada formalmente. Isolar atrás de uma trait e ter testes de contrato.

### 2.2 Modo **embutido** — `tailscale` crate (tailscale-rs, oficial)
"tsnet para Rust": o processo vira **um nó próprio** da tailnet com netstack userspace (smoltcp), sem TUN, sem precisar do Tailscale instalado.

Estado verificado no repositório em 15/10/2026 (`main`, crate `tailscale` **0.6.1**, 17/09/2026):

- ✅ Sockets TCP e UDP na tailnet (`Device::tcp_listen`, `tcp_connect`, `udp_bind`).
- ✅ Comunicação via DERP e **conexões diretas "em muitos casos"** (STUN + disco; `ts_netmon` em Linux, Windows e — desde 0.6.0 — macOS).
- ✅ Interoperável com o cliente Go, `tsnet` e `libtailscale`.
- ✅ **Auditoria de criptografia de terceiros concluída** na 0.6.0; os avisos de segurança do README e a variável `TS_RS_EXPERIMENT` foram removidos (a lib ainda loga um aviso de "work-in-progress").
- ✅ Opção `ephemeral` (default `false` desde 0.5.0).
- ✅ Lookup de peers: `peer_by_name`, `peer_by_tailnet_ip`, `peers_with_route`, `self_node`.
- ✅ Bindings C/Python/Elixir; utilitários para `axum` e um servidor SSH experimental (`russh`).
- ⏳ "Coming soon": DNS forwarding, peer lookup por hostname completo, port mapping.
- ❌ Não suportado: MagicDNS, exit nodes, subnet routers, **DERP privado**, peer relays, app capabilities (como feature de alto nível), HTTPS certs, Serve/Funnel, Taildrop, Tailnet Lock, métricas.
- ❌ Plataformas não suportadas: **Android, iOS**, BSDs. Suportadas: Linux x86_64/ARM64, macOS ARM64, Windows x86_64.
- Edição Rust 2024, MSRV rolante (atual 1.97 no changelog). Pre-1.0: **sem garantia de compatibilidade** — fixe a versão exata (`=0.6.1`) e encapsule.
- Licença **BSD-3-Clause** (+ arquivo PATENTS).

API pública (de `src/lib.rs`):

```rust
use tailscale::{Config, Device};

let dev = Device::new(
    &Config::default_with_key_file("halyard_state.json").await?,
    Some(auth_key),            // tskey-auth-... (com tag:remote-host)
).await?;

let me   = dev.ipv4_addr().await?;                 // 100.x.y.z
let udp  = dev.udp_bind((me, 7420).into()).await?;   // endpoint QUIC único (controle + mídia)
let dbg  = dev.tcp_listen((me, 7421).into()).await?; // opcional: diagnóstico
let peer = dev.peer_by_tailnet_ip(remote_ip).await?; // Option<NodeInfo>
// NodeInfo (= ts_control::Node): id, stable_id, hostname, tailnet, tags,
// node_capabilities, tailnet_address, node_key, disco_key, underlay_addresses...
dev.shutdown(Some(Duration::from_secs(2))).await;
```

Observações importantes para o produto:
- `NodeInfo` traz **tags** e `node_capabilities`, mas **não traz o perfil de usuário (login)** como o `whois` da LocalAPI. Autorização por *usuário* no modo embutido é uma lacuna — ver `08-roadmap.md` (pergunta em aberto Q1). Há crate interno `ts_peercapability`; verificar se/quando expõe *peer caps* na API pública.
- Netstack userspace (smoltcp) com buffers TCP de 64 KiB (0.6.0). **Throughput para vídeo de alta taxa ainda precisa ser medido** — é um *gate* do roadmap (fase 1).
- Arquitetura interna: `ts_runtime` (atores + barramento de mensagens), `ts_control` (+ Noise IK), `ts_dataplane`, `ts_tunnel` (WireGuard), `ts_derp`, `ts_netstack_smoltcp*`, `ts_packetfilter` (aplica ACL/grants localmente). Ler `ARCHITECTURE.md` do repo.
- Funciona com **Headscale** (o control client fala o mesmo protocolo).

### 2.3 Modo `libtailscale` (C) / `tsnet` (Go) — **não recomendado**
`libtailscale` sobe um runtime Go inteiro dentro do processo; o próprio time do Tailscale descreve conflitos de runtime. Em Rust, `tailscale-rs` substitui isso.

## 3. Recomendação de integração

```
trait Overlay  ── EmbeddedOverlay  (tailscale-rs)      → desktop host/cliente (padrão)
               ── SystemOverlay    (tailscaled+LocalAPI)→ mobile, ou quando o usuário já usa Tailscale
               ── DirectLanOverlay (sockets crus, dev/LAN)→ só para testes e PyroWave em LAN controlada (opcional)
```

Escolha em runtime: se o SO tem Tailscale ativo e o usuário prefere → `SystemOverlay`; senão `EmbeddedOverlay`. No Android/iOS: sempre `SystemOverlay` (até o tailscale-rs suportar mobile).

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
- O app lê as capabilities do peer via LocalAPI `whois` (modo sistema). No modo embutido, ver Q1.
- **Mesmo com policy, o host deve pedir consentimento local** em modo "assistido" (UX tipo "Fulano quer controlar sua tela — Permitir?"). Modo "não assistido" só para `tag:remote-host` explicitamente configurado.

## 5. Provisionamento

| Cenário | Como |
|---|---|
| Host fixo da empresa | Auth key reutilizável, **pré-autorizada**, com `tag:remote-host`, gerada pela API do Tailscale (OAuth client). Instalador grava estado do nó em local protegido (DPAPI/Keychain/arquivo 0600). |
| Suporte avulso a cliente | Auth key **efêmera** de uso único com tag `tag:support-session`; nó some ao fechar o app. |
| Técnico (cliente) | Login interativo (SSO) no modo sistema, ou nó embutido com tag de cliente. |

API REST do Tailscale (v2) pode ser chamada com `reqwest`; há crate comunitário `tailscale-client` (pequeno, opcional).

## 6. Portas e pegadinhas

- Use uma porta fixa na tailnet: `7420/udp` (endpoint QUIC único para controle e mídia; ver `07-protocolo.md`). Não precisa abrir nada no firewall do SO no modo embutido (é userspace) — **no modo sistema** o firewall local pode precisar permitir a interface Tailscale.
- **MTU**: WireGuard na tailnet usa MTU ~1280. Datagramas de mídia devem caber em **≤ 1200 bytes de payload** (sem fragmentação IP). QUIC já respeita 1200 por padrão.
- DERP: detectar caminho relayed (no modo sistema: `status` mostra `CurAddr` vazio/`Relay`); no embutido, medir RTT/banda. Em DERP: limitar bitrate, **desabilitar PyroWave**, preferir AV1/H.265.
- Key expiry: nós com login de usuário expiram (default 180 dias); hosts com tag normalmente têm expiry desabilitado.
