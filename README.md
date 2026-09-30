# Transcreve.ai

App de ditado por voz para desktop: segure um atalho, fale e o texto aparece onde o cursor estiver. O comportamento de referência é o do [Wispr Flow](specs/product/research-wispr-flow.md). A transcrição roda **localmente** (offline) ou numa **API com a sua própria chave**.

> **Status:** pré-alfa. O MVP (v0.1) está em construção. O roadmap está em [`specs/tasks.md`](specs/tasks.md).

## Baseado no Handy

O Transcreve.ai é um fork divergente do [Handy](https://github.com/cjpais/Handy), de CJ Pais, distribuído sob a licença MIT. A decisão e o que herdamos estão no [ADR-0001](docs/adr/0001-fork-do-handy-como-base.md). Os modelos de voz continuam sendo baixados da infraestrutura do Handy (`blob.handy.computer` e a organização `handy-computer` no Hugging Face). Obrigado ao CJ e aos colaboradores do Handy.

## Plataformas

Windows, macOS e Linux. O MVP é entregue e validado primeiro no **Windows 11**; o suporte a macOS e Linux herdado do Handy continua no código e volta a ser validado na T-084.

## Pré-requisitos no Windows

1. **Rust** estável, via [rustup](https://rustup.rs/).
2. **[Bun](https://bun.sh/)**.
3. **Visual Studio Build Tools 2022** com a carga de trabalho _Desenvolvimento para desktop com C++_ (MSVC).
4. **CMake** no `PATH`: `winget install Kitware.CMake`.
5. **Vulkan SDK** (LunarG), usado pelo backend Vulkan do whisper: `winget install KhronosGroup.VulkanSDK`. Ele define `VULKAN_SDK`.

Depois de instalar, **abra um terminal novo** para que o `PATH` e o `VULKAN_SDK` sejam lidos. Linux e macOS: ver [BUILD.md](BUILD.md#prerequisites).

## Desenvolvimento

No **PowerShell** (não no Git Bash):

```powershell
bun install
. .\scripts\windows-dev-env.ps1 -BypassJunction   # prepara o ambiente de build desta sessão
bun run tauri dev
```

O script define `VULKAN_SDK`, um `CARGO_TARGET_DIR` curto e contorna um problema do MSBuild com junctions (erro `MSB1009`) — detalhes em [BUILD.md](BUILD.md#windows-build-fails-with-msb1009-project-file-does-not-exist).

Checagens antes de commitar:

```powershell
bun run lint
bun run format:check
bun run check:translations
bun run test:unit
cd src-tauri; cargo test; cargo clippy
```

Os ícones do app e da bandeja são gerados a partir da marca de barras de som: `bun scripts/generate-brand-icons.ts`.

## Documentação

- [`specs/`](specs/README.md): constituição, spec de produto, arquitetura e specs por feature (SDD).
- [`docs/adr/`](docs/adr/): decisões de arquitetura.
- [`AGENTS.md`](AGENTS.md): comandos e visão da arquitetura do código.
- [`BUILD.md`](BUILD.md): build por plataforma e solução de problemas.
- [`CONTRIBUTING.md`](CONTRIBUTING.md): como contribuir.

## Licença

[MIT](LICENSE). Copyright (c) 2025 CJ Pais e (c) 2026 Creator4all.
