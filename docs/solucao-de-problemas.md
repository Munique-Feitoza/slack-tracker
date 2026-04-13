# Solução de problemas

Coisas que já me morderam, com o que eu fiz pra resolver.

## Duas instâncias rodando ao mesmo tempo → itens duplicados na Slack List

Foi o que aconteceu quando eu movi o projeto de pasta. Uma versão antiga ficou rodando a partir do `~/.local/share/Trash/...`, e quando eu subi a nova daqui, as duas dispararam o sync às 17h e o Slack ficou com itens duplicados.

**Como diagnosticar:**

```bash
ps aux | grep slack-tracker | grep -v grep
```

Se aparecer mais de um processo, eu confiro de qual pasta cada um está rodando:

```bash
readlink -f /proc/<PID>/cwd
readlink -f /proc/<PID>/exe
```

**Como resolver:**

```bash
kill <PID-da-instância-velha>
```

E só depois eu subo a nova. Se for via systemd, `systemctl --user restart slack-tracker.service` resolve.

## Scheduler logou `apply=false` mesmo com `TODO_SYNC_APPLY` setado

Provavelmente eu setei a variável **depois** de subir o daemon. O scheduler lê `TODO_SYNC_APPLY` uma vez só, no `start`.

**Como resolver:** exportar a variável e reiniciar o daemon.

```bash
export TODO_SYNC_APPLY=true
# se for via systemd, está no Environment= do unit file
systemctl --user restart slack-tracker.service
```

## `nenhum pai da semana atual encontrado na lista`

O `todo sync` precisa de um item-pai cujo nome bata com o padrão `DD/MM/YYYY - DD/MM/YYYY` cobrindo a data de hoje. Se eu não tenho esse item, ele falha.

**Como resolver:** criar manualmente no Slack um item da lista com esse formato no nome (ex: `07/04/2026 - 13/04/2026`). O `todo inspect` confirma se ele detectou:

```bash
slack-tracker todo inspect
```

A última linha imprime `Semana atual detectada: ...` ou `(nenhum pai da semana atual foi detectado...)`.

## `xdotool` não está pegando a janela ativa

Isso acontece em sessões Wayland (o `xdotool` é X11). O fallback do `xprop` também depende de X11.

**Como resolver:** rodar a sessão em X11. Se eu estiver no GDM/SDDM, basta escolher "Plasma (X11)" ou "GNOME on Xorg" no login.

## A LLM devolveu mais de 10 itens

O código já trunca pra 10 (com `warn!`), então não é problema. Mas se eu vejo isso no log com frequência, pode ser sinal de que o prompt do `SYSTEM_PROMPT_TODO` está sendo ignorado pelo modelo. Vale tentar:

- subir a `temperature` para baixo demais (já está em `0.2`, mas posso descer mais);
- trocar o modelo (`ANTHROPIC_MODEL` ou `OLLAMA_MODEL`) pra algo mais robusto;
- olhar o payload bruto com `RUST_LOG=debug` pra ver o que está sendo enviado.

## `SLACK_API_TOKEN não definido` ou `SLACK_LIST_ID não definido`

Os comandos do `todo` exigem essas duas variáveis. O daemon iniciado pelo systemd não enxerga o que está no `~/.zshrc` — preciso colocar no `Environment=` do service ou no `.env` da pasta de execução.

## `(working tree limpo)` o dia todo, mas eu sei que mexi em coisa

O `todo sync` só vê o que está dentro de `PROJECT_ROOTS`. Se eu mexi em um repo que não está na lista, ele simplesmente não aparece.

**Como resolver:** adicionar a pasta em `PROJECT_ROOTS` (separada por vírgula) e reiniciar o daemon.

## O resumo do `report` está em inglês ou genérico demais

O `SYSTEM_PROMPT` do `report` é bem curto. Se eu estiver usando Ollama com um modelo fraco, ele pode dar resultado pobre. Para esse fluxo eu prefiro `llama3.2` (que segue instruções razoavelmente). Para qualidade real, uso o `todo sync` com Claude — o prompt lá é muito mais detalhado.

## Resetando o estado local

Se eu quero começar do zero (apaga banco e marcador de último sync):

```bash
rm -rf ~/.local/share/slack-tracker
```

Próximo `start` recria tudo. Não toca em nada do Slack — só limpa o estado local.
