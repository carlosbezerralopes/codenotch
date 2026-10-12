# Codenotch para Windows — projeto pessoal

Versão pessoal em [carlosbezerralopes/codenotch](https://github.com/carlosbezerralopes/codenotch),
baseado no [Codenotch de vinzdg](https://github.com/vinzdg/codenotch).
Uma pequena barra na borda da tela mostra as cotas dos assistentes de código e
se eles estão trabalhando ou aguardando você. A versão Windows usa Rust,
Tauri 2 e WebView2.

Este repositório é privado e independente da rede de forks do GitHub.
A distribuição exige acesso autorizado, preservando a origem e a licença do projeto.

Esta branch prepara créditos Kiro no tooltip, personalização, histórico de
consumo e comparação entre cota e tempo. **Essas novidades ainda não têm uma
release publicada.** O instalador anterior com Kiro foi produzido pelo CI;
isso não comprova a instalação nem a validação do conjunto novo.

## Funcionalidades Windows

- **Kiro:** cota mensal pela CLI nativa instalada e autenticada. O tooltip
  preserva o percentual oficial e mostra os créditos consumidos e o limite
  quando a fonte informa esse par, incluindo bônus e overage. Um percentual
  arredondado nunca vira uma quantidade inventada.
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
- **Codex: cota e tempo:** nas janelas semanais com duração conhecida, compare
  consumo oficial, tempo corrido e tempo útil. O calendário é configurável,
  começa em Fortaleza/CE e considera fins de semana e feriados. As barras de
  tempo são referências calculadas; não são uma previsão de esgotamento.

## Configurar o histórico

Abra **General → Usage history**. No primeiro uso, escolha a pasta e salve:
nenhuma coleta começa antes dessa confirmação. A sugestão é
`Projetos/Codenotch/telemetria` no OneDrive Business; outra pasta pode ser
selecionada com Browse. Depois, é possível mudar o destino, pausar a coleta e
configurar mapeamentos de pasta para projeto.

Eventos e pontos de leitura são gravados em batches por máquina e instalação,
e consolidados sem contar novamente o mesmo evento. Ao reinstalar, selecionar
o mesmo destino permite reconstruir o histórico. O cache SQLite é local;
a sincronização entre máquinas depende de os arquivos chegarem pelo OneDrive.

Consulte [fontes, cobertura e recuperação](windows/docs/telemetry.md) e
[calendário de dias úteis](windows/docs/work-calendar.md).

## Instalar ou compilar

**Ainda não há release Windows publicada neste fork.** Para obter um instalador
de desenvolvimento, abra uma execução concluída de
[Windows Package no Actions](https://github.com/carlosbezerralopes/codenotch/actions/workflows/windows-package.yml),
autenticado no GitHub com acesso ao repositório,
e baixe o artifact da revisão desejada. Verifique os checks e o commit da
execução; um artifact anterior pode não conter as novidades desta branch.

Quando houver uma release com o asset `Codenotch-Setup.exe`, o
[download fixo da última release](https://github.com/carlosbezerralopes/codenotch/releases/latest/download/Codenotch-Setup.exe)
apontará para esse instalador publicado. Esse endereço não aponta para artifacts
do Actions e exige autenticação e permissão de acesso. Instalar o aplicativo
não instala nem autentica o Kiro.

O updater que consulta releases públicas anonimamente não funciona para um
repositório privado sem um mecanismo autenticado. Esse mecanismo ainda precisa
ser definido; a distribuição prevista é por download autenticado do instalador,
sem promessa de atualização automática.

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
