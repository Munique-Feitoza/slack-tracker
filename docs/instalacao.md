# Instalação

## Dependências de sistema

Eu rodo no Manjaro (Arch). O essencial:

```bash
sudo pacman -S xdotool
```

`xdotool` é o que o daemon usa por padrão pra ler a janela ativa. Se ele não estiver instalado, o código tenta cair no fallback via `xprop` — mas o `xdotool` é mais confiável.

Para usar a LLM **localmente** (recomendado, evita mandar código da empresa pra fora), eu instalo o Ollama:

```bash
sudo pacman -S ollama
sudo systemctl enable --now ollama.service
ollama pull llama3.2
```

Se eu prefiro usar Claude (que é o que eu uso de verdade pra `todo sync`), basta ter `ANTHROPIC_API_KEY` no ambiente — não precisa de Ollama.

## Build

O projeto é Rust puro, builda com `cargo`:

```bash
cd /caminho/para/slack-tracker
cargo build --release
```

O binário fica em `./target/release/slack-tracker`. Para chamar de qualquer lugar, eu copio (ou linko) pra `~/.local/bin`:

```bash
mkdir -p ~/.local/bin
cp ./target/release/slack-tracker ~/.local/bin/
```

E garanto que `~/.local/bin` está no `PATH` (no meu `~/.zshrc`):

```bash
export PATH="$HOME/.local/bin:$PATH"
```

## Verificando que tudo subiu

```bash
slack-tracker --help
```

Se aparecer a lista de subcomandos (`start`, `report`, `todo`), está pronto. Próximo passo é a [configuração](configuracao.md).
