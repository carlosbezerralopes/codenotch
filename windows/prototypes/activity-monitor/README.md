# Monitor de atividade — protótipo Windows

Prévia isolada do layout e dos sensores. Não modifica o Codenotch instalado.

```powershell
pwsh -NoProfile -File windows/scripts/preview-activity-monitor.ps1
```

Requer Windows, PowerShell 7 e Node.js 18 ou superior. Abra a URL local impressa
no terminal; `Ctrl+C` encerra o servidor e o coletor. A porta é livre por padrão;
`-Port 47831` escolhe uma porta. `-Check` imprime uma leitura sem abrir o servidor.
Não exige instalação, credenciais ou acesso de administrador. Execute em um
terminal comum: sandboxes que bloqueiam CIM ou loopback podem impedir a prévia.

## Layout proposto

- Anéis de IA no topo, preservando a prioridade e o estilo do notch.
- Divisor horizontal e CPU, RAM, discos, cada GPU e rede por último.
- Passe o mouse sobre um anel para ver somente o gráfico, taxas e capacidade
  daquele recurso. Foco pelo teclado e clique também abrem o detalhe.
- O detalhe fecha ao sair; é possível mover o cursor para o painel sem fechá-lo.
- **Indicadores**, também disponível na engrenagem do notch, escolhe quais
  recursos aparecem e sua ordem dentro dos grupos **IA** e **Máquina**.
- O detalhe e a trilha de indicadores não usam barras de rolagem.
- Atualização de 3 segundos por padrão, opções de 5 e 10 segundos.
- Janela de 5 minutos por padrão, opções de 1 e 15 minutos, pausa e tema.

A seleção e a ordem ficam em RAM nesta prévia e voltam ao padrão ao recarregar.
Ocultar um indicador altera sua visibilidade; o coletor continua lendo os sensores
para que o histórico esteja disponível ao mostrá-lo novamente.

As cotas de IA desta prévia são **ilustrativas**. Hardware vem de sensores reais.
Os símbolos de CPU/RAM/SSD/rede são SVGs locais; Intel/NVIDIA usam identificadores
tipográficos nesta proposta. A integração definitiva pode incorporar os símbolos
oficiais licenciados dos fabricantes. Os glyphs de IA existentes conservam a
proveniência e a licença descritas em `codenotch/glyphs/NOTICE.md`.

## Dados e limites

O navegador mantém até 15 minutos de amostras **somente em RAM**. Recarregar a
página elimina o histórico. Sem arquivos de métricas, banco, localStorage,
IndexedDB ou exportação. O coletor conserva apenas os contadores anteriores e
o servidor a última resposta, também em RAM; coleta sob demanda. Pausar ou
fechar a página interrompe novos pedidos. Falhas, pausas e valores indisponíveis
quebram o traçado; a primeira leitura de uma taxa precisa de uma baseline.

| Recurso | Leitura |
|---|---|
| CPU | Uso pela diferença de `GetSystemTimes`; modelo do processador. Máquinas com mais de 64 processadores precisam de leitura adicional por grupo. |
| RAM | Memória física usada/total/livre por `GlobalMemoryStatusEx`. |
| Disco | Atividade por tempo não ocioso; leitura/escrita em bytes/s por disco físico. Não é espaço ocupado. |
| GPU | Adaptadores identificados por DXGI LUID e VendorId; motor mais ocupado após agregar processos do mesmo motor. Não soma motores diferentes. |
| Rede | Recepção/envio em bytes/s por interface física; filtros e adaptadores virtuais são excluídos. Ethernet ou Wi-Fi conforme disponível. |

Uma GPU sem contadores disponíveis aparece sem leitura. Temperatura não é
garantida pelas APIs genéricas e não recebe um número inventado. A identificação
de hardware é capturada ao iniciar; reinicie a prévia após conectar ou remover
dispositivos. Mudanças de contadores e intervalos longos exigem nova baseline.

Os números representam **a máquina inteira**. A coincidência com uma atividade
de IA serve como contexto temporal, sem atribuir CPU/GPU/rede ou energia àquela
tarefa. O refresh dos sensores é independente das consultas de cota dos provedores.

## Plano da assinatura no histórico de tokens

O histórico atual já tem modelo, modelo solicitado e horários de evento/coleta.
O plano da assinatura aparece em alguns adaptadores de cota, mas não é persistido
como plano efetivo de cada evento. Proposta de extensão futura:

```json
{
  "subscription_plan": {
    "value": "nome informado pela fonte",
    "source": "fonte verificável",
    "observed_at": "2026-10-13T12:00:00Z",
    "account_ref": "referência não secreta",
    "basis": "reported_at_event"
  }
}
```

Campo opcional; desconhecido quando não houver evidência de conta e plano no
momento do evento. Uma observação atual do plano não preenche eventos antigos.
Observações feitas pelo coletor teriam `basis: observed`, com seu próprio horário,
e associação somente quando conta e validade temporal forem comprovadas.
Preservar lotes antigos e IDs de eventos; definir merge determinístico das
observações. O journal de cotas possui schema/hash próprios e exige versionamento
se for estendido. Esta prévia **não altera os registros de tokens nem o journal**.

Sem gravar métricas da máquina, a correlação com tokens fica limitada à janela
que ainda estiver em RAM. Um resumo durável por atividade seria uma decisão
separada de retenção e ainda não comprovaria atribuição de consumo.

## Portabilidade

O frontend e o DTO JSON são reutilizáveis. O coletor deste protótipo é Windows.
Na integração Rust/Tauri, trocar o transporte loopback por comandos/eventos do
aplicativo e manter um buffer circular com o mesmo contrato. Linux e macOS precisam
de adaptadores próprios, principalmente para GPU, disco e rede; não foram
implementados ou validados nesta entrega. Falta de sensor continuará explícita.

## Verificação

```powershell
node --test windows/test-activity-monitor.cjs
node --check windows/prototypes/activity-monitor/server.cjs
node --check windows/prototypes/activity-monitor/monitor.js
```

Os testes cobrem retenção por tempo, valores desconhecidos, pausas/gaps,
ordem de amostras e limites do servidor local. A validação dos sensores exige
`-Check` ou a prévia em um Windows real; fixtures não comprovam sensores.

Validação nesta máquina em 13/10/2026: CPU, RAM, SSD, Intel, NVIDIA e Ethernet
com leituras; outro adaptador Intel exposto por DXGI sem contadores apareceu
indisponível. Duas leituras do servidor levaram 79,73 e 80,09 ms, sem erros.
Isso mede latência da coleta, não percentual de CPU do monitor.

Fontes: [GetSystemTimes](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getsystemtimes),
[RAM física](https://learn.microsoft.com/en-us/windows/win32/api/sysinfoapi/ns-sysinfoapi-memorystatusex),
[identidade DXGI](https://learn.microsoft.com/en-us/windows/win32/api/dxgi/ns-dxgi-dxgi_adapter_desc1),
[motores de GPU](https://devblogs.microsoft.com/directx/gpus-in-the-task-manager/),
[fórmulas dos contadores](https://learn.microsoft.com/en-gb/windows/win32/perfctrs/calculating-counter-values),
[temporizador do disco](https://learn.microsoft.com/en-us/windows/win32/wmisdk/precision-timer-algorithm-counter-types).
