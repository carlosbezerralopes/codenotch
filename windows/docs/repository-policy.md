# Acesso e revisão

O destino é `carlosbezerralopes/codenotch`, fork público de `vinzdg/codenotch`.
O proprietário autorizou publicar as evoluções Windows em um único commit
na branch da PR existente. Isso não autoriza merge nem release estável.

Somente a conta proprietária deve ter escrita/admin. A publicidade permite
leitura, não escrita. Alterações seguem PR e os checks reais de Windows,
Linux e instalador, com aval explícito antes do merge ou release.

A política global está em `~/.agents/regras/contexto-git.md`; seu helper
`~/.agents/scripts/github/repositorio.py --action audit --repo dono/nome`
confere acesso, visibilidade, rulesets e Actions sem alterações. Ao associar
um destino novo, aplicar a política autorizada e verificar as capacidades do
plano. Repositórios existentes conservam sua visibilidade.

A conta proprietária não aprova nativamente uma PR da própria autoria. O
controle vem da escrita exclusiva e das regras da branch principal; o aval
humano antes do merge continua necessário.

Os endereços do updater e download devem corresponder ao fork de distribuição.
Se ele for privado, os downloads exigem autenticação. Publicar uma branch ou
passar no CI não autoriza release.
