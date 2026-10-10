# Codenotch — fork Windows

Fork: [carlosbezerralopes/codenotch](https://github.com/carlosbezerralopes/codenotch).
Base: [vinzdg/codenotch](https://github.com/vinzdg/codenotch), commit
`bcb28894d1514fef5a810595d64b44a3817c2076`.

O desenvolvimento deste fork concentra-se em `windows/`. A implementação macOS
permanece como referência upstream. Licença e atribuições originais são preservadas.

O suporte Kiro usa a CLI nativa e sua sessão existente para ler `/usage`.
Veja [pré-requisitos e comportamento](windows/README.md). A IDE sozinha não
fornece essa integração; instalar este fork não instala nem autentica o Kiro.

O repositório foi desvinculado da rede de forks e tornado privado. A distribuição
Windows usa downloads autenticados das releases ou dos artifacts do Actions.
O updater anônimo não acessa releases privadas; atualizações automáticas precisam
de um mecanismo autenticado e da chave de assinatura antes de serem habilitadas.

O Windows também oferece créditos absolutos Kiro no tooltip, ordem e nomes
personalizados, detalhes de atividade recolhidos por padrão e histórico de
consumo com destino configurável. Os eventos e pontos de leitura são guardados
por máquina e consolidados. Veja [telemetria](windows/docs/telemetry.md) e
[calendário de dias úteis](windows/docs/work-calendar.md).

## Verificação

No diretório `windows/`:

```powershell
node scripts/check-ui-scripts.mjs
node --test test-kiro.cjs scripts/test-fork-updates.cjs
cargo build --locked
cargo test --locked
```

Rust/MSVC são pré-requisitos para o build Windows. A leitura real do Kiro exige
`kiro-cli` instalado e autenticado. Fixtures não comprovam leitura da conta real.
Os workflows Windows verificam build/testes e produzem o instalador do fork.
