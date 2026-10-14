# Codenotch para Windows — projeto pessoal

Versão pessoal em [carlosbezerralopes/codenotch](https://github.com/carlosbezerralopes/codenotch),
baseado no [Codenotch de vinzdg](https://github.com/vinzdg/codenotch).
Uma pequena barra na borda da tela mostra as cotas dos assistentes de código e
se eles estão trabalhando ou aguardando você. A versão Windows usa Rust,
Tauri 2 e WebView2.

Este fork público mantém o vínculo com `vinzdg/codenotch`. Somente a conta
proprietária tem escrita; a branch principal exige PR e os checks do projeto.

Esta branch prepara a versão Windows 1.22.3, com histórico integrado às
configurações e orçamento por dias úteis. **Ainda não há release deste pacote.**
A versão 1.22.1 foi validada no CI e instalada pelo proprietário; esse aceite
não comprova o conjunto novo.

## Funcionalidades Windows

- **Kiro:** cota mensal pela CLI nativa instalada e autenticada. O tooltip
  preserva o percentual oficial e mostra os créditos consumidos e o limite
  quando a fonte informa esse par, incluindo bônus e overage. Um percentual
  arredondado nunca vira uma quantidade inventada.
  Após três falhas, as consultas pausam e preservam a última leitura;
  [a frequência acompanha a atividade](windows/docs/kiro-polling.md).
- **Claude e Codex:** cotas e horários de renovação das janelas informadas pelo
  provedor. Os demais provedores e suas fontes estão no
  [guia Windows](windows/README.md).
- **Ordem e nomes:** em Accounts, arraste os provedores ou use as setas e altere
  seus nomes de exibição. A personalização preserva a identidade das contas e
  suas cotas; um provedor reativado entra no fim da ordem.
- **Hover com privacidade:** detalhes de atividade ficam recolhidos. Clique no
  cartão ou em Show details para revelá-los; fechar ou trocar a ferramenta
  recolhe os detalhes novamente.
- **Histórico por projeto e pasta:** tokens reportados pelo Claude e Codex,
  créditos reportados pelo Kiro v3, com data e hora do evento. O painel agrupa
  por evento, hora, dia, semana ou mês e filtra ferramenta, projeto, pasta e
  máquina. Tokens e créditos são unidades separadas; dados ausentes mostram
  `—`. Não são exportados prompts, respostas ou credenciais.
- **Cotas e dias úteis:** Claude, Codex e Kiro usam o intervalo real até a
  renovação. A visão **Saldo e prazo** mostra quanto pode usar por dia útil e
  se o ritmo medido esgotaria o saldo antes da renovação, com uma linha visual
  de prazo. Claude/Codex usam percentuais; Kiro mostra créditos e percentuais
  do plano mensal confirmado. Limites de sessão e de modelos ficam compactos.
  Sem histórico suficiente, a previsão fica pendente; uma média do ciclo é
  identificada como tal. As cotas oficiais continuam definidas pelo provedor.
- **Bandeja e configurações:** o menu oferece os mesmos indicadores do notch;
  clicar abre **Cotas** no indicador escolhido. **Histórico** é uma seção lateral
  das configurações, com os filtros e dados de telemetria existentes.

## Configurar o histórico

Abra **Configurações → Histórico** ou o atalho **Histórico** na bandeja.
No primeiro uso, escolha a pasta e salve:
nenhuma coleta começa antes dessa confirmação. A sugestão é
`Projetos/Codenotch/telemetria` no OneDrive Business; outra pasta pode ser
selecionada com Browse. Depois, é possível mudar o destino, pausar a coleta e
configurar mapeamentos de pasta para projeto.

Eventos e pontos de leitura são gravados em batches por máquina e instalação,
e consolidados sem contar novamente o mesmo evento. Ao reinstalar, selecionar
o mesmo destino permite reconstruir o histórico. O cache SQLite é local;
a sincronização entre máquinas depende de os arquivos chegarem pelo OneDrive.
Leituras de cota e orçamento ficam em `quota-readings`, separadas dos eventos
de consumo: percentuais não são somados como tokens ou créditos gastos.

Consulte [fontes, cobertura e recuperação](windows/docs/telemetry.md) e
[calendário de dias úteis](windows/docs/work-calendar.md).

## Instalar ou compilar

Para testar uma alteração **antes de enviar a PR**, execute na raiz:

```powershell
pwsh -NoProfile -File .\windows\scripts\build-local.ps1 -Installer
```

O comando roda os testes e gera o aplicativo e o setup localmente. Consulte
[build local e teste manual](windows/docs/local-build.md) para os requisitos,
arquivos gerados e uso dos dados existentes.

**[Baixar a última build Windows validada — ZIP](https://nightly.link/carlosbezerralopes/codenotch/workflows/windows-package.yml/feat%2Fkiro-windows/Codenotch-Windows.zip)**

Descompacte e execute `Codenotch-Setup.exe`. O link acompanha a última execução
de push bem-sucedida da branch `feat/kiro-windows`, após testes e validação do
instalador. Um build em andamento ou com falha não substitui o último válido.
O link usa o serviço público [nightly.link](https://github.com/oprypin/nightly.link);
não é uma release estável nem uma atualização automática. Artifacts podem expirar.

Alternativa: abra uma execução concluída de
[Windows Package no Actions](https://github.com/carlosbezerralopes/codenotch/actions/workflows/windows-package.yml),
autenticado no GitHub com acesso ao repositório,
e baixe o artifact da revisão desejada. Verifique os checks e o commit da
execução; um artifact anterior pode não conter as novidades desta branch.
Após o merge, a [última build da main](https://nightly.link/carlosbezerralopes/codenotch/workflows/windows-package.yml/main/Codenotch-Windows.zip)
acompanha os pushes validados daquela branch.

Quando houver uma release com o asset `Codenotch-Setup.exe`, o
[download fixo da última release](https://github.com/carlosbezerralopes/codenotch/releases/latest/download/Codenotch-Setup.exe)
apontará para esse instalador publicado. Esse endereço não aponta para artifacts
do Actions. A disponibilidade depende da visibilidade e das permissões do
repositório de distribuição. Instalar o aplicativo não instala nem autentica o Kiro.

Os endereços do updater devem acompanhar o fork de distribuição antes de uma
release. Downloads de um repositório privado exigem um mecanismo autenticado;
a visibilidade pública não dispensa assinatura dos pacotes de atualização.

Para compilar, são necessários Rust com toolchain MSVC e WebView2. Veja os
comandos, pré-requisitos e comportamento das atualizações no
[README Windows](windows/README.md) e o escopo do fork em [FORK.md](FORK.md).

## Documentação e origem

- [Contribuição e convenções](CONTRIBUTING.md).
- [Acesso privado e revisão](windows/docs/repository-policy.md).
- [Documentação original preservada](docs/upstream/README.md), voltada ao
  projeto upstream e ao macOS; suas imagens não representam a versão Windows.
- [Projeto upstream](https://github.com/vinzdg/codenotch) e
  [implementação macOS](Sources).
- [Licença MIT](LICENSE). O nome, o design e as atribuições originais do
  Codenotch são preservados.
