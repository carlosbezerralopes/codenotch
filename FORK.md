# Codenotch — fork Windows

Fork: [carlosbezerralopes/codenotch](https://github.com/carlosbezerralopes/codenotch),
vinculado ao projeto original. Esta árvore inclui o port Kiro inicial e as
evoluções Windows, consolidadas na PR do proprietário.
Base: [vinzdg/codenotch](https://github.com/vinzdg/codenotch), commit
`bcb28894d1514fef5a810595d64b44a3817c2076`.

O desenvolvimento deste fork concentra-se em `windows/`. A implementação macOS
permanece como referência upstream. Licença e atribuições originais são preservadas.

O suporte Kiro usa a CLI nativa e sua sessão existente para ler `/usage`.
Veja [pré-requisitos e comportamento](windows/README.md). A IDE sozinha não
fornece essa integração; instalar este fork não instala nem autentica o Kiro.

O remote e os endereços de distribuição apontam para esse fork.
O updater exige pacotes assinados; um destino privado também exige acesso
autenticado. A origem/licença do repositório original devem ser preservadas.

O Windows também oferece créditos absolutos Kiro no tooltip, ordem e nomes
personalizados, detalhes de atividade recolhidos por padrão e histórico de
consumo integrado às configurações e orçamento diário/semanal por dias úteis.
Os eventos e pontos de leitura são guardados
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
A versão 1.22.2 passa pelos checks do fork. O README aponta para o artifact
Windows de nome fixo da última execução de push validada. Merge e release
estável continuam dependendo do aval do proprietário.
