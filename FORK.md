# Codenotch — fork Windows

Fork: [carlosbezerralopes/codenotch](https://github.com/carlosbezerralopes/codenotch).
Base: [vinzdg/codenotch](https://github.com/vinzdg/codenotch), commit
`bcb28894d1514fef5a810595d64b44a3817c2076`.

O desenvolvimento deste fork concentra-se em `windows/`. A implementação macOS
permanece como referência upstream. Licença e atribuições originais são preservadas.

O suporte Kiro usa a CLI nativa e sua sessão existente para ler `/usage`.
Veja [pré-requisitos e comportamento](windows/README.md). A IDE sozinha não
fornece essa integração; instalar este fork não instala nem autentica o Kiro.

As atualizações Windows apontam para as releases deste fork, para preservar suas
funcionalidades. Até publicarmos uma release Windows, a consulta pode indicar
que não conseguiu verificar atualizações. A chave pública do updater ainda
precisa ser configurada antes de publicar atualizações assinadas.

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
