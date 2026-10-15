# 11 — Teclado: tradução automática entre layouts

> Requisito do Yuri (15/10/2026): um Windows com **US International** controlando um Linux com
> **ABNT2** (e o contrário) tem que digitar certo, sem o usuário configurar nada. O TailDesk
> **descobre os dois layouts e traduz sozinho**. Complementa `04-plataformas.md` §3.6 e
> substitui as mensagens de teclado de `07-protocolo.md` §3.

## 1. O problema

Ferramentas de acesso remoto costumam escolher um de dois modos, e os dois falham quando os layouts diferem:

| Modo | O que manda | Onde quebra (Windows US-Intl → Linux ABNT2) |
|---|---|---|
| **Físico** (scancode) | "a tecla na posição X foi apertada" | O host interpreta a posição no ABNT2: a tecla `;` do US vira `ç`, `/` vira `;`, `'` vira `~`. Atalhos funcionam, texto sai errado. |
| **Texto** (unicode) | "o caractere `é`" | Atalhos quebram (Ctrl+C não é um caractere), teclas mortas viram lixo (`'` + `e` chega como `'` e `e` separados ou nada), e jogos/apps que leem scancode não funcionam. |

Casos clássicos de dor neste par de layouts:
- **Teclas mortas diferentes.** No US-Intl `'` e `"` são mortas (`'`+`e` = `é`, `'`+`c` = `ç` no Windows); no ABNT2 as mortas são `´ \` ~ ^ ¨` em outras posições.
- **Teclas que só existem no ABNT2**: a tecla `/ ?` ao lado do Shift direito (HID 0x87, *International1*) e a vírgula/ponto extra do teclado numérico (HID 0x85, *Keypad Comma*). Teclado US não tem como apertá-las fisicamente.
- **AltGr.** No ABNT2, AltGr+Q = `/`, AltGr+W = `?`, AltGr+E = `°`. O Windows representa AltGr como **Ctrl esquerdo falso + Alt direito**, então um tradutor ingênuo acha que é um atalho Ctrl+Alt.
- **Atalhos devem seguir a letra, não a posição** (Ctrl+Z no ABNT2 e no US estão no mesmo lugar, mas em AZERTY não; e Ctrl+; / Ctrl+ç dependem do app).

## 2. Solução: modo **Auto** (padrão), com inferência dos layouts

### 2.1 Cada lado descobre o próprio layout
| SO | Como descobrir | Como ler o mapa completo (tecla × nível → caractere) |
|---|---|---|
| Windows | `GetKeyboardLayout(thread da janela em foco)` → HKL; `GetKeyboardLayoutNameW` → KLID (ex.: `00020409` = US-Intl, `00010416` = ABNT2). **No Windows o layout é por thread/janela**: reler ao trocar o foco e em `WM_INPUTLANGCHANGE`. | `MapVirtualKeyExW` + `ToUnicodeEx` (com e sem Shift/AltGr, e cada tecla morta + base para montar a tabela de composição) e `VkKeyScanExW` para a busca reversa. |
| Linux X11 | `XkbGetState` (grupo ativo) + `_XKB_RULES_NAMES` (layout/variante, ex.: `br` / `abnt2`, `us` / `intl`). | `xkbcommon` (crate 0.9): `xkb_keymap_key_get_syms_by_level` para todas as teclas e níveis + `xkb_compose` (tabela Compose do locale) para teclas mortas. |
| Linux Wayland | O compositor manda o **keymap XKB** ao cliente libei (`reis`); sem libei, ler `org.gnome.desktop.input-sources` / `kxkbrc` ou `localectl`. | `xkbcommon` sobre o keymap recebido. |

Cada lado manda ao outro um `KeyboardInfo` no handshake e um `KeyboardLayoutChanged` quando o layout muda no meio da sessão. Os dois lados guardam a tabela do **próprio** layout; ninguém precisa ter a tabela do layout alheio.

