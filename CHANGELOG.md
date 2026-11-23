# Changelog

Mudanças de cada versão do TailDesk. O formato segue o [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/)
e as versões seguem o [SemVer](https://semver.org/lang/pt-BR/). A release do GitHub usa a seção da
versão daqui como notas.

## [0.2.0] - 2026-11-23

Primeira versão com o app gráfico: área de trabalho remota e troca de arquivos entre computadores
Windows e Linux da mesma tailnet.

### Novo
- **App gráfico** em Slint com ícone na bandeja, Wayland nativo e cada sessão na sua janela.
- **Sessão remota sobre QUIC** pela tailnet do Tailscale instalado: pedido de acesso com
  consentimento no host, "Lembrar deste dispositivo" e acesso com senha.
- **Vídeo por monitor** com cursor separado: captura PipeWire (Wayland, pelo portal), X11 sem portal
  e DXGI (Windows); render wgpu.
- **Codecs com hardware quando há**: NVENC/NVDEC, VA-API e Media Foundation, com H.264, HEVC e AV1
  negociados; openh264 como reserva e PyroWave opcional. Painel de diagnóstico na sessão.
- **Rede adaptativa**: BBR, ReceiverReport, taxa e resolução que se ajustam; teste de conexão.
- **Teclado traduzido entre layouts** (modo Auto: teclas mortas, atalhos pela letra, ABNT2 e outros).
- **Multimonitor**: abas, seguir o mouse, monitor em janela própria e "Usar todos os meus monitores".
- **Arquivos**: painel de transferências, navegador do host, arrastar e soltar, retomada e `.part`.
- **Área de transferência** compartilhada (texto, imagem, arquivos e pastas).
- **Som do computador** (Opus por PipeWire e WASAPI) e **chat** na sessão.
- **Wake on LAN**: botão "Ligar" para computadores da tailnet.
- **Iniciar com o sistema** (systemd do usuário, serviço do Windows) e agente da tela de entrada
  no Windows (Ctrl+Alt+Del, UAC).
- **Histórico de acessos** nas Configurações.
- **Atualização automática** do AppImage (zsync).
- Pacotes: AppImage, .deb, .rpm, .tar.gz, MSI e .zip; MetaInfo do AppStream, Flatpak, COPR e AUR.

### Corrigido
- Linux: a permissão de tela lembrada no diálogo do portal não valia quando o app abria por outro
  caminho (menu × início com o sistema), e o diálogo voltava. O app agora se registra no portal com
  o id `io.github.yurisanp.TailDesk`, e a unidade do início com o sistema passa a ser
  `app-io.github.yurisanp.TailDesk-autostart.service` (a `taildesk.service` antiga é migrada).
  Depois de atualizar, o diálogo aparece uma última vez.

## [0.1.0] - 2026-10-15

Primeira versão, só de linha de comando (`taildesk-cli`): transporte QUIC pela tailnet e medição
de rede.

[0.2.0]: https://github.com/yurisanp/taildesk/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/yurisanp/taildesk/releases/tag/v0.1.0
