# slack-tracker

Daemon em Rust que eu uso para acompanhar minha rotina de trabalho no Linux/X11. Ele observa qual janela está ativa, descobre em qual projeto eu estou mexendo, junta o que o Git mostra do dia e gera um resumo via LLM (Ollama local, Claude ou OpenAI) para alimentar a minha lista da semana no Slack — automaticamente, todo dia às 17h.

A ideia é simples: no fim do expediente eu não preciso parar pra escrever "o que fiz hoje", o próprio tracker monta isso e atualiza minha Slack List.

## Início rápido

```bash
# build
cargo build --release

# config mínima (no ~/.zshrc)
export SLACK_API_TOKEN="xoxb-..."
export SLACK_LIST_ID="F0123456789"
export ANTHROPIC_API_KEY="sk-ant-..."
export PROJECT_ROOTS="/caminho/do/meu/projeto"
export TODO_SYNC_APPLY=true

# rodar o daemon
./target/release/slack-tracker start
```

Pronto. Ele já fica logando a janela ativa a cada 60s e dispara o `todo sync` automaticamente às 17h (horário local).

## Documentação completa

Toda a documentação está em [docs/](docs/):

- [docs/README.md](docs/README.md) — índice
- [docs/arquitetura.md](docs/arquitetura.md) — como os módulos conversam, fluxo do `todo sync`, persistência
- [docs/instalacao.md](docs/instalacao.md) — dependências de sistema (`xdotool`, Ollama) e build
- [docs/configuracao.md](docs/configuracao.md) — todas as variáveis de ambiente
- [docs/comandos.md](docs/comandos.md) — `start`, `report`, `todo inspect`, `todo sync`
- [docs/scheduler-e-servico.md](docs/scheduler-e-servico.md) — sync diário das 17h e service do systemd
- [docs/solucao-de-problemas.md](docs/solucao-de-problemas.md) — instâncias duplicadas, systemd sem env, etc.