### 2.2 Decisão automática
- **Layouts iguais** (ex.: ABNT2 ↔ ABNT2) → **Físico puro**. É o mais fiel: atalhos, jogos e teclas mortas funcionam como no teclado local.
- **Layouts diferentes** → **Auto híbrido** (abaixo).
- O usuário pode forçar **Físico** (jogos) ou **Texto** por host; a escolha fica salva para aquele host.

### 2.3 Auto híbrido: cada tecla vai pelo caminho certo
O **controlador** classifica cada evento antes de mandar:

| Tipo de tecla | O que o controlador manda | O que o host faz |
|---|---|---|
| **Não imprimível** (Esc, F1–F24, setas, Enter, Backspace, Tab, Delete, Home/End, PgUp/PgDn, Insert, PrintScreen, modificadores sozinhos, navegação do numpad) | `Key{hid, down}` físico | Injeta a mesma tecla física. Igual em todo layout. |
| **Atalho** (Ctrl, Alt ou Win/Super + tecla, **exceto AltGr**) | `KeyShortcut{base_char, hid_hint, mods, down}`, onde `base_char` é o caractere sem modificador que **o layout do controlador** produz naquela tecla (Ctrl+C → `c`) | Procura no **próprio** layout a tecla cujo nível base produz `base_char` e injeta com os modificadores. Se não achar (ex.: Ctrl+`ç` num host US), usa a posição física `hid_hint`. |
| **Caractere** (tudo que produz texto, incluindo AltGr e Shift) | `KeyChar{text, hid_hint}`. As **teclas mortas são resolvidas no controlador**, com o comportamento do SO dele: no Windows US-Intl, `'` + `c` chega ao host como `ç`; `'` + espaço chega como `'`. | Busca reversa no layout do host (§2.4) e injeta a sequência que produz exatamente `text`. |

Por que resolver as mortas no controlador: o usuário digita do jeito que está acostumado no **seu** teclado e o resultado é o que ele veria localmente. O host nunca vê uma tecla morta "pela metade".

Detalhes que precisam estar certos:
- **AltGr no Windows**: o evento `VK_LCONTROL` sintético que o Windows gera antes de `VK_RMENU` (mesmo timestamp, scancode 0x21D) tem que ser descartado, e Ctrl+Alt causado por AltGr conta como **caractere**, não atalho. Ctrl+Alt de verdade (dois dedos) continua sendo atalho.
- **Repetição de tecla**: o controlador encaminha cada repetição gerada pelo SO dele como um novo `KeyChar`; o host faz press+release imediato, então a repetição do host nunca dispara.
- **Caps Lock / Num Lock**: no modo caractere o estado do Caps do host não pode interferir; o host lê o próprio estado e compensa o Shift ao injetar (ou desliga o Caps temporariamente). No modo físico, o estado dos locks é sincronizado do controlador para o host no início e a cada mudança.
- **Foco**: ao perder o foco da janela da sessão, o controlador manda "soltar tudo"; ao ganhar, manda o estado atual dos modificadores. Evita Shift ou Ctrl "presos" no host.
- **Teclas exclusivas do ABNT2** (`/ ?` HID 0x87 e Keypad Comma 0x85): quando o controlador é ABNT2 e o host não tem essas teclas, elas viram `KeyChar` (`/`, `?`, `,`), então funcionam do mesmo jeito.

### 2.4 Busca reversa no host (caractere → teclas)
Na conexão e a cada troca de layout, o host monta uma tabela `char → sequência`:
1. **Tecla direta**: varre todas as teclas × níveis (base, Shift, AltGr, Shift+AltGr). Ex. ABNT2: `ç` = tecla HID 0x33 sem modificador; `/` = AltGr+Q ou a tecla 0x87.
2. **Via tecla morta**: para cada tecla morta do layout × cada tecla base, registra o resultado da composição. Ex. ABNT2: `é` = `´` (HID 0x2F) + `e`; `ã` = `~` (HID 0x34) + `a`; `ü` = Shift+`6` (¨) + `u`.
3. Entre várias opções, prefere a que tem menos teclas e não usa AltGr.
4. **Caractere fora do layout** (ex.: `ß` num host ABNT2):
   - Windows: `SendInput` com `KEYEVENTF_UNICODE`. Funciona em quase todo app.
   - Linux X11: remapeia temporariamente um keycode livre para o keysym (`XChangeKeyboardMapping`), injeta, e restaura. É a técnica do `xdotool`.
   - Linux Wayland: tenta `NotifyKeyboardKeysym` do portal RemoteDesktop. Se o compositor não aceitar, o caractere é **descartado com aviso no HUD**. Wayland não tem injeção de unicode padronizada; não vamos colar via clipboard sem o usuário pedir, porque sobrescreve o clipboard dele.

