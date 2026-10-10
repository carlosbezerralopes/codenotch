# Cota e tempo

O tooltip do Codex mantém o consumo oficial e acrescenta duas comparações nas
janelas cuja duração informada é de 6 a 8 dias: tempo corrido e tempo útil.
A identificação usa `duration_seconds`, nunca o nome/id da janela. O anel
principal continua seguindo a janela `primary` do provedor.
O anel semanal separado também exige duração informada de 6 a 8 dias em
`secondary`. Se `primary` já for a semana, o anel fino não a duplica. `secondary`
mensal ou sem duração não recebe um anel semanal; extras não substituem a semana.

O início é `resets_at - duration_seconds`. Leituras sem duração/reset, fora do
ciclo, expiradas, stale ou com mais de 15 minutos não recebem comparação.
Nenhum valor é extrapolado de um percentual ou confundido com tokens.

Tempo corrido é a fração de segundos passados no ciclo real. Tempo útil soma
apenas os segundos do intervalo que caem de segunda a sexta, fora dos feriados,
no deslocamento UTC configurado. Primeiro, último e dia atual podem ser parciais.
O deslocamento é fixo; não representa uma base completa de regras de horário de
verão. O serviço por IP oferece o deslocamento atual como sugestão.

Sábado/domingo e feriados não avançam o tempo útil. Seu consumo continua na cota
oficial. Não há pesos inventados para o consumo desses dias. A diferença exibida
em pontos percentuais compara consumo oficial e tempo útil transcorrido; não é
previsão, alerta ou estimativa de esgotamento. Ciclos sem segundos úteis mostram
indisponível, nunca 0% inventado.

## Calendário e fontes

Regras verificadas em 10/10/2026; são recalculadas para o ano corrente, anterior
e seguinte. Datas excepcionais mantêm seu ano próprio. Fora da cobertura de anos,
o tempo útil fica indisponível. Mudanças legislativas futuras exigem atualização.

| Escopo | Regra incluída | Fonte primária |
|---|---|---|
| Brasil | Datas nacionais fixas e Sexta-feira da Paixão calculada pela Páscoa; Consciência Negra a partir de 2024 | [Calendário MGI 2026, Portaria 11.460/2025, reproduzido pela Agência Gov](https://agenciagov.ebc.com.br/noticias/202512/confira-o-calendario-oficial-de-feriados-nacionais-e-pontos-facultativos-em-2026) |
| Ceará | Data Magna, 25 de março | [ALECE, Constituição estadual art. 18](https://portaldoservidor.al.ce.gov.br/noticias/portaria-suspende-expediente-da-alece-na-data-magna-do-ceara-2503) |
| Fortaleza | São José, 19 de março; Assunção, 15 de agosto; Sexta-feira Santa e Corpus Christi (Páscoa + 60 dias) | [Lei municipal 8.796/2003 no Diário Oficial, Câmara Municipal](https://sapl.fortaleza.ce.leg.br/media/sapl/public/normajuridica/2003/3331/3331_texto_integral.pdf) |
| Fortaleza, somente 2026 | Tricentenário, 13 de abril | [Lei 11.649/2026, art. 1º](https://sapl.fortaleza.ce.leg.br/media/sapl/public/normajuridica/2026/14390/lei_no_11.649_de_08_de_abril_de_2026_-_rfpl_0185-2026.pdf) |

Corpus Christi é municipal em Fortaleza; não é convertido em feriado nacional.
Carnaval, quinta-feira santa, dia do servidor, emendas e outras dispensas de
expediente da Justiça/serviço público não entram como feriados gerais. O usuário
pode cadastrar datas específicas da sua rotina como sem trabalho. Uma data
explicitamente marcada com trabalho substitui o feriado de um dia de semana;
sábados e domingos permanecem extras.

A cobertura municipal é confirmada somente para Fortaleza/CE. Outras cidades
usam as datas nacionais brasileiras, a estadual do Ceará quando aplicável e
datas manuais; a barra informa **calendário parcial** e feriados locais não
confirmados. Países fora do Brasil recebem apenas a rotina segunda–sexta e suas
datas manuais, também com cobertura parcial.

## Configuração e privacidade

Appearance → Work calendar permite país/estado/cidade, deslocamento UTC em
minutos e datas manuais. O padrão é BR/CE/Fortaleza, UTC−180 minutos. O arquivo
`work-calendar.json` fica junto de `config.json`, é escrito atomicamente e só é
aplicado depois da confirmação de escrita. Não altera o login, conta ou cota.

**Localizar por IP (ipapi.co)** é uma ação opcional e explícita. Não é chamada na
abertura. Consulta somente [o endpoint público HTTPS documentado](https://ipapi.co/api/),
com timeout de 5 segundos, resposta limitada a 16 KiB e sem redirects. Usa apenas
país, código da região, cidade, timezone e offset. Não guarda endereço IP,
coordenadas, resposta bruta nem credenciais. A sugestão não é salva automaticamente;
cancelamento durante a consulta ignora sua resposta. O calendário precisa ser
revisado e salvo pelo usuário. Um Save já despachado não é apresentado como cancelável.

Os detalhes privados de atividade continuam recolhidos no hover; clicar revela
as mesmas linhas existentes, e fechar ou trocar provedor recolhe novamente.

## Verificação

De `windows/`:

```powershell
node --test test-work-pace.cjs test-activity-privacy.cjs test-kiro-credits.cjs test-codex-headline.cjs
node scripts/check-ui-scripts.mjs
cargo test --locked
```

As fixtures são sintéticas, sem conta/IP real. Cobrem feriado na segunda-feira,
fins de semana, segundos parciais, virada de ano, calendário parcial/indisponível,
datas manuais, nomes de janela enganosos, zero conhecido, stale/expiração e
cancelamento/falhas de configuração. Testes Rust cobrem transportes/duração,
cache antigo, feriados recorrentes e exceção de 2026, validação e parser IP.
