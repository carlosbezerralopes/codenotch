# Cotas, dias úteis e orçamento

As seções **Cotas** e **Histórico** ficam em Configurações. Clicar em um
indicador do menu da bandeja abre Cotas naquela janela. O notch e a bandeja
usam o mesmo motor de cálculo; a interface apenas formata o resultado.

## Intervalo real até a renovação

O cálculo usa a data/hora real de reset de cada janela. Quando a duração está
informada, o início é `reset - duração`. As janelas Claude conhecidas seguem
sua semântica de cinco horas ou sete dias; nomes arbitrários não definem uma
semana. O Kiro mensal confirmado usa os primeiros dias dos meses em UTC,
com 28, 29, 30 ou 31 dias, sem presumir sempre 30 dias.

O tempo útil soma os trechos do intervalo que caem de segunda a sexta, fora
dos feriados configurados. Primeiro, último e dia atual podem ser parciais.
Um dia útil equivalente representa 24 horas desse calendário: não presumimos
jornada das 8h às 18h. O offset UTC é configurável e fixo; não é uma base
completa de regras históricas de horário de verão.

- Saldo diário de referência = saldo restante / dias úteis restantes até reset.
- Referência restante desta semana = saldo restante × úteis restantes da semana / úteis restantes do ciclo.
- A semana vai de segunda 00h até a próxima segunda, no offset configurado, sempre recortada no reset.
- A diferença em pontos percentuais compara consumo oficial e tempo útil já decorrido no ciclo.

Claude/Codex usam percentuais. Kiro usa créditos explícitos somente quando a
fonte identifica o plano mensal; saldo não é reconstruído de percentual
arredondado. Bônus combinados, escopo desconhecido e gastos extras não recebem
um orçamento mensal inventado. Sem início conhecido, o orçamento restante
pode ser disponível, mas a referência do ciclo inteiro fica desconhecida.

Esses valores distribuem o saldo disponível. Não alteram limites do provedor
e não comprovam quanto foi gasto nesta semana; o consumo
observado pertence ao Histórico. Sábado, domingo e feriados não avançam tempo
útil, mas o saldo oficial pode ser usado neles. Sem dias úteis restantes, a
interface informa isso e preserva o saldo, sem divisão por zero.

Leituras antigas, futuras, ausentes ou sem reset válido não geram referências
atuais. Cada conta Claude mantém sua própria origem e validade; a atualização
de outra conta não revalida seus dados. O journal no OneDrive preserva essa
origem por janela.

## Saldo e prazo

O cartão principal da cota semanal Claude/Codex ou mensal Kiro destaca saldo,
disponível por dia útil e prazo. A linha visual compara esgotamento projetado
com renovação e informa quantos dias úteis antes o saldo acabaria. Limites de
sessão e específicos de modelo ficam compactos, com detalhes sob expansão.
Kiro mostra créditos e a porcentagem equivalente apenas com limite explícito.

O ritmo observado usa diferenças de leituras oficiais da mesma conta, cota,
ciclo e limite. Examina os três dias úteis equivalentes mais recentes e exige
ao menos um dia útil observado, duas leituras e cobertura de 50%. Intervalos
que atravessam dias extras não são distribuídos artificialmente em dias úteis.
Tokens dos históricos de sessão não são convertidos em percentuais de cota.

Quando não é possível medir um ritmo recente, uma média do ciclo pode ser
mostrada com esse nome, desde que o início e o tempo útil sejam conhecidos.
Histórico incompleto, conflitante ou truncado não vira previsão. Leituras
antigas mostram sua data, sem previsão atual. Se restar menos de um dia útil,
o cartão mostra saldo até renovar; se não restar tempo útil, mostra dias extras.
Uma cota oficial esgotada continua visível mesmo sem histórico.

Dias úteis decorrem até a renovação real, não apenas até sexta-feira. No primeiro
dia de um ciclo iniciado em feriado, tempo útil decorrido igual a zero é válido:
a previsão aguarda dados, enquanto o orçamento dos dias restantes pode aparecer.

## Calendário e cobertura

O padrão é BR/CE/Fortaleza, UTC−180 minutos. Em **Appearance → Work calendar**,
revise país, estado, cidade, offset e datas manuais. Uma data com trabalho
substitui um feriado em dia de semana; fins de semana continuam dias extras.
Regras verificadas em 10/10/2026, para o ano corrente e os dois adjacentes.
Mudanças legislativas futuras exigem atualização.

| Escopo | Datas e fonte |
|---|---|
| Brasil | Datas nacionais fixas, Sexta-feira da Paixão e Consciência Negra a partir de 2024. [Calendário MGI 2026](https://agenciagov.ebc.com.br/noticias/202512/confira-o-calendario-oficial-de-feriados-nacionais-e-pontos-facultativos-em-2026). |
| Ceará | Data Magna, 25 de março. [ALECE](https://portaldoservidor.al.ce.gov.br/noticias/portaria-suspende-expediente-da-alece-na-data-magna-do-ceara-2503). |
| Fortaleza | São José, Assunção, Sexta-feira Santa e Corpus Christi. [Lei 8.796/2003](https://sapl.fortaleza.ce.leg.br/media/sapl/public/normajuridica/2003/3331/3331_texto_integral.pdf). |
| Fortaleza, 2026 | Tricentenário em 13 de abril, somente nesse ano. [Lei 11.649/2026](https://sapl.fortaleza.ce.leg.br/media/sapl/public/normajuridica/2026/14390/lei_no_11.649_de_08_de_abril_de_2026_-_rfpl_0185-2026.pdf). |

Corpus Christi é municipal em Fortaleza. Carnaval, emendas e dispensas de
expediente de órgãos não viram feriados nacionais. A cobertura municipal é
confirmada somente para Fortaleza. Outras cidades usam datas conhecidas e
manuais e mostram calendário parcial; fora dos anos cobertos, ficam indisponíveis.

Localização por IP é opcional e explícita ([ipapi.co](https://ipapi.co/api/));
nunca ocorre ao abrir o aplicativo e não salva a sugestão automaticamente.
Não guardamos IP, coordenadas ou resposta bruta. O usuário revisa e salva.

## Verificação

Em `windows/`: `node --test test-usage-insights-ui.cjs test-settings-history.cjs`
e `node scripts/check-ui-scripts.mjs`. O motor e a persistência têm fixtures
Rust: `cargo test --locked`. Incluem reset na terça, mês curto/bissexto,
feriado, trechos parciais, contas com validades diferentes, desconhecidos,
falha de journal e reabertura do indicador. Fixtures não validam a conta real
nem a sincronização na nuvem.