### 2.5 O que fica fora do Auto
- **Métodos de entrada (IME)** de chinês, japonês e coreano no controlador: o texto composto chega como `KeyChar` com string completa; funciona para texto, sem pré-edição remota.
- **Jogos**: precisam do modo Físico (forçado pelo usuário ou detectado em fase futura).

## 3. Mensagens (substituem `Key`/`Text` de `07-protocolo.md` §3)

```proto
message KeyboardInfo {          // em Hello / HelloAck
  string os_layout_id = 1;      // KLID ("00020409") ou "xkb:us(intl)"
  string layout_tag = 2;        // normalizado: "us-intl", "br-abnt2", "us", "pt", ...
  repeated uint32 extra_hid = 3;// teclas físicas que existem (ex.: 0x87, 0x85 no ABNT2)
}
message KeyboardLayoutChanged { KeyboardInfo info = 1; }
message KeyboardMode { enum Mode { AUTO = 0; PHYSICAL = 1; TEXT = 2; } Mode mode = 1; }
message Key         { uint32 hid = 1; bool down = 2; uint32 locks = 3; }      // físico
message KeyShortcut { string base_char = 1; uint32 hid_hint = 2; uint32 mods = 3; bool down = 4; }
message KeyChar     { string text = 1; uint32 hid_hint = 2; }                // um ou mais graphemes
message ReleaseAll  {}
```
O `layout_tag` serve para UI, logs e decidir "iguais vs diferentes". A tradução nunca depende dele: usa as tabelas reais lidas do SO, então layouts que não conhecemos (Dvorak, layouts customizados) também funcionam.

## 4. Onde fica no código
- `taildesk-platform/keyboard/`: `KeymapReader` (Windows / xkbcommon), `ReverseMap` (busca reversa), `KeyInjector` (SendInput / XTest / portal+libei), classificador do controlador.
- `taildesk-proto`: mensagens acima.
- UI: seletor Auto / Físico / Texto na barra da sessão; indicador "US-Intl → ABNT2" no HUD.

## 5. Testes (obrigatórios para fechar a fase 2)
Tabelas de layout gravadas como fixtures (dump de `ToUnicodeEx` e de keymaps XKB) permitem testar a tradução **sem SO**, em CI, para qualquer par.

Matriz mínima de pares: Win US-Intl → Linux ABNT2, Linux ABNT2 → Win US-Intl, Win ABNT2 → Linux US, Linux US → Win ABNT2, ABNT2 ↔ ABNT2 (deve cair em Físico), US ↔ US-Intl.

Casos por par:
- `ç Ç á é í ó ú à â ê ô ã õ ü` e as mesmas maiúsculas
- `' " \` ~ ^ ´ ¨` digitados sozinhos (morta + espaço) e seguidos de consoante (`'t` deve sair `'t`)
- `/ ? ; : [ ] { } \ |` e a tecla 0x87 do ABNT2
- AltGr+Q/W/E no ABNT2 (`/ ? °`)
- Ctrl+C, Ctrl+V, Ctrl+Z, Ctrl+Shift+T, Alt+Tab, Win+E, Ctrl+Alt+T (Linux)
- Numpad com Num Lock ligado e desligado, Keypad Comma (0x85)
- Caps Lock ligado no host e desligado no controlador (e o contrário)
- Trocar o layout do controlador no meio da sessão
- Segurar uma tecla (repetição) e perder o foco com Shift apertado
