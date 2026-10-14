# Build local no Windows

Use `scripts/build-local.ps1` para validar antes de propor uma PR. O script acompanha
as etapas de testes e compilação de [Windows Package](../../.github/workflows/windows-package.yml).
O padrão gera o aplicativo e o hook; `-Installer` também gera o instalador NSIS.

## Requisitos

Instale os requisitos separadamente, conforme as instruções oficiais:

- [PowerShell 7](https://learn.microsoft.com/powershell/scripting/install/installing-powershell-on-windows).
- [Rust via rustup](https://www.rust-lang.org/tools/install), com toolchain ativo
  `x86_64-pc-windows-msvc`. O script procura `cargo` e `rustup` no PATH e em
  `~/.cargo/bin`, e verifica o host de `rustc`.
- [Visual Studio Build Tools / Desktop development with C++](https://v2.tauri.app/start/prerequisites/#windows),
  incluindo MSVC x64/x86 e um Windows SDK. `vswhere` deve estar no PATH ou no
  diretório padrão do Visual Studio Installer. O script exige o workload C++ e
  o componente MSVC; erros de linker/SDK interrompem a compilação.
- [Node.js](https://nodejs.org/en/download) com suporte a `node --test` (use LTS).
  O instalador exige também `npx`, normalmente incluído no Node.js.
- [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)
  para executar o aplicativo; normalmente já está presente no Windows 11.

O script não instala esses requisitos. Cargo pode baixar dependências; `-Installer`
usa `npx --yes @tauri-apps/cli@2.11.4` e pode baixar ferramentas de empacotamento.
A primeira compilação pode demorar; as seguintes reutilizam o cache incremental
de dependências em `windows/target`. Não há prazo fixo: hardware e rede influenciam.

## Comandos

Na raiz do repositório:

```powershell
pwsh -NoProfile -File .\windows\scripts\build-local.ps1
pwsh -NoProfile -File .\windows\scripts\build-local.ps1 -Installer
```

Também funciona a partir de outra pasta, passando o caminho do script. A raiz é
resolvida pelo próprio arquivo; o diretório atual e as variáveis de ambiente que
ele altera são restaurados ao terminar, inclusive em falha.

As etapas são: sintaxe dos scripts inline HTML e dos JavaScript externos da UI,
todos os testes `test-*.cjs` em `windows/` e `windows/scripts/`, `cargo test --locked`,
hook release com CRT estático em `target/hook` e aplicativo release. No build padrão,
Cargo usa `--features tauri/custom-protocol` (modo de produção). Com `-Installer`,
a CLI Tauri compila o aplicativo e gera o NSIS com `tauri.bundle.conf.json`, sem
uma compilação Cargo extra do aplicativo. Qualquer comando com código de saída
diferente de zero interrompe o restante. Não são executados testes Rust marcados
`ignored`, nem os testes de instalação/desinstalação de CI.

## Artefatos e teste manual

| Saída | Caminho relativo a `windows/` |
|---|---|
| Aplicativo | `target/release/codenotch.exe` |
| Hook ao lado do aplicativo | `target/release/codenotch-hook.exe` |
| Hook com CRT estático, usado pelo bundler | `target/hook/release/codenotch-hook.exe` |
| Instalador opcional, nome fixo | `target/local-build/Codenotch-Setup.exe` |
| Log e manifesto com horário, resultado e SHA256 | `target/local-build/<horário>/build.log` e `manifest.json` |

Considere o resultado válido somente se a execução terminar sem erro e o seu
manifesto indicar `passed`; artefatos de uma execução antiga podem continuar no
diretório depois de uma falha. O script verifica aplicativo e hook e calcula
SHA256 dos artefatos finais. O instalador é local, sem assinatura e sem feed de
atualização; ele não publica release nem envia arquivos.

Antes de testar, feche manualmente qualquer Codenotch em execução. Depois, execute
`windows/target/release/codenotch.exe` e confira a alteração na interface. O build
não inicia, encerra, instala ou desinstala o aplicativo automaticamente.

**O executável local usa os mesmos dados de produção do usuário**:
`%APPDATA%\codenotch`, credenciais já existentes dos provedores e o destino de
telemetria configurado, incluindo OneDrive. Não há isolamento de perfil neste
procedimento; mudanças manuais nas configurações ou hooks têm efeitos reais.

Após os testes do executável, o instalador opcional pode ser executado manualmente
para validar a instalação no usuário atual. Ele pode substituir a instalação
existente; SmartScreen pode avisar por ser um instalador sem assinatura. Registre
os resultados e só então encaminhe a PR aprovada. Build local e PR não publicam
uma release; publicação permanece uma ação posterior explícita.
