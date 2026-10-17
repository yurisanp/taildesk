# Spec do TailDesk para o COPR (Fedora). O COPR compila do código-fonte nos builders dele
# (com rede para o cargo), então este spec constrói a partir do tarball do GitHub Releases.
#
# Projeto COPR sugerido: yurisanp/taildesk, método SCM apontando para este arquivo, ou
# `copr-cli build yurisanp/taildesk <srpm>` a cada versão (ver packaging/copr/README.md).
Name:           taildesk
Version:        %{?taildesk_version}%{!?taildesk_version:0.0.0}
Release:        1%{?dist}
Summary:        Acesso remoto pela tailnet do Tailscale (desktop remoto e troca de arquivos)

License:        MIT OR Apache-2.0
URL:            https://github.com/yurisanp/taildesk
# Código da versão; o COPR baixa este tarball.
Source0:        %{url}/archive/v%{version}/%{name}-%{version}.tar.gz

ExclusiveArch:  x86_64

BuildRequires:  cargo
BuildRequires:  gcc
BuildRequires:  gcc-c++
BuildRequires:  nasm
BuildRequires:  cmake
BuildRequires:  pkgconfig(fontconfig)
BuildRequires:  pkgconfig(libpipewire-0.3)
BuildRequires:  clang-devel
BuildRequires:  desktop-file-utils
BuildRequires:  libappstream-glib

# O TailDesk usa o Tailscale instalado na máquina (ADR-009), não embute nó nenhum.
Requires:       tailscale

%global debug_package %{nil}

%description
TailDesk dá acesso remoto (ver e controlar a tela, além de troca de arquivos) entre
computadores da mesma tailnet do Tailscale, com tradução automática de layout de teclado,
vários monitores, áudio e seleção adaptativa de rede. Precisa do cliente Tailscale
(>= 1.102) instalado e conectado.

%prep
%autosetup -n %{name}-%{version}

%build
# openh264 precisa do nasm para o SIMD; o build falha sem ele (como no CI).
export TAILDESK_EXIGIR_NASM=1
cargo build --release --locked -p taildesk -p taildesk-cli

%install
install -Dm0755 target/release/taildesk     %{buildroot}%{_bindir}/taildesk
install -Dm0755 target/release/taildesk-cli  %{buildroot}%{_bindir}/taildesk-cli
install -Dm0644 packaging/linux/taildesk.desktop %{buildroot}%{_datadir}/applications/taildesk.desktop
install -Dm0644 packaging/linux/taildesk.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/taildesk.svg
install -Dm0644 packaging/linux/io.github.yurisanp.TailDesk.metainfo.xml %{buildroot}%{_metainfodir}/io.github.yurisanp.TailDesk.metainfo.xml
install -Dm0644 LICENSE-MIT    %{buildroot}%{_datadir}/licenses/%{name}/LICENSE-MIT
install -Dm0644 LICENSE-APACHE %{buildroot}%{_datadir}/licenses/%{name}/LICENSE-APACHE

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/taildesk.desktop
appstream-util validate-relax --nonet %{buildroot}%{_metainfodir}/io.github.yurisanp.TailDesk.metainfo.xml

%files
%license LICENSE-MIT LICENSE-APACHE
%{_bindir}/taildesk
%{_bindir}/taildesk-cli
%{_datadir}/applications/taildesk.desktop
%{_datadir}/icons/hicolor/scalable/apps/taildesk.svg
%{_metainfodir}/io.github.yurisanp.TailDesk.metainfo.xml

%changelog
* Fri Oct 17 2026 TailDesk contributors - 0.2.0-1
- Empacotamento inicial para o COPR (fase 5).
