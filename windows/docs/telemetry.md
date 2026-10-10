# Histórico local de consumo

Na primeira abertura, escolha a pasta em **Telemetria**. O aplicativo sugere
`Projetos/Codenotch/telemetria` no OneDrive Business; na ausência dele, tenta o
OneDrive pessoal. É possível procurar outra pasta e alterar a configuração depois.
Nenhuma coleta começa antes de salvar essa escolha.

## Arquivos e recuperação

Cada leitura com progresso publica um arquivo imutável contendo os eventos
numéricos e os respectivos pontos de leitura. Eles ficam em
`machines/<maquina>/<instalacao>/batches/<mes-da-leitura>/`. O aplicativo consolida
essas pastas e deduplica os eventos; não soma novamente uma requisição copiada
entre máquinas.

Após reinstalar ou formatar, selecione o mesmo destino para reconstruir o resumo.
O cache SQLite fica na pasta local do aplicativo, fora do OneDrive, e não é a
fonte única dos dados. Um ponto de leitura só é reutilizado quando a identificação
e a âncora do arquivo original conferem; arquivo alterado exige reprocessamento
seguro. O período inicial dos relatórios não apaga consumo já lido.

Os arquivos são gravados e confirmados no sistema de arquivos antes de avançar
a leitura. A sincronização para a nuvem é responsabilidade do OneDrive: a gravação
local não confirma que o serviço já enviou o arquivo. Aguarde a sincronização do
OneDrive antes de formatar ou de esperar que outra máquina veja os mesmos dados.

## Fontes e unidades

| Ferramenta | Fonte | Unidade e limite |
|---|---|---|
| Claude | JSONL de sessões: `message.usage`, IDs de mensagem/requisição e pasta | Tokens reportados; inclui cache na entrada total sem somá-lo novamente. Snapshots de uma mesma mensagem atualizam o registro. |
| Codex | Rollouts: contexto da sessão e `token_count.info` | Tokens reportados; contadores acumulados viram deltas somente com baseline conhecido. Entrada em cache e raciocínio são subconjuntos. |
| Kiro v3 | `session.json` e `messages.jsonl`, evento `usage_summary` | Créditos por execução, inclusive execuções interrompidas com consumo. Tokens não estão disponíveis nessa fonte. |

Percentuais de cota não são convertidos em tokens. Créditos e tokens ficam em
colunas separadas. Valores ausentes mostram `—`; lacunas não são gastos iguais a zero.
Os registros não contêm prompts, respostas, conteúdo de arquivos ou credenciais.

Data e hora vêm do evento, não da modificação do arquivo. Os relatórios oferecem
evento, hora, dia, semana iniciada na segunda-feira e mês, com fuso configurável.
Filtros permitem selecionar ferramenta, projeto, pasta e máquina de coleta.

Projetos podem vir de uma raiz Git identificada ou dos mapeamentos de pasta
configurados pelo usuário. Uma pasta comum pode não ter projeto. Sessões com
várias raízes preservam essa informação sem escolher uma pasta arbitrariamente.
Mapeamentos atuais também se aplicam ao histórico exibido.

**Máquina de coleta** não comprova onde uma sessão copiada foi executada. As
máquinas que observaram o mesmo evento são preservadas no detalhe; a consolidação
conta o evento uma vez. Metadados atuais de uma sessão Kiro também não comprovam
seu modelo ou pasta em cada momento histórico; essa origem é identificada.

## Cobertura

O painel informa fontes ausentes, leituras incompletas, filas de indexação e erros.
A importação inicial é feita em partes para não bloquear a interface. Sessões
transportadas com o marcador do sincronizador são excluídas dessa contabilização.
Arquivos antigos, versões com outro formato e sessões sem resumo de uso podem
não fornecer consumo completo. Os limites de linhas exibidas não limitam os
totais do recorte; restrinja os filtros para inspecionar mais registros.

Não é necessário ativar `telemetry.enabled` do Kiro para ler os resumos locais.
Essa opção é telemetria do produto, não este histórico por pasta. A exportação
OpenTelemetry Enterprise tem outra cobertura e outro grão.

Fontes oficiais: [Claude Code Monitoring](https://code.claude.com/docs/en/monitoring-usage),
[Codex App Server](https://learn.chatgpt.com/docs/app-server),
[Kiro v3 Observability](https://github.com/aws-samples/sample-kiro-cli-multiagent-development/blob/main/docs/observability.md),
[Kiro Settings](https://kiro.dev/docs/reference/settings/).

## Verificação

No diretório `windows/`, execute `node --test test-telemetry-ui.cjs` e
`cargo test --locked telemetry`. Os testes usam arquivos sintéticos para validar
retomada, cópias entre máquinas, repetição de snapshots, valores desconhecidos e
falhas de armazenamento. Eles não comprovam sincronização na nuvem nem substituem
a validação da cobertura das sessões reais após instalar o aplicativo.
