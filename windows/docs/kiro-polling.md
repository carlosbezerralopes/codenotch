# Kiro: pausa e frequência de leitura

O aplicativo mantém a consulta pela CLI usando sua sessão existente. Após até
três falhas consecutivas de leitura, entra em standby e preserva os valores
anteriores como desatualizados. Uma resposta sem cota utilizável também conta
como falha. O contador sobrevive ao reinício; estado ilegível ou falha de escrita
pausam consultas automáticas em vez de reiniciar um ciclo de tentativas.

**Atualizar tudo** ou clicar para atualizar o Kiro retoma uma tentativa explícita,
desde que o controle de pausa possa ser gravado. Uma alteração de autenticação
confirmável também pode retomar. Quando a origem não é observável ou a mudança
é ambígua, use a atualização explícita; não prometemos detectar todo login.

A leitura saudável ocorre a cada 30 segundos com atividade local recente e a
cada 30 minutos em repouso. Falhas antes do limite usam intervalo de 30 segundos.
Atividade é um sinal limitado dos arquivos locais de sessões v3; não é apenas
ter a CLI aberta e não é apresentada como monitoramento completo de trabalho.
Sessões importadas e as sondas do próprio aplicativo são excluídas desse sinal.

Codenotch não executa `login` nem renova credenciais. A própria CLI pode iniciar
uma etapa de autenticação durante as sondas: o limite escolhido reduz a insistência,
mas não garante ausência de navegador. Não lemos cookies do navegador, não
exportamos tokens e não escrevemos no armazenamento de autenticação do Kiro.
