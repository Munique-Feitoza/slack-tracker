# Documentação do slack-tracker

Aqui eu reuni tudo o que preciso lembrar sobre o projeto. O [README.md](../README.md) na raiz é um resumo rápido — esta pasta é o material completo.

## Índice

- [arquitetura.md](arquitetura.md) — como os módulos conversam entre si e onde os dados ficam.
- [instalacao.md](instalacao.md) — dependências de sistema, build e onde colocar o binário.
- [configuracao.md](configuracao.md) — todas as variáveis de ambiente que eu uso, com exemplos de `~/.zshrc`.
- [comandos.md](comandos.md) — o que cada subcomando da CLI faz (`start`, `report`, `todo inspect`, `todo sync`).
- [scheduler-e-servico.md](scheduler-e-servico.md) — o sync diário das 17h e como deixo rodando como serviço do systemd.
- [solucao-de-problemas.md](solucao-de-problemas.md) — coisas que já me morderam (instâncias duplicadas, systemd sem env, etc).

## Como eu uso o projeto no dia a dia

1. O daemon (`slack-tracker start`) fica rodando o tempo todo em background, ligado pelo systemd no login.
2. A cada 60s ele anota qual janela está ativa no SQLite local.
3. Às 17h ele roda o `todo sync` automaticamente: lê meus commits, diffs e tempo de tela do dia, pede pra LLM montar uma lista curta em primeira pessoa e atualiza a minha Slack List da semana atual.
4. Eu nunca preciso parar pra escrever "o que fiz hoje" — é só conferir e seguir.
