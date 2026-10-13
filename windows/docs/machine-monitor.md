# Monitor da máquina — Windows

Os recursos da máquina aparecem depois dos assistentes de IA, separados por uma
linha. Passe o mouse sobre CPU, memória, disco, GPU ou rede para abrir somente o
detalhe daquele recurso, com seu gráfico recente. A seção **Monitor** nas
configurações escolhe os recursos visíveis, sua ordem, o intervalo e a janela.
O menu de contexto de um recurso também abre essa seção.

Marque/desmarque os recursos, use as setas e clique em **Salvar**. A interface
confirma a gravação antes de aplicar a escolha. **Cancelar alterações** restaura
o que foi salvo; **Recarregar** consulta novamente a configuração. A seleção e
a ordem dos assistentes continuam em **Contas**.

## Preferências e histórico

As preferências ficam em `machine_monitor` dentro do `config.json` existente
do Codenotch. Elas incluem o monitor ativado/desativado, atualização de 3/5/10
segundos, janela de 1/5/15 minutos, ordem e indicadores ocultos. A gravação
substitui o arquivo atomicamente e preserva as outras configurações.

O aplicativo restaura essas preferências ao iniciar. Identidades de dispositivos
ausentes permanecem lembradas para quando retornarem. GPUs usam identidade de
hardware para a seleção; o LUID da sessão do Windows serve apenas para encontrar
os contadores e não é a identidade salva. Adaptadores iguais usam um índice para
distingui-los. CPU e memória têm identidades fixas; a rede usa o GUID da interface.

As amostras e o histórico de até 15 minutos ficam **somente em RAM**. Reiniciar o
app limpa o gráfico, preservando as preferências. Nenhuma amostra entra no histórico
de tokens, nos journals de cota ou em arquivos de métricas. Desativar o monitor
interrompe a coleta; ocultar um anel altera sua visibilidade.

## Sensores

O coletor é nativo, dentro do processo Rust do Codenotch. Não depende do servidor
do protótipo, Node.js, PowerShell, ferramentas externas, login ou administrador.

| Recurso | Medição |
|---|---|
| CPU | Diferença dos tempos do Windows; memória física por `GlobalMemoryStatusEx`. |
| Disco físico | Percentual de tempo ativo e bytes/s de leitura e escrita por PDH. Não é espaço ocupado. SSD/HDD só recebe esse rótulo quando há evidência do dispositivo. |
| GPU | DXGI identifica o adaptador; PDH fornece as instâncias dos motores. Soma processos do mesmo motor e exibe o motor mais ocupado, sem somar motores diferentes. |
| Ethernet/Wi-Fi | Recepção e envio da interface física, por diferenças dos contadores e tempo monotônico. Adaptadores virtuais e filtros são excluídos. |

Primeira leitura, reset de contador, sensor ausente ou intervalo longo ficam
explicitamente sem número até haver uma medição válida. Temperatura depende de
suporte do sensor; não é estimada. O coletor atualiza o inventário periodicamente.
Em máquinas com mais de 64 processadores, a leitura de CPU por `GetSystemTimes`
tem a limitação de grupos descrita pela Microsoft.

Essas são métricas da máquina inteira. A coincidência com uma atividade de IA
fornece contexto temporal, sem atribuir CPU, GPU, rede, energia ou custo à tarefa.
Linux usa um estado de monitor indisponível; esse coletor ainda não foi portado.
O aplicativo macOS em Swift permanece com sua implementação existente.

## Verificação

No diretório `windows/`:

```powershell
node scripts/check-ui-scripts.mjs
node --test test-machine-monitor-ui.cjs test-machine-monitor-settings.cjs
cargo test --locked
pwsh -NoProfile -File scripts/build-local.ps1 -Installer
./target/release/codenotch.exe machine-monitor
```

`machine-monitor` faz duas leituras nativas, separadas por três segundos, e imprime
a segunda em JSON. Não inicializa os provedores de IA nem grava as amostras.
Os testes de configuração gravam e releem um arquivo isolado, incluindo ordem,
todos os indicadores ocultos e dispositivos ausentes, sem tocar as preferências
do usuário. Testes de UI cobrem a gravação confirmada, falhas e o histórico em RAM.

Fontes: [GetSystemTimes](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getsystemtimes),
[RAM física](https://learn.microsoft.com/en-us/windows/win32/api/sysinfoapi/nf-sysinfoapi-globalmemorystatusex),
[DXGI](https://learn.microsoft.com/en-us/windows/win32/api/dxgi/ns-dxgi-dxgi_adapter_desc1),
[PDH](https://learn.microsoft.com/en-us/windows/win32/api/pdh/nf-pdh-pdhaddenglishcounterw),
[motores de GPU](https://devblogs.microsoft.com/directx/gpus-in-the-task-manager/),
[interfaces de rede](https://learn.microsoft.com/en-us/windows/win32/api/netioapi/nf-netioapi-getiftable2).
