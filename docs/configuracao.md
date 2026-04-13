# Configuração

Tudo é via variável de ambiente. Eu mantenho no `~/.zshrc` (e replico no `Environment=` do service do systemd, porque ele não herda o shell).

## Mínimo pra rodar `start` (só monitoramento)

```bash
# nada obrigatório — o daemon sobe e começa a logar janela ativa
```

O comando `start` por si só não precisa de chave de API nenhuma. Ele só vai gravar no SQLite. Mas se eu não setar nada de Slack, o scheduler interno vai falhar ao tentar rodar o `todo sync` às 17h.

## Mínimo pra `report --send`

```bash
export SLACK_WEBHOOK_URL="https://hooks.slack.com/services/..."
# E pelo menos uma LLM:
export OLLAMA_MODEL="llama3.2"        # ou
export OPENAI_API_KEY="sk-..."
```

> O `report` não usa Claude — só Ollama ou OpenAI. Se eu quiser Claude, é via `todo sync`.

## Mínimo pra `todo sync`

```bash
export SLACK_API_TOKEN="xoxb-..."
export SLACK_LIST_ID="F0123456789"
export PROJECT_ROOTS="$HOME/projetos/slack-tracker,$HOME/projetos/outro-repo"

# uma LLM (eu uso Claude aqui):
export ANTHROPIC_API_KEY="sk-ant-..."
# ou Ollama
```

## Tabela completa de variáveis

### Selecionando o provider de LLM

| Variável | Default | O que faz |
|---|---|---|
| `SLACK_TRACKER_LLM` | autodetect | força `ollama`, `openai` ou `claude`/`anthropic` |

A detecção automática segue essa ordem:
1. `ANTHROPIC_API_KEY` setada → Claude
2. `OPENAI_API_KEY` setada → OpenAI
3. nada setado → Ollama local

### Ollama (local)

| Variável | Default | O que faz |
|---|---|---|
| `OLLAMA_BASE_URL` | `http://localhost:11434` | endpoint do servidor Ollama |
| `OLLAMA_MODEL` | `llama3.1` | modelo a usar (eu uso `llama3.2`) |

### OpenAI

| Variável | Default | O que faz |
|---|---|---|
| `OPENAI_API_KEY` | — | obrigatória para usar OpenAI |
| `OPENAI_MODEL` | `gpt-4o-mini` | modelo |
| `OPENAI_BASE_URL` | `https://api.openai.com/v1` | endpoint (útil pra proxy interno) |

### Claude

| Variável | Default | O que faz |
|---|---|---|
| `ANTHROPIC_API_KEY` | — | obrigatória para usar Claude |
| `ANTHROPIC_MODEL` | `claude-sonnet-4-6` | modelo |
| `ANTHROPIC_BASE_URL` | `https://api.anthropic.com` | endpoint |

### Slack

| Variável | Default | O que faz |
|---|---|---|
| `SLACK_WEBHOOK_URL` | — | webhook usado pelo `report --send` |
| `SLACK_API_TOKEN` | — | token de bot (`xoxb-...`) com escopo de Slack Lists, usado pelo `todo` |
| `SLACK_LIST_ID` | — | ID da minha lista no Slack (começa com `F`) |
| `SLACK_COL_NAME` | `Col0` | ID da coluna de nome da minha Slack List |
| `SLACK_COL_DONE` | `Col00` | ID da coluna checkbox de concluído |
| `SLACK_COL_DATE` | `Col02` | ID da coluna de data |

### Coleta de atividade

| Variável | Default | O que faz |
|---|---|---|
| `PROJECT_ROOTS` | — | lista de pastas dos meus projetos, separadas por vírgula. Usada pelo `todo sync` para varrer commits/diffs/arquivos. |

> Sem `PROJECT_ROOTS`, o `todo sync` não tem nada pra analisar e devolve um plano vazio.

### Scheduler interno

| Variável | Default | O que faz |
|---|---|---|
| `TODO_SYNC_TIME` | `17:00` | horário em que o `start` dispara o `todo sync` automaticamente (formato `HH:MM`, fuso local) |
| `TODO_SYNC_APPLY` | `false` | se `true`/`1`/`yes`, aplica de verdade no Slack; senão é dry-run |

### Logging

| Variável | Default | O que faz |
|---|---|---|
| `RUST_LOG` | `info` | nível de log do `env_logger` |

## Carregamento via `.env`

O binário chama `dotenvy::dotenv_override()` no boot, então também aceita um arquivo `.env` na pasta de execução. Eu uso isso quando quero subir manualmente sem mexer no shell:

```
SLACK_API_TOKEN=xoxb-...
SLACK_LIST_ID=F0123456789
ANTHROPIC_API_KEY=sk-ant-...
PROJECT_ROOTS=/caminho/absoluto/do/projeto
TODO_SYNC_APPLY=true
```

## Como descobrir o `SLACK_LIST_ID`

A forma mais fácil é abrir a lista no Slack pela web — a URL contém algo como `.../lists/F0123456789`. Esse `F...` é o que vai em `SLACK_LIST_ID`.

Para conferir se o token tem permissão e ver a estrutura da lista:

```bash
slack-tracker todo inspect
```

Ele imprime os itens com `row_id`, hierarquia e status. Veja [comandos.md](comandos.md) pros detalhes.
